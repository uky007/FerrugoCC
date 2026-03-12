//! コード生成（Codegen）モジュール
//!
//! TACKY IR をアセンブリ AST に変換する。
//! 各 TACKY 命令を機械的に x86-64 の命令列にマッピングし、
//! レジスタ割り当て + fixup パスを経て有効なアセンブリを出力する。
//!
//! ```text
//! TackyProgram → [generate_with_pseudos] → Asm(Pseudo)
//!              → [regalloc] → Asm(Reg+Stack)
//!              → [fixup] → Asm(valid)
//!              → [ASM-level obfuscation] → AsmProgram
//! ```

pub mod asm_ast;
pub mod generator;
pub mod regalloc;

pub use asm_ast::{AsmFunction, AsmProgram};

use crate::error::Result;
use crate::obfuscation::ObfuscationConfig;
use crate::tacky::tacky_ast::TackyProgram;
use asm_ast::{AsmBinaryOp, AsmType, AsmUnaryOp, Instruction, Operand, Reg};

/// TACKY プログラムをアセンブリ AST に変換する（Chapter 20: レジスタ割り当て統合）。
///
/// `obf_config` が Some の場合、regalloc + fixup の後に ASM レベルの難読化パスを適用する:
/// - スタックフレーム難読化（偽スタックスロット挿入）
/// - レジスタシャッフル（dead mov 挿入）
/// - 命令置換（同等命令列への置換）
/// - 反逆アセンブリ（ゴミバイト挿入）
/// - 関数呼び出しの間接化
pub fn generate(
    program: &TackyProgram,
    obf_config: Option<&ObfuscationConfig>,
) -> Result<AsmProgram> {
    // 1. Pseudo 付きの Asm を生成
    let (results, static_vars, static_constants) = generator::generate(program)?;

    // 2. 各関数にレジスタ割り当て + fixup
    let mut functions = Vec::new();
    for result in results {
        let alloc = regalloc::allocate_registers(result.func.instructions, &result.var_types);
        let fixed_instructions = regalloc::fixup_instructions(
            alloc.instructions,
            alloc.spill_bytes,
            &alloc.callee_saved_used,
        );
        functions.push(AsmFunction {
            name: result.func.name,
            instructions: fixed_instructions,
            global: result.func.global,
        });
    }

    // 3. ASM レベル難読化（fixup 後に適用）
    if let Some(config) = obf_config {
        if config.stack_frame_obf {
            obfuscate_stack_frame(
                &mut functions,
                config.stack_frame_padding,
                config.stack_frame_fake_freq,
            );
        }
        if config.reg_shuffle {
            register_shuffle(&mut functions, config.reg_shuffle_freq);
        }
        if config.instr_subst {
            instruction_substitution(&mut functions, config.instr_subst_freq);
        }
        if config.anti_disassembly {
            insert_anti_disassembly(&mut functions);
        }
        if config.indirect_calls {
            let local_names: std::collections::HashSet<String> =
                functions.iter().map(|f| f.name.clone()).collect();
            indirect_calls(&mut functions, &local_names);
        }
    }

    Ok(AsmProgram {
        functions,
        static_vars,
        static_constants,
    })
}

