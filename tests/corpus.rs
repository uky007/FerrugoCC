//! E2E テスト — 実コーパス
//!
//! 外部の実 C プロジェクトを FerrugoCC でコンパイルし、
//! 正しい結果を返すことを検証する。

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
                    if !sym.is_empty()
                        && !sym.starts_with('.')
                        && !sym.starts_with('*')
                    {
                        new_line = new_line.replacen(
                            &format!("{prefix}{sym}"),
                            &format!("{prefix}_{sym}"),
                            1,
                        );
                    }
                }
            }
            // Prefix _ on sym(%rip) references (data/function addresses)
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
                    let replacement = format!("_{sym}(%rip)");
                    let original = format!("{sym}(%rip)");
                    new_line = new_line.replacen(&original, &replacement, 1);
                    // Skip past the replacement to avoid infinite loop
                    search_from = sym_start + replacement.len();
                } else {
                    search_from = rip_idx + 6; // skip past "(%rip)"
                }
            }
        }
        result.push(new_line);
    }
    result.join("\n") + "\n"
}

/// jsmn (JSON parser) をコンパイル・リンク・実行して正しい結果を返すことを確認。
fn compile_and_run_corpus(source_path: &str, obfuscate: bool) -> i32 {
    let dir = TempDir::new().unwrap();
    let asm_path = dir.path().join("test.s");
    let bin_path = dir.path().join("test");

    let mut cmd = Command::new(env!("CARGO_BIN_EXE_ferrugocc"));
    if obfuscate {
        cmd.arg("--fobfuscate");
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

    assert!(
        output.status.success(),
        "compilation failed (obfuscate={obfuscate}):\nstderr: {}",
        String::from_utf8_lossy(&output.stderr),
    );

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

// ── inih (Tier 1) ──

#[test]
fn inih_compile_and_run() {
    if !can_run_x86_64() {
        return;
    }
    assert_eq!(
        compile_and_run_corpus("corpus/inih/test_inih.c", false),
        42
    );
}

#[test]
fn inih_compile_and_run_obfuscated() {
    if !can_run_x86_64() {
        return;
    }
    assert_eq!(
        compile_and_run_corpus("corpus/inih/test_inih.c", true),
        42
    );
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
