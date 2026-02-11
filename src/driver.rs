//! コンパイラドライバー
//!
//! コンパイルパイプラインの各ステージを順に実行し、
//! 最終的に gcc を呼び出して実行可能バイナリを生成する。
//!
//! # パイプライン
//! ```text
//! source.c → [Lex] → [Parse] → [Validate] → [TackyGen] → [Optimize] → [Codegen] → [Emit] → source.s → [gcc] → source
//! ```
//!
//! Chapter 19 で TACKY IR（三アドレスコード中間表現）パスが追加された。
//! C AST → TACKY IR → 最適化 → Asm AST というパイプラインになる。
//!
//! `--lex`, `--parse`, `--validate`, `--tacky`, `--codegen`, `-S` フラグで途中のステージで停止できる。
//! これは本のテストスイートとの互換性のために必要。

use std::path::Path;
use std::process::Command;

use crate::error::{CompileError, Result};
use crate::lex;
use crate::parse;
use crate::typecheck;
use crate::tacky;
use crate::codegen;
use crate::emit;

/// コンパイルをどのステージまで実行するかを指定する列挙型。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stage {
    /// 字句解析のみ（トークン化が成功すれば OK）
    Lex,
    /// 構文解析まで（AST 構築が成功すれば OK）
    Parse,
    /// 型検査まで
    Validate,
    /// TACKY IR 生成まで
    Tacky,
    /// コード生成まで（アセンブリ AST 構築が成功すれば OK）
    Codegen,
    /// アセンブリ出力まで（.s ファイルを書き出す）
    EmitAsm,
    /// フルコンパイル（gcc でバイナリまで生成）
    Full,
}

/// コンパイルパイプラインを実行する。
///
/// `source_path` のCソースファイルを読み込み、`stage` で指定された
/// ステージまで処理を行う。
pub fn run(source_path: &Path, stage: Stage) -> Result<()> {
    let source = std::fs::read_to_string(source_path)?;

    // ── Stage 1: 字句解析 ──
    let tokens = lex::lex(&source)?;
    if stage == Stage::Lex {
        return Ok(());
    }

    // ── Stage 2: 構文解析 ──
    let mut program = parse::parse(&tokens)?;
    if stage == Stage::Parse {
        return Ok(());
    }

    // ── Stage 2.5: 型検査 ──
    typecheck::typecheck(&mut program)?;
    if stage == Stage::Validate {
        return Ok(());
    }

    // ── Stage 3: TACKY IR 生成 ──
    let tacky_program = tacky::generate_tacky(&program)?;
    if stage == Stage::Tacky {
        return Ok(());
    }

    // ── Stage 3.5: TACKY 最適化 ──
    let tacky_program = tacky::optimize(tacky_program);

    // ── Stage 4: コード生成（TACKY → Asm AST）──
    let asm_program = codegen::generate(&tacky_program)?;
    if stage == Stage::Codegen {
        return Ok(());
    }

    // ── Stage 5: アセンブリ出力 ──
    let asm_text = emit::emit(&asm_program)?;
    let asm_path = source_path.with_extension("s");
    std::fs::write(&asm_path, &asm_text)?;

    if stage == Stage::EmitAsm {
        return Ok(());
    }

    // ── Stage 6: アセンブル＆リンク ──
    // gcc に .s ファイルを渡してバイナリを生成する。
    // gcc は内部で as（アセンブラ）と ld（リンカ）を呼び出す。
    let output_path = source_path.with_extension("");
    let status = Command::new("gcc")
        .arg(&asm_path)
        .arg("-o")
        .arg(&output_path)
        .status()?;

    // アセンブリファイルは中間生成物なので削除
    let _ = std::fs::remove_file(&asm_path);

    if !status.success() {
        return Err(CompileError::ExternalToolError(format!(
            "gcc exited with status {}",
            status
        )));
    }

    Ok(())
}
