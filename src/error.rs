//! コンパイラ全体で使用するエラー型の定義
//!
//! FerrugoCC のパイプライン（字句解析 → 構文解析 → 型検査 → TACKY IR 生成 → 最適化 → コード生成 → 出力）で
//! 発生しうるエラーを `CompileError` 列挙型にまとめる。
//! `thiserror` クレートを使うことで、各バリアントに `Display` 実装を自動導出している。
//!
//! # 設計方針
//! - 各パイプラインステージごとにバリアントを用意（`LexError`, `ParseError`, ...）
//! - `std::io::Error` は `#[from]` で自動変換
//! - `Result<T>` 型エイリアスでボイラープレートを削減

use thiserror::Error;

/// コンパイラのあらゆるフェーズで発生しうるエラーを統一的に扱う列挙型。
///
/// `thiserror::Error` を derive することで、各バリアントの `#[error("...")]`
/// アトリビュートから `Display` トレイトが自動実装される。
#[derive(Debug, Error)]
#[allow(clippy::enum_variant_names)]
pub enum CompileError {
    /// 字句解析（Lexer）で不正な文字やトークンを検出した場合
    #[error("Lexer error: {0}")]
    LexError(String),

    /// 構文解析（Parser）で文法に合わないトークン列を検出した場合
    #[error("Parse error: {0}")]
    ParseError(String),

    /// 型検査（Typecheck）で型の不一致を検出した場合（Chapter 11）
    #[error("Type error: {0}")]
    TypeError(String),

    /// コード生成（Codegen）でサポート外の構文に遭遇した場合
    #[error("Codegen error: {0}")]
    CodegenError(String),

    /// アセンブリテキスト出力（Emit）で書き込みに失敗した場合
    #[error("Emit error: {0}")]
    EmitError(String),

    /// ファイル読み書きなどの I/O エラー。`std::io::Error` から自動変換される。
    #[error("I/O error: {0}")]
    IoError(#[from] std::io::Error),

    /// 外部ツール（gcc など）の呼び出しが失敗した場合
    #[error("External tool failed: {0}")]
    ExternalToolError(String),
}

/// コンパイラ内部で統一的に使う `Result` 型エイリアス。
///
/// 各関数は `Result<T>` を返すだけで、エラー型を毎回書く必要がなくなる。
pub type Result<T> = std::result::Result<T, CompileError>;
