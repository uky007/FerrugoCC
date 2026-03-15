//! E2E テスト — 固定回帰コーパス
//!
//! 外部の実 C プロジェクトを FerrugoCC でコンパイルし、
//! 正しい結果を返すことを検証する。
//!
//! ## Tier 1 (必須・CI 必須経路)
//! - **jsmn**: JSON parser (~500 行) — 最小の自己完結コーパス
//! - **inih**: INI parser (~200 行) — 文字列処理 + コールバック
//! - **sds**: 動的文字列ライブラリ (~1200 行) — ポインタ演算 + 可変長引数
//!
//! 各コーパスは normal + obfuscated の両モードでテストする。
//! 各 corpus ディレクトリの ORIGIN ファイルに upstream 由来情報を記載。

use std::process::Command;
use tempfile::TempDir;

fn can_run_x86_64() -> bool {
    if cfg!(target_arch = "x86_64") {
        return true;
    }
    Command::new("arch")
        .args(["-x86_64", "true"])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

fn fixup_asm_for_macos(asm: &str) -> String {
    use std::collections::HashSet;

    let mut result = Vec::new();
    // Collect all non-local symbols (labels without . prefix)
    let mut all_symbols: HashSet<String> = HashSet::new();
    for line in asm.lines() {
        let trimmed = line.trim();
        if let Some(rest) = trimmed.strip_prefix(".globl ") {
            all_symbols.insert(rest.trim().to_string());
        }
        // Labels: non-local (no . prefix)
        if trimmed.ends_with(':') && !trimmed.starts_with('.') {
            let label = trimmed.trim_end_matches(':');
            all_symbols.insert(label.to_string());
        }
    }

    for line in asm.lines() {
        let trimmed = line.trim();
        if trimmed.contains(".note.GNU-stack") {
            continue;
        }
        if trimmed.starts_with(".section .rodata") {
            result.push("    .section __TEXT,__const".to_string());
            continue;
        }
        let mut new_line = line.to_string();
        if let Some(rest) = trimmed.strip_prefix(".globl ") {
            let sym = rest.trim();
            new_line = format!("    .globl _{sym}");
        } else if trimmed.ends_with(':') && !trimmed.starts_with('.') {
            let label = trimmed.trim_end_matches(':');
            new_line = format!("_{label}:");
        } else {
            // Prefix _ on call targets (non-local, non-indirect)
            for prefix in &["call ", "call\t"] {
                if let Some(idx) = new_line.find(prefix) {
                    let after = &new_line[idx + prefix.len()..];
                    let sym = after.split_whitespace().next().unwrap_or("");
                    if !sym.is_empty() && !sym.starts_with('.') && !sym.starts_with('*') {
                        new_line = new_line.replacen(
                            &format!("{prefix}{sym}"),
                            &format!("{prefix}_{sym}"),
                            1,
                        );
                    }
                }
            }
            // Prefix _ on data directives referencing symbols (.quad sym, .long sym)
            for directive in &[".quad ", ".long "] {
                if let Some(idx) = trimmed.find(directive) {
                    let after = &trimmed[idx + directive.len()..];
                    let sym = after.split_whitespace().next().unwrap_or("");
                    if !sym.is_empty()
                        && !sym.starts_with('.')
                        && sym
                            .chars()
                            .next()
                            .is_some_and(|c| c.is_alphabetic() || c == '_')
                    {
                        new_line = new_line.replacen(
                            &format!("{directive}{sym}"),
                            &format!("{directive}_{sym}"),
                            1,
                        );
                    }
                }
            }
            // Prefix _ on sym(%rip) references (data/function addresses)
            // External symbols need @GOTPCREL for leaq on macOS
            let mut search_from = 0;
            while let Some(rel_idx) = new_line[search_from..].find("(%rip)") {
                let rip_idx = search_from + rel_idx;
                // Find the symbol before (%rip)
                let before = &new_line[..rip_idx];
                // Walk backwards to find the start of the symbol (include . for local labels)
                let sym_start = before
                    .rfind(|c: char| !c.is_alphanumeric() && c != '_' && c != '.')
                    .map(|i| i + 1)
                    .unwrap_or(0);
                let sym = &new_line[sym_start..rip_idx];
                if !sym.is_empty() && !sym.starts_with('.') {
                    let is_external = !all_symbols.contains(sym);
                    let trimmed_line = new_line.trim_start();
                    if is_external && trimmed_line.starts_with("leaq ") {
                        // leaq external(%rip), %reg → movq _external@GOTPCREL(%rip), %reg
                        let original = format!("leaq {sym}(%rip)");
                        let replacement = format!("movq _{sym}@GOTPCREL(%rip)");
                        new_line = new_line.replacen(&original, &replacement, 1);
                        search_from = sym_start + replacement.len();
                    } else if is_external
                        && (trimmed_line.starts_with("movq ") || trimmed_line.starts_with("movl "))
                    {
                        // movq/movl external(%rip), %reg → GOT load + deref
                        let is_quad = trimmed_line.starts_with("movq ");
                        // Extract destination register
                        let after_rip = &new_line[rip_idx + 6..]; // after "(%rip)"
                        let dst_part = after_rip.trim().trim_start_matches(',').trim();
                        let dst_reg = dst_part.split_whitespace().next().unwrap_or("").to_string();
                        // Use the 64-bit version of the register for GOT load
                        let got_reg = if let Some(r) = dst_reg.strip_prefix('%') {
                            // Convert 32-bit reg to 64-bit for GOT load
                            let r64 = match r {
                                "eax" => "rax",
                                "ebx" => "rbx",
                                "ecx" => "rcx",
                                "edx" => "rdx",
                                "esi" => "rsi",
                                "edi" => "rdi",
                                "r8d" => "r8",
                                "r9d" => "r9",
                                "r10d" => "r10",
                                "r11d" => "r11",
                                "r12d" => "r12",
                                "r13d" => "r13",
                                "r14d" => "r14",
                                "r15d" => "r15",
                                other => other,
                            };
                            format!("%{r64}")
                        } else {
                            dst_reg.clone()
                        };
                        let deref_instr = if is_quad { "movq" } else { "movl" };
                        new_line = format!(
                            "    movq _{sym}@GOTPCREL(%rip), {got_reg}\n    {deref_instr} ({got_reg}), {dst_reg}"
                        );
                        search_from = new_line.len();
                    } else {
                        let replacement = format!("_{sym}(%rip)");
                        let original = format!("{sym}(%rip)");
                        new_line = new_line.replacen(&original, &replacement, 1);
                        search_from = sym_start + replacement.len();
                    }
                } else {
                    search_from = rip_idx + 6; // skip past "(%rip)"
                }
            }
        }
        result.push(new_line);
    }
    result.join("\n") + "\n"
}

/// Linux glibc 環境で FORTIFY_SOURCE inline wrapper を抑制するための undefs。
/// macOS では不要（ヘッダ構成が異なる）。
fn platform_undefs() -> Vec<&'static str> {
    if cfg!(target_os = "linux") {
        vec!["_FORTIFY_SOURCE"]
    } else {
        vec![]
    }
}

