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
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
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
fn preprocess_external(
    source_path: &Path,
    pp_defines: &[String],
    pp_undefs: &[String],
) -> Result<String> {
    let source_dir = source_path.parent().unwrap_or(Path::new("."));
    let mut cmd = Command::new("gcc");
    cmd.arg("-E").arg("-P").arg("-I").arg(source_dir);
    for undef in pp_undefs {
        cmd.arg(format!("-U{undef}"));
    }
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

/// 単一の .c ファイルをコンパイルパイプラインで処理する。
/// `stage` で停止。EmitAsm/Full まで進んだ場合は .s ファイルのパスを返す。
fn compile_one(
    source_path: &Path,
    stage: Stage,
    obf_config: &Option<ObfuscationConfig>,
    preprocess: PreprocessMode,
    pp_defines: &[String],
    pp_undefs: &[String],
) -> Result<Option<PathBuf>> {
    let source = match preprocess {
        PreprocessMode::None => std::fs::read_to_string(source_path)?,
        PreprocessMode::External => preprocess_external(source_path, pp_defines, pp_undefs)?,
    };

    let tokens = lex::lex(&source)?;
    if stage == Stage::Lex {
        return Ok(None);
    }

    let mut program = parse::parse(&tokens)?;
    if stage == Stage::Parse {
        return Ok(None);
    }

    typecheck::typecheck(&mut program)?;
    if stage == Stage::Validate {
        return Ok(None);
    }

    let tacky_program = tacky::generate_tacky(&program)?;
    if stage == Stage::Tacky {
        return Ok(None);
    }

    let tacky_program = if let Some(config) = obf_config.as_ref() {
        tacky::obfuscate(tacky_program, config)?
    } else {
        tacky::optimize(tacky_program)
    };

    let asm_program = codegen::generate(&tacky_program, obf_config.as_ref())?;
    if stage == Stage::Codegen {
        return Ok(None);
    }

    let asm_text = emit::emit(&asm_program)?;
    let asm_path = asm_output_path(source_path);
    std::fs::write(&asm_path, &asm_text)?;
    Ok(Some(asm_path))
}

/// .s → .o にアセンブルする（gcc -c）
fn assemble_to_object(asm_path: &Path, obj_path: &Path) -> Result<()> {
    let mut cmd = Command::new("gcc");
    cmd.arg("-c").arg(asm_path).arg("-o").arg(obj_path);
    if cfg!(target_os = "linux") {
        cmd.arg("-no-pie");
    }
    let status = cmd.status()?;
    if !status.success() {
        return Err(CompileError::ExternalToolError(format!(
            "gcc -c failed for {}",
            asm_path.display()
        )));
    }
    Ok(())
}

/// 複数の .o をリンクして実行ファイルを生成する
fn link_objects(objects: &[PathBuf], output_path: &Path) -> Result<()> {
    let mut cmd = Command::new("gcc");
    for obj in objects {
        cmd.arg(obj);
    }
    cmd.arg("-o").arg(output_path);
    if cfg!(target_os = "linux") {
        cmd.arg("-no-pie");
    }
    let status = cmd.status()?;
    if !status.success() {
        return Err(CompileError::ExternalToolError(format!(
            "gcc link failed (exit {})",
            status
        )));
    }
    Ok(())
}

/// 複数ファイル対応のコンパイルパイプライン。
///
/// 各 .c ファイルを独立にコンパイルし、最後に gcc でリンクする。
/// .o ファイルはそのままリンカに渡す。
#[allow(clippy::too_many_arguments)]
pub fn run_multi(
    sources: &[PathBuf],
    stage: Stage,
    obf_config: Option<ObfuscationConfig>,
    preprocess: PreprocessMode,
    pp_defines: &[String],
    pp_undefs: &[String],
    compile_only: bool,
    output: Option<&Path>,
) -> Result<()> {
    // 入力ファイルの分類
    let mut c_files: Vec<&Path> = Vec::new();
    let mut o_files: Vec<PathBuf> = Vec::new();
    for source in sources {
        match source.extension().and_then(|e| e.to_str()) {
            Some("c") | Some("h") => c_files.push(source),
            Some("o") => o_files.push(source.clone()),
            _ => {
                return Err(CompileError::ExternalToolError(format!(
                    "unrecognized file type: {}",
                    source.display()
                )));
            }
        }
    }

    if compile_only && output.is_some() && c_files.len() > 1 {
        return Err(CompileError::ExternalToolError(
            "-o cannot be used with -c and multiple source files".to_string(),
        ));
    }

    // Multi-file または -c（リンク用 .o 生成）の場合、global シンボルの OPSEC リネームを抑制
    let mut obf_config = obf_config;
    if ((c_files.len() + o_files.len()) > 1 || compile_only)
        && let Some(ref mut config) = obf_config
    {
        config.preserve_globals = true;
    }

    // 各 .c ファイルをコンパイル
    let mut asm_paths: Vec<PathBuf> = Vec::new();
    for c_file in &c_files {
        if let Some(asm_path) =
            compile_one(c_file, stage, &obf_config, preprocess, pp_defines, pp_undefs)?
        {
            asm_paths.push(asm_path);
        }
    }

    // EmitAsm 以前のステージでは .s も生成しない
    if stage < Stage::EmitAsm {
        return Ok(());
    }
    // -S: .s ファイル出力で停止
    if stage == Stage::EmitAsm {
        return Ok(());
    }

    // -c: .s → .o にアセンブルして停止（リンクしない）
    if compile_only {
        for asm_path in &asm_paths {
            let obj_path = if let Some(out) = output {
                out.to_path_buf()
            } else {
                asm_path.with_extension("o")
            };
            assemble_to_object(asm_path, &obj_path)?;
            let _ = std::fs::remove_file(asm_path);
        }
        return Ok(());
    }

    // Full: .s → .o → リンク
    let mut all_objects: Vec<PathBuf> = Vec::new();
    for asm_path in &asm_paths {
        let obj_path = asm_path.with_extension("o");
        assemble_to_object(asm_path, &obj_path)?;
        let _ = std::fs::remove_file(asm_path);
        all_objects.push(obj_path);
    }
    all_objects.extend(o_files);

    let output_path = if let Some(out) = output {
        out.to_path_buf()
    } else if c_files.len() == 1 && all_objects.len() == 1 {
        // 後方互換: 単一 .c → 拡張子なしバイナリ
        c_files[0].with_extension("")
    } else {
        PathBuf::from("a.out")
    };

    link_objects(&all_objects, &output_path)?;

    // 生成した .o を掃除（引数で渡された .o は残す）
    for asm_path in &asm_paths {
        let obj_path = asm_path.with_extension("o");
        let _ = std::fs::remove_file(&obj_path);
    }

    // OPSEC strip
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

    // ELF ウォーターマーク埋め込み（strip の後に実行 — strip が e_ident padding をクリアするため）
    apply_elf_watermark(&output_path)?;

    // OPSEC バイナリ監査
    if let Some(config) = &obf_config
        && config.opsec_audit
    {
        opsec_audit_binary(&output_path, config.opsec_policy)?;
    }

    Ok(())
}

/// 単一ファイルコンパイル（後方互換ラッパー）
#[allow(dead_code)]
pub fn run(
    source_path: &Path,
    stage: Stage,
    obf_config: Option<ObfuscationConfig>,
    preprocess: PreprocessMode,
    pp_defines: &[String],
    pp_undefs: &[String],
) -> Result<()> {
    run_multi(
        &[source_path.to_path_buf()],
        stage,
        obf_config,
        preprocess,
        pp_defines,
        pp_undefs,
        false,
        None,
    )
}

/// リンク後バイナリの OPSEC 監査
///
/// `strings` コマンドでバイナリ内の文字列をスキャンし、
/// ELF バイナリに LSB ステガノグラフィでウォーターマークを埋め込む。
/// e_ident[9..16] (EI_PAD, 7 bytes) に "FERRUGO" の各ビットを LSB エンコードし、
/// e_flags (offset 48..52, 4 bytes) にバージョン情報を LSB エンコードする。
/// ELF 以外（macOS Mach-O 等）の場合はサイレントにスキップする。
fn apply_elf_watermark(binary_path: &Path) -> Result<()> {
    let mut data = std::fs::read(binary_path)
        .map_err(|e| CompileError::ExternalToolError(format!("read binary: {e}")))?;

    // ELF magic check: 0x7F 'E' 'L' 'F'
    if data.len() < 64 || data[0..4] != [0x7F, b'E', b'L', b'F'] {
        return Ok(()); // Not ELF, skip silently
    }

    // Verify 64-bit ELF (ELFCLASS64)
    if data[4] != 2 {
        return Ok(());
    }

    // LSB encode "FERRUGO" (7 bytes) into e_ident[9..16] (EI_PAD)
    let magic = b"FERRUGO";
    for (i, &byte) in magic.iter().enumerate() {
        // Spread 8 bits of magic[i] across byte at position 9+i using LSB
        // Since EI_PAD is normally all zeros, we write the LSB directly
        data[9 + i] = (data[9 + i] & 0xFE) | (byte & 0x01);
    }

    // LSB encode version [0x01, 0x00, 0x00, 0x00] into e_flags (offset 48..52)
    let version: [u8; 4] = [0x01, 0x00, 0x00, 0x00];
    for (i, &byte) in version.iter().enumerate() {
        data[48 + i] = (data[48 + i] & 0xFE) | (byte & 0x01);
    }

    std::fs::write(binary_path, &data)
        .map_err(|e| CompileError::ExternalToolError(format!("write binary: {e}")))?;

    Ok(())
}

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
