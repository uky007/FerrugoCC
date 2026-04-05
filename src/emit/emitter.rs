//! アセンブリテキスト出力（Emitter）
//!
//! アセンブリ AST を AT&T 構文の x86-64 アセンブリテキストに変換する。
//! 出力は `.s` ファイルに書き込まれ、gcc（as + ld）でバイナリに変換される。
//!
//! # AT&T 構文のポイント
//! - オペランドの順序は `命令 src, dst`（Intel 構文と逆）
//! - 即値には `$` プレフィックス: `$42`
//! - レジスタには `%` プレフィックス: `%eax`, `%rax`
//! - 命令にはサイズサフィックス: `movl`（32ビット）、`movq`（64ビット）、`sete`（8ビット）
//!
//! # Chapter 11: 型に応じた命令サイズの切り替え
//! `AsmType::Longword` (32bit) → `l` サフィックス + 32ビットレジスタ (`%eax` 等)
//! `AsmType::Quadword` (64bit) → `q` サフィックス + 64ビットレジスタ (`%rax` 等)
//! 新規命令: `movslq` (int→long 符号拡張), `cqo` (RAX→RDX:RAX 符号拡張)
//!
//! # Chapter 12: 符号なし整数対応
//! `div` (符号なし除算), `MovZeroExtend` (32→64 ゼロ拡張),
//! 符号なし比較条件コード (`a`, `ae`, `b`, `be`)
//!
//! # Chapter 13: 浮動小数点数 (`double`)
//! SSE 命令出力: `movsd`, `addsd`, `subsd`, `mulsd`, `divsd`, `xorpd`, `comisd`,
//! `cvtsi2sd{l,q}`, `cvttsd2si{l,q}`。XMM レジスタ (`%xmm0`〜`%xmm15`)。
//! `double` 定数は `.rodata` セクションに `.quad` (ビットパターン) として配置。
//! XMM レジスタの Push/Pop は `subq $8,%rsp; movsd` / `movsd; addq $8,%rsp` で実装。
//!
//! # Chapter 14: ポインタ
//! - `Lea` 命令: `leaq src, dst`（アドレスロード）
//! - `Memory(Reg)` オペランド: `(%rax)`, `(%rcx)` 等（レジスタ間接アドレッシング）
//!
//! # Chapter 15: 配列とポインタ算術
//! - `ZeroInit(n)` → `.bss` セクションに `.zero n` で配列をゼロ初期化配置
//!
//! # Chapter 18: 構造体
//! - `MemoryOffset(Reg, i32)` オペランド: `N(%rax)` 等（レジスタ+オフセット間接アドレッシング）
//!   構造体メンバアクセスで使用する。
//!
//! # Chapter 20: レジスタ割り当て対応
//! - プロローグ/エピローグは emitter ではなく fixup パスが `Push`/`Pop`/`AllocateStack`/
//!   `DeallocateStack` 命令として生成する。emitter はこれらを個々の命令として出力するだけ。
//! - 新レジスタ（BX, R10-R15, SP, BP, XMM8-XMM13）のフォーマットを追加。
//!
//! # 難読化（`--fobfuscate`）対応
//! - `JmpIndirect(op, _)` → `jmp *%rax` 等の間接ジャンプ出力
//! - `CallIndirect(op)` → `call *%r10` 等の間接呼び出し出力
//! - `RawBytes(bytes)` → `.byte 0xe8, ...` の生バイト出力
//! - `PointerArrayInit(labels)` → `.quad label0, label1, ...` のラベルアドレス配列出力
//! - `ByteArrayInit(bytes)` → `.byte 0xNN, ...` の暗号化文字列バイト出力
//! - `Operand::Pseudo` が emit まで残った場合はバグとして `panic!` する。
//!
//! # 難読化対応
//! - `ByteArrayInit`: 暗号化された文字列リテラルを `.data` セクションに `.byte 0xNN, ...` として出力。
//!   難読化パスの文字列暗号化（`obfuscate.rs`）で生成される。
//!
//! # .note.GNU-stack セクション
//! 出力末尾に `.section .note.GNU-stack,"",@progbits` を付加する。
//! これはスタックが実行不可であることをリンカに伝えるセキュリティ上の慣習。
//! なくても動作するが、セキュリティ警告が出る場合がある。

use std::fmt::Write;

use crate::codegen::asm_ast::{
    AsmBinaryOp, AsmProgram, AsmStaticConstant, AsmStaticVar, AsmType, AsmUnaryOp, CondCode,
    Instruction, Operand, Reg, StaticInit,
};
use crate::error::{CompileError, Result};

/// アセンブリ AST をテキストに変換する（Chapter 11: 型サイズ対応）。
pub fn emit(program: &AsmProgram) -> Result<String> {
    let mut out = String::new();
    for func in &program.functions {
        emit_function(&mut out, func)?;
    }
    for var in &program.static_vars {
        emit_static_var(&mut out, var)?;
    }
    for constant in &program.static_constants {
        emit_static_constant(&mut out, constant)?;
    }
    // FerrugoCC watermark section (SHF_ALLOC: survives strip)
    writeln!(out, "    .section .ferrugo_sig,\"a\",@note")
        .map_err(|e| CompileError::EmitError(e.to_string()))?;
    writeln!(out, "    .byte 0x46,0x45,0x52,0x52,0x55,0x47,0x4f")
        .map_err(|e| CompileError::EmitError(e.to_string()))?;
    writeln!(out, "    .byte 0x01,0x00").map_err(|e| CompileError::EmitError(e.to_string()))?;
    // スタック非実行セクション（セキュリティ慣習）
    writeln!(out, "    .section .note.GNU-stack,\"\",@progbits")
        .map_err(|e| CompileError::EmitError(e.to_string()))?;
    Ok(out)
}

/// 関数のアセンブリ出力（Chapter 10: 条件付き .globl）。
///
/// `global` が true のとき `.globl` ディレクティブでシンボルを外部公開する。
/// `static` 関数は `global: false` なので `.globl` を出力しない。
fn emit_function(out: &mut String, func: &crate::codegen::asm_ast::AsmFunction) -> Result<()> {
    // .globl で関数シンボルをリンカに公開（static 関数は除く）
    if func.global {
        writeln!(out, "    .globl {}", func.name)
            .map_err(|e| CompileError::EmitError(e.to_string()))?;
    }
    // 関数ラベル
    writeln!(out, "{}:", func.name).map_err(|e| CompileError::EmitError(e.to_string()))?;

    // Chapter 20: プロローグ/エピローグは fixup パスが Push/Mov/AllocateStack/
    // DeallocateStack/Pop/Ret 命令として生成するため、ここでは出力しない。

    for instr in &func.instructions {
        emit_instruction(out, instr)?;
    }

    Ok(())
}

/// StaticInit の要素サイズを返す（配列アラインメント計算用）。
fn static_init_size(init: &StaticInit, asm_type: &AsmType) -> usize {
    match init {
        StaticInit::IntInit(_) => match asm_type {
            AsmType::Byte => 1,
            AsmType::Word => 2,
            AsmType::Longword => 4,
            _ => 8,
        },
        StaticInit::FloatInit(_) => 4,
        StaticInit::DoubleInit(_) => 8,
        StaticInit::ZeroInit(n) => *n,
        StaticInit::StringInit(_, n) => *n,
        StaticInit::ByteArrayInit(bytes) => bytes.len(),
        StaticInit::PointerArrayInit(labels) => labels.len() * 8,
        StaticInit::ArrayInit(elems) => elems.iter().map(|e| static_init_size(e, asm_type)).sum(),
    }
}

/// 静的初期化子が全ゼロかどうかを再帰的に判定する
fn is_static_init_zero(init: &StaticInit) -> bool {
    match init {
        StaticInit::IntInit(v) => *v == 0,
        StaticInit::DoubleInit(_) => false,
        StaticInit::ZeroInit(_) => true,
        StaticInit::ByteArrayInit(bytes) => bytes.iter().all(|&b| b == 0),
        StaticInit::ArrayInit(elems) => elems.iter().all(is_static_init_zero),
        _ => false,
    }
}