/// Linux glibc 環境で必要な追加 defines（全 corpus 共通）。
fn platform_defines() -> Vec<&'static str> {
    vec![]
}

/// コンパイラ stderr からエラー行番号を抽出する。
fn extract_error_line(stderr: &str) -> Option<usize> {
    // "at line 274, column 42" or "at line 1460, column 24"
    for part in stderr.split("at line ") {
        if let Some(comma) = part.find(',')
            && let Ok(n) = part[..comma].trim().parse::<usize>()
        {
            return Some(n);
        }
    }
    None
}

/// corpus テストの共通ヘルパー: コンパイル → リンク → 実行 → exit code を返す。
fn compile_and_run_corpus(source_path: &str, obfuscate: bool) -> i32 {
    let dir = TempDir::new().unwrap();
    let asm_path = dir.path().join("test.s");
    let bin_path = dir.path().join("test");

    let mut cmd = Command::new(env!("CARGO_BIN_EXE_ferrugocc"));
    if obfuscate {
        cmd.arg("--fobfuscate");
    }
    for undef in platform_undefs() {
        cmd.arg(format!("-U{undef}"));
    }
    for def in platform_defines() {
        cmd.arg(format!("-D{def}"));
    }
    cmd.arg("-S").arg(source_path);
    // Override output path
    cmd.env("FERRUGOCC_ASM_OUTPUT", asm_path.to_str().unwrap());

    let output = cmd.output().expect("failed to run compiler");

    // The compiler writes .s next to the source; copy it
    let source = std::path::Path::new(source_path);
    let default_asm = source.with_extension("s");
    if default_asm.exists() {
        std::fs::copy(&default_asm, &asm_path).unwrap();
        let _ = std::fs::remove_file(&default_asm);
    }

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let diag = extract_error_line(&stderr)
            .map(|line| dump_preprocessed_context(source_path, line))
            .unwrap_or_default();
        panic!("compilation failed (obfuscate={obfuscate}):\nstderr: {stderr}{diag}");
    }

    assert!(asm_path.exists(), "assembly file not generated");

    if cfg!(target_os = "macos") {
        let asm = std::fs::read_to_string(&asm_path).unwrap();
        let asm = fixup_asm_for_macos(&asm);
        std::fs::write(&asm_path, asm).unwrap();
    }

    let gcc_output = if cfg!(target_arch = "x86_64") {
        Command::new("gcc")
            .arg(&asm_path)
            .arg("-o")
            .arg(&bin_path)
            .output()
            .expect("failed to run gcc")
    } else {
        Command::new("arch")
            .args(["-x86_64", "gcc"])
            .arg(&asm_path)
            .arg("-o")
            .arg(&bin_path)
            .output()
            .expect("failed to run arch -x86_64 gcc")
    };

    assert!(
        gcc_output.status.success(),
        "gcc failed (obfuscate={obfuscate}):\nstderr: {}",
        String::from_utf8_lossy(&gcc_output.stderr),
    );

    let run_output = if cfg!(target_arch = "x86_64") {
        Command::new(&bin_path)
            .output()
            .expect("failed to run binary")
    } else {
        Command::new("arch")
            .arg("-x86_64")
            .arg(&bin_path)
            .output()
            .expect("failed to run binary via arch -x86_64")
    };

    run_output.status.code().unwrap_or(-1)
}