/// スタックフレーム難読化: 偽のスタックスロットと偽の read/write 操作を挿入する。
///
/// デコンパイラ（Hex-Rays, Ghidra）がスタック変数を復元する際、
/// 偽のローカル変数を生成させてソースコード復元を困難にする。
///
/// 1. AllocateStack/DeallocateStack を拡張して dead スタックスロットを追加
/// 2. dead スロットへの偽の store/load 操作を N 命令ごとに挿入
fn obfuscate_stack_frame(functions: &mut [AsmFunction], num_fake_slots: usize, freq: usize) {
    let num_fake_slots = if num_fake_slots == 0 {
        4
    } else {
        num_fake_slots
    };
    let freq = if freq == 0 { 8 } else { freq };
    let padding_size = (num_fake_slots * 8 + 15) & !15;

    for func in functions {
        // Phase 1: AllocateStack を探す
        let alloc_size = match find_allocate_stack(&func.instructions) {
            Some(size) => size,
            None => continue,
        };

        // 最も深い Stack(offset) を探す
        let min_offset = find_min_stack_offset(&func.instructions);
        if min_offset >= 0 {
            continue; // spill スロットなし
        }

        // Phase 2: フレーム拡張
        // AllocateStack(N) → AllocateStack(N + padding_size)
        // DeallocateStack(N) → DeallocateStack(N + padding_size) （元の alloc_size と一致するもののみ）
        for instr in func.instructions.iter_mut() {
            match instr {
                Instruction::AllocateStack(n) if *n == alloc_size => {
                    *n += padding_size;
                }
                Instruction::DeallocateStack(n) if *n == alloc_size => {
                    *n += padding_size;
                }
                _ => {}
            }
        }

        // 偽スロットのオフセットを計算: min_offset - 8, min_offset - 16, ...
        let fake_offsets: Vec<i32> = (1..=num_fake_slots as i32)
            .map(|i| min_offset - 8 * i)
            .collect();

        // Phase 3: 偽操作の挿入
        let mut new_instrs = Vec::new();
        let mut counter: usize = 0;
        let mut pattern: usize = 0;
        let mut slot_idx: usize = 0;
        let mut written_slots: Vec<i32> = Vec::new();
        let orig = func.instructions.clone();
        for (i, instr) in orig.iter().enumerate() {
            new_instrs.push(instr.clone());
            counter += 1;
            if counter < freq {
                continue;
            }
            // 安全な挿入位置か判定
            let next = orig.get(i + 1);
            if !is_safe_shuffle_point(instr, next) {
                continue;
            }
            let offset = fake_offsets[slot_idx % num_fake_slots];
            match pattern % 3 {
                0 => {
                    // Fake int store: movl %eXX, fake_offset(%rbp)
                    let src_reg = extract_source_reg(instr);
                    new_instrs.push(Instruction::Mov {
                        asm_type: AsmType::Longword,
                        src: Operand::Register(src_reg),
                        dst: Operand::Stack(offset),
                    });
                    if !written_slots.contains(&offset) {
                        written_slots.push(offset);
                    }
                }
                1 => {
                    // Fake quad store: movq %rXX, fake_offset(%rbp)
                    let src_reg = extract_source_reg(instr);
                    new_instrs.push(Instruction::Mov {
                        asm_type: AsmType::Quadword,
                        src: Operand::Register(src_reg),
                        dst: Operand::Stack(offset),
                    });
                    if !written_slots.contains(&offset) {
                        written_slots.push(offset);
                    }
                }
                2 => {
                    // Fake load: movl fake_offset(%rbp), %r10d
                    // 書き込み済みスロットから読む。なければパターン0にフォールバック
                    if written_slots.is_empty() {
                        let src_reg = extract_source_reg(instr);
                        new_instrs.push(Instruction::Mov {
                            asm_type: AsmType::Longword,
                            src: Operand::Register(src_reg),
                            dst: Operand::Stack(offset),
                        });
                        if !written_slots.contains(&offset) {
                            written_slots.push(offset);
                        }
                    } else {
                        let read_offset = written_slots[slot_idx % written_slots.len()];
                        new_instrs.push(Instruction::Mov {
                            asm_type: AsmType::Longword,
                            src: Operand::Stack(read_offset),
                            dst: Operand::Register(Reg::R10),
                        });
                    }
                }
                _ => unreachable!(),
            }
            counter = 0;
            pattern += 1;
            slot_idx += 1;
        }
        func.instructions = new_instrs;
    }
}

/// AllocateStack 命令を探してサイズを返す。
fn find_allocate_stack(instructions: &[Instruction]) -> Option<usize> {
    for instr in instructions {
        if let Instruction::AllocateStack(n) = instr {
            return Some(*n);
        }
    }
    None
}

