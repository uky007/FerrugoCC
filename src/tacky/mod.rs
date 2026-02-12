//! TACKY IR（三アドレスコード中間表現）モジュール
//!
//! C の AST を TACKY IR に変換し、最適化パスを適用する。
//!
//! ```text
//! C AST → [TackyGen] → TACKY IR → [Optimize] → TACKY IR → [Codegen] → Asm AST
//! ```

pub mod tacky_ast;
pub mod tacky_gen;
pub mod optimize;
pub mod obfuscate;

pub use tacky_ast::*;
pub use tacky_gen::generate_tacky;
pub use optimize::optimize;
pub use obfuscate::obfuscate;
