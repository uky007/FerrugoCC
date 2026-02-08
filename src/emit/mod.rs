//! アセンブリ出力（Emit）モジュール
//!
//! アセンブリ AST を AT&T 構文のテキストに変換する。
//! コンパイラパイプラインの最終ステージ（テキスト出力まで）。
//!
//! ```text
//! AsmProgram { ... }
//!   → "    .globl main\nmain:\n    movl $2, %eax\n    ret\n..."
//! ```

pub mod emitter;

pub use emitter::emit;
