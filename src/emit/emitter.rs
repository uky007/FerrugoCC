//! アセンブリテキスト出力（Emitter）
//!
//! アセンブリ AST を AT&T 構文の x86-64 アセンブリテキストに変換する。
//! 出力は `.s` ファイルに書き込まれ、gcc（as + ld）でバイナリに変換される。
//!
//! # AT&T 構文のポイント
//! - オペランドの順序は `命令 src, dst`（Intel 構文と逆）
//! - 即値には `$` プレフィックス: `$42`
//! - レジスタには `%` プレフィックス: `%eax`
//! - 命令にはサイズサフィックス: `movl`（32ビット）、`sete`（8ビット）
//!
//! # .note.GNU-stack セクション
//! 出力末尾に `.section .note.GNU-stack,"",@progbits` を付加する。
//! これはスタックが実行不可であることをリンカに伝えるセキュリティ上の慣習。
//! なくても動作するが、セキュリティ警告が出る場合がある。

use std::fmt::Write;

use crate::error::{CompileError, Result};
use crate::codegen::asm_ast::{
    AsmProgram, AsmStaticVar, Instruction, Operand, Reg, AsmUnaryOp, AsmBinaryOp, CondCode,
};

/// アセンブリ AST をテキストに変換する（Chapter 10: 静的変数対応）。
pub fn emit(program: &AsmProgram) -> Result<String> {
    let mut out = String::new();
    for func in &program.functions {
        emit_function(&mut out, func)?;
    }
    for var in &program.static_vars {
        emit_static_var(&mut out, var)?;
    }
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
    writeln!(out, "{}:", func.name)
        .map_err(|e| CompileError::EmitError(e.to_string()))?;

    // Chapter 5: プロローグ（スタックフレームの設定）
    writeln!(out, "    pushq %rbp")
        .map_err(|e| CompileError::EmitError(e.to_string()))?;
    writeln!(out, "    movq %rsp, %rbp")
        .map_err(|e| CompileError::EmitError(e.to_string()))?;

    for instr in &func.instructions {
        emit_instruction(out, instr)?;
    }

    Ok(())
}

/// 静的変数のアセンブリ出力（Chapter 10）。
///
/// 初期値が 0 の場合は `.bss` セクション、非0 の場合は `.data` セクションに配置する。
/// `global` が true のとき `.globl` ディレクティブを出力する。
fn emit_static_var(out: &mut String, var: &AsmStaticVar) -> Result<()> {
    if var.global {
        writeln!(out, "    .globl {}", var.name)
            .map_err(|e| CompileError::EmitError(e.to_string()))?;
    }
    if var.init != 0 {
        writeln!(out, "    .data")
            .map_err(|e| CompileError::EmitError(e.to_string()))?;
        writeln!(out, "    .align 4")
            .map_err(|e| CompileError::EmitError(e.to_string()))?;
        writeln!(out, "{}:", var.name)
            .map_err(|e| CompileError::EmitError(e.to_string()))?;
        writeln!(out, "    .long {}", var.init)
            .map_err(|e| CompileError::EmitError(e.to_string()))?;
    } else {
        writeln!(out, "    .bss")
            .map_err(|e| CompileError::EmitError(e.to_string()))?;
        writeln!(out, "    .align 4")
            .map_err(|e| CompileError::EmitError(e.to_string()))?;
        writeln!(out, "{}:", var.name)
            .map_err(|e| CompileError::EmitError(e.to_string()))?;
        writeln!(out, "    .zero 4")
            .map_err(|e| CompileError::EmitError(e.to_string()))?;
    }
    Ok(())
}

