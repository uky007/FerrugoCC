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
    let mut result = Vec::new();
    let mut symbols: std::collections::HashSet<String> = std::collections::HashSet::new();
    for line in asm.lines() {
        let trimmed = line.trim();
        if let Some(rest) = trimmed.strip_prefix(".globl ") {
            symbols.insert(rest.trim().to_string());
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
            if symbols.contains(label) {
                new_line = format!("_{label}:");
            }
        } else {
            for sym in &symbols {
                if new_line.contains(&format!("call {sym}"))
                    || new_line.contains(&format!("call\t{sym}"))
                {
                    new_line = new_line.replace(
                        &format!("call {sym}"),
                        &format!("call _{sym}"),
                    );
                    new_line = new_line.replace(
                        &format!("call\t{sym}"),
                        &format!("call\t_{sym}"),
                    );
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
