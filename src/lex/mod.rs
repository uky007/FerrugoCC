//! 字句解析（Lexer）モジュール
//!
//! Cソースコードの文字列をトークン列に変換する。
//! コンパイラパイプラインの最初のステージ。
//!
//! ```text
//! "int main(void) { return 2; }"
//!   → [KwInt, Identifier("main"), OpenParen, KwVoid, CloseParen,
//!      OpenBrace, KwReturn, IntLiteral(2), Semicolon, CloseBrace]
//! ```

pub mod token;
pub mod lexer;

pub use token::{Token, TokenKind};
pub use lexer::lex;