/// 個々の命令をテキスト行に変換する。
fn emit_instruction(out: &mut String, instr: &Instruction) -> Result<()> {
    match instr {
        Instruction::Mov { src, dst } => {
            writeln!(out, "    movl {}, {}", format_operand(src), format_operand(dst))
                .map_err(|e| CompileError::EmitError(e.to_string()))?;
        }
        // Chapter 2: 単項演算命令
        Instruction::Unary { op, operand } => {
            let mnemonic = match op {
                AsmUnaryOp::Neg => "negl",
                AsmUnaryOp::Not => "notl",
            };
            writeln!(out, "    {} {}", mnemonic, format_operand(operand))
                .map_err(|e| CompileError::EmitError(e.to_string()))?;
        }
        // Chapter 2: 比較命令（論理否定 ! で使用）
        Instruction::Cmp { src, dst } => {
            writeln!(out, "    cmpl {}, {}", format_operand(src), format_operand(dst))
                .map_err(|e| CompileError::EmitError(e.to_string()))?;
        }
        // Chapter 2: 条件付きバイト設定（論理否定 ! で使用）
        // SetCC は 8ビットレジスタに対して動作するため、%al を使う
        Instruction::SetCC { condition, operand } => {
            let suffix = format_condition(condition);
            writeln!(out, "    set{} {}", suffix, format_operand_byte(operand))
                .map_err(|e| CompileError::EmitError(e.to_string()))?;
        }
        // Chapter 3: 二項演算命令
        Instruction::Binary { op, src, dst } => {
            let mnemonic = match op {
                AsmBinaryOp::Add => "addl",
                AsmBinaryOp::Sub => "subl",
                AsmBinaryOp::Mult => "imull",
            };
            writeln!(out, "    {} {}, {}", mnemonic, format_operand(src), format_operand(dst))
                .map_err(|e| CompileError::EmitError(e.to_string()))?;
        }
        // Chapter 3: 符号付き除算
        Instruction::Idiv(operand) => {
            writeln!(out, "    idivl {}", format_operand(operand))
                .map_err(|e| CompileError::EmitError(e.to_string()))?;
        }
        // Chapter 3: EAX → EDX:EAX 符号拡張
        Instruction::Cdq => {
            writeln!(out, "    cdq")
                .map_err(|e| CompileError::EmitError(e.to_string()))?;
        }
        // Chapter 3: スタックにプッシュ
        // x86-64 では push/pop は64ビットレジスタを使う
        Instruction::Push(operand) => {
            writeln!(out, "    push {}", format_operand_quad(operand))
                .map_err(|e| CompileError::EmitError(e.to_string()))?;
        }
        // Chapter 3: スタックからポップ
        Instruction::Pop(operand) => {
            writeln!(out, "    pop {}", format_operand_quad(operand))
                .map_err(|e| CompileError::EmitError(e.to_string()))?;
        }
        // Chapter 4: 無条件ジャンプ
        Instruction::Jmp(label) => {
            writeln!(out, "    jmp {label}")
                .map_err(|e| CompileError::EmitError(e.to_string()))?;
        }
        // Chapter 4: 条件ジャンプ
        Instruction::JmpCC(condition, label) => {
            let suffix = format_condition(condition);
            writeln!(out, "    j{suffix} {label}")
                .map_err(|e| CompileError::EmitError(e.to_string()))?;
        }
        // Chapter 4: ラベル定義
        Instruction::Label(label) => {
            writeln!(out, "{label}:")
                .map_err(|e| CompileError::EmitError(e.to_string()))?;
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
            writeln!(out, "    call {name}")
                .map_err(|e| CompileError::EmitError(e.to_string()))?;
        }
        // Chapter 5: ret はエピローグを含む
        Instruction::Ret => {
            writeln!(out, "    movq %rbp, %rsp")
                .map_err(|e| CompileError::EmitError(e.to_string()))?;
            writeln!(out, "    popq %rbp")
                .map_err(|e| CompileError::EmitError(e.to_string()))?;
            writeln!(out, "    ret")
                .map_err(|e| CompileError::EmitError(e.to_string()))?;
        }
    }
    Ok(())
}

/// オペランドを AT&T 構文の文字列に変換する（32ビット）。
fn format_operand(operand: &Operand) -> String {
    match operand {
        Operand::Imm(value) => format!("${value}"),
        Operand::Register(reg) => format_register(reg).to_string(),
        Operand::Stack(offset) => format!("{offset}(%rbp)"),
        Operand::Data(name) => format!("{name}(%rip)"),
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
        Operand::Stack(offset) => format!("{offset}(%rbp)"),
        Operand::Data(name) => format!("{name}(%rip)"),
    }
}

/// オペランドを AT&T 構文の文字列に変換する（64ビット版）。
///
/// x86-64 の `push`/`pop` は 64ビットレジスタを使用する。
fn format_operand_quad(operand: &Operand) -> String {
    match operand {
        Operand::Imm(value) => format!("${value}"),
        Operand::Register(reg) => format_register_quad(reg).to_string(),
        Operand::Stack(offset) => format!("{offset}(%rbp)"),
        Operand::Data(name) => format!("{name}(%rip)"),
    }
}

/// レジスタ名を64ビット表記で返す（push/pop 命令用）。
fn format_register_quad(reg: &Reg) -> &'static str {
    match reg {
        Reg::AX => "%rax",
        Reg::CX => "%rcx",
        Reg::DX => "%rdx",
        Reg::DI => "%rdi",
        Reg::SI => "%rsi",
        Reg::R8 => "%r8",
        Reg::R9 => "%r9",
    }
}