/// 拡張版ヘルパー: -D/-U フラグ + 実行引数対応。exit code, stdout, stderr を返す。
fn compile_and_run_corpus_ex(
    source_path: &str,
    obfuscate: bool,
    defines: &[&str],
    run_args: &[&str],
) -> (i32, String, String) {
    let dir = TempDir::new().unwrap();
    let asm_path = dir.path().join("test.s");
    let bin_path = dir.path().join("test");

    let mut cmd = Command::new(env!("CARGO_BIN_EXE_ferrugocc"));
    if obfuscate {
        cmd.arg("--fobfuscate");
    }
    for undef in platform_undefs() {
        cmd.arg(format!("-U{undef}"));
    }
    for def in platform_defines() {
        cmd.arg(format!("-D{def}"));
    }
    for def in defines {
        cmd.arg(format!("-D{def}"));
    }
    cmd.arg("-S").arg(source_path);
    cmd.env("FERRUGOCC_ASM_OUTPUT", asm_path.to_str().unwrap());

    let output = cmd.output().expect("failed to run compiler");

    let source = std::path::Path::new(source_path);
    let default_asm = source.with_extension("s");
    if default_asm.exists() {
        std::fs::copy(&default_asm, &asm_path).unwrap();
        let _ = std::fs::remove_file(&default_asm);
    }

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let diag = extract_error_line(&stderr)
            .map(|line| dump_preprocessed_context(source_path, line))
            .unwrap_or_default();
        panic!("compilation failed (obfuscate={obfuscate}):\nstderr: {stderr}{diag}");
    }
    assert!(asm_path.exists(), "assembly file not generated");

    if cfg!(target_os = "macos") {
        let asm = std::fs::read_to_string(&asm_path).unwrap();
        let asm = fixup_asm_for_macos(&asm);
        std::fs::write(&asm_path, asm).unwrap();
    }

    let gcc_output = if cfg!(target_arch = "x86_64") {
        let mut cmd = Command::new("gcc");
        cmd.arg(&asm_path).arg("-o").arg(&bin_path);
        if cfg!(target_os = "macos") {
            // macOS: 未使用のプラットフォームインライン関数が弱シンボルを参照するため
            cmd.arg("-Wl,-undefined,dynamic_lookup");
        }
        cmd.output().expect("failed to run gcc")
    } else {
        let mut cmd = Command::new("arch");
        cmd.args(["-x86_64", "gcc"])
            .arg(&asm_path)
            .arg("-o")
            .arg(&bin_path);
        if cfg!(target_os = "macos") {
            cmd.arg("-Wl,-undefined,dynamic_lookup");
        }
        cmd.output().expect("failed to run arch -x86_64 gcc")
    };

    assert!(
        gcc_output.status.success(),
        "gcc link failed (obfuscate={obfuscate}):\nstderr: {}",
        String::from_utf8_lossy(&gcc_output.stderr),
    );

    let run_output = if cfg!(target_arch = "x86_64") {
        Command::new(&bin_path)
            .args(run_args)
            .output()
            .expect("failed to run binary")
    } else {
        Command::new("arch")
            .arg("-x86_64")
            .arg(&bin_path)
            .args(run_args)
            .output()
            .expect("failed to run binary via arch -x86_64")
    };

    let code = run_output.status.code().unwrap_or(-1);
    let stdout = String::from_utf8_lossy(&run_output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&run_output.stderr).to_string();
    (code, stdout, stderr)
}

