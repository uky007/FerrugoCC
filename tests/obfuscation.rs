//! E2E テスト — 難読化コンパイルの正当性検証
//!
//! C ソースを `--fobfuscate` 付きでコンパイル → 実行 → 正しい終了コードを検証。
//! 通常コンパイルとの結果一致も確認。
//!
//! 注: FerrugoCC は x86_64 アセンブリを出力するため、ARM64 macOS では
//! `arch -x86_64` 経由で gcc/実行を行う。

use std::process::Command;
use tempfile::TempDir;

/// x86_64 バイナリを実行可能か判定（ARM64 macOS で Rosetta が使えるか）
fn can_run_x86_64() -> bool {
    if cfg!(target_arch = "x86_64") {
        return true;
    }
    // ARM64 macOS: arch -x86_64 で Rosetta 経由で実行可能か
    Command::new("arch")
        .args(["-x86_64", "true"])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// テストヘルパー: C ソースをコンパイルして実行し、終了コードを返す。
///
/// FerrugoCC で .s まで生成 → gcc でバイナリ化 → 実行。
/// ARM64 macOS では `arch -x86_64 gcc` と `arch -x86_64` で実行する。
fn compile_and_run(source: &str, obfuscate: bool) -> i32 {
    let dir = TempDir::new().unwrap();
    let src_path = dir.path().join("test.c");
    let asm_path = dir.path().join("test.s");
    let bin_path = dir.path().join("test");

    std::fs::write(&src_path, source).unwrap();

    // Step 1: FerrugoCC で .s を生成
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

    // macOS 用: Linux 向けアセンブリを macOS 向けに変換
    if cfg!(target_os = "macos") {
        let asm = std::fs::read_to_string(&asm_path).unwrap();
        let asm = fixup_asm_for_macos(&asm);
        std::fs::write(&asm_path, asm).unwrap();
    }

    // Step 2: gcc でバイナリ化（ARM64 Mac では arch -x86_64 経由）
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

    // Step 3: 実行（ARM64 Mac では arch -x86_64 経由）
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

/// macOS 向けにアセンブリを修正する。
/// - `.section .note.GNU-stack,...` 行を削除
/// - シンボル名に `_` プレフィックスを付加（.globl と関数ラベル）
/// - `call` 命令のターゲットに `_` プレフィックスを付加
fn fixup_asm_for_macos(asm: &str) -> String {
    let mut result = Vec::new();
    // .globl で宣言されるシンボル名を収集
    let mut symbols: std::collections::HashSet<String> = std::collections::HashSet::new();
    for line in asm.lines() {
        let trimmed = line.trim();
        if let Some(rest) = trimmed.strip_prefix(".globl ") {
            symbols.insert(rest.trim().to_string());
        }
    }

    for line in asm.lines() {
        let trimmed = line.trim();

        // GNU-stack ディレクティブを削除
        if trimmed.contains(".note.GNU-stack") {
            continue;
        }

        // .section .rodata → .section __TEXT,__const (macOS)
        if trimmed == ".section .rodata" {
            result.push("    .section __TEXT,__const".to_string());
            continue;
        }

        // .globl シンボル → .globl _シンボル
        if let Some(rest) = trimmed.strip_prefix(".globl ") {
            let sym = rest.trim();
            result.push(format!("    .globl _{sym}"));
            continue;
        }

        // シンボルラベル定義: `main:` → `_main:`
        if let Some(label) = trimmed.strip_suffix(':') {
            if symbols.contains(label) {
                result.push(format!("_{label}:"));
                continue;
            }
        }

        // call 命令: `call func` → `call _func`
        if trimmed.starts_with("call ") {
            let target = trimmed.strip_prefix("call ").unwrap().trim();
            if symbols.contains(target) {
                result.push(format!("    call _{target}"));
                continue;
            }
        }

        // leaq 命令: `leaq func(%rip), reg` → `leaq _func(%rip), reg`
        // （間接呼び出し変換で生成される lea 命令の macOS 対応）
        if trimmed.starts_with("leaq ") {
            if let Some(rip_pos) = trimmed.find("(%rip)") {
                let after_leaq = &trimmed[5..rip_pos]; // "leaq " is 5 chars
                if symbols.contains(after_leaq) {
                    let suffix = &trimmed[rip_pos..];
                    result.push(format!("    leaq _{after_leaq}{suffix}"));
                    continue;
                }
            }
        }

        result.push(line.to_string());
    }

    result.join("\n") + "\n"
}

/// テストヘルパー: 指定レベルで難読化コンパイルして実行し、終了コードを返す。
fn compile_and_run_with_level(source: &str, level: u8) -> i32 {
    let dir = TempDir::new().unwrap();
    let src_path = dir.path().join("test.c");
    let asm_path = dir.path().join("test.s");
    let bin_path = dir.path().join("test");

    std::fs::write(&src_path, source).unwrap();

    // Step 1: FerrugoCC で .s を生成（指定レベルで難読化）
    let output = Command::new(env!("CARGO_BIN_EXE_ferrugocc"))
        .arg("--fobfuscate")
        .arg(format!("--obf-level={level}"))
        .arg("-S")
        .arg(&src_path)
        .output()
        .expect("failed to run compiler");
    assert!(
        output.status.success(),
        "compilation failed (level={level}):\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );

    assert!(asm_path.exists(), "assembly file not generated");

    if cfg!(target_os = "macos") {
        let asm = std::fs::read_to_string(&asm_path).unwrap();
        let asm = fixup_asm_for_macos(&asm);
        std::fs::write(&asm_path, asm).unwrap();
    }

    // Step 2: gcc でバイナリ化
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
        "gcc failed (level={level}):\nstderr: {}",
        String::from_utf8_lossy(&gcc_output.stderr),
    );

    // Step 3: 実行
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

/// テストヘルパー: 通常コンパイルと難読化コンパイルの結果を比較。
fn assert_obfuscation_preserves_behavior(source: &str, expected_exit_code: i32) {
    if !can_run_x86_64() {
        eprintln!("skipping: x86_64 execution not available");
        return;
    }

    let normal = compile_and_run(source, false);
    assert_eq!(normal, expected_exit_code, "normal compilation: expected {expected_exit_code}, got {normal}");

    let obfuscated = compile_and_run(source, true);
    assert_eq!(obfuscated, expected_exit_code, "obfuscated compilation: expected {expected_exit_code}, got {obfuscated}");
}

#[test]
fn test_constant_return() {
    assert_obfuscation_preserves_behavior(
        "int main(void) { return 42; }",
        42,
    );
}

#[test]
fn test_arithmetic() {
    assert_obfuscation_preserves_behavior(
        "int main(void) { int a = 10; int b = 20; return a + b; }",
        30,
    );
}

#[test]
fn test_conditional() {
    assert_obfuscation_preserves_behavior(
        "int main(void) { int x = 5; if (x > 3) return 1; return 0; }",
        1,
    );
}

#[test]
fn test_loop() {
    assert_obfuscation_preserves_behavior(
        r#"
        int main(void) {
            int s = 0;
            for (int i = 0; i < 10; i = i + 1)
                s = s + i;
            return s;
        }
        "#,
        45,
    );
}

#[test]
fn test_function_call() {
    assert_obfuscation_preserves_behavior(
        r#"
        int add(int a, int b) { return a + b; }
        int main(void) { return add(20, 22); }
        "#,
        42,
    );
}

#[test]
fn test_string_literal() {
    assert_obfuscation_preserves_behavior(
        r#"
        int main(void) {
            char *s = "Hello";
            return s[0];
        }
        "#,
        72, // 'H' == 72
    );
}

#[test]
fn test_pointer_aliasing() {
    if !can_run_x86_64() {
        eprintln!("skipping: x86_64 execution not available");
        return;
    }

    let source = r#"
        int main(void) {
            int x = 5;
            int *p = &x;
            *p = 100;
            return x;
        }
    "#;

    // 最適化後もポインタ経由の書き込みが正しく反映されることを検証
    let normal = compile_and_run(source, false);
    assert_eq!(normal, 100, "pointer aliasing: expected 100, got {normal}");
}

#[test]
fn test_nested_control_flow() {
    assert_obfuscation_preserves_behavior(
        r#"
        int main(void) {
            int r = 0;
            for (int i = 0; i < 5; i = i + 1) {
                if (i % 2 == 0)
                    r = r + i;
                else
                    r = r - 1;
            }
            return r;
        }
        "#,
        4,
    );
}

// ─────────────────────────────────────────────────────────────
// VM仮想化テスト（Level 4）
// ─────────────────────────────────────────────────────────────

#[test]
fn test_vm_simple_arithmetic() {
    if !can_run_x86_64() {
        eprintln!("skipping: x86_64 execution not available");
        return;
    }
    let source = r#"
        int add(int a, int b) { return a + b; }
        int main(void) { return add(20, 22); }
    "#;
    let normal = compile_and_run(source, false);
    assert_eq!(normal, 42, "normal: expected 42, got {normal}");
    let vm = compile_and_run_with_level(source, 4);
    assert_eq!(vm, 42, "vm level 4: expected 42, got {vm}");
}

#[test]
fn test_vm_conditional() {
    if !can_run_x86_64() {
        eprintln!("skipping: x86_64 execution not available");
        return;
    }
    let source = r#"
        int check(int x) {
            if (x > 10) return x + 32;
            else return x;
        }
        int main(void) { return check(10); }
    "#;
    let normal = compile_and_run(source, false);
    assert_eq!(normal, 10, "normal: expected 10, got {normal}");
    let vm = compile_and_run_with_level(source, 4);
    assert_eq!(vm, 10, "vm level 4: expected 10, got {vm}");
}

#[test]
fn test_vm_loop() {
    if !can_run_x86_64() {
        eprintln!("skipping: x86_64 execution not available");
        return;
    }
    let source = r#"
        int sum(int n) {
            int s = 0;
            for (int i = 0; i < n; i = i + 1)
                s = s + i;
            return s;
        }
        int main(void) { return sum(10); }
    "#;
    let normal = compile_and_run(source, false);
    assert_eq!(normal, 45, "normal: expected 45, got {normal}");
    let vm = compile_and_run_with_level(source, 4);
    assert_eq!(vm, 45, "vm level 4: expected 45, got {vm}");
}

#[test]
fn test_vm_nested_calls() {
    if !can_run_x86_64() {
        eprintln!("skipping: x86_64 execution not available");
        return;
    }
    let source = r#"
        int double_val(int x) { return x + x; }
        int add_doubled(int a, int b) { return double_val(a) + double_val(b); }
        int main(void) { return add_doubled(10, 10); }
    "#;
    let normal = compile_and_run(source, false);
    assert_eq!(normal, 40, "normal: expected 40, got {normal}");
    let vm = compile_and_run_with_level(source, 4);
    assert_eq!(vm, 40, "vm level 4: expected 40, got {vm}");
}

#[test]
fn test_vm_type_conversion() {
    if !can_run_x86_64() {
        eprintln!("skipping: x86_64 execution not available");
        return;
    }
    let source = r#"
        int narrow(long x) { return (int)x; }
        int main(void) { long v = 42; return narrow(v); }
    "#;
    let normal = compile_and_run(source, false);
    assert_eq!(normal, 42, "normal: expected 42, got {normal}");
    let vm = compile_and_run_with_level(source, 4);
    assert_eq!(vm, 42, "vm level 4: expected 42, got {vm}");
}

// ─────────────────────────────────────────────────────────────
// Pass 15: ライブラリ関数難読化テスト
// ─────────────────────────────────────────────────────────────

/// テストヘルパー: カスタムフラグで難読化コンパイルして実行し、終了コードを返す。
fn compile_and_run_with_flags(source: &str, extra_flags: &[&str]) -> i32 {
    let dir = TempDir::new().unwrap();
    let src_path = dir.path().join("test.c");
    let asm_path = dir.path().join("test.s");
    let bin_path = dir.path().join("test");

    std::fs::write(&src_path, source).unwrap();

    let mut cmd = Command::new(env!("CARGO_BIN_EXE_ferrugocc"));
    cmd.arg("--fobfuscate");
    for flag in extra_flags {
        cmd.arg(flag);
    }
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
            .arg(&asm_path).arg("-o").arg(&bin_path)
            .output().expect("failed to run gcc")
    } else {
        Command::new("arch")
            .args(["-x86_64", "gcc"])
            .arg(&asm_path).arg("-o").arg(&bin_path)
            .output().expect("failed to run arch -x86_64 gcc")
    };
    assert!(
        gcc_output.status.success(),
        "gcc failed:\nstderr: {}",
        String::from_utf8_lossy(&gcc_output.stderr),
    );

    let run_output = if cfg!(target_arch = "x86_64") {
        Command::new(&bin_path).output().expect("failed to run binary")
    } else {
        Command::new("arch").arg("-x86_64").arg(&bin_path)
            .output().expect("failed to run binary via arch -x86_64")
    };

    run_output.status.code().unwrap_or(-1)
}

/// テストヘルパー: 難読化コンパイルしてアセンブリ文字列を返す。
fn compile_to_asm(source: &str, extra_flags: &[&str]) -> String {
    let dir = TempDir::new().unwrap();
    let src_path = dir.path().join("test.c");
    let asm_path = dir.path().join("test.s");

    std::fs::write(&src_path, source).unwrap();

    let mut cmd = Command::new(env!("CARGO_BIN_EXE_ferrugocc"));
    cmd.arg("--fobfuscate");
    for flag in extra_flags {
        cmd.arg(flag);
    }
    cmd.arg("-S").arg(&src_path);

    let output = cmd.output().expect("failed to run compiler");
    assert!(
        output.status.success(),
        "compilation failed:\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );

    std::fs::read_to_string(&asm_path).unwrap()
}

#[test]
fn test_lib_obfuscate_strlen() {
    if !can_run_x86_64() {
        eprintln!("skipping: x86_64 execution not available");
        return;
    }
    // strlen("hello") == 5 → exit code 5
    let source = r#"
        long strlen(char *s);
        int main(void) {
            long n = strlen("hello");
            return (int)n;
        }
    "#;
    let result = compile_and_run_with_flags(source, &["--obf-level=1"]);
    assert_eq!(result, 5, "strlen(\"hello\") should be 5, got {result}");
}

#[test]
fn test_lib_obfuscate_strlen_empty() {
    if !can_run_x86_64() {
        eprintln!("skipping: x86_64 execution not available");
        return;
    }
    // strlen("") == 0 → exit code 0
    let source = r#"
        long strlen(char *s);
        int main(void) {
            long n = strlen("");
            return (int)n;
        }
    "#;
    let result = compile_and_run_with_flags(source, &["--obf-level=1"]);
    assert_eq!(result, 0, "strlen(\"\") should be 0, got {result}");
}

#[test]
fn test_lib_obfuscate_strlen_no_libc_call() {
    // アセンブリに `call strlen` が存在しないことを確認
    let source = r#"
        long strlen(char *s);
        int main(void) {
            long n = strlen("hello");
            return (int)n;
        }
    "#;
    let asm = compile_to_asm(source, &["--obf-level=1"]);
    // `call strlen` or `call _strlen` should NOT appear
    assert!(
        !asm.contains("call strlen") && !asm.contains("call _strlen"),
        "assembly should not contain 'call strlen': found libc call in output"
    );
    // `_obf_strlen` SHOULD appear
    assert!(
        asm.contains("_obf_strlen"),
        "assembly should contain '_obf_strlen': obfuscated implementation missing"
    );
}

#[test]
fn test_lib_obfuscate_strcmp_equal() {
    if !can_run_x86_64() {
        eprintln!("skipping: x86_64 execution not available");
        return;
    }
    // strcmp("abc", "abc") == 0 → exit code 0
    let source = r#"
        int strcmp(char *s1, char *s2);
        int main(void) {
            return strcmp("abc", "abc");
        }
    "#;
    let result = compile_and_run_with_flags(source, &["--obf-level=1"]);
    assert_eq!(result, 0, "strcmp(\"abc\", \"abc\") should be 0, got {result}");
}

#[test]
fn test_lib_obfuscate_strcmp_less() {
    if !can_run_x86_64() {
        eprintln!("skipping: x86_64 execution not available");
        return;
    }
    // strcmp("abc", "abd") < 0 → negative value; use (result < 0) ? 1 : 0
    let source = r#"
        int strcmp(char *s1, char *s2);
        int main(void) {
            int r = strcmp("abc", "abd");
            if (r < 0) return 1;
            return 0;
        }
    "#;
    let result = compile_and_run_with_flags(source, &["--obf-level=1"]);
    assert_eq!(result, 1, "strcmp(\"abc\", \"abd\") should be negative, got non-negative");
}

#[test]
fn test_lib_obfuscate_strcmp_greater() {
    if !can_run_x86_64() {
        eprintln!("skipping: x86_64 execution not available");
        return;
    }
    // strcmp("abd", "abc") > 0 → positive value
    let source = r#"
        int strcmp(char *s1, char *s2);
        int main(void) {
            int r = strcmp("abd", "abc");
            if (r > 0) return 1;
            return 0;
        }
    "#;
    let result = compile_and_run_with_flags(source, &["--obf-level=1"]);
    assert_eq!(result, 1, "strcmp(\"abd\", \"abc\") should be positive, got non-positive");
}

#[test]
fn test_lib_obfuscate_strcmp_no_libc_call() {
    let source = r#"
        int strcmp(char *s1, char *s2);
        int main(void) {
            return strcmp("abc", "abc");
        }
    "#;
    let asm = compile_to_asm(source, &["--obf-level=1"]);
    assert!(
        !asm.contains("call strcmp") && !asm.contains("call _strcmp"),
        "assembly should not contain 'call strcmp'"
    );
    assert!(
        asm.contains("_obf_strcmp"),
        "assembly should contain '_obf_strcmp'"
    );
}

#[test]
fn test_lib_obfuscate_strcpy() {
    if !can_run_x86_64() {
        eprintln!("skipping: x86_64 execution not available");
        return;
    }
    // strcpy でコピーし、コピー先の先頭文字を検証
    let source = r#"
        char *strcpy(char *dst, char *src);
        int main(void) {
            char buf[6];
            strcpy(buf, "Hi");
            return buf[0];
        }
    "#;
    let result = compile_and_run_with_flags(source, &["--obf-level=1"]);
    // 'H' == 72
    assert_eq!(result, 72, "strcpy: buf[0] should be 'H' (72), got {result}");
}

#[test]
fn test_lib_obfuscate_strcpy_empty() {
    if !can_run_x86_64() {
        eprintln!("skipping: x86_64 execution not available");
        return;
    }
    // strcpy で空文字列をコピー → buf[0] == '\0' == 0
    let source = r#"
        char *strcpy(char *dst, char *src);
        int main(void) {
            char buf[4];
            strcpy(buf, "");
            return buf[0];
        }
    "#;
    let result = compile_and_run_with_flags(source, &["--obf-level=1"]);
    assert_eq!(result, 0, "strcpy(\"\") should set buf[0]=0, got {result}");
}

#[test]
fn test_lib_obfuscate_strcpy_no_libc_call() {
    let source = r#"
        char *strcpy(char *dst, char *src);
        int main(void) {
            char buf[6];
            strcpy(buf, "Hi");
            return buf[0];
        }
    "#;
    let asm = compile_to_asm(source, &["--obf-level=1"]);
    assert!(
        !asm.contains("call strcpy") && !asm.contains("call _strcpy"),
        "assembly should not contain 'call strcpy'"
    );
    assert!(
        asm.contains("_obf_strcpy"),
        "assembly should contain '_obf_strcpy'"
    );
}

// ── memcpy テスト ──

#[test]
fn test_lib_obfuscate_memcpy() {
    if !can_run_x86_64() {
        eprintln!("skipping: x86_64 execution not available");
        return;
    }
    // memcpy で3バイトコピーし、先頭文字を検証
    let source = r#"
        char *memcpy(char *dst, char *src, long n);
        int main(void) {
            char buf[4];
            memcpy(buf, "XY", 3);
            return buf[0];
        }
    "#;
    let result = compile_and_run_with_flags(source, &["--obf-level=1"]);
    // 'X' == 88
    assert_eq!(result, 88, "memcpy: buf[0] should be 'X' (88), got {result}");
}

#[test]
fn test_lib_obfuscate_memcpy_zero_length() {
    if !can_run_x86_64() {
        eprintln!("skipping: x86_64 execution not available");
        return;
    }
    // memcpy with n=0 should not modify destination
    let source = r#"
        char *memcpy(char *dst, char *src, long n);
        int main(void) {
            char buf[4];
            buf[0] = 42;
            memcpy(buf, "ZZ", 0);
            return buf[0];
        }
    "#;
    let result = compile_and_run_with_flags(source, &["--obf-level=1"]);
    assert_eq!(result, 42, "memcpy(n=0): buf[0] should remain 42, got {result}");
}

#[test]
fn test_lib_obfuscate_memcpy_no_libc_call() {
    let source = r#"
        char *memcpy(char *dst, char *src, long n);
        int main(void) {
            char buf[4];
            memcpy(buf, "AB", 3);
            return buf[0];
        }
    "#;
    let asm = compile_to_asm(source, &["--obf-level=1"]);
    assert!(
        !asm.contains("call memcpy") && !asm.contains("call _memcpy"),
        "assembly should not contain 'call memcpy'"
    );
    assert!(
        asm.contains("_obf_memcpy"),
        "assembly should contain '_obf_memcpy'"
    );
}

// ── memset テスト ──

#[test]
fn test_lib_obfuscate_memset() {
    if !can_run_x86_64() {
        eprintln!("skipping: x86_64 execution not available");
        return;
    }
    // memset(buf, 65, 3) → buf[0..2] = 'A' (65)
    let source = r#"
        char *memset(char *s, int c, long n);
        int main(void) {
            char buf[4];
            memset(buf, 65, 3);
            return buf[0];
        }
    "#;
    let result = compile_and_run_with_flags(source, &["--obf-level=1"]);
    assert_eq!(result, 65, "memset(65): buf[0] should be 65 ('A'), got {result}");
}

#[test]
fn test_lib_obfuscate_memset_zero() {
    if !can_run_x86_64() {
        eprintln!("skipping: x86_64 execution not available");
        return;
    }
    // memset(buf, 0, 4) → buf をゼロクリア
    let source = r#"
        char *memset(char *s, int c, long n);
        int main(void) {
            char buf[4];
            buf[0] = 99;
            memset(buf, 0, 4);
            return buf[0];
        }
    "#;
    let result = compile_and_run_with_flags(source, &["--obf-level=1"]);
    assert_eq!(result, 0, "memset(0): buf[0] should be 0, got {result}");
}

#[test]
fn test_lib_obfuscate_memset_no_libc_call() {
    let source = r#"
        char *memset(char *s, int c, long n);
        int main(void) {
            char buf[4];
            memset(buf, 0, 4);
            return buf[0];
        }
    "#;
    let asm = compile_to_asm(source, &["--obf-level=1"]);
    assert!(
        !asm.contains("call memset") && !asm.contains("call _memset"),
        "assembly should not contain 'call memset'"
    );
    assert!(
        asm.contains("_obf_memset"),
        "assembly should contain '_obf_memset'"
    );
}

// ===== memcmp テスト =====

#[test]
fn test_lib_obfuscate_memcmp_equal() {
    if !can_run_x86_64() {
        eprintln!("skipping: x86_64 execution not available");
        return;
    }
    // memcmp("abc", "abc", 3) == 0
    let source = r#"
        int memcmp(char *s1, char *s2, long n);
        int main(void) {
            return memcmp("abc", "abc", 3);
        }
    "#;
    let result = compile_and_run_with_flags(source, &["--obf-level=1"]);
    assert_eq!(result, 0, "memcmp(\"abc\", \"abc\", 3) should be 0, got {result}");
}

#[test]
fn test_lib_obfuscate_memcmp_less() {
    if !can_run_x86_64() {
        eprintln!("skipping: x86_64 execution not available");
        return;
    }
    // memcmp("abc", "abd", 3) < 0 → negative
    let source = r#"
        int memcmp(char *s1, char *s2, long n);
        int main(void) {
            int r = memcmp("abc", "abd", 3);
            if (r < 0) return 1;
            return 0;
        }
    "#;
    let result = compile_and_run_with_flags(source, &["--obf-level=1"]);
    assert_eq!(result, 1, "memcmp(\"abc\", \"abd\", 3) should be negative");
}

#[test]
fn test_lib_obfuscate_memcmp_greater() {
    if !can_run_x86_64() {
        eprintln!("skipping: x86_64 execution not available");
        return;
    }
    // memcmp("abd", "abc", 3) > 0 → positive
    let source = r#"
        int memcmp(char *s1, char *s2, long n);
        int main(void) {
            int r = memcmp("abd", "abc", 3);
            if (r > 0) return 1;
            return 0;
        }
    "#;
    let result = compile_and_run_with_flags(source, &["--obf-level=1"]);
    assert_eq!(result, 1, "memcmp(\"abd\", \"abc\", 3) should be positive");
}

#[test]
fn test_lib_obfuscate_memcmp_no_libc_call() {
    let source = r#"
        int memcmp(char *s1, char *s2, long n);
        int main(void) {
            return memcmp("abc", "abc", 3);
        }
    "#;
    let asm = compile_to_asm(source, &["--obf-level=1"]);
    assert!(
        !asm.contains("call memcmp") && !asm.contains("call _memcmp"),
        "assembly should not contain 'call memcmp'"
    );
    assert!(
        asm.contains("_obf_memcmp"),
        "assembly should contain '_obf_memcmp'"
    );
}

// ===== strncmp テスト =====

#[test]
fn test_lib_obfuscate_strncmp_equal() {
    if !can_run_x86_64() {
        eprintln!("skipping: x86_64 execution not available");
        return;
    }
    // strncmp("abcX", "abcY", 3) == 0 (最初の3文字だけ比較)
    let source = r#"
        int strncmp(char *s1, char *s2, long n);
        int main(void) {
            return strncmp("abcX", "abcY", 3);
        }
    "#;
    let result = compile_and_run_with_flags(source, &["--obf-level=1"]);
    assert_eq!(result, 0, "strncmp(\"abcX\", \"abcY\", 3) should be 0, got {result}");
}

#[test]
fn test_lib_obfuscate_strncmp_differ() {
    if !can_run_x86_64() {
        eprintln!("skipping: x86_64 execution not available");
        return;
    }
    // strncmp("abc", "abd", 3) < 0
    let source = r#"
        int strncmp(char *s1, char *s2, long n);
        int main(void) {
            int r = strncmp("abc", "abd", 3);
            if (r < 0) return 1;
            return 0;
        }
    "#;
    let result = compile_and_run_with_flags(source, &["--obf-level=1"]);
    assert_eq!(result, 1, "strncmp(\"abc\", \"abd\", 3) should be negative");
}

#[test]
fn test_lib_obfuscate_strncmp_zero_len() {
    if !can_run_x86_64() {
        eprintln!("skipping: x86_64 execution not available");
        return;
    }
    // strncmp("abc", "xyz", 0) == 0 (0文字比較は常に等しい)
    let source = r#"
        int strncmp(char *s1, char *s2, long n);
        int main(void) {
            return strncmp("abc", "xyz", 0);
        }
    "#;
    let result = compile_and_run_with_flags(source, &["--obf-level=1"]);
    assert_eq!(result, 0, "strncmp with n=0 should be 0, got {result}");
}

#[test]
fn test_lib_obfuscate_strncmp_no_libc_call() {
    let source = r#"
        int strncmp(char *s1, char *s2, long n);
        int main(void) {
            return strncmp("abc", "abc", 3);
        }
    "#;
    let asm = compile_to_asm(source, &["--obf-level=1"]);
    assert!(
        !asm.contains("call strncmp") && !asm.contains("call _strncmp"),
        "assembly should not contain 'call strncmp'"
    );
    assert!(
        asm.contains("_obf_strncmp"),
        "assembly should contain '_obf_strncmp'"
    );
}

// ===== strncpy テスト =====

#[test]
fn test_lib_obfuscate_strncpy() {
    if !can_run_x86_64() {
        eprintln!("skipping: x86_64 execution not available");
        return;
    }
    // strncpy でコピーし、コピー先の先頭文字を検証
    let source = r#"
        char *strncpy(char *dst, char *src, long n);
        int main(void) {
            char buf[6];
            strncpy(buf, "Hi", 6);
            return buf[0];
        }
    "#;
    let result = compile_and_run_with_flags(source, &["--obf-level=1"]);
    // 'H' == 72
    assert_eq!(result, 72, "strncpy: buf[0] should be 'H' (72), got {result}");
}

#[test]
fn test_lib_obfuscate_strncpy_pad() {
    if !can_run_x86_64() {
        eprintln!("skipping: x86_64 execution not available");
        return;
    }
    // strncpy は src が短い場合、残りを '\0' で埋める
    let source = r#"
        char *strncpy(char *dst, char *src, long n);
        int main(void) {
            char buf[4];
            buf[2] = 88;
            strncpy(buf, "A", 4);
            return buf[2];
        }
    "#;
    let result = compile_and_run_with_flags(source, &["--obf-level=1"]);
    // buf[2] should be '\0' (0) due to zero-padding
    assert_eq!(result, 0, "strncpy should zero-pad: buf[2] should be 0, got {result}");
}

#[test]
fn test_lib_obfuscate_strncpy_no_libc_call() {
    let source = r#"
        char *strncpy(char *dst, char *src, long n);
        int main(void) {
            char buf[4];
            strncpy(buf, "Hi", 4);
            return buf[0];
        }
    "#;
    let asm = compile_to_asm(source, &["--obf-level=1"]);
    assert!(
        !asm.contains("call strncpy") && !asm.contains("call _strncpy"),
        "assembly should not contain 'call strncpy'"
    );
    assert!(
        asm.contains("_obf_strncpy"),
        "assembly should contain '_obf_strncpy'"
    );
}

// ===== strchr テスト =====

#[test]
fn test_lib_obfuscate_strchr_found() {
    if !can_run_x86_64() {
        eprintln!("skipping: x86_64 execution not available");
        return;
    }
    // strchr("hello", 'l') should return pointer to first 'l' → *p == 'l' == 108
    let source = r#"
        char *strchr(char *s, int c);
        int main(void) {
            char *p = strchr("hello", 108);
            if (p == 0) return 0;
            return *p;
        }
    "#;
    let result = compile_and_run_with_flags(source, &["--obf-level=1"]);
    assert_eq!(result, 108, "strchr('hello','l') should find 'l' (108), got {result}");
}

#[test]
fn test_lib_obfuscate_strchr_not_found() {
    if !can_run_x86_64() {
        eprintln!("skipping: x86_64 execution not available");
        return;
    }
    // strchr("hello", 'z') should return NULL → 0
    let source = r#"
        char *strchr(char *s, int c);
        int main(void) {
            char *p = strchr("hello", 122);
            if (p == 0) return 1;
            return 0;
        }
    "#;
    let result = compile_and_run_with_flags(source, &["--obf-level=1"]);
    assert_eq!(result, 1, "strchr('hello','z') should return NULL");
}

#[test]
fn test_lib_obfuscate_strchr_null_char() {
    if !can_run_x86_64() {
        eprintln!("skipping: x86_64 execution not available");
        return;
    }
    // strchr("hi", '\0') should find the null terminator → *p == 0
    let source = r#"
        char *strchr(char *s, int c);
        int main(void) {
            char *p = strchr("hi", 0);
            if (p == 0) return 99;
            return *p;
        }
    "#;
    let result = compile_and_run_with_flags(source, &["--obf-level=1"]);
    assert_eq!(result, 0, "strchr('hi','\\0') should find null terminator, got {result}");
}

#[test]
fn test_lib_obfuscate_strchr_no_libc_call() {
    let source = r#"
        char *strchr(char *s, int c);
        int main(void) {
            char *p = strchr("hello", 108);
            if (p == 0) return 0;
            return *p;
        }
    "#;
    let asm = compile_to_asm(source, &["--obf-level=1"]);
    assert!(
        !asm.contains("call strchr") && !asm.contains("call _strchr"),
        "assembly should not contain 'call strchr'"
    );
    assert!(
        asm.contains("_obf_strchr"),
        "assembly should contain '_obf_strchr'"
    );
}

// ===== strcat テスト =====

#[test]
fn test_lib_obfuscate_strcat() {
    if !can_run_x86_64() {
        eprintln!("skipping: x86_64 execution not available");
        return;
    }
    // strcat で文字列連結し、連結後の文字を検証
    let source = r#"
        char *strcpy(char *dst, char *src);
        char *strcat(char *dst, char *src);
        int main(void) {
            char buf[6];
            strcpy(buf, "Hi");
            strcat(buf, "!");
            return buf[2];
        }
    "#;
    let result = compile_and_run_with_flags(source, &["--obf-level=1"]);
    // '!' == 33
    assert_eq!(result, 33, "strcat: buf[2] should be '!' (33), got {result}");
}

#[test]
fn test_lib_obfuscate_strcat_empty() {
    if !can_run_x86_64() {
        eprintln!("skipping: x86_64 execution not available");
        return;
    }
    // strcat で空文字列を連結 → 変わらない
    let source = r#"
        char *strcpy(char *dst, char *src);
        char *strcat(char *dst, char *src);
        int main(void) {
            char buf[6];
            strcpy(buf, "AB");
            strcat(buf, "");
            return buf[0];
        }
    "#;
    let result = compile_and_run_with_flags(source, &["--obf-level=1"]);
    // 'A' == 65
    assert_eq!(result, 65, "strcat with empty: buf[0] should be 'A' (65), got {result}");
}

#[test]
fn test_lib_obfuscate_strcat_no_libc_call() {
    let source = r#"
        char *strcat(char *dst, char *src);
        char *strcpy(char *dst, char *src);
        int main(void) {
            char buf[6];
            strcpy(buf, "A");
            strcat(buf, "B");
            return buf[0];
        }
    "#;
    let asm = compile_to_asm(source, &["--obf-level=1"]);
    assert!(
        !asm.contains("call strcat") && !asm.contains("call _strcat"),
        "assembly should not contain 'call strcat'"
    );
    assert!(
        asm.contains("_obf_strcat"),
        "assembly should contain '_obf_strcat'"
    );
}

// === CFF + ライブラリ関数の組み合わせテスト (Level 2) ===

#[test]
fn test_cff_strcmp_level2() {
    if !can_run_x86_64() {
        eprintln!("skipping: x86_64 execution not available");
        return;
    }
    let source = r#"
        int strcmp(char *s1, char *s2);
        int main(void) {
            char a[4];
            a[0] = 'a'; a[1] = 'b'; a[2] = 'c'; a[3] = 0;
            char b[4];
            b[0] = 'a'; b[1] = 'b'; b[2] = 'c'; b[3] = 0;
            if (strcmp(a, b) == 0) { return 42; }
            return 0;
        }
    "#;
    let result = compile_and_run_with_level(source, 2);
    assert_eq!(result, 42, "CFF+strcmp level 2: expected 42, got {result}");
}
