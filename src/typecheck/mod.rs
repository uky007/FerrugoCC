//! 型検査（Type Checker）モジュール
//!
//! パース後の AST を走査し、型情報の補完と暗黙的型変換の挿入を行う。
//! パースとコード生成の間に挿入されるパス。
//!
//! - Chapter 11 で導入（`int`/`long` の暗黙変換）
//! - Chapter 12 で `unsigned int`/`unsigned long` に拡張（通常算術変換）
//! - Chapter 13 で `double` に拡張（`double` が最上位型、`~`/`%` を `double` に対してエラー）
//! - Chapter 14 でポインタ型に拡張（`&`/`*` 演算子、左辺値検証、null ポインタ、ポインタ比較）
//! - Chapter 15 で配列・ポインタ算術に拡張（配列 decay、`sizeof` 解決、ポインタ加減算型規則）

pub mod checker;

pub use checker::typecheck;