// ── diagnostic: dump preprocessed lines around known error locations on Linux ──

/// gcc -E -P の出力から error_line 周辺を返す診断ヘルパー。
fn dump_preprocessed_context(source_path: &str, error_line: usize) -> String {
    let source_dir = std::path::Path::new(source_path)
        .parent()
        .unwrap_or(std::path::Path::new("."));
    let mut cmd = Command::new("gcc");
    cmd.arg("-E").arg("-P").arg("-I").arg(source_dir);
    for undef in platform_undefs() {
        cmd.arg(format!("-U{undef}"));
    }
    for def in platform_defines() {
        cmd.arg(format!("-D{def}"));
    }
    cmd.arg(source_path);
    let output = match cmd.output() {
        Ok(o) if o.status.success() => o,
        _ => return "[DIAG] gcc -E -P failed".to_string(),
    };
    let pp = String::from_utf8_lossy(&output.stdout);
    let lines: Vec<&str> = pp.lines().collect();
    let start = error_line.saturating_sub(6);
    let end = (error_line + 5).min(lines.len());
    let mut buf = format!("\n[DIAG] preprocessed lines {start}..{end} (error at {error_line}):\n");
    for (i, line) in lines.iter().enumerate().take(end).skip(start) {
        let marker = if i + 1 == error_line { ">>>" } else { "   " };
        buf.push_str(&format!("{marker} {:>5}: {}\n", i + 1, line));
    }
    buf
}

// ── kilo (Tier 2) ──

/// kilo smoke test: コンパイル → リンク → 引数なし実行 → "Usage:" 出力確認
#[test]
fn kilo_compile_link_smoke() {
    if !can_run_x86_64() {
        return;
    }
    let (code, _stdout, stderr) = compile_and_run_corpus_ex(
        "corpus/kilo/kilo.c",
        false,
        &["_FORTIFY_SOURCE=0", "_DONT_USE_CTYPE_INLINE_"],
        &[], // no args → usage message
    );
    assert_eq!(
        code, 1,
        "kilo with no args should exit with 1, stderr: {stderr}"
    );
    assert!(
        stderr.contains("Usage:"),
        "kilo stderr should contain 'Usage:', got: {stderr}"
    );
}