/// 最も深い（最も負の）Stack(offset) を探す。
fn find_min_stack_offset(instructions: &[Instruction]) -> i32 {
    let mut min_offset: i32 = 0;
    for instr in instructions {
        let check = |op: &Operand| {
            if let Operand::Stack(off) = op {
                *off
            } else {
                0
            }
        };
        let offset = match instr {
            Instruction::Mov { src, dst, .. } => check(src).min(check(dst)),
            Instruction::Binary { src, dst, .. } => check(src).min(check(dst)),
            Instruction::Cmp { src, dst, .. } => check(src).min(check(dst)),
            Instruction::Unary { operand, .. } => check(operand),
            Instruction::Idiv { operand, .. } => check(operand),
            Instruction::Div { operand, .. } => check(operand),
            Instruction::SetCC { operand, .. } => check(operand),
            Instruction::Push(op) => check(op),
            Instruction::Movsx { src, dst } => check(src).min(check(dst)),
            Instruction::MovsxByte { src, dst, .. } => check(src).min(check(dst)),
            Instruction::MovZeroExtend { src, dst } => check(src).min(check(dst)),
            Instruction::MovZeroExtendByte { src, dst, .. } => check(src).min(check(dst)),
            Instruction::Truncate { src, dst } => check(src).min(check(dst)),
            Instruction::Cvtsi2sd { src, dst, .. } => check(src).min(check(dst)),
            Instruction::Cvttsd2si { src, dst, .. } => check(src).min(check(dst)),
            Instruction::Lea { src, dst } => check(src).min(check(dst)),
            _ => 0,
        };
        if offset < min_offset {
            min_offset = offset;
        }
    }
    min_offset
}

/// 反逆アセンブリ: 無条件ジャンプ直後にゴミバイトを挿入する。
///
/// `0xE8` は x86 の `call rel32` オペコード。逆アセンブラが 5 バイト命令として
/// 解釈しようとするため、後続命令の命令境界認識が破壊される。
/// `Jmp` と `JmpIndirect` の直後に挿入する。`JmpCC` には挿入しない（fallthrough パスがあるため）。
fn insert_anti_disassembly(functions: &mut [AsmFunction]) {
    for func in functions {
        let mut new_instrs = Vec::new();
        for instr in &func.instructions {
            new_instrs.push(instr.clone());
            if matches!(instr, Instruction::Jmp(_) | Instruction::JmpIndirect(_, _)) {
                new_instrs.push(Instruction::RawBytes(vec![0xE8]));
            }
        }
        func.instructions = new_instrs;
    }
}

/// 関数呼び出しの間接化: `call func` を `lea func(%rip), %r10; call *%r10` に変換する。
///
/// R10 は caller-saved の scratch レジスタで、Call 直前に使っても安全。
fn indirect_calls(
    functions: &mut [AsmFunction],
    local_names: &std::collections::HashSet<String>,
) {
    for func in functions {
        let mut new_instrs = Vec::new();
        for instr in &func.instructions {
            match instr {
                Instruction::Call(name) if local_names.contains(name) => {
                    // Only convert locally-defined functions to indirect calls.
                    // External/libc functions need PLT/GOT and cannot use
                    // simple RIP-relative `lea` (especially on macOS).
                    new_instrs.push(Instruction::Lea {
                        src: Operand::Data(name.clone()),
                        dst: Operand::Register(Reg::R10),
                    });
                    new_instrs.push(Instruction::CallIndirect(Operand::Register(Reg::R10)));
                }
                _ => new_instrs.push(instr.clone()),
            }
        }
        func.instructions = new_instrs;
    }
}