/// 静的変数のアセンブリ出力（Chapter 10, 11, 15: 型サイズ対応、配列ゼロ初期化）。
///
/// 初期値が 0 の場合は `.bss` セクション、非0 の場合は `.data` セクションに配置する。
/// `global` が true のとき `.globl` ディレクティブを出力する。
fn emit_static_var(out: &mut String, var: &AsmStaticVar) -> Result<()> {
    let (align, size) = match var.init {
        StaticInit::ZeroInit(n) => (16, n), // 配列は16バイトアラインメント
        StaticInit::PointerArrayInit(ref labels) => (8, labels.len() * 8),
        StaticInit::ArrayInit(ref elems) => {
            if let Some(ref vt) = var.var_type {
                // 構造体型: 型情報からサイズとアラインメントを取得
                (vt.alignment().max(16), vt.size())
            } else {
                let total_size: usize = elems
                    .iter()
                    .map(|e| static_init_size(e, &var.asm_type))
                    .sum();
                let elem_align = match var.asm_type {
                    AsmType::Byte => 1,
                    AsmType::Word => 2,
                    AsmType::Longword | AsmType::Float => 4,
                    AsmType::Quadword | AsmType::Double => 8,
                };
                (std::cmp::max(elem_align, 16), total_size)
            }
        }
        _ => match var.asm_type {
            AsmType::Byte => (1, 1),
            AsmType::Word => (2, 2),
            AsmType::Longword | AsmType::Float => (4, 4),
            AsmType::Quadword | AsmType::Double => (8, 8),
        },
    };
    if var.global {
        writeln!(out, "    .globl {}", var.name)
            .map_err(|e| CompileError::EmitError(e.to_string()))?;
    }

    let is_zero = match &var.init {
        StaticInit::IntInit(v) => *v == 0,
        // 浮動小数点は -0.0 と +0.0 を区別するため常に .data に配置
        StaticInit::FloatInit(_) => false,
        StaticInit::DoubleInit(_) => false,
        // 配列のゼロ初期化は .bss に配置（Chapter 15）
        StaticInit::ZeroInit(_) => true,
        // 文字列は .data に配置
        StaticInit::StringInit(_, _) => false,
        // バイト配列は .data に配置
        StaticInit::ByteArrayInit(_) => false,
        // ポインタ配列は .data に配置
        StaticInit::PointerArrayInit(_) => false,
        // 配列初期化子リスト: 全要素がゼロなら .bss に配置
        StaticInit::ArrayInit(elems) => elems.iter().all(is_static_init_zero),
    };

    if !is_zero {
        writeln!(out, "    .data").map_err(|e| CompileError::EmitError(e.to_string()))?;
        writeln!(out, "    .align {align}").map_err(|e| CompileError::EmitError(e.to_string()))?;
        writeln!(out, "{}:", var.name).map_err(|e| CompileError::EmitError(e.to_string()))?;
        match &var.init {
            StaticInit::IntInit(v) => {
                let directive = match var.asm_type {
                    AsmType::Byte => "byte",
                    AsmType::Word => "word",
                    AsmType::Longword => "long",
                    _ => "quad",
                };
                writeln!(out, "    .{directive} {v}")
                    .map_err(|e| CompileError::EmitError(e.to_string()))?;
            }
            StaticInit::FloatInit(v) => {
                writeln!(out, "    .long {}", v.to_bits())
                    .map_err(|e| CompileError::EmitError(e.to_string()))?;
            }
            StaticInit::DoubleInit(v) => {
                writeln!(out, "    .quad {}", v.to_bits())
                    .map_err(|e| CompileError::EmitError(e.to_string()))?;
            }
            StaticInit::ZeroInit(_) => unreachable!(),
            StaticInit::StringInit(content, _) => {
                writeln!(out, "    .asciz \"{}\"", escape_string_for_asm(content))
                    .map_err(|e| CompileError::EmitError(e.to_string()))?;
            }
            StaticInit::ByteArrayInit(bytes) => {
                let byte_strs: Vec<String> = bytes.iter().map(|b| format!("0x{b:02x}")).collect();
                writeln!(out, "    .byte {}", byte_strs.join(", "))
                    .map_err(|e| CompileError::EmitError(e.to_string()))?;
            }
            StaticInit::PointerArrayInit(labels) => {
                for label in labels {
                    writeln!(out, "    .quad {label}")
                        .map_err(|e| CompileError::EmitError(e.to_string()))?;
                }
            }
            StaticInit::ArrayInit(elems) => {
                emit_array_init_data(out, elems, &var.asm_type)?;
            }
        }
    } else {
        writeln!(out, "    .bss").map_err(|e| CompileError::EmitError(e.to_string()))?;
        writeln!(out, "    .align {align}").map_err(|e| CompileError::EmitError(e.to_string()))?;
        writeln!(out, "{}:", var.name).map_err(|e| CompileError::EmitError(e.to_string()))?;
        writeln!(out, "    .zero {size}").map_err(|e| CompileError::EmitError(e.to_string()))?;
    }
    Ok(())
}

/// 配列初期化子リストのデータ出力
fn emit_array_init_data(out: &mut String, elems: &[StaticInit], asm_type: &AsmType) -> Result<()> {
    for elem in elems {
        match elem {
            StaticInit::IntInit(v) => {
                let directive = match asm_type {
                    AsmType::Byte => "byte",
                    AsmType::Word => "word",
                    AsmType::Longword => "long",
                    _ => "quad",
                };
                writeln!(out, "    .{directive} {v}")
                    .map_err(|e| CompileError::EmitError(e.to_string()))?;
            }
            StaticInit::FloatInit(v) => {
                writeln!(out, "    .long {}", v.to_bits())
                    .map_err(|e| CompileError::EmitError(e.to_string()))?;
            }
            StaticInit::DoubleInit(v) => {
                writeln!(out, "    .quad {}", v.to_bits())
                    .map_err(|e| CompileError::EmitError(e.to_string()))?;
            }
            StaticInit::ZeroInit(n) => {
                writeln!(out, "    .zero {n}")
                    .map_err(|e| CompileError::EmitError(e.to_string()))?;
            }
            StaticInit::PointerArrayInit(labels) => {
                for label in labels {
                    writeln!(out, "    .quad {label}")
                        .map_err(|e| CompileError::EmitError(e.to_string()))?;
                }
            }
            StaticInit::ByteArrayInit(bytes) => {
                if !bytes.is_empty() {
                    let byte_strs: Vec<String> =
                        bytes.iter().map(|b| format!("0x{b:02x}")).collect();
                    writeln!(out, "    .byte {}", byte_strs.join(", "))
                        .map_err(|e| CompileError::EmitError(e.to_string()))?;
                }
            }
            StaticInit::ArrayInit(inner) => {
                emit_array_init_data(out, inner, asm_type)?;
            }
            StaticInit::StringInit(content, _) => {
                writeln!(out, "    .asciz \"{}\"", escape_string_for_asm(content))
                    .map_err(|e| CompileError::EmitError(e.to_string()))?;
            }
        }
    }
    Ok(())
}

/// 読み取り専用の静的定数（.rodata セクション）の出力（Chapter 13）。
fn emit_static_constant(out: &mut String, constant: &AsmStaticConstant) -> Result<()> {
    writeln!(out, "    .section .rodata").map_err(|e| CompileError::EmitError(e.to_string()))?;
    writeln!(out, "    .align {}", constant.alignment)
        .map_err(|e| CompileError::EmitError(e.to_string()))?;
    writeln!(out, "{}:", constant.name).map_err(|e| CompileError::EmitError(e.to_string()))?;
    match &constant.init {
        StaticInit::IntInit(v) => {
            writeln!(out, "    .quad {v}").map_err(|e| CompileError::EmitError(e.to_string()))?;
        }
        StaticInit::FloatInit(v) => {
            writeln!(out, "    .long {}", v.to_bits())
                .map_err(|e| CompileError::EmitError(e.to_string()))?;
            // xorps reads 128 bits; pad to 16 bytes when 16-byte aligned (sign mask)
            if constant.alignment >= 16 {
                writeln!(out, "    .zero 12")
                    .map_err(|e| CompileError::EmitError(e.to_string()))?;
            }
        }
        StaticInit::DoubleInit(v) => {
            writeln!(out, "    .quad {}", v.to_bits())
                .map_err(|e| CompileError::EmitError(e.to_string()))?;
        }
        StaticInit::ZeroInit(n) => {
            writeln!(out, "    .zero {n}").map_err(|e| CompileError::EmitError(e.to_string()))?;
        }
        StaticInit::StringInit(content, _) => {
            writeln!(out, "    .asciz \"{}\"", escape_string_for_asm(content))
                .map_err(|e| CompileError::EmitError(e.to_string()))?;
        }
        StaticInit::ByteArrayInit(bytes) => {
            let byte_strs: Vec<String> = bytes.iter().map(|b| format!("0x{b:02x}")).collect();
            writeln!(out, "    .byte {}", byte_strs.join(", "))
                .map_err(|e| CompileError::EmitError(e.to_string()))?;
        }
        StaticInit::PointerArrayInit(labels) => {
            for label in labels {
                writeln!(out, "    .quad {label}")
                    .map_err(|e| CompileError::EmitError(e.to_string()))?;
            }
        }
        StaticInit::ArrayInit(elems) => {
            // 静的定数の配列初期化子は想定外だが、安全のため処理
            for elem in elems {
                match elem {
                    StaticInit::IntInit(v) => {
                        writeln!(out, "    .quad {v}")
                            .map_err(|e| CompileError::EmitError(e.to_string()))?;
                    }
                    StaticInit::FloatInit(v) => {
                        writeln!(out, "    .long {}", v.to_bits())
                            .map_err(|e| CompileError::EmitError(e.to_string()))?;
                    }
                    StaticInit::DoubleInit(v) => {
                        writeln!(out, "    .quad {}", v.to_bits())
                            .map_err(|e| CompileError::EmitError(e.to_string()))?;
                    }
                    StaticInit::ZeroInit(n) => {
                        writeln!(out, "    .zero {n}")
                            .map_err(|e| CompileError::EmitError(e.to_string()))?;
                    }
                    _ => unreachable!("unsupported element type in array initializer"),
                }
            }
        }
    }
    Ok(())
}