/// レジスタ名を32ビット表記で返す。
fn format_register(reg: &Reg) -> &'static str {
    match reg {
        Reg::AX => "%eax",
        Reg::CX => "%ecx",
        Reg::DX => "%edx",
        Reg::DI => "%edi",
        Reg::SI => "%esi",
        Reg::R8 => "%r8d",
        Reg::R9 => "%r9d",
    }
}

/// レジスタ名を8ビット表記で返す（SetCC 命令用）。
fn format_register_byte(reg: &Reg) -> &'static str {
    match reg {
        Reg::AX => "%al",
        Reg::CX => "%cl",
        Reg::DX => "%dl",
        Reg::DI => "%dil",
        Reg::SI => "%sil",
        Reg::R8 => "%r8b",
        Reg::R9 => "%r9b",
    }
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
    }
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
        }
    }

    /// Chapter 1: return 2 の出力確認（プロローグ/エピローグ付き）
    #[test]
    fn emit_return_constant() {
        let program = test_program(vec![
            Instruction::Mov {
                src: Operand::Imm(2),
                dst: Operand::Register(Reg::AX),
            },
            Instruction::Ret,
        ]);
        let asm = emit(&program).unwrap();
        let expected = "    .globl main\nmain:\n    pushq %rbp\n    movq %rsp, %rbp\n    movl $2, %eax\n    movq %rbp, %rsp\n    popq %rbp\n    ret\n    .section .note.GNU-stack,\"\",@progbits\n";
        assert_eq!(asm, expected);
    }

    /// Chapter 2: return -5 のアセンブリ出力
    #[test]
    fn emit_negation() {
        let program = test_program(vec![
            Instruction::Mov {
                src: Operand::Imm(5),
                dst: Operand::Register(Reg::AX),
            },
            Instruction::Unary {
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
                src: Operand::Imm(0),
                dst: Operand::Register(Reg::AX),
            },
            Instruction::Unary {
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
                src: Operand::Imm(1),
                dst: Operand::Register(Reg::AX),
            },
            Instruction::Cmp {
                src: Operand::Imm(0),
                dst: Operand::Register(Reg::AX),
            },
            Instruction::Mov {
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
        assert!(asm.contains("sete %al"));  // 8ビットレジスタ名になる
    }

    // ── Chapter 3 テスト ──

    /// Chapter 3: return 1 + 2 のアセンブリ出力
    #[test]
    fn emit_addition() {
        let program = test_program(vec![
            Instruction::Mov { src: Operand::Imm(1), dst: Operand::Register(Reg::AX) },
            Instruction::Push(Operand::Register(Reg::AX)),
            Instruction::Mov { src: Operand::Imm(2), dst: Operand::Register(Reg::AX) },
            Instruction::Pop(Operand::Register(Reg::CX)),
            Instruction::Binary {
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
            Instruction::Mov { src: Operand::Imm(7), dst: Operand::Register(Reg::AX) },
            Instruction::Push(Operand::Register(Reg::AX)),
            Instruction::Mov { src: Operand::Imm(2), dst: Operand::Register(Reg::AX) },
            Instruction::Mov { src: Operand::Register(Reg::AX), dst: Operand::Register(Reg::CX) },
            Instruction::Pop(Operand::Register(Reg::AX)),
            Instruction::Cdq,
            Instruction::Idiv(Operand::Register(Reg::CX)),
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
            Instruction::Mov { src: Operand::Imm(1), dst: Operand::Register(Reg::AX) },
            Instruction::Push(Operand::Register(Reg::AX)),
            Instruction::Mov { src: Operand::Imm(2), dst: Operand::Register(Reg::AX) },
            Instruction::Pop(Operand::Register(Reg::CX)),
            Instruction::Cmp { src: Operand::Register(Reg::AX), dst: Operand::Register(Reg::CX) },
            Instruction::Mov { src: Operand::Imm(0), dst: Operand::Register(Reg::AX) },
            Instruction::SetCC { condition: CondCode::L, operand: Operand::Register(Reg::AX) },
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
            Instruction::Mov { src: Operand::Imm(1), dst: Operand::Register(Reg::AX) },
            Instruction::Cmp { src: Operand::Imm(0), dst: Operand::Register(Reg::AX) },
            Instruction::JmpCC(CondCode::E, ".Land_false0".to_string()),
            Instruction::Mov { src: Operand::Imm(2), dst: Operand::Register(Reg::AX) },
            Instruction::Cmp { src: Operand::Imm(0), dst: Operand::Register(Reg::AX) },
            Instruction::JmpCC(CondCode::E, ".Land_false0".to_string()),
            Instruction::Mov { src: Operand::Imm(1), dst: Operand::Register(Reg::AX) },
            Instruction::Jmp(".Land_end0".to_string()),
            Instruction::Label(".Land_false0".to_string()),
            Instruction::Mov { src: Operand::Imm(0), dst: Operand::Register(Reg::AX) },
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
            Instruction::Mov { src: Operand::Imm(0), dst: Operand::Register(Reg::AX) },
            Instruction::Cmp { src: Operand::Imm(0), dst: Operand::Register(Reg::AX) },
            Instruction::JmpCC(CondCode::NE, ".Lor_true0".to_string()),
            Instruction::Mov { src: Operand::Imm(3), dst: Operand::Register(Reg::AX) },
            Instruction::Cmp { src: Operand::Imm(0), dst: Operand::Register(Reg::AX) },
            Instruction::JmpCC(CondCode::NE, ".Lor_true0".to_string()),
            Instruction::Mov { src: Operand::Imm(0), dst: Operand::Register(Reg::AX) },
            Instruction::Jmp(".Lor_end0".to_string()),
            Instruction::Label(".Lor_true0".to_string()),
            Instruction::Mov { src: Operand::Imm(1), dst: Operand::Register(Reg::AX) },
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
            Instruction::Mov { src: Operand::Imm(7), dst: Operand::Register(Reg::AX) },
            Instruction::Push(Operand::Register(Reg::AX)),
            Instruction::Mov { src: Operand::Imm(2), dst: Operand::Register(Reg::AX) },
            Instruction::Mov { src: Operand::Register(Reg::AX), dst: Operand::Register(Reg::CX) },
            Instruction::Pop(Operand::Register(Reg::AX)),
            Instruction::Cdq,
            Instruction::Idiv(Operand::Register(Reg::CX)),
            Instruction::Mov { src: Operand::Register(Reg::DX), dst: Operand::Register(Reg::AX) },
            Instruction::Ret,
        ]);
        let asm = emit(&program).unwrap();
        assert!(asm.contains("idivl %ecx"));
        assert!(asm.contains("movl %edx, %eax"));
    }

    // ── Chapter 5 テスト ──

    /// Chapter 5: AllocateStack と Stack オペランドの出力
    #[test]
    fn emit_allocate_stack_and_stack_operand() {
        let program = test_program(vec![
            Instruction::AllocateStack(4),
            Instruction::Mov { src: Operand::Imm(5), dst: Operand::Register(Reg::AX) },
            Instruction::Mov { src: Operand::Register(Reg::AX), dst: Operand::Stack(-4) },
            Instruction::Mov { src: Operand::Stack(-4), dst: Operand::Register(Reg::AX) },
            Instruction::Ret,
        ]);
        let asm = emit(&program).unwrap();
        assert!(asm.contains("subq $4, %rsp"));
        assert!(asm.contains("movl %eax, -4(%rbp)"));
        assert!(asm.contains("movl -4(%rbp), %eax"));
        // プロローグ
        assert!(asm.contains("pushq %rbp"));
        assert!(asm.contains("movq %rsp, %rbp"));
        // エピローグ (ret 命令に含まれる)
        assert!(asm.contains("movq %rbp, %rsp"));
        assert!(asm.contains("popq %rbp"));
    }

    /// Chapter 5: int a = 5; return a; の完全な出力
    #[test]
    fn emit_var_declaration_full() {
        let program = test_program(vec![
            Instruction::AllocateStack(4),
            Instruction::Mov { src: Operand::Imm(5), dst: Operand::Register(Reg::AX) },
            Instruction::Mov { src: Operand::Register(Reg::AX), dst: Operand::Stack(-4) },
            Instruction::Mov { src: Operand::Stack(-4), dst: Operand::Register(Reg::AX) },
            Instruction::Ret,
        ]);
        let asm = emit(&program).unwrap();
        let expected = "    .globl main\nmain:\n    pushq %rbp\n    movq %rsp, %rbp\n    subq $4, %rsp\n    movl $5, %eax\n    movl %eax, -4(%rbp)\n    movl -4(%rbp), %eax\n    movq %rbp, %rsp\n    popq %rbp\n    ret\n    .section .note.GNU-stack,\"\",@progbits\n";
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
                    Instruction::Mov { src: Operand::Imm(42), dst: Operand::Register(Reg::AX) },
                    Instruction::Ret,
                ],
                global: false,
            }],
            static_vars: vec![],
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
                init: 5,
            }],
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
                init: 0,
            }],
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
                init: 0,
            }],
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
            Instruction::Mov { src: Operand::Data("x".to_string()), dst: Operand::Register(Reg::AX) },
            Instruction::Ret,
        ]);
        let asm = emit(&program).unwrap();
        assert!(asm.contains("movl x(%rip), %eax"));
    }
}