/// レジスタシャッフル: dead な `mov` 命令を挿入し、偽のレジスタ間依存関係を生成する。
///
/// デコンパイラ（Hex-Rays, Ghidra）がデータフローグラフを構築する際、
/// R10/R11 への偽コピーが変数追跡・式復元を困難にする。
///
/// 3種のパターンをローテーション:
/// - パターン0: Dead copy（`movq %rX, %r10`）
/// - パターン1: Copy chain（`movq %rX, %r10; movq %r10, %r11`）
/// - パターン2: Round-trip（`movq %rX, %r10; movq %r10, %rX`）
fn register_shuffle(functions: &mut [AsmFunction], freq: usize) {
    let freq = if freq == 0 { 5 } else { freq };
    for func in functions {
        let mut new_instrs = Vec::new();
        let mut counter: usize = 0;
        let mut pattern: usize = 0;
        let orig = &func.instructions;
        for (i, instr) in orig.iter().enumerate() {
            new_instrs.push(instr.clone());
            counter += 1;
            if counter < freq {
                continue;
            }
            // 安全な挿入位置か判定
            let next = orig.get(i + 1);
            if !is_safe_shuffle_point(instr, next) {
                continue;
            }
            // ソースレジスタを選択
            let src_reg = extract_source_reg(instr);
            let src = Operand::Register(src_reg);
            match pattern % 3 {
                0 => {
                    // Dead copy: movq %rX, %r10
                    new_instrs.push(Instruction::Mov {
                        asm_type: AsmType::Quadword,
                        src,
                        dst: Operand::Register(Reg::R10),
                    });
                }
                1 => {
                    // Copy chain: movq %rX, %r10; movq %r10, %r11
                    new_instrs.push(Instruction::Mov {
                        asm_type: AsmType::Quadword,
                        src,
                        dst: Operand::Register(Reg::R10),
                    });
                    new_instrs.push(Instruction::Mov {
                        asm_type: AsmType::Quadword,
                        src: Operand::Register(Reg::R10),
                        dst: Operand::Register(Reg::R11),
                    });
                }
                2 => {
                    // Round-trip: movq %rX, %r10; movq %r10, %rX
                    new_instrs.push(Instruction::Mov {
                        asm_type: AsmType::Quadword,
                        src: src.clone(),
                        dst: Operand::Register(Reg::R10),
                    });
                    new_instrs.push(Instruction::Mov {
                        asm_type: AsmType::Quadword,
                        src: Operand::Register(Reg::R10),
                        dst: src,
                    });
                }
                _ => unreachable!(),
            }
            counter = 0;
            pattern += 1;
        }
        func.instructions = new_instrs;
    }
}