/// 個々の命令をテキスト行に変換する。
fn emit_instruction(out: &mut String, instr: &Instruction) -> Result<()> {
    match instr {
        Instruction::Mov { asm_type, src, dst } => {
            if *asm_type == AsmType::Double {
                writeln!(
                    out,
                    "    movsd {}, {}",
                    format_operand_typed(src, asm_type),
                    format_operand_typed(dst, asm_type)
                )
                .map_err(|e| CompileError::EmitError(e.to_string()))?;
            } else if *asm_type == AsmType::Float {
                writeln!(
                    out,
                    "    movss {}, {}",
                    format_operand_typed(src, asm_type),
                    format_operand_typed(dst, asm_type)
                )
                .map_err(|e| CompileError::EmitError(e.to_string()))?;
            } else {
                let suffix = type_suffix(asm_type);
                writeln!(
                    out,
                    "    mov{suffix} {}, {}",
                    format_operand_typed(src, asm_type),
                    format_operand_typed(dst, asm_type)
                )
                .map_err(|e| CompileError::EmitError(e.to_string()))?;
            }
        }
        // Chapter 2: 単項演算命令
        Instruction::Unary {
            asm_type,
            op,
            operand,
        } => {
            let base = match op {
                AsmUnaryOp::Neg => "neg",
                AsmUnaryOp::Not => "not",
            };
            let suffix = type_suffix(asm_type);
            writeln!(
                out,
                "    {base}{suffix} {}",
                format_operand_typed(operand, asm_type)
            )
            .map_err(|e| CompileError::EmitError(e.to_string()))?;
        }
        // Chapter 2/13: 比較命令
        Instruction::Cmp { asm_type, src, dst } => {
            if *asm_type == AsmType::Double {
                writeln!(
                    out,
                    "    comisd {}, {}",
                    format_operand_typed(src, asm_type),
                    format_operand_typed(dst, asm_type)
                )
                .map_err(|e| CompileError::EmitError(e.to_string()))?;
            } else if *asm_type == AsmType::Float {
                writeln!(
                    out,
                    "    comiss {}, {}",
                    format_operand_typed(src, asm_type),
                    format_operand_typed(dst, asm_type)
                )
                .map_err(|e| CompileError::EmitError(e.to_string()))?;
            } else {
                let suffix = type_suffix(asm_type);
                writeln!(
                    out,
                    "    cmp{suffix} {}, {}",
                    format_operand_typed(src, asm_type),
                    format_operand_typed(dst, asm_type)
                )
                .map_err(|e| CompileError::EmitError(e.to_string()))?;
            }
        }
        // Chapter 2: 条件付きバイト設定（論理否定 ! で使用）
        // SetCC は 8ビットレジスタに対して動作するため、%al を使う
        Instruction::SetCC { condition, operand } => {
            let suffix = format_condition(condition);
            writeln!(out, "    set{} {}", suffix, format_operand_byte(operand))
                .map_err(|e| CompileError::EmitError(e.to_string()))?;
        }
        // Chapter 3/13: 二項演算命令
        Instruction::Binary {
            asm_type,
            op,
            src,
            dst,
        } => {
            if *asm_type == AsmType::Double {
                let mnemonic = match op {
                    AsmBinaryOp::Add => "addsd",
                    AsmBinaryOp::Sub => "subsd",
                    AsmBinaryOp::Mult => "mulsd",
                    AsmBinaryOp::DivDouble => "divsd",
                    AsmBinaryOp::Xor => "xorpd",
                    AsmBinaryOp::And
                    | AsmBinaryOp::Or
                    | AsmBinaryOp::BitXor
                    | AsmBinaryOp::Sal
                    | AsmBinaryOp::Sar
                    | AsmBinaryOp::Shr => unreachable!("bitwise/shift ops not for Double type"),
                };
                writeln!(
                    out,
                    "    {mnemonic} {}, {}",
                    format_operand_typed(src, asm_type),
                    format_operand_typed(dst, asm_type)
                )
                .map_err(|e| CompileError::EmitError(e.to_string()))?;
            } else if *asm_type == AsmType::Float {
                let mnemonic = match op {
                    AsmBinaryOp::Add => "addss",
                    AsmBinaryOp::Sub => "subss",
                    AsmBinaryOp::Mult => "mulss",
                    AsmBinaryOp::DivDouble => "divss",
                    AsmBinaryOp::Xor => "xorps",
                    AsmBinaryOp::And
                    | AsmBinaryOp::Or
                    | AsmBinaryOp::BitXor
                    | AsmBinaryOp::Sal
                    | AsmBinaryOp::Sar
                    | AsmBinaryOp::Shr => unreachable!("bitwise/shift ops not for Float type"),
                };
                writeln!(
                    out,
                    "    {mnemonic} {}, {}",
                    format_operand_typed(src, asm_type),
                    format_operand_typed(dst, asm_type)
                )
                .map_err(|e| CompileError::EmitError(e.to_string()))?;
            } else {
                // Shift instructions need %cl as source
                let is_shift = matches!(op, AsmBinaryOp::Sal | AsmBinaryOp::Sar | AsmBinaryOp::Shr);
                let base = match op {
                    AsmBinaryOp::Add => "add",
                    AsmBinaryOp::Sub => "sub",
                    AsmBinaryOp::Mult => "imul",
                    AsmBinaryOp::And => "and",
                    AsmBinaryOp::Or => "or",
                    AsmBinaryOp::BitXor => "xor",
                    AsmBinaryOp::Sal => "sal",
                    AsmBinaryOp::Sar => "sar",
                    AsmBinaryOp::Shr => "shr",
                    AsmBinaryOp::DivDouble => unreachable!("DivDouble only for Double type"),
                    AsmBinaryOp::Xor => unreachable!("Xor only for Double type"),
                };
                let suffix = type_suffix(asm_type);
                // For shift instructions, source operand must be %cl (byte register)
                let src_str = if is_shift {
                    format_operand_byte(src)
                } else {
                    format_operand_typed(src, asm_type)
                };
                writeln!(
                    out,
                    "    {base}{suffix} {}, {}",
                    src_str,
                    format_operand_typed(dst, asm_type)
                )
                .map_err(|e| CompileError::EmitError(e.to_string()))?;
            }
        }
        // Chapter 3: 符号付き除算
        Instruction::Idiv { asm_type, operand } => {
            let suffix = type_suffix(asm_type);
            writeln!(
                out,
                "    idiv{suffix} {}",
                format_operand_typed(operand, asm_type)
            )
            .map_err(|e| CompileError::EmitError(e.to_string()))?;
        }
        // Chapter 12: 符号なし除算
        Instruction::Div { asm_type, operand } => {
            let suffix = type_suffix(asm_type);
            writeln!(
                out,
                "    div{suffix} {}",
                format_operand_typed(operand, asm_type)
            )
            .map_err(|e| CompileError::EmitError(e.to_string()))?;
        }
        // Chapter 3/11: 符号拡張（cdq for Longword, cqo for Quadword）
        Instruction::SignExtend(asm_type) => {
            let mnemonic = match asm_type {
                AsmType::Longword => "cdq",
                AsmType::Quadword => "cqo",
                AsmType::Byte | AsmType::Word | AsmType::Double | AsmType::Float => {
                    unreachable!("SignExtend not applicable to {:?}", asm_type)
                }
            };
            writeln!(out, "    {mnemonic}").map_err(|e| CompileError::EmitError(e.to_string()))?;
        }
        // Chapter 11: int → long 符号拡張（movslq）
        Instruction::Movsx { src, dst } => {
            writeln!(
                out,
                "    movslq {}, {}",
                format_operand(src),
                format_operand_quad(dst)
            )
            .map_err(|e| CompileError::EmitError(e.to_string()))?;
        }
        // Chapter 11: long → int 切り詰め（32ビット mov で上位ビットをゼロクリア）
        Instruction::Truncate { src, dst } => {
            writeln!(
                out,
                "    movl {}, {}",
                format_operand(src),
                format_operand(dst)
            )
            .map_err(|e| CompileError::EmitError(e.to_string()))?;
        }
        // Chapter 12: 32→64 ゼロ拡張（movl で上位32ビットを自動ゼロクリア）
        Instruction::MovZeroExtend { src, dst } => {
            writeln!(
                out,
                "    movl {}, {}",
                format_operand(src),
                format_operand(dst)
            )
            .map_err(|e| CompileError::EmitError(e.to_string()))?;
        }
        // Chapter 16: byte → int/long 符号拡張（movsbl/movsbq）
        Instruction::MovsxByte { asm_type, src, dst } => {
            let suffix = type_suffix(asm_type);
            writeln!(
                out,
                "    movsb{suffix} {}, {}",
                format_operand_byte(src),
                format_operand_typed(dst, asm_type)
            )
            .map_err(|e| CompileError::EmitError(e.to_string()))?;
        }
        // Chapter 16: byte → int/long ゼロ拡張（movzbl/movzbq）
        Instruction::MovZeroExtendByte { asm_type, src, dst } => {
            let suffix = type_suffix(asm_type);
            writeln!(
                out,
                "    movzb{suffix} {}, {}",
                format_operand_byte(src),
                format_operand_typed(dst, asm_type)
            )
            .map_err(|e| CompileError::EmitError(e.to_string()))?;
        }
        // word → int/long 符号拡張（movswl/movswq）
        Instruction::MovsxWord { asm_type, src, dst } => {
            let suffix = type_suffix(asm_type);
            writeln!(
                out,
                "    movsw{suffix} {}, {}",
                format_operand_word(src),
                format_operand_typed(dst, asm_type)
            )
            .map_err(|e| CompileError::EmitError(e.to_string()))?;
        }
        // word → int/long ゼロ拡張（movzwl/movzwq）
        Instruction::MovZeroExtendWord { asm_type, src, dst } => {
            let suffix = type_suffix(asm_type);
            writeln!(
                out,
                "    movzw{suffix} {}, {}",
                format_operand_word(src),
                format_operand_typed(dst, asm_type)
            )
            .map_err(|e| CompileError::EmitError(e.to_string()))?;
        }
        // Chapter 3/13: スタックにプッシュ
        // XMM レジスタは push できないので subq + movsd で代替
        Instruction::Push(operand) => {
            if let Operand::Register(reg) = operand {
                if is_xmm_register(reg) {
                    writeln!(out, "    subq $8, %rsp")
                        .map_err(|e| CompileError::EmitError(e.to_string()))?;
                    writeln!(out, "    movsd {}, (%rsp)", format_register_xmm(reg))
                        .map_err(|e| CompileError::EmitError(e.to_string()))?;
                } else {
                    writeln!(out, "    push {}", format_operand_quad(operand))
                        .map_err(|e| CompileError::EmitError(e.to_string()))?;
                }
            } else {
                writeln!(out, "    push {}", format_operand_quad(operand))
                    .map_err(|e| CompileError::EmitError(e.to_string()))?;
            }
        }
        // Chapter 3/13: スタックからポップ
        // XMM レジスタは pop できないので movsd + addq で代替
        Instruction::Pop(operand) => {
            if let Operand::Register(reg) = operand {
                if is_xmm_register(reg) {
                    writeln!(out, "    movsd (%rsp), {}", format_register_xmm(reg))
                        .map_err(|e| CompileError::EmitError(e.to_string()))?;
                    writeln!(out, "    addq $8, %rsp")
                        .map_err(|e| CompileError::EmitError(e.to_string()))?;
                } else {
                    writeln!(out, "    pop {}", format_operand_quad(operand))
                        .map_err(|e| CompileError::EmitError(e.to_string()))?;
                }
            } else {
                writeln!(out, "    pop {}", format_operand_quad(operand))
                    .map_err(|e| CompileError::EmitError(e.to_string()))?;
            }
        }
        // Chapter 4: 無条件ジャンプ
        Instruction::Jmp(label) => {
            writeln!(out, "    jmp {label}").map_err(|e| CompileError::EmitError(e.to_string()))?;
        }
        // Chapter 4: 条件ジャンプ
        Instruction::JmpCC(condition, label) => {
            let suffix = format_condition(condition);
            writeln!(out, "    j{suffix} {label}")
                .map_err(|e| CompileError::EmitError(e.to_string()))?;
        }
        // Chapter 4: ラベル定義
        Instruction::Label(label) => {
            writeln!(out, "{label}:").map_err(|e| CompileError::EmitError(e.to_string()))?;
        }
        // Chapter 5: スタック領域の確保
        Instruction::AllocateStack(size) => {
            writeln!(out, "    subq ${size}, %rsp")
                .map_err(|e| CompileError::EmitError(e.to_string()))?;
        }
        // Chapter 9: スタック領域の解放
        Instruction::DeallocateStack(size) => {
            writeln!(out, "    addq ${size}, %rsp")
                .map_err(|e| CompileError::EmitError(e.to_string()))?;
        }
        // Chapter 9: 関数呼び出し
        Instruction::Call(name) => {
            writeln!(out, "    call {name}").map_err(|e| CompileError::EmitError(e.to_string()))?;
        }
        // Chapter 13: 整数→double 変換
        Instruction::Cvtsi2sd { asm_type, src, dst } => {
            let suffix = type_suffix(asm_type);
            writeln!(
                out,
                "    cvtsi2sd{suffix} {}, {}",
                format_operand_typed(src, asm_type),
                format_operand_xmm(dst)
            )
            .map_err(|e| CompileError::EmitError(e.to_string()))?;
        }
        // Chapter 13: double→整数 変換（切り捨て）
        Instruction::Cvttsd2si { asm_type, src, dst } => {
            let suffix = type_suffix(asm_type);
            writeln!(
                out,
                "    cvttsd2si{suffix} {}, {}",
                format_operand_xmm(src),
                format_operand_typed(dst, asm_type)
            )
            .map_err(|e| CompileError::EmitError(e.to_string()))?;
        }
        // 整数→float 変換
        Instruction::Cvtsi2ss { asm_type, src, dst } => {
            let suffix = type_suffix(asm_type);
            writeln!(
                out,
                "    cvtsi2ss{suffix} {}, {}",
                format_operand_typed(src, asm_type),
                format_operand_xmm(dst)
            )
            .map_err(|e| CompileError::EmitError(e.to_string()))?;
        }
        // float→整数 変換（切り捨て）
        Instruction::Cvttss2si { asm_type, src, dst } => {
            let suffix = type_suffix(asm_type);
            writeln!(
                out,
                "    cvttss2si{suffix} {}, {}",
                format_operand_xmm(src),
                format_operand_typed(dst, asm_type)
            )
            .map_err(|e| CompileError::EmitError(e.to_string()))?;
        }
        // float→double 変換
        Instruction::Cvtss2sd { src, dst } => {
            writeln!(
                out,
                "    cvtss2sd {}, {}",
                format_operand_xmm(src),
                format_operand_xmm(dst)
            )
            .map_err(|e| CompileError::EmitError(e.to_string()))?;
        }
        // double→float 変換
        Instruction::Cvtsd2ss { src, dst } => {
            writeln!(
                out,
                "    cvtsd2ss {}, {}",
                format_operand_xmm(src),
                format_operand_xmm(dst)
            )
            .map_err(|e| CompileError::EmitError(e.to_string()))?;
        }
        // Chapter 14: LEA（実効アドレスロード）
        Instruction::Lea { src, dst } => {
            writeln!(
                out,
                "    leaq {}, {}",
                format_operand_quad(src),
                format_operand_quad(dst)
            )
            .map_err(|e| CompileError::EmitError(e.to_string()))?;
        }
        // Chapter 20: ret のみ出力。エピローグは fixup パスが Ret の前に挿入する。
        Instruction::Ret => {
            writeln!(out, "    ret").map_err(|e| CompileError::EmitError(e.to_string()))?;
        }

        // 間接ジャンプ（難読化の CFF ジャンプテーブル用）
        Instruction::JmpIndirect(operand, _) => {
            writeln!(out, "    jmp *{}", format_operand_quad(operand))
                .map_err(|e| CompileError::EmitError(e.to_string()))?;
        }

        // 間接関数呼び出し（難読化用）
        Instruction::CallIndirect(operand) => {
            writeln!(out, "    call *{}", format_operand_quad(operand))
                .map_err(|e| CompileError::EmitError(e.to_string()))?;
        }

        // 生バイト出力（反逆アセンブリ用）
        Instruction::RawBytes(bytes) => {
            let byte_strs: Vec<String> = bytes.iter().map(|b| format!("0x{b:02x}")).collect();
            writeln!(out, "    .byte {}", byte_strs.join(", "))
                .map_err(|e| CompileError::EmitError(e.to_string()))?;
        }
    }
    Ok(())
}

