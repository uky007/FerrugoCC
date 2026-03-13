//! コンパイラドライバー
//!
//! コンパイルパイプラインの各ステージを順に実行し、
//! 最終的に gcc を呼び出して実行可能バイナリを生成する。
//!
//! # パイプライン
//! ```text
//! source.c → [Preprocess] → [Lex] → [Parse] → [Validate] → [TackyGen]
//!          → [Optimize or Obfuscate] → [Codegen] → [Emit] → source.s → [gcc] → binary
//! ```
//!
//! - デフォルト: TACKY IR 最適化パス（定数畳み込み・コピー伝播・不要コード除去）
//! - `--fobfuscate`: 難読化パス（TACKY: 関数インライン展開・定数間接化・算術置換・ジャンクコード・不透明述語・関数アウトライン化・CFF・文字列暗号化・OPSEC衛生化、
//!   ASM: スタックフレーム難読化・レジスタシャッフル・命令置換・反逆アセンブリ・間接呼出）
//! - `--obf-level=N`: 難読化強度レベル（1=軽量, 2=標準, 3=全パス有効, 4=最大）
//!
//! `--lex`, `--parse`, `--validate`, `--tacky`, `--codegen`, `-S` フラグで途中のステージで停止できる。
//! これは本のテストスイートとの互換性のために必要。

use std::path::{Path, PathBuf};
use std::process::Command;

use crate::codegen;
use crate::emit;
use crate::error::{CompileError, Result};
use crate::lex;
use crate::obfuscation::{ObfuscationConfig, OpsecPolicy};
use crate::parse;
use crate::tacky;
use crate::typecheck;

/// 前処理モード。
///
/// ソースファイルを字句解析に渡す前にどのように前処理するかを指定する。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)]
pub enum PreprocessMode {
    /// 前処理をスキップ（ソースをそのまま字句解析に渡す）
    None,
    /// 外部プリプロセッサ（gcc -E -P）を使用
    External,
}

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

/// 外部プリプロセッサ（gcc -E -P）を実行する。
///
/// ソースファイルのディレクトリを `-I` に含めることで、
/// ローカルの `#include` を解決できるようにする。
fn preprocess_external(source_path: &Path, pp_defines: &[String]) -> Result<String> {
    let source_dir = source_path.parent().unwrap_or(Path::new("."));
    let mut cmd = Command::new("gcc");
    cmd.arg("-E").arg("-P").arg("-I").arg(source_dir);
    for def in pp_defines {
        cmd.arg(format!("-D{def}"));
    }
    let output = cmd.arg(source_path).output().map_err(|e| {
        CompileError::ExternalToolError(format!("failed to run preprocessor (gcc -E): {e}"))
    })?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(CompileError::ExternalToolError(format!(
            "preprocessing failed:\n{stderr}"
        )));
    }

    String::from_utf8(output.stdout).map_err(|e| {
        CompileError::ExternalToolError(format!("preprocessor output is not valid UTF-8: {e}"))
    })
}

fn asm_output_path(source_path: &Path) -> PathBuf {
    std::env::var_os("FERRUGOCC_ASM_OUTPUT")
        .map(PathBuf::from)
        .unwrap_or_else(|| source_path.with_extension("s"))
}

/// コンパイルパイプラインを実行する。
///
/// `source_path` のCソースファイルを読み込み、`stage` で指定された
/// ステージまで処理を行う。
///
/// `obf_config` が Some の場合、TACKY IR 最適化の代わりに難読化パスを適用する。
/// `preprocess` で前処理モードを指定する。
pub fn run(
    source_path: &Path,
    stage: Stage,
    obf_config: Option<ObfuscationConfig>,
    preprocess: PreprocessMode,
    pp_defines: &[String],
) -> Result<()> {
    // ── Stage 0: 前処理 ──
    let source = match preprocess {
        PreprocessMode::None => std::fs::read_to_string(source_path)?,
        PreprocessMode::External => preprocess_external(source_path, pp_defines)?,
    };

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
        tacky::obfuscate(tacky_program, config)?
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
    let asm_path = asm_output_path(source_path);
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

    // 難読化の OPSEC strip が有効ならバイナリを strip
    if let Some(config) = &obf_config
        && config.opsec_strip
    {
        let strip_status = Command::new("strip").arg(&output_path).status();
        match strip_status {
            Ok(s) if s.success() => {}
            _ => {
                eprintln!("[OPSEC] warning: strip command failed or not found");
            }
        }
    }

    // OPSEC バイナリ監査（リンク後バイナリの文字列・シンボルをスキャン）
    if let Some(config) = &obf_config
        && config.opsec_audit
    {
        opsec_audit_binary(&output_path, config.opsec_policy)?;
    }

    Ok(())
}

