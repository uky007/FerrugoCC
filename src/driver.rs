//! コンパイラドライバー
//!
//! コンパイルパイプラインの各ステージを順に実行し、
//! 最終的に gcc を呼び出して実行可能バイナリを生成する。
//!
//! # パイプライン
//! ```text
//! source.c → [Lex] → [Parse] → [Validate] → [TackyGen]
//!          → [Optimize or Obfuscate] → [Codegen] → [Emit] → source.s → [gcc] → binary
//! ```
//!
//! - デフォルト: TACKY IR 最適化パス（定数畳み込み・コピー伝播・不要コード除去）
//! - `--fobfuscate`: 難読化パス（TACKY: 関数インライン展開・定数間接化・算術置換・ジャンクコード・不透明述語・関数アウトライン化・CFF・文字列暗号化、
//!   ASM: スタックフレーム難読化・レジスタシャッフル・命令置換・反逆アセンブリ・間接呼出）
//! - `--obf-level=N`: 難読化強度レベル（1=軽量, 2=標準, 3=全パス有効, 4=最大）
//!
//! `--lex`, `--parse`, `--validate`, `--tacky`, `--codegen`, `-S` フラグで途中のステージで停止できる。
//! これは本のテストスイートとの互換性のために必要。

use std::path::Path;
use std::process::Command;

use crate::codegen;
use crate::emit;
use crate::error::{CompileError, Result};
use crate::lex;
use crate::obfuscation::ObfuscationConfig;
use crate::parse;
use crate::tacky;
use crate::typecheck;

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
///
/// `obf_config` が Some の場合、TACKY IR 最適化の代わりに難読化パスを適用する。
pub fn run(source_path: &Path, stage: Stage, obf_config: Option<ObfuscationConfig>) -> Result<()> {
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

    // ── Stage 3.5: TACKY 最適化 or 難読化 ──
    let tacky_program = if let Some(ref config) = obf_config {
        tacky::obfuscate(tacky_program, config)
    } else {
        tacky::optimize(tacky_program)
    };

    // ── Stage 4: コード生成（TACKY → Asm AST）──
    let asm_program = codegen::generate(&tacky_program, obf_config.as_ref())?;
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