/// AsmType に対応する命令サフィックスを返す。
fn type_suffix(asm_type: &AsmType) -> &'static str {
    match asm_type {
        AsmType::Byte => "b",
        AsmType::Word => "w",
        AsmType::Longword => "l",
        AsmType::Quadword => "q",
        AsmType::Double => "sd",
        AsmType::Float => "ss",
    }
}

/// AsmType に応じたオペランドのフォーマット（32ビットまたは64ビット）。
fn format_operand_typed(operand: &Operand, asm_type: &AsmType) -> String {
    match asm_type {
        AsmType::Byte => format_operand_byte(operand),
        AsmType::Word => format_operand_word(operand),
        AsmType::Longword => format_operand(operand),
        AsmType::Quadword => format_operand_quad(operand),
        AsmType::Double => format_operand_xmm(operand),
        AsmType::Float => format_operand_xmm(operand),
    }
}

/// オペランドを AT&T 構文の文字列に変換する（32ビット）。
fn format_operand(operand: &Operand) -> String {
    match operand {
        Operand::Imm(value) => format!("${value}"),
        Operand::Register(reg) => format_register(reg).to_string(),
        Operand::Pseudo(name) => panic!("BUG: Pseudo '{}' survived to emission", name),
        Operand::Stack(offset) => format!("{offset}(%rbp)"),
        Operand::Data(name) => format!("{name}(%rip)"),
        Operand::Memory(reg) => format!("({})", format_register_quad(reg)),
        Operand::MemoryOffset(reg, offset) => format!("{offset}({})", format_register_quad(reg)),
    }
}