/// 命令置換: 特定の命令パターンを意味的に等価な別の命令列に置換する。
///
/// regalloc + fixup 後の命令列に対し、x86-64 命令を意味的に等価だが
/// パターンの異なる命令列に置換する。デコンパイラ・逆アセンブラの
/// パターンマッチングを妨害する。
///
/// 4種のパターンをローテーション:
/// - パターン0: Add→Sub 即値スワップ（`addl $42, %eax` → `subl $-42, %eax`）
/// - パターン1: Sub→Add 即値スワップ（`subl $10, %ecx` → `addl $-10, %ecx`）
/// - パターン2: Neg 展開（`negl %edx` → `notl %edx; addl $1, %edx`）
/// - パターン3: Mov 即値分割（`movl $100, %eax` → `movl $142, %eax; subl $42, %eax`）
fn instruction_substitution(functions: &mut [AsmFunction], freq: usize) {
    let freq = if freq == 0 { 4 } else { freq };
    for func in functions {
        let mut new_instrs = Vec::new();
        let mut counter: usize = 0;
        let mut pattern_idx: usize = 0;
        let orig = &func.instructions;
        for (i, instr) in orig.iter().enumerate() {
            counter += 1;
            if counter < freq {
                new_instrs.push(instr.clone());
                continue;
            }
            // 安全な置換位置か判定
            let next = orig.get(i + 1);
            if !is_safe_subst_point(instr, next) {
                new_instrs.push(instr.clone());
                continue;
            }
            let pat = pattern_idx % 4;
            let substituted = match (pat, instr) {
                // パターン0: Add Imm → Sub -Imm
                (
                    0,
                    Instruction::Binary {
                        asm_type,
                        op: AsmBinaryOp::Add,
                        src: Operand::Imm(n),
                        dst,
                    },
                ) if *asm_type != AsmType::Double => {
                    let neg_n = -(*n);
                    if neg_n >= i32::MIN as i64 && neg_n <= i32::MAX as i64 {
                        new_instrs.push(Instruction::Binary {
                            asm_type: *asm_type,
                            op: AsmBinaryOp::Sub,
                            src: Operand::Imm(neg_n),
                            dst: dst.clone(),
                        });
                        true
                    } else {
                        false
                    }
                }
                // パターン1: Sub Imm → Add -Imm
                (
                    1,
                    Instruction::Binary {
                        asm_type,
                        op: AsmBinaryOp::Sub,
                        src: Operand::Imm(n),
                        dst,
                    },
                ) if *asm_type != AsmType::Double => {
                    let neg_n = -(*n);
                    if neg_n >= i32::MIN as i64 && neg_n <= i32::MAX as i64 {
                        new_instrs.push(Instruction::Binary {
                            asm_type: *asm_type,
                            op: AsmBinaryOp::Add,
                            src: Operand::Imm(neg_n),
                            dst: dst.clone(),
                        });
                        true
                    } else {
                        false
                    }
                }
                // パターン2: Neg → Not + Add 1
                (
                    2,
                    Instruction::Unary {
                        asm_type,
                        op: AsmUnaryOp::Neg,
                        operand,
                    },
                ) if *asm_type != AsmType::Double => {
                    new_instrs.push(Instruction::Unary {
                        asm_type: *asm_type,
                        op: AsmUnaryOp::Not,
                        operand: operand.clone(),
                    });
                    new_instrs.push(Instruction::Binary {
                        asm_type: *asm_type,
                        op: AsmBinaryOp::Add,
                        src: Operand::Imm(1),
                        dst: operand.clone(),
                    });
                    true
                }
                // パターン3: Mov Imm → Mov (N+K) + Sub K
                (
                    3,
                    Instruction::Mov {
                        asm_type,
                        src: Operand::Imm(n),
                        dst,
                    },
                ) if *asm_type != AsmType::Double && *n != 0 => {
                    let k = ((pattern_idx * 7 + 3) % 127 + 1) as i64;
                    let n_plus_k = *n + k;
                    if n_plus_k >= i32::MIN as i64 && n_plus_k <= i32::MAX as i64 {
                        new_instrs.push(Instruction::Mov {
                            asm_type: *asm_type,
                            src: Operand::Imm(n_plus_k),
                            dst: dst.clone(),
                        });
                        new_instrs.push(Instruction::Binary {
                            asm_type: *asm_type,
                            op: AsmBinaryOp::Sub,
                            src: Operand::Imm(k),
                            dst: dst.clone(),
                        });
                        true
                    } else {
                        false
                    }
                }
                _ => false,
            };
            if substituted {
                counter = 0;
                pattern_idx += 1;
            } else {
                new_instrs.push(instr.clone());
            }
        }
        func.instructions = new_instrs;
    }
}

/// 命令置換の安全な位置か判定する。
///
/// 以下をすべて満たす場合に true:
/// 1. 現在の命令が Cmp/SetCC でない（フラグ操作命令は置換しない）
/// 2. プロローグ/エピローグ命令でない（Push/Pop/AllocateStack/DeallocateStack/Ret）
/// 3. 次の命令が Label でない
/// 4. 次の命令が JmpCC/SetCC でない（フラグ読み取り保護）
fn is_safe_subst_point(current: &Instruction, next: Option<&Instruction>) -> bool {
    // 条件1: Cmp/SetCC はスキップ
    if matches!(current, Instruction::Cmp { .. } | Instruction::SetCC { .. }) {
        return false;
    }
    // 条件2: プロローグ/エピローグ命令はスキップ
    if matches!(
        current,
        Instruction::Push(_)
            | Instruction::Pop(_)
            | Instruction::AllocateStack(_)
            | Instruction::DeallocateStack(_)
            | Instruction::Ret
    ) {
        return false;
    }
    if let Some(next_instr) = next {
        // 条件3: 次が Label なら置換しない
        if matches!(next_instr, Instruction::Label(_)) {
            return false;
        }
        // 条件4: 次の命令が JmpCC/SetCC ならフラグを読むため置換しない
        if matches!(
            next_instr,
            Instruction::JmpCC(..) | Instruction::SetCC { .. }
        ) {
            return false;
        }
    }
    true
}

