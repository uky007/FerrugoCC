//! コード生成（Codegen）モジュール
//!
//! C の AST をアセンブリ AST に変換する。
//! 意味的な変換（Cの構文 → x86-64 の命令列）をここで行う。
//!
//! ```text
//! Program { declarations: [TopLevelDecl::Function(FunctionDecl { name: "main",
//!     body: Return(Constant(2)) })]  }
//!   → AsmProgram { functions: [AsmFunction { name: "main",
//!       instructions: [Mov(Imm(2), Reg(AX)), Ret] }],
//!       static_vars: [], static_constants: [] }
//! ```
//!
//! Chapter 13 で `double` 型のコード生成を追加。SSE 命令 (`addsd`, `subsd`,
//! `mulsd`, `divsd`, `comisd`, `xorpd`) と XMM レジスタを使用する。
//! `double` 定数は `.rodata` セクションに `static_constants` として配置される。

pub mod asm_ast;
pub mod generator;

pub use asm_ast::{AsmProgram, AsmFunction, AsmStaticVar, AsmStaticConstant, StaticInit, Instruction, Operand, Reg, AsmUnaryOp, AsmBinaryOp, CondCode, AsmType};
pub use generator::generate;