/// オペランドを AT&T 構文の文字列に変換する（8ビット版）。
///
/// `SetCC` 命令は8ビットレジスタに対して動作する。
/// `Reg::AX` → `%al`（EAX の下位8ビット）
fn format_operand_byte(operand: &Operand) -> String {
    match operand {
        Operand::Imm(value) => format!("${value}"),
        Operand::Register(reg) => format_register_byte(reg).to_string(),
        Operand::Pseudo(name) => panic!("BUG: Pseudo '{}' survived to emission", name),
        Operand::Stack(offset) => format!("{offset}(%rbp)"),
        Operand::Data(name) => format!("{name}(%rip)"),
        Operand::Memory(reg) => format!("({})", format_register_quad(reg)),
        Operand::MemoryOffset(reg, offset) => format!("{offset}({})", format_register_quad(reg)),
    }
}

/// オペランドを AT&T 構文の文字列に変換する（16ビット版）。
///
/// `Reg::AX` → `%ax`（EAX の下位16ビット）
fn format_operand_word(operand: &Operand) -> String {
    match operand {
        Operand::Imm(value) => format!("${value}"),
        Operand::Register(reg) => format_register_word(reg).to_string(),
        Operand::Pseudo(name) => panic!("BUG: Pseudo '{}' survived to emission", name),
        Operand::Stack(offset) => format!("{offset}(%rbp)"),
        Operand::Data(name) => format!("{name}(%rip)"),
        Operand::Memory(reg) => format!("({})", format_register_quad(reg)),
        Operand::MemoryOffset(reg, offset) => format!("{offset}({})", format_register_quad(reg)),
    }
}

/// オペランドを AT&T 構文の文字列に変換する（64ビット版）。
///
/// Quadword (64bit) 命令および push/pop で使用する。
fn format_operand_quad(operand: &Operand) -> String {
    match operand {
        Operand::Imm(value) => format!("${value}"),
        Operand::Register(reg) => format_register_quad(reg).to_string(),
        Operand::Pseudo(name) => panic!("BUG: Pseudo '{}' survived to emission", name),
        Operand::Stack(offset) => format!("{offset}(%rbp)"),
        Operand::Data(name) => format!("{name}(%rip)"),
        Operand::Memory(reg) => format!("({})", format_register_quad(reg)),
        Operand::MemoryOffset(reg, offset) => format!("{offset}({})", format_register_quad(reg)),
    }
}

/// オペランドを XMM レジスタ表記でフォーマット（Double 型用）。
fn format_operand_xmm(operand: &Operand) -> String {
    match operand {
        Operand::Imm(value) => format!("${value}"),
        Operand::Register(reg) => format_register_xmm(reg).to_string(),
        Operand::Pseudo(name) => panic!("BUG: Pseudo '{}' survived to emission", name),
        Operand::Stack(offset) => format!("{offset}(%rbp)"),
        Operand::Data(name) => format!("{name}(%rip)"),
        Operand::Memory(reg) => format!("({})", format_register_quad(reg)),
        Operand::MemoryOffset(reg, offset) => format!("{offset}({})", format_register_quad(reg)),
    }
}

/// XMM レジスタ名を返す。GPR の場合は64ビット名を返す（cvtsi2sd 等で使用）。
fn format_register_xmm(reg: &Reg) -> &'static str {
    match reg {
        Reg::XMM0 => "%xmm0",
        Reg::XMM1 => "%xmm1",
        Reg::XMM2 => "%xmm2",
        Reg::XMM3 => "%xmm3",
        Reg::XMM4 => "%xmm4",
        Reg::XMM5 => "%xmm5",
        Reg::XMM6 => "%xmm6",
        Reg::XMM7 => "%xmm7",
        Reg::XMM8 => "%xmm8",
        Reg::XMM9 => "%xmm9",
        Reg::XMM10 => "%xmm10",
        Reg::XMM11 => "%xmm11",
        Reg::XMM12 => "%xmm12",
        Reg::XMM13 => "%xmm13",
        Reg::XMM14 => "%xmm14",
        Reg::XMM15 => "%xmm15",
        // GPR は64ビット名（cvtsi2sd/cvttsd2si で使う場合）
        _ => format_register_quad(reg),
    }
}

/// レジスタ名を64ビット表記で返す（Quadword 命令、push/pop 命令用）。
fn format_register_quad(reg: &Reg) -> &'static str {
    match reg {
        Reg::AX => "%rax",
        Reg::BX => "%rbx",
        Reg::CX => "%rcx",
        Reg::DX => "%rdx",
        Reg::DI => "%rdi",
        Reg::SI => "%rsi",
        Reg::R8 => "%r8",
        Reg::R9 => "%r9",
        Reg::R10 => "%r10",
        Reg::R11 => "%r11",
        Reg::R12 => "%r12",
        Reg::R13 => "%r13",
        Reg::R14 => "%r14",
        Reg::R15 => "%r15",
        Reg::SP => "%rsp",
        Reg::BP => "%rbp",
        Reg::XMM0
        | Reg::XMM1
        | Reg::XMM2
        | Reg::XMM3
        | Reg::XMM4
        | Reg::XMM5
        | Reg::XMM6
        | Reg::XMM7
        | Reg::XMM8
        | Reg::XMM9
        | Reg::XMM10
        | Reg::XMM11
        | Reg::XMM12
        | Reg::XMM13
        | Reg::XMM14
        | Reg::XMM15 => format_register_xmm(reg),
    }
}

/// レジスタ名を32ビット表記で返す。
fn format_register(reg: &Reg) -> &'static str {
    match reg {
        Reg::AX => "%eax",
        Reg::BX => "%ebx",
        Reg::CX => "%ecx",
        Reg::DX => "%edx",
        Reg::DI => "%edi",
        Reg::SI => "%esi",
        Reg::R8 => "%r8d",
        Reg::R9 => "%r9d",
        Reg::R10 => "%r10d",
        Reg::R11 => "%r11d",
        Reg::R12 => "%r12d",
        Reg::R13 => "%r13d",
        Reg::R14 => "%r14d",
        Reg::R15 => "%r15d",
        Reg::SP => "%esp",
        Reg::BP => "%ebp",
        Reg::XMM0
        | Reg::XMM1
        | Reg::XMM2
        | Reg::XMM3
        | Reg::XMM4
        | Reg::XMM5
        | Reg::XMM6
        | Reg::XMM7
        | Reg::XMM8
        | Reg::XMM9
        | Reg::XMM10
        | Reg::XMM11
        | Reg::XMM12
        | Reg::XMM13
        | Reg::XMM14
        | Reg::XMM15 => format_register_xmm(reg),
    }
}

/// レジスタ名を8ビット表記で返す（SetCC 命令用）。
fn format_register_byte(reg: &Reg) -> &'static str {
    match reg {
        Reg::AX => "%al",
        Reg::BX => "%bl",
        Reg::CX => "%cl",
        Reg::DX => "%dl",
        Reg::DI => "%dil",
        Reg::SI => "%sil",
        Reg::R8 => "%r8b",
        Reg::R9 => "%r9b",
        Reg::R10 => "%r10b",
        Reg::R11 => "%r11b",
        Reg::R12 => "%r12b",
        Reg::R13 => "%r13b",
        Reg::R14 => "%r14b",
        Reg::R15 => "%r15b",
        Reg::SP => "%spl",
        Reg::BP => "%bpl",
        Reg::XMM0
        | Reg::XMM1
        | Reg::XMM2
        | Reg::XMM3
        | Reg::XMM4
        | Reg::XMM5
        | Reg::XMM6
        | Reg::XMM7
        | Reg::XMM8
        | Reg::XMM9
        | Reg::XMM10
        | Reg::XMM11
        | Reg::XMM12
        | Reg::XMM13
        | Reg::XMM14
        | Reg::XMM15 => format_register_xmm(reg),
    }
}