/// リンク後バイナリの OPSEC 監査
///
/// `strings` コマンドでバイナリ内の文字列をスキャンし、
/// IP アドレス・ファイルパス・URL・デバッグキーワード・資格情報キーワードを検出する。
/// `nm` コマンドで main/_ 以外のユーザー定義シンボルをフラグする（informational）。
fn opsec_audit_binary(binary_path: &Path, policy: OpsecPolicy) -> Result<()> {
    let tag = match policy {
        OpsecPolicy::Warn => "OPSEC AUDIT WARNING",
        OpsecPolicy::Deny => "OPSEC AUDIT ERROR",
    };

    let mut violations: Vec<String> = Vec::new();
    let mut strings_ran = false;

    // strings コマンドでバイナリ内の文字列をスキャン
    if let Ok(output) = Command::new("strings").arg(binary_path).output()
        && output.status.success()
    {
        strings_ran = true;
        let stdout = String::from_utf8_lossy(&output.stdout);
        for line in stdout.lines() {
            let lower = line.to_lowercase();

            // IP アドレスパターン
            if contains_ip_pattern(line) {
                violations.push(format!(
                    "[{tag}] Binary string may contain IP address: \"{}\"",
                    truncate_str(line, 60)
                ));
            }

            // ファイルパス
            if line.contains("/home/")
                || line.contains("/tmp/")
                || line.contains("/etc/")
                || line.contains("C:\\")
                || line.contains("\\\\")
            {
                violations.push(format!(
                    "[{tag}] Binary string may contain file path: \"{}\"",
                    truncate_str(line, 60)
                ));
            }

            // URL
            if lower.contains("http://") || lower.contains("https://") || lower.contains("ftp://") {
                violations.push(format!(
                    "[{tag}] Binary string may contain URL: \"{}\"",
                    truncate_str(line, 60)
                ));
            }

            // デバッグ系キーワード
            for keyword in &["debug", "todo", "fixme"] {
                if lower.contains(keyword) {
                    violations.push(format!(
                        "[{tag}] Binary string contains debug keyword \"{keyword}\": \"{}\"",
                        truncate_str(line, 60)
                    ));
                    break;
                }
            }

            // 資格情報関連キーワード
            for keyword in &[
                "password",
                "passwd",
                "secret",
                "api_key",
                "token",
                "credential",
            ] {
                if lower.contains(keyword) {
                    violations.push(format!(
                        "[{tag}] Binary string contains sensitive keyword \"{keyword}\": \"{}\"",
                        truncate_str(line, 60)
                    ));
                    break;
                }
            }
        }
    }
    // strings が実行できなかった場合: deny ポリシーでは fail-closed
    if !strings_ran {
        if policy == OpsecPolicy::Deny {
            return Err(CompileError::OpsecViolation(
                "binary audit: 'strings' command not available (required for deny policy)"
                    .to_string(),
            ));
        }
        eprintln!("[OPSEC] warning: 'strings' command not available, binary audit skipped");
        return Ok(());
    }

    // nm コマンドでユーザー定義シンボルをフラグ（informational）
    // ツールチェイン由来の既知シンボルは除外
    const TOOLCHAIN_SYMBOLS: &[&str] = &[
        "deregister_tm_clones",
        "register_tm_clones",
        "frame_dummy",
        "__do_global_dtors_aux",
        "__libc_csu_init",
        "__libc_csu_fini",
        "__libc_start_main",
        "_dl_relocate_static_pie",
        "_fini",
        "_init",
        "_start",
    ];
    if let Ok(output) = Command::new("nm").arg(binary_path).output()
        && output.status.success()
    {
        let stdout = String::from_utf8_lossy(&output.stdout);
        for line in stdout.lines() {
            // nm 出力形式: "addr T symbol_name"
            let parts: Vec<&str> = line.split_whitespace().collect();
            if parts.len() >= 3 && (parts[1] == "T" || parts[1] == "t") {
                let sym = parts[2];
                // main, _ プレフィックス, . プレフィックス, ツールチェイン由来シンボルをスキップ
                if sym != "main"
                    && !sym.starts_with('_')
                    && !sym.starts_with('.')
                    && !TOOLCHAIN_SYMBOLS.contains(&sym)
                {
                    eprintln!("[OPSEC AUDIT INFO] User-defined symbol in binary: {sym}");
                }
            }
        }
    }
    // nm が存在しない場合は黙ってスキップ（informational のみなので非致命的）

    // 違反を一括出力
    for v in &violations {
        eprintln!("{v}");
    }

    if policy == OpsecPolicy::Deny && !violations.is_empty() {
        return Err(CompileError::OpsecViolation(format!(
            "binary audit: {} violation(s) detected",
            violations.len()
        )));
    }

    if violations.is_empty() {
        eprintln!("[OPSEC] binary audit passed");
    }

    Ok(())
}

/// 文字列中に IP アドレスパターン（N.N.N.N）が含まれるか簡易判定
fn contains_ip_pattern(s: &str) -> bool {
    let bytes = s.as_bytes();
    let len = bytes.len();
    let mut i = 0;
    while i < len {
        if bytes[i].is_ascii_digit() {
            let mut dots = 0;
            let mut j = i;
            let mut valid = true;
            for _ in 0..4 {
                if j >= len || !bytes[j].is_ascii_digit() {
                    valid = false;
                    break;
                }
                let start = j;
                while j < len && bytes[j].is_ascii_digit() {
                    j += 1;
                }
                if j - start > 3 {
                    valid = false;
                    break;
                }
                dots += 1;
                if dots < 4 {
                    if j >= len || bytes[j] != b'.' {
                        valid = false;
                        break;
                    }
                    j += 1;
                }
            }
            if valid && dots == 4 {
                return true;
            }
        }
        i += 1;
    }
    false
}

/// 文字列を最大 max_len 文字（char 単位）に切り詰める
fn truncate_str(s: &str, max_len: usize) -> String {
    if s.chars().count() <= max_len {
        s.to_string()
    } else {
        let truncated: String = s.chars().take(max_len).collect();
        format!("{truncated}...")
    }
}
