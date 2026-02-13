//! FerrugoCC — Rust製Cコンパイラ
//!
//! "Writing a C Compiler" (Nora Sandler) に沿って開発する学習用Cコンパイラ。
//!
//! # パイプライン
//! ```text
//! source.c → [Lex] → [Parse] → [Validate] → [TackyGen] → [Optimize/Obfuscate]
//!          → [Codegen] → [RegAlloc+Coalescing] → [Fixup] → [Emit] → source.s → [gcc] → binary
//! ```
//!
//! `--fobfuscate` 指定時は最適化の代わりに難読化パス（5パス）を適用する。
//!
//! # 使い方
//! ```text
//! ferrugocc <source.c>                      # フルコンパイル（実行ファイル生成）
//! ferrugocc --lex <source.c>                # 字句解析のみ
//! ferrugocc --parse <source.c>              # 構文解析まで
//! ferrugocc --validate <source.c>           # 型検査まで
//! ferrugocc --tacky <source.c>              # TACKY IR 生成まで
//! ferrugocc --codegen <source.c>            # コード生成まで（Asm AST 構築）
//! ferrugocc -S <source.c>                   # アセンブリ出力まで（.s ファイル生成）
//! ferrugocc --fobfuscate <source.c>         # 難読化コンパイル
//! ferrugocc --fobfuscate -S <source.c>      # 難読化アセンブリ出力
//! ```

mod error;
mod lex;
mod parse;
mod typecheck;
mod tacky;
mod codegen;
mod emit;
mod driver;

use std::path::PathBuf;
use std::process;

use clap::Parser;

use driver::Stage;

/// FerrugoCC のコマンドライン引数定義。
///
/// `clap` の derive マクロにより、構造体のフィールドから
/// 自動的にコマンドライン引数のパーサーが生成される。
#[derive(Parser)]
#[command(name = "ferrugocc", about = "A C compiler written in Rust")]
struct Cli {
    /// Run the lexer only
    #[arg(long)]
    lex: bool,

    /// Run through parsing
    #[arg(long)]
    parse: bool,

    /// Run through type checking (validate)
    #[arg(long)]
    validate: bool,

    /// Run through TACKY IR generation
    #[arg(long)]
    tacky: bool,

    /// Run through code generation
    #[arg(long)]
    codegen: bool,

    /// Emit assembly (.s) only
    #[arg(short = 'S')]
    emit_asm: bool,

    /// 難読化コンパイル（最適化の代わりに5つの難読化パスを適用）
    #[arg(long = "fobfuscate")]
    obfuscate: bool,

    /// Input C source file
    source: PathBuf,
}

fn main() {
    let cli = Cli::parse();

    // フラグに応じて停止ステージを決定（フラグなしならフルコンパイル）
    let stage = if cli.lex {
        Stage::Lex
    } else if cli.parse {
        Stage::Parse
    } else if cli.validate {
        Stage::Validate
    } else if cli.tacky {
        Stage::Tacky
    } else if cli.codegen {
        Stage::Codegen
    } else if cli.emit_asm {
        Stage::EmitAsm
    } else {
        Stage::Full
    };

    if let Err(e) = driver::run(&cli.source, stage, cli.obfuscate) {
        eprintln!("ferrugocc: {e}");
        process::exit(1);
    }
}