/// レジスタ名を16ビット表記で返す。
fn format_register_word(reg: &Reg) -> &'static str {
    match reg {
        Reg::AX => "%ax",
        Reg::BX => "%bx",
        Reg::CX => "%cx",
        Reg::DX => "%dx",
        Reg::DI => "%di",
        Reg::SI => "%si",
        Reg::R8 => "%r8w",
        Reg::R9 => "%r9w",
        Reg::R10 => "%r10w",
        Reg::R11 => "%r11w",
        Reg::R12 => "%r12w",
        Reg::R13 => "%r13w",
        Reg::R14 => "%r14w",
        Reg::R15 => "%r15w",
        Reg::SP => "%sp",
        Reg::BP => "%bp",
        Reg::XMM0
        | Reg::XMM1
        | Reg::XMM2
        | Reg::XMM3
        | Reg::XMM4
        | Reg::XMM5
        | Reg::XMM6
        | Reg::XMM7
        | Reg::XMM8
        | Reg::XMM9
        | Reg::XMM10
        | Reg::XMM11
        | Reg::XMM12
        | Reg::XMM13
        | Reg::XMM14
        | Reg::XMM15 => format_register_xmm(reg),
    }
}

/// XMM レジスタかどうかを判定する。
fn is_xmm_register(reg: &Reg) -> bool {
    matches!(
        reg,
        Reg::XMM0
            | Reg::XMM1
            | Reg::XMM2
            | Reg::XMM3
            | Reg::XMM4
            | Reg::XMM5
            | Reg::XMM6
            | Reg::XMM7
            | Reg::XMM8
            | Reg::XMM9
            | Reg::XMM10
            | Reg::XMM11
            | Reg::XMM12
            | Reg::XMM13
            | Reg::XMM14
            | Reg::XMM15
    )
}

/// 条件コードをアセンブリの接尾辞に変換する。
fn format_condition(cc: &CondCode) -> &'static str {
    match cc {
        CondCode::E => "e",
        CondCode::NE => "ne",
        CondCode::L => "l",
        CondCode::LE => "le",
        CondCode::G => "g",
        CondCode::GE => "ge",
        CondCode::A => "a",
        CondCode::AE => "ae",
        CondCode::B => "b",
        CondCode::BE => "be",
        CondCode::P => "p",
        CondCode::NP => "np",
    }
}