/// 安全な挿入位置か判定する。
///
/// 以下をすべて満たす場合に true:
/// 1. 次の命令が R10/R11 を読まない（fixup 生成シーケンス保護）
/// 2. 現在の命令が Cmp/SetCC でない（フラグレジスタのライブ区間を避ける）
/// 3. プロローグ/エピローグ命令でない（Push/Pop/AllocateStack/DeallocateStack/Ret）
/// 4. 次の命令が Label でない（ラベルの直前には挿入しない）
fn is_safe_shuffle_point(current: &Instruction, next: Option<&Instruction>) -> bool {
    // 条件2: Cmp/SetCC はスキップ
    if matches!(current, Instruction::Cmp { .. } | Instruction::SetCC { .. }) {
        return false;
    }
    // 条件3: プロローグ/エピローグ命令はスキップ
    if matches!(
        current,
        Instruction::Push(_)
            | Instruction::Pop(_)
            | Instruction::AllocateStack(_)
            | Instruction::DeallocateStack(_)
            | Instruction::Ret
    ) {
        return false;
    }
    if let Some(next_instr) = next {
        // 条件4: 次が Label なら挿入しない
        if matches!(next_instr, Instruction::Label(_)) {
            return false;
        }
        // 条件1: 次の命令が R10/R11 を読むならスキップ
        if reads_r10_or_r11(next_instr) {
            return false;
        }
    }
    true
}

/// 命令からデスティネーションレジスタを抽出し、ソースレジスタとして使用する。
///
/// Mov/Binary/Lea の dst が GP レジスタ（R10/R11/SP/BP/XMM* 以外）ならそのまま返す。
/// 抽出できない場合は AX をフォールバックとして返す。
fn extract_source_reg(instr: &Instruction) -> Reg {
    let reg = match instr {
        Instruction::Mov {
            dst: Operand::Register(r),
            ..
        } => Some(*r),
        Instruction::Binary {
            dst: Operand::Register(r),
            ..
        } => Some(*r),
        Instruction::Lea {
            dst: Operand::Register(r),
            ..
        } => Some(*r),
        _ => None,
    };
    match reg {
        Some(r) if is_valid_shuffle_source(r) => r,
        _ => Reg::AX,
    }
}

/// シャッフルソースとして有効なレジスタか判定する。
/// R10/R11（scratch）、SP/BP（フレーム）、XMM*（浮動小数点）は除外する。
fn is_valid_shuffle_source(r: Reg) -> bool {
    !matches!(
        r,
        Reg::R10
            | Reg::R11
            | Reg::SP
            | Reg::BP
            | Reg::XMM0
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

/// 命令が R10 または R11 を読むか判定する。
///
/// Binary/Unary は read-modify-write なので dst/operand も読み取り対象。
/// fixup が生成する R11 シーケンス（`mov Stack, R11; imul src, R11; mov R11, Stack`）を
/// 保護するため、dst に R10/R11 がある場合もスキップ対象とする。
fn reads_r10_or_r11(instr: &Instruction) -> bool {
    let check = |op: &Operand| matches!(op, Operand::Register(Reg::R10 | Reg::R11));
    match instr {
        Instruction::Mov { src, .. } => check(src),
        Instruction::Binary { src, dst, .. } => check(src) || check(dst),
        Instruction::Cmp { src, dst, .. } => check(src) || check(dst),
        Instruction::Unary { operand, .. } => check(operand),
        Instruction::Idiv { operand, .. } => check(operand),
        Instruction::Div { operand, .. } => check(operand),
        Instruction::SetCC { operand, .. } => check(operand),
        Instruction::Push(op) => check(op),
        Instruction::Movsx { src, dst } => check(src) || check(dst),
        Instruction::MovsxByte { src, dst, .. } => check(src) || check(dst),
        Instruction::MovZeroExtend { src, dst } => check(src) || check(dst),
        Instruction::MovZeroExtendByte { src, dst, .. } => check(src) || check(dst),
        Instruction::Truncate { src, dst } => check(src) || check(dst),
        Instruction::Cvtsi2sd { src, dst, .. } => check(src) || check(dst),
        Instruction::Cvttsd2si { src, dst, .. } => check(src) || check(dst),
        Instruction::Lea { src, dst } => check(src) || check(dst),
        Instruction::CallIndirect(op) => check(op),
        _ => false,
    }
}
