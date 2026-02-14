//! TACKY IR（三アドレスコード中間表現）モジュール
//!
//! C の AST を TACKY IR に変換し、最適化または難読化パスを適用する。
//!
//! ```text
//! C AST → [TackyGen] → TACKY IR → [Optimize or Obfuscate] → TACKY IR → [Codegen] → Asm AST
//! ```
//!
//! - `optimize`: 定数畳み込み・コピー伝播・不要コード除去（デフォルト）
//! - `obfuscate`: 定数間接化・算術置換・ジャンクコード・不透明述語・CFF・文字列暗号化 + ASM レベルのレジスタシャッフル等（`--fobfuscate`）

pub mod tacky_ast;
pub mod tacky_gen;
pub mod optimize;
pub mod obfuscate;

pub use tacky_gen::generate_tacky;
pub use optimize::optimize;
pub use obfuscate::obfuscate;