/// 文字列をアセンブリの `.asciz` ディレクティブ用にエスケープする。
///
/// 特殊文字をアセンブリのエスケープシーケンスに変換する。
fn escape_string_for_asm(s: &str) -> String {
    let mut result = String::with_capacity(s.len());
    for ch in s.chars() {
        match ch {
            '\\' => result.push_str("\\\\"),
            '"' => result.push_str("\\\""),
            '\n' => result.push_str("\\n"),
            '\t' => result.push_str("\\t"),
            '\r' => result.push_str("\\r"),
            '\0' => result.push_str("\\0"),
            '\x07' => result.push_str("\\a"),
            '\x08' => result.push_str("\\b"),
            '\x0c' => result.push_str("\\f"),
            '\x0b' => result.push_str("\\v"),
            c => result.push(c),
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::codegen::asm_ast::*;

    /// ヘルパー: テスト用の main 関数を含む AsmProgram を構築する
    fn test_program(instructions: Vec<Instruction>) -> AsmProgram {
        AsmProgram {
            functions: vec![AsmFunction {
                name: "main".to_string(),
                instructions,
                global: true,
            }],
            static_vars: vec![],
            static_constants: vec![],
        }
    }

    // ── ヘルパー: Longword (32ビット int) のショートカット ──
    const LW: AsmType = AsmType::Longword;

    /// Chapter 1/20: return 2 の出力確認（プロローグ/エピローグは fixup が生成するため emitter は出力しない）
    #[test]
    fn emit_return_constant() {
        let program = test_program(vec![
            Instruction::Push(Operand::Register(Reg::BP)),
            Instruction::Mov {
                asm_type: AsmType::Quadword,
                src: Operand::Register(Reg::SP),
                dst: Operand::Register(Reg::BP),
            },
            Instruction::Mov {
                asm_type: LW,
                src: Operand::Imm(2),
                dst: Operand::Register(Reg::AX),
            },
            Instruction::Pop(Operand::Register(Reg::BP)),
            Instruction::Ret,
        ]);
        let asm = emit(&program).unwrap();
        let expected = "    .globl main\nmain:\n    push %rbp\n    movq %rsp, %rbp\n    movl $2, %eax\n    pop %rbp\n    ret\n    .section .ferrugo_sig,\"a\",@note\n    .byte 0x46,0x45,0x52,0x52,0x55,0x47,0x4f\n    .byte 0x01,0x00\n    .section .note.GNU-stack,\"\",@progbits\n";
        assert_eq!(asm, expected);
    }

    /// Chapter 2: return -5 のアセンブリ出力
    #[test]
    fn emit_negation() {
        let program = test_program(vec![
            Instruction::Mov {
                asm_type: LW,
                src: Operand::Imm(5),
                dst: Operand::Register(Reg::AX),
            },
            Instruction::Unary {
                asm_type: LW,
                op: AsmUnaryOp::Neg,
                operand: Operand::Register(Reg::AX),
            },
            Instruction::Ret,
        ]);
        let asm = emit(&program).unwrap();
        assert!(asm.contains("negl %eax"));
    }

    /// Chapter 2: return ~0 のアセンブリ出力
    #[test]
    fn emit_complement() {
        let program = test_program(vec![
            Instruction::Mov {
                asm_type: LW,
                src: Operand::Imm(0),
                dst: Operand::Register(Reg::AX),
            },
            Instruction::Unary {
                asm_type: LW,
                op: AsmUnaryOp::Not,
                operand: Operand::Register(Reg::AX),
            },
            Instruction::Ret,
        ]);
        let asm = emit(&program).unwrap();
        assert!(asm.contains("notl %eax"));
    }

    /// Chapter 2: return !1 のアセンブリ出力（cmpl + movl + sete パターン）
    #[test]
    fn emit_logical_not() {
        let program = test_program(vec![
            Instruction::Mov {
                asm_type: LW,
                src: Operand::Imm(1),
                dst: Operand::Register(Reg::AX),
            },
            Instruction::Cmp {
                asm_type: LW,
                src: Operand::Imm(0),
                dst: Operand::Register(Reg::AX),
            },
            Instruction::Mov {
                asm_type: LW,
                src: Operand::Imm(0),
                dst: Operand::Register(Reg::AX),
            },
            Instruction::SetCC {
                condition: CondCode::E,
                operand: Operand::Register(Reg::AX),
            },
            Instruction::Ret,
        ]);
        let asm = emit(&program).unwrap();
        assert!(asm.contains("cmpl $0, %eax"));
        assert!(asm.contains("sete %al")); // 8ビットレジスタ名になる
    }

    // ── Chapter 3 テスト ──

    /// Chapter 3: return 1 + 2 のアセンブリ出力
    #[test]
    fn emit_addition() {
        let program = test_program(vec![
            Instruction::Mov {
                asm_type: LW,
                src: Operand::Imm(1),
                dst: Operand::Register(Reg::AX),
            },
            Instruction::Push(Operand::Register(Reg::AX)),
            Instruction::Mov {
                asm_type: LW,
                src: Operand::Imm(2),
                dst: Operand::Register(Reg::AX),
            },
            Instruction::Pop(Operand::Register(Reg::CX)),
            Instruction::Binary {
                asm_type: LW,
                op: AsmBinaryOp::Add,
                src: Operand::Register(Reg::CX),
                dst: Operand::Register(Reg::AX),
            },
            Instruction::Ret,
        ]);
        let asm = emit(&program).unwrap();
        assert!(asm.contains("push %rax"));
        assert!(asm.contains("pop %rcx"));
        assert!(asm.contains("addl %ecx, %eax"));
    }

    /// Chapter 3: return 7 / 2 のアセンブリ出力
    #[test]
    fn emit_division() {
        let program = test_program(vec![
            Instruction::Mov {
                asm_type: LW,
                src: Operand::Imm(7),
                dst: Operand::Register(Reg::AX),
            },
            Instruction::Push(Operand::Register(Reg::AX)),
            Instruction::Mov {
                asm_type: LW,
                src: Operand::Imm(2),
                dst: Operand::Register(Reg::AX),
            },
            Instruction::Mov {
                asm_type: LW,
                src: Operand::Register(Reg::AX),
                dst: Operand::Register(Reg::CX),
            },
            Instruction::Pop(Operand::Register(Reg::AX)),
            Instruction::SignExtend(LW),
            Instruction::Idiv {
                asm_type: LW,
                operand: Operand::Register(Reg::CX),
            },
            Instruction::Ret,
        ]);
        let asm = emit(&program).unwrap();
        assert!(asm.contains("movl %eax, %ecx"));
        assert!(asm.contains("cdq"));
        assert!(asm.contains("idivl %ecx"));
    }

    // ── Chapter 4 テスト ──

    /// Chapter 4: return 1 < 2 のアセンブリ出力（cmpl + setl パターン）
    #[test]
    fn emit_less_than() {
        let program = test_program(vec![
            Instruction::Mov {
                asm_type: LW,
                src: Operand::Imm(1),
                dst: Operand::Register(Reg::AX),
            },
            Instruction::Push(Operand::Register(Reg::AX)),
            Instruction::Mov {
                asm_type: LW,
                src: Operand::Imm(2),
                dst: Operand::Register(Reg::AX),
            },
            Instruction::Pop(Operand::Register(Reg::CX)),
            Instruction::Cmp {
                asm_type: LW,
                src: Operand::Register(Reg::AX),
                dst: Operand::Register(Reg::CX),
            },
            Instruction::Mov {
                asm_type: LW,
                src: Operand::Imm(0),
                dst: Operand::Register(Reg::AX),
            },
            Instruction::SetCC {
                condition: CondCode::L,
                operand: Operand::Register(Reg::AX),
            },
            Instruction::Ret,
        ]);
        let asm = emit(&program).unwrap();
        assert!(asm.contains("cmpl %eax, %ecx"));
        assert!(asm.contains("setl %al"));
    }

    /// Chapter 4: 論理ANDの短絡評価（ジャンプ + ラベル）
    #[test]
    fn emit_logical_and() {
        let program = test_program(vec![
            Instruction::Mov {
                asm_type: LW,
                src: Operand::Imm(1),
                dst: Operand::Register(Reg::AX),
            },
            Instruction::Cmp {
                asm_type: LW,
                src: Operand::Imm(0),
                dst: Operand::Register(Reg::AX),
            },
            Instruction::JmpCC(CondCode::E, ".Land_false0".to_string()),
            Instruction::Mov {
                asm_type: LW,
                src: Operand::Imm(2),
                dst: Operand::Register(Reg::AX),
            },
            Instruction::Cmp {
                asm_type: LW,
                src: Operand::Imm(0),
                dst: Operand::Register(Reg::AX),
            },
            Instruction::JmpCC(CondCode::E, ".Land_false0".to_string()),
            Instruction::Mov {
                asm_type: LW,
                src: Operand::Imm(1),
                dst: Operand::Register(Reg::AX),
            },
            Instruction::Jmp(".Land_end0".to_string()),
            Instruction::Label(".Land_false0".to_string()),
            Instruction::Mov {
                asm_type: LW,
                src: Operand::Imm(0),
                dst: Operand::Register(Reg::AX),
            },
            Instruction::Label(".Land_end0".to_string()),
            Instruction::Ret,
        ]);
        let asm = emit(&program).unwrap();
        assert!(asm.contains("je .Land_false0"));
        assert!(asm.contains("jmp .Land_end0"));
        assert!(asm.contains(".Land_false0:"));
        assert!(asm.contains(".Land_end0:"));
    }

    /// Chapter 4: 論理ORの短絡評価
    #[test]
    fn emit_logical_or() {
        let program = test_program(vec![
            Instruction::Mov {
                asm_type: LW,
                src: Operand::Imm(0),
                dst: Operand::Register(Reg::AX),
            },
            Instruction::Cmp {
                asm_type: LW,
                src: Operand::Imm(0),
                dst: Operand::Register(Reg::AX),
            },
            Instruction::JmpCC(CondCode::NE, ".Lor_true0".to_string()),
            Instruction::Mov {
                asm_type: LW,
                src: Operand::Imm(3),
                dst: Operand::Register(Reg::AX),
            },
            Instruction::Cmp {
                asm_type: LW,
                src: Operand::Imm(0),
                dst: Operand::Register(Reg::AX),
            },
            Instruction::JmpCC(CondCode::NE, ".Lor_true0".to_string()),
            Instruction::Mov {
                asm_type: LW,
                src: Operand::Imm(0),
                dst: Operand::Register(Reg::AX),
            },
            Instruction::Jmp(".Lor_end0".to_string()),
            Instruction::Label(".Lor_true0".to_string()),
            Instruction::Mov {
                asm_type: LW,
                src: Operand::Imm(1),
                dst: Operand::Register(Reg::AX),
            },
            Instruction::Label(".Lor_end0".to_string()),
            Instruction::Ret,
        ]);
        let asm = emit(&program).unwrap();
        assert!(asm.contains("jne .Lor_true0"));
        assert!(asm.contains("jmp .Lor_end0"));
        assert!(asm.contains(".Lor_true0:"));
        assert!(asm.contains(".Lor_end0:"));
    }

    /// Chapter 3: return 7 % 2 のアセンブリ出力（剰余: movl %edx, %eax が追加）
    #[test]
    fn emit_remainder() {
        let program = test_program(vec![
            Instruction::Mov {
                asm_type: LW,
                src: Operand::Imm(7),
                dst: Operand::Register(Reg::AX),
            },
            Instruction::Push(Operand::Register(Reg::AX)),
            Instruction::Mov {
                asm_type: LW,
                src: Operand::Imm(2),
                dst: Operand::Register(Reg::AX),
            },
            Instruction::Mov {
                asm_type: LW,
                src: Operand::Register(Reg::AX),
                dst: Operand::Register(Reg::CX),
            },
            Instruction::Pop(Operand::Register(Reg::AX)),
            Instruction::SignExtend(LW),
            Instruction::Idiv {
                asm_type: LW,
                operand: Operand::Register(Reg::CX),
            },
            Instruction::Mov {
                asm_type: LW,
                src: Operand::Register(Reg::DX),
                dst: Operand::Register(Reg::AX),
            },
            Instruction::Ret,
        ]);
        let asm = emit(&program).unwrap();
        assert!(asm.contains("idivl %ecx"));
        assert!(asm.contains("movl %edx, %eax"));
    }

    // ── Chapter 5 テスト ──

    /// Chapter 5/20: AllocateStack と Stack オペランドの出力
    /// プロローグ/エピローグは fixup パスが命令として挿入する。
    #[test]
    fn emit_allocate_stack_and_stack_operand() {
        let program = test_program(vec![
            Instruction::Push(Operand::Register(Reg::BP)),
            Instruction::Mov {
                asm_type: AsmType::Quadword,
                src: Operand::Register(Reg::SP),
                dst: Operand::Register(Reg::BP),
            },
            Instruction::AllocateStack(16),
            Instruction::Mov {
                asm_type: LW,
                src: Operand::Imm(5),
                dst: Operand::Register(Reg::AX),
            },
            Instruction::Mov {
                asm_type: LW,
                src: Operand::Register(Reg::AX),
                dst: Operand::Stack(-4),
            },
            Instruction::Mov {
                asm_type: LW,
                src: Operand::Stack(-4),
                dst: Operand::Register(Reg::AX),
            },
            Instruction::DeallocateStack(16),
            Instruction::Pop(Operand::Register(Reg::BP)),
            Instruction::Ret,
        ]);
        let asm = emit(&program).unwrap();
        assert!(asm.contains("subq $16, %rsp"));
        assert!(asm.contains("movl %eax, -4(%rbp)"));
        assert!(asm.contains("movl -4(%rbp), %eax"));
        assert!(asm.contains("push %rbp"));
        assert!(asm.contains("movq %rsp, %rbp"));
        assert!(asm.contains("addq $16, %rsp"));
        assert!(asm.contains("pop %rbp"));
    }

    /// Chapter 5/20: int a = 5; return a; の完全な出力
    /// fixup パスが生成するプロローグ/エピローグも命令として含める。
    #[test]
    fn emit_var_declaration_full() {
        let program = test_program(vec![
            Instruction::Push(Operand::Register(Reg::BP)),
            Instruction::Mov {
                asm_type: AsmType::Quadword,
                src: Operand::Register(Reg::SP),
                dst: Operand::Register(Reg::BP),
            },
            Instruction::AllocateStack(16),
            Instruction::Mov {
                asm_type: LW,
                src: Operand::Imm(5),
                dst: Operand::Register(Reg::AX),
            },
            Instruction::Mov {
                asm_type: LW,
                src: Operand::Register(Reg::AX),
                dst: Operand::Stack(-4),
            },
            Instruction::Mov {
                asm_type: LW,
                src: Operand::Stack(-4),
                dst: Operand::Register(Reg::AX),
            },
            Instruction::DeallocateStack(16),
            Instruction::Pop(Operand::Register(Reg::BP)),
            Instruction::Ret,
        ]);
        let asm = emit(&program).unwrap();
        let expected = "    .globl main\nmain:\n    push %rbp\n    movq %rsp, %rbp\n    subq $16, %rsp\n    movl $5, %eax\n    movl %eax, -4(%rbp)\n    movl -4(%rbp), %eax\n    addq $16, %rsp\n    pop %rbp\n    ret\n    .section .ferrugo_sig,\"a\",@note\n    .byte 0x46,0x45,0x52,0x52,0x55,0x47,0x4f\n    .byte 0x01,0x00\n    .section .note.GNU-stack,\"\",@progbits\n";
        assert_eq!(asm, expected);
    }

    // ── Chapter 10 テスト ──

    /// Chapter 10: static 関数は .globl を出力しない
    #[test]
    fn emit_static_function_no_globl() {
        let program = AsmProgram {
            functions: vec![AsmFunction {
                name: "helper".to_string(),
                instructions: vec![
                    Instruction::Mov {
                        asm_type: LW,
                        src: Operand::Imm(42),
                        dst: Operand::Register(Reg::AX),
                    },
                    Instruction::Ret,
                ],
                global: false,
            }],
            static_vars: vec![],
            static_constants: vec![],
        };
        let asm = emit(&program).unwrap();
        assert!(!asm.contains(".globl helper"));
        assert!(asm.contains("helper:"));
        assert!(asm.contains("movl $42, %eax"));
    }

    /// Chapter 10: 初期化済み静的変数は .data セクションに配置
    #[test]
    fn emit_initialized_static_var() {
        let program = AsmProgram {
            functions: vec![],
            static_vars: vec![AsmStaticVar {
                name: "x".to_string(),
                global: true,
                init: StaticInit::IntInit(5),
                asm_type: LW,
                var_type: None,
            }],
            static_constants: vec![],
        };
        let asm = emit(&program).unwrap();
        assert!(asm.contains("    .globl x"));
        assert!(asm.contains("    .data"));
        assert!(asm.contains("    .align 4"));
        assert!(asm.contains("x:"));
        assert!(asm.contains("    .long 5"));
    }

    /// Chapter 10: 未初期化静的変数は .bss セクションに配置
    #[test]
    fn emit_zero_initialized_static_var() {
        let program = AsmProgram {
            functions: vec![],
            static_vars: vec![AsmStaticVar {
                name: "y".to_string(),
                global: true,
                init: StaticInit::IntInit(0),
                asm_type: LW,
                var_type: None,
            }],
            static_constants: vec![],
        };
        let asm = emit(&program).unwrap();
        assert!(asm.contains("    .globl y"));
        assert!(asm.contains("    .bss"));
        assert!(asm.contains("    .align 4"));
        assert!(asm.contains("y:"));
        assert!(asm.contains("    .zero 4"));
    }

    /// Chapter 10: 内部リンケージ（static）変数は .globl なし
    #[test]
    fn emit_static_internal_linkage_var() {
        let program = AsmProgram {
            functions: vec![],
            static_vars: vec![AsmStaticVar {
                name: "c.0".to_string(),
                global: false,
                init: StaticInit::IntInit(0),
                asm_type: LW,
                var_type: None,
            }],
            static_constants: vec![],
        };
        let asm = emit(&program).unwrap();
        assert!(!asm.contains(".globl c.0"));
        assert!(asm.contains("c.0:"));
        assert!(asm.contains("    .zero 4"));
    }

    /// Chapter 10: Data オペランドが RIP相対アドレスで出力される
    #[test]
    fn emit_data_operand() {
        let program = test_program(vec![
            Instruction::Mov {
                asm_type: LW,
                src: Operand::Data("x".to_string()),
                dst: Operand::Register(Reg::AX),
            },
            Instruction::Ret,
        ]);
        let asm = emit(&program).unwrap();
        assert!(asm.contains("movl x(%rip), %eax"));
    }

    // ── Chapter 11 テスト ──

    /// Chapter 11: 64ビット mov (movq)
    #[test]
    fn emit_mov_quadword() {
        let program = test_program(vec![
            Instruction::Mov {
                asm_type: AsmType::Quadword,
                src: Operand::Imm(2147483648),
                dst: Operand::Register(Reg::AX),
            },
            Instruction::Ret,
        ]);
        let asm = emit(&program).unwrap();
        assert!(asm.contains("movq $2147483648, %rax"));
    }

    /// Chapter 11: movslq (int → long 符号拡張)
    #[test]
    fn emit_movsx() {
        let program = test_program(vec![
            Instruction::Movsx {
                src: Operand::Register(Reg::AX),
                dst: Operand::Register(Reg::AX),
            },
            Instruction::Ret,
        ]);
        let asm = emit(&program).unwrap();
        assert!(asm.contains("movslq %eax, %rax"));
    }

    /// Chapter 11: Truncate (long → int)
    #[test]
    fn emit_truncate() {
        let program = test_program(vec![
            Instruction::Truncate {
                src: Operand::Register(Reg::AX),
                dst: Operand::Register(Reg::AX),
            },
            Instruction::Ret,
        ]);
        let asm = emit(&program).unwrap();
        assert!(asm.contains("movl %eax, %eax"));
    }

    /// Chapter 11: cqo (Quadword 符号拡張)
    #[test]
    fn emit_sign_extend_quadword() {
        let program = test_program(vec![
            Instruction::SignExtend(AsmType::Quadword),
            Instruction::Ret,
        ]);
        let asm = emit(&program).unwrap();
        assert!(asm.contains("cqo"));
    }

    // ── Chapter 12 テスト ──

    /// Chapter 12: 符号なし除算 (divl)
    #[test]
    fn emit_unsigned_division() {
        let program = test_program(vec![
            Instruction::Mov {
                asm_type: LW,
                src: Operand::Imm(7),
                dst: Operand::Register(Reg::AX),
            },
            Instruction::Mov {
                asm_type: LW,
                src: Operand::Imm(0),
                dst: Operand::Register(Reg::DX),
            },
            Instruction::Div {
                asm_type: LW,
                operand: Operand::Register(Reg::CX),
            },
            Instruction::Ret,
        ]);
        let asm = emit(&program).unwrap();
        assert!(asm.contains("divl %ecx"));
    }

    /// Chapter 12: 符号なし除算 64ビット (divq)
    #[test]
    fn emit_unsigned_division_quadword() {
        let program = test_program(vec![
            Instruction::Mov {
                asm_type: AsmType::Quadword,
                src: Operand::Imm(0),
                dst: Operand::Register(Reg::DX),
            },
            Instruction::Div {
                asm_type: AsmType::Quadword,
                operand: Operand::Register(Reg::CX),
            },
            Instruction::Ret,
        ]);
        let asm = emit(&program).unwrap();
        assert!(asm.contains("divq %rcx"));
    }

    /// Chapter 12: ゼロ拡張 (MovZeroExtend)
    #[test]
    fn emit_zero_extend() {
        let program = test_program(vec![
            Instruction::MovZeroExtend {
                src: Operand::Register(Reg::AX),
                dst: Operand::Register(Reg::AX),
            },
            Instruction::Ret,
        ]);
        let asm = emit(&program).unwrap();
        assert!(asm.contains("movl %eax, %eax"));
    }

    /// Chapter 12: 符号なし比較条件コード
    #[test]
    fn emit_unsigned_condition_codes() {
        let program = test_program(vec![
            Instruction::SetCC {
                condition: CondCode::A,
                operand: Operand::Register(Reg::AX),
            },
            Instruction::SetCC {
                condition: CondCode::AE,
                operand: Operand::Register(Reg::AX),
            },
            Instruction::SetCC {
                condition: CondCode::B,
                operand: Operand::Register(Reg::AX),
            },
            Instruction::SetCC {
                condition: CondCode::BE,
                operand: Operand::Register(Reg::AX),
            },
            Instruction::JmpCC(CondCode::A, ".Ltest".to_string()),
            Instruction::JmpCC(CondCode::B, ".Ltest2".to_string()),
            Instruction::Ret,
        ]);
        let asm = emit(&program).unwrap();
        assert!(asm.contains("seta %al"));
        assert!(asm.contains("setae %al"));
        assert!(asm.contains("setb %al"));
        assert!(asm.contains("setbe %al"));
        assert!(asm.contains("ja .Ltest"));
        assert!(asm.contains("jb .Ltest2"));
    }

    /// Chapter 11: 64ビット静的変数
    #[test]
    fn emit_quadword_static_var() {
        let program = AsmProgram {
            functions: vec![],
            static_vars: vec![AsmStaticVar {
                name: "big".to_string(),
                global: true,
                init: StaticInit::IntInit(4294967296),
                asm_type: AsmType::Quadword,
                var_type: None,
            }],
            static_constants: vec![],
        };
        let asm = emit(&program).unwrap();
        assert!(asm.contains("    .align 8"));
        assert!(asm.contains("    .quad 4294967296"));
    }
}
