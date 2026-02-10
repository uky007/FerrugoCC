//! 型検査（Type Checker）モジュール
//!
//! パース後の AST を走査し、型情報の補完と暗黙的型変換の挿入を行う。
//! パースとコード生成の間に挿入されるパス。
//!
//! - Chapter 11 で導入（`int`/`long` の暗黙変換）
//! - Chapter 12 で `unsigned int`/`unsigned long` に拡張（通常算術変換）
//! - Chapter 13 で `double` に拡張（`double` が最上位型、`~`/`%` を `double` に対してエラー）

pub mod checker;

pub use checker::typecheck;
