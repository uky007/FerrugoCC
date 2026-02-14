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

pub use asm_ast::{AsmProgram, AsmFunction};

use crate::error::Result;
use crate::obfuscation::ObfuscationConfig;
use crate::tacky::tacky_ast::TackyProgram;
use asm_ast::{Instruction, Operand, Reg};

/// TACKY プログラムをアセンブリ AST に変換する（Chapter 20: レジスタ割り当て統合）。
///
/// `obf_config` が Some の場合、regalloc + fixup の後に ASM レベルの難読化パスを適用する:
/// - 反逆アセンブリ（ゴミバイト挿入）
/// - 関数呼び出しの間接化
pub fn generate(program: &TackyProgram, obf_config: Option<&ObfuscationConfig>) -> Result<AsmProgram> {
    // 1. Pseudo 付きの Asm を生成
    let (results, static_vars, static_constants) = generator::generate(program)?;

    // 2. 各関数にレジスタ割り当て + fixup
    let mut functions = Vec::new();
    for result in results {
        let alloc = regalloc::allocate_registers(
            result.func.instructions,
            &result.var_types,
        );
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
        if config.anti_disassembly {
            insert_anti_disassembly(&mut functions);
        }
        if config.indirect_calls {
            indirect_calls(&mut functions);
        }
    }

    Ok(AsmProgram {
        functions,
        static_vars,
        static_constants,
    })
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
fn indirect_calls(functions: &mut [AsmFunction]) {
    for func in functions {
        let mut new_instrs = Vec::new();
        for instr in &func.instructions {
            match instr {
                Instruction::Call(name) => {
                    // lea func(%rip), %r10
                    new_instrs.push(Instruction::Lea {
                        src: Operand::Data(name.clone()),
                        dst: Operand::Register(Reg::R10),
                    });
                    // call *%r10
                    new_instrs.push(Instruction::CallIndirect(
                        Operand::Register(Reg::R10),
                    ));
                }
                _ => new_instrs.push(instr.clone()),
            }
        }
        func.instructions = new_instrs;
    }
}
