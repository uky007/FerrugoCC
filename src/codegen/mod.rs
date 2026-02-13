//! コード生成（Codegen）モジュール
//!
//! TACKY IR をアセンブリ AST に変換する。
//! 各 TACKY 命令を機械的に x86-64 の命令列にマッピングし、
//! レジスタ割り当て + fixup パスを経て有効なアセンブリを出力する。
//!
//! ```text
//! TackyProgram → [generate_with_pseudos] → Asm(Pseudo)
//!              → [regalloc] → Asm(Reg+Stack)
//!              → [fixup] → Asm(valid) → AsmProgram
//! ```

pub mod asm_ast;
pub mod generator;
pub mod regalloc;

pub use asm_ast::{AsmProgram, AsmFunction};

use crate::error::Result;
use crate::tacky::tacky_ast::TackyProgram;

/// TACKY プログラムをアセンブリ AST に変換する（Chapter 20: レジスタ割り当て統合）。
pub fn generate(program: &TackyProgram) -> Result<AsmProgram> {
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

    Ok(AsmProgram {
        functions,
        static_vars,
        static_constants,
    })
}