/// kilo obfuscated smoke test: コンパイル → リンク → 引数なし実行 → "Usage:" 出力確認
#[test]
fn kilo_compile_link_smoke_obfuscated() {
    if !can_run_x86_64() {
        return;
    }
    let (code, _stdout, stderr) = compile_and_run_corpus_ex(
        "corpus/kilo/kilo.c",
        true,
        &["_FORTIFY_SOURCE=0", "_DONT_USE_CTYPE_INLINE_"],
        &[], // no args → usage message
    );
    assert_eq!(
        code, 1,
        "kilo obfuscated with no args should exit with 1, stderr: {stderr}"
    );
    assert!(
        stderr.contains("Usage:"),
        "kilo obfuscated stderr should contain 'Usage:', got: {stderr}"
    );
}

/// kilo unit test: 主要 C パターン (struct/realloc/memmove/snprintf/switch/bitwise/fn_ptr) → 42
#[test]
fn kilo_unit_test() {
    if !can_run_x86_64() {
        return;
    }
    let (code, _stdout, _stderr) = compile_and_run_corpus_ex(
        "corpus/kilo/kilo_unit.c",
        false,
        &["_FORTIFY_SOURCE=0", "_DONT_USE_CTYPE_INLINE_"],
        &[],
    );
    assert_eq!(code, 42, "kilo unit test failed with exit code {code}");
}

/// kilo unit test obfuscated → 42
#[test]
fn kilo_unit_test_obfuscated() {
    if !can_run_x86_64() {
        return;
    }
    let (code, _stdout, _stderr) = compile_and_run_corpus_ex(
        "corpus/kilo/kilo_unit.c",
        true,
        &["_FORTIFY_SOURCE=0", "_DONT_USE_CTYPE_INLINE_"],
        &[],
    );
    assert_eq!(
        code, 42,
        "kilo unit test obfuscated failed with exit code {code}"
    );
}

// ── inih (Tier 1) ──

/// inih は自前の isspace() を提供するため、glibc ctype.h を _CTYPE_H=1 で抑制。
fn inih_defines() -> Vec<&'static str> {
    if cfg!(target_os = "linux") {
        vec!["_CTYPE_H=1"]
    } else {
        vec![]
    }
}

#[test]
fn inih_compile_and_run() {
    if !can_run_x86_64() {
        return;
    }
    let (code, _stdout, _stderr) =
        compile_and_run_corpus_ex("corpus/inih/test_inih.c", false, &inih_defines(), &[]);
    assert_eq!(code, 42);
}

#[test]
fn inih_compile_and_run_obfuscated() {
    if !can_run_x86_64() {
        return;
    }
    let (code, _stdout, _stderr) =
        compile_and_run_corpus_ex("corpus/inih/test_inih.c", true, &inih_defines(), &[]);
    assert_eq!(code, 42);
}

// ── sds (Tier 1) ──

#[test]
fn sds_compile_and_run() {
    if !can_run_x86_64() {
        return;
    }
    assert_eq!(compile_and_run_corpus("corpus/sds/test_sds.c", false), 42);
}

#[test]
fn sds_compile_and_run_obfuscated() {
    if !can_run_x86_64() {
        return;
    }
    assert_eq!(compile_and_run_corpus("corpus/sds/test_sds.c", true), 42);
}

// ── jsmn (Tier 1) ──

#[test]
fn jsmn_compile_and_run() {
    if !can_run_x86_64() {
        return;
    }
    assert_eq!(compile_and_run_corpus("corpus/jsmn/test_jsmn.c", false), 42);
}

#[test]
fn jsmn_compile_and_run_obfuscated() {
    if !can_run_x86_64() {
        return;
    }
    assert_eq!(compile_and_run_corpus("corpus/jsmn/test_jsmn.c", true), 42);
}
