//! E2E テスト — 外部プリプロセッサ統合
//!
//! gcc -E -P による前処理を経たコンパイルの検証。
//! #define, #include "local.h", const 修飾子の受理を確認。

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

fn compile_and_run(source: &str, obfuscate: bool) -> i32 {
    let dir = TempDir::new().unwrap();
    let src_path = dir.path().join("test.c");
    let asm_path = dir.path().join("test.s");
    let bin_path = dir.path().join("test");

    std::fs::write(&src_path, source).unwrap();

    let mut cmd = Command::new(env!("CARGO_BIN_EXE_ferrugocc"));
    if obfuscate {
        cmd.arg("--fobfuscate");
    }
    cmd.arg("-S").arg(&src_path);

    let output = cmd.output().expect("failed to run compiler");
    assert!(
        output.status.success(),
        "compilation failed (obfuscate={obfuscate}):\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
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
                    new_line = new_line.replace(&format!("call {sym}"), &format!("call _{sym}"));
                    new_line = new_line.replace(&format!("call\t{sym}"), &format!("call\t_{sym}"));
                }
            }
        }
        result.push(new_line);
    }
    result.join("\n") + "\n"
}

// ── #define ──

#[test]
fn preprocess_define_constant() {
    if !can_run_x86_64() {
        return;
    }
    let src = r#"
#define ANSWER 42
int main(void) {
    return ANSWER;
}
"#;
    assert_eq!(compile_and_run(src, false), 42);
}

#[test]
fn preprocess_define_expression() {
    if !can_run_x86_64() {
        return;
    }
    let src = r#"
#define A 10
#define B 20
#define SUM (A + B)
int main(void) {
    return SUM + 2;  // 32
}
"#;
    assert_eq!(compile_and_run(src, false), 32);
}

#[test]
fn preprocess_define_function_macro() {
    if !can_run_x86_64() {
        return;
    }
    let src = r#"
#define MAX(a, b) ((a) > (b) ? (a) : (b))
int main(void) {
    return MAX(30, 42);  // 42
}
"#;
    assert_eq!(compile_and_run(src, false), 42);
}

// ── #include "local.h" ──

#[test]
fn preprocess_include_local() {
    if !can_run_x86_64() {
        return;
    }
    let dir = TempDir::new().unwrap();

    // ヘッダファイル
    let header = r#"
#define MAGIC 42
int add(int a, int b);
"#;
    std::fs::write(dir.path().join("myheader.h"), header).unwrap();

    // ソースファイル
    let source = r#"
#include "myheader.h"
int add(int a, int b) { return a + b; }
int main(void) {
    return add(MAGIC, 8) - 8;  // 42
}
"#;
    let src_path = dir.path().join("test.c");
    let asm_path = dir.path().join("test.s");
    let bin_path = dir.path().join("test");

    std::fs::write(&src_path, source).unwrap();

    let mut cmd = Command::new(env!("CARGO_BIN_EXE_ferrugocc"));
    cmd.arg("-S").arg(&src_path);
    let output = cmd.output().expect("failed to run compiler");
    assert!(
        output.status.success(),
        "compilation failed:\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
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
        "gcc failed:\nstderr: {}",
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

    assert_eq!(run_output.status.code().unwrap_or(-1), 42);
}

// ── #ifdef ──

#[test]
fn preprocess_ifdef() {
    if !can_run_x86_64() {
        return;
    }
    let src = r#"
#define USE_FAST 1

int main(void) {
#ifdef USE_FAST
    return 42;
#else
    return 0;
#endif
}
"#;
    assert_eq!(compile_and_run(src, false), 42);
}

// ── const 修飾子と #define の組み合わせ ──

#[test]
fn preprocess_const_with_define() {
    if !can_run_x86_64() {
        return;
    }
    let src = r#"
#define TABLE_SIZE 4

int main(void) {
    const int size = TABLE_SIZE;
    int result = 0;
    int i = 0;
    for (i = 0; i < size; i = i + 1) {
        result = result + i;
    }
    return result;  // 0+1+2+3 = 6
}
"#;
    assert_eq!(compile_and_run(src, false), 6);
}

// ── Mirai 風断片: マクロ定数 + ビット演算 + const ──

#[test]
fn mirai_style_fragment() {
    if !can_run_x86_64() {
        return;
    }
    let src = r#"
#define TABLE_KEY 0x5A
#define TABLE_SIZE 4

int main(void) {
    int table[4];
    table[0] = 42 ^ TABLE_KEY;
    table[1] = 17 ^ TABLE_KEY;
    table[2] = 99 ^ TABLE_KEY;
    table[3] = 7 ^ TABLE_KEY;

    // Decrypt
    int sum = 0;
    int i = 0;
    for (i = 0; i < TABLE_SIZE; i = i + 1) {
        sum = sum + (table[i] ^ TABLE_KEY);
    }
    // sum = 42 + 17 + 99 + 7 = 165
    return sum - 123;  // 42
}
"#;
    assert_eq!(compile_and_run(src, false), 42);
}

// ── システムヘッダ受理（Phase 1.5: GCC extension tolerance） ──

/// `gcc -E -P` 出力を `--parse` で受理できることを確認。
/// 各ヘッダは __attribute__, __extension__, restrict 等の GCC 拡張を含む。
fn assert_system_header_parses(header: &str) {
    let dir = TempDir::new().unwrap();
    let source = format!("#include <{header}>\nint main(void) {{ return 0; }}\n");
    let src_path = dir.path().join("test.c");
    std::fs::write(&src_path, &source).unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_ferrugocc"))
        .arg("--parse")
        .arg(&src_path)
        .output()
        .expect("failed to run compiler");

    assert!(
        output.status.success(),
        "parsing {header} failed:\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
}

#[test]
fn system_header_stdio() {
    assert_system_header_parses("stdio.h");
}

#[test]
fn system_header_stdlib() {
    assert_system_header_parses("stdlib.h");
}

#[test]
fn system_header_string() {
    assert_system_header_parses("string.h");
}

#[test]
fn system_header_stdint() {
    assert_system_header_parses("stdint.h");
}

#[test]
fn system_header_sys_types() {
    assert_system_header_parses("sys/types.h");
}

// ── エラーケース ──

#[test]
fn preprocess_missing_header_fails() {
    let dir = TempDir::new().unwrap();
    let source = r#"
#include "nonexistent_header.h"
int main(void) { return 0; }
"#;
    let src_path = dir.path().join("test.c");
    std::fs::write(&src_path, source).unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_ferrugocc"))
        .arg("-S")
        .arg(&src_path)
        .output()
        .expect("failed to run compiler");

    assert!(
        !output.status.success(),
        "compilation should fail for missing header"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("preprocessing failed") || stderr.contains("nonexistent_header.h"),
        "stderr should mention preprocessing failure: {stderr}"
    );
}

#[test]
fn preprocess_error_directive_fails() {
    let dir = TempDir::new().unwrap();
    let source = "#error \"intentional error\"\nint main(void) { return 0; }\n";
    let src_path = dir.path().join("test.c");
    std::fs::write(&src_path, source).unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_ferrugocc"))
        .arg("-S")
        .arg(&src_path)
        .output()
        .expect("failed to run compiler");

    assert!(
        !output.status.success(),
        "compilation should fail for #error directive"
    );
}
