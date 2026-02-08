//! 構文解析（Parser）モジュール
//!
//! トークン列を受け取り、抽象構文木（AST）を構築する。
//! 再帰下降パーサーにより、Cの文法に従ってトークンを消費していく。
//!
//! ```text
//! [KwInt, Identifier("main"), OpenParen, KwVoid, CloseParen,
//!  OpenBrace, KwReturn, IntLiteral(2), Semicolon, CloseBrace]
//!   → Program { function: Function { name: "main",
//!       body: [Statement(Return(Constant(2)))] } }
//! ```

pub mod ast;
pub mod parser;

pub use ast::{Program, Function, BlockItem, Declaration, Statement, Expr, UnaryOp, BinaryOp};
pub use parser::parse;
