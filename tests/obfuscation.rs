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
        let mut cmd = Command::new("gcc");
        cmd.arg(&asm_path).arg("-o").arg(&bin_path);
        if cfg!(target_os = "linux") {
            cmd.arg("-no-pie"); // FerrugoCC は non-PIC コードを生成する
        }
        cmd.output().expect("failed to run gcc")
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
    use std::collections::HashSet;

    let mut result = Vec::new();
    // 全シンボル（.globl + ラベル定義）を収集
    let mut all_symbols: HashSet<String> = HashSet::new();
    for line in asm.lines() {
        let trimmed = line.trim();
        if let Some(rest) = trimmed.strip_prefix(".globl ") {
            all_symbols.insert(rest.trim().to_string());
        }
        if trimmed.ends_with(':') && !trimmed.starts_with('.') {
            all_symbols.insert(trimmed.trim_end_matches(':').to_string());
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
            // call 命令: 非ローカル・非間接ターゲットに _ プレフィクス
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
            // .quad/.long シンボル参照
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
            // sym(%rip) 参照: 外部シンボルは @GOTPCREL、内部は _prefix
            let mut search_from = 0;
            while let Some(rel_idx) = new_line[search_from..].find("(%rip)") {
                let rip_idx = search_from + rel_idx;
                let before = &new_line[..rip_idx];
                let sym_start = before
                    .rfind(|c: char| !c.is_alphanumeric() && c != '_' && c != '.')
                    .map(|i| i + 1)
                    .unwrap_or(0);
                let sym = &new_line[sym_start..rip_idx];
                if !sym.is_empty() && !sym.starts_with('.') {
                    let is_external = !all_symbols.contains(sym);
                    let trimmed_line = new_line.trim_start();
                    if is_external && trimmed_line.starts_with("leaq ") {
                        let original = format!("leaq {sym}(%rip)");
                        let replacement = format!("movq _{sym}@GOTPCREL(%rip)");
                        new_line = new_line.replacen(&original, &replacement, 1);
                        search_from = sym_start + replacement.len();
                    } else {
                        let replacement = format!("_{sym}(%rip)");
                        let original = format!("{sym}(%rip)");
                        new_line = new_line.replacen(&original, &replacement, 1);
                        search_from = sym_start + replacement.len();
                    }
                } else {
                    search_from = rip_idx + 6;
                }
            }
        }
        result.push(new_line);
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
        let mut cmd = Command::new("gcc");
        cmd.arg(&asm_path).arg("-o").arg(&bin_path);
        if cfg!(target_os = "linux") {
            cmd.arg("-no-pie");
        }
        cmd.output().expect("failed to run gcc")
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
    assert_eq!(
        normal, expected_exit_code,
        "normal compilation: expected {expected_exit_code}, got {normal}"
    );

    let obfuscated = compile_and_run(source, true);
    assert_eq!(
        obfuscated, expected_exit_code,
        "obfuscated compilation: expected {expected_exit_code}, got {obfuscated}"
    );
}

#[test]
fn test_constant_return() {
    assert_obfuscation_preserves_behavior("int main(void) { return 42; }", 42);
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
    assert_eq!(
        result, 0,
        "strcmp(\"abc\", \"abc\") should be 0, got {result}"
    );
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
    assert_eq!(
        result, 1,
        "strcmp(\"abc\", \"abd\") should be negative, got non-negative"
    );
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
    assert_eq!(
        result, 1,
        "strcmp(\"abd\", \"abc\") should be positive, got non-positive"
    );
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
    assert_eq!(
        result, 72,
        "strcpy: buf[0] should be 'H' (72), got {result}"
    );
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
    assert_eq!(
        result, 88,
        "memcpy: buf[0] should be 'X' (88), got {result}"
    );
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
    assert_eq!(
        result, 42,
        "memcpy(n=0): buf[0] should remain 42, got {result}"
    );
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
    assert_eq!(
        result, 65,
        "memset(65): buf[0] should be 65 ('A'), got {result}"
    );
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
    assert_eq!(
        result, 0,
        "memcmp(\"abc\", \"abc\", 3) should be 0, got {result}"
    );
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
    assert_eq!(
        result, 0,
        "strncmp(\"abcX\", \"abcY\", 3) should be 0, got {result}"
    );
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
    assert_eq!(
        result, 72,
        "strncpy: buf[0] should be 'H' (72), got {result}"
    );
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
    assert_eq!(
        result, 0,
        "strncpy should zero-pad: buf[2] should be 0, got {result}"
    );
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
    assert_eq!(
        result, 108,
        "strchr('hello','l') should find 'l' (108), got {result}"
    );
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
    assert_eq!(
        result, 0,
        "strchr('hi','\\0') should find null terminator, got {result}"
    );
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
    assert_eq!(
        result, 33,
        "strcat: buf[2] should be '!' (33), got {result}"
    );
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
    assert_eq!(
        result, 65,
        "strcat with empty: buf[0] should be 'A' (65), got {result}"
    );
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

// === OPSEC パス (Pass 16) テスト ===

/// テストヘルパー: 指定オプションでコンパイルし、アセンブリ出力とstderrを返す
fn compile_to_asm_with_opts(source: &str, extra_args: &[&str]) -> (String, String) {
    let dir = TempDir::new().unwrap();
    let src_path = dir.path().join("test.c");
    let asm_path = dir.path().join("test.s");

    std::fs::write(&src_path, source).unwrap();

    let mut cmd = Command::new(env!("CARGO_BIN_EXE_ferrugocc"));
    for arg in extra_args {
        cmd.arg(arg);
    }
    cmd.arg("-S").arg(&src_path);

    let output = cmd.output().expect("failed to run compiler");
    assert!(
        output.status.success(),
        "compilation failed:\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );

    let asm = if asm_path.exists() {
        std::fs::read_to_string(&asm_path).unwrap()
    } else {
        String::new()
    };
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();

    (asm, stderr)
}

/// テストヘルパー: 指定オプションでコンパイル → 実行して終了コードを返す
fn compile_and_run_with_opts(source: &str, extra_args: &[&str]) -> i32 {
    let dir = TempDir::new().unwrap();
    let src_path = dir.path().join("test.c");
    let asm_path = dir.path().join("test.s");
    let bin_path = dir.path().join("test");

    std::fs::write(&src_path, source).unwrap();

    let mut cmd = Command::new(env!("CARGO_BIN_EXE_ferrugocc"));
    for arg in extra_args {
        cmd.arg(arg);
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

    run_output.status.code().unwrap_or(-1)
}

#[test]
fn test_opsec_symbol_rename() {
    // 関数名がアセンブリ出力に残らないことを確認
    let source = r#"
        int my_secret_func(int x) { return x + 1; }
        int main(void) { return my_secret_func(41); }
    "#;
    let (asm, _) = compile_to_asm_with_opts(source, &["--fobfuscate", "--obf-level=3"]);
    assert!(
        !asm.contains("my_secret_func"),
        "OPSEC: function name 'my_secret_func' should not appear in assembly output"
    );
}

#[test]
fn test_opsec_main_preserved() {
    // main はリネームされないことを確認
    let source = r#"
        int helper(int x) { return x * 2; }
        int main(void) { return helper(21); }
    "#;
    let (asm, _) = compile_to_asm_with_opts(source, &["--fobfuscate", "--obf-level=3"]);
    assert!(
        asm.contains("main:") || asm.contains("main :"),
        "OPSEC: 'main' label should be preserved in assembly output"
    );
}

#[test]
fn test_opsec_external_preserved() {
    // 外部関数（printf）はリネームされないことを確認
    let source = r#"
        int printf(char *fmt, ...);
        int main(void) {
            printf("hi");
            return 0;
        }
    "#;
    let (asm, _) = compile_to_asm_with_opts(source, &["--fobfuscate", "--obf-level=3"]);
    assert!(
        asm.contains("printf"),
        "OPSEC: external function 'printf' should be preserved in assembly output"
    );
}

#[test]
fn test_opsec_correctness() {
    // リネーム後もプログラムが正しく動作すること
    if !can_run_x86_64() {
        eprintln!("skipping: x86_64 execution not available");
        return;
    }
    let source = r#"
        int compute(int a, int b) { return a * b + 2; }
        int wrapper(int x) { return compute(x, 3); }
        int main(void) { return wrapper(10); }
    "#;
    let result = compile_and_run_with_opts(source, &["--fobfuscate", "--obf-level=3"]);
    assert_eq!(result, 32, "OPSEC correctness: expected 32, got {result}");
}

#[test]
fn test_opsec_warn() {
    // IP アドレスを含む文字列リテラル → stderr に OPSEC WARNING が出ること
    let source = r#"
        int main(void) {
            char *s = "connect to 192.168.1.1";
            return s[0];
        }
    "#;
    let (_, stderr) = compile_to_asm_with_opts(source, &["--fobfuscate", "--obf-level=3"]);
    assert!(
        stderr.contains("OPSEC WARNING"),
        "OPSEC: stderr should contain 'OPSEC WARNING' for IP address string, got: {stderr}"
    );
}

// === OPSEC Strip テスト ===

#[test]
fn test_opsec_strip_no_globl() {
    // Level 3 で .globl が main 以外に出力されないことを確認
    let source = r#"
        int helper(int x) { return x + 1; }
        int compute(int a, int b) { return helper(a) + b; }
        int main(void) { return compute(20, 21); }
    "#;
    let (asm, _) = compile_to_asm_with_opts(source, &["--fobfuscate", "--obf-level=3"]);

    // .globl main は存在するべき
    assert!(
        asm.contains(".globl main"),
        "OPSEC strip: '.globl main' should be present in assembly output"
    );

    // .globl 行を収集し、main 以外の .globl が無いことを確認
    let other_globls: Vec<&str> = asm
        .lines()
        .filter(|line| {
            let trimmed = line.trim();
            trimmed.starts_with(".globl ") && trimmed != ".globl main"
        })
        .collect();
    assert!(
        other_globls.is_empty(),
        "OPSEC strip: no .globl directives other than 'main' should exist, found: {other_globls:?}"
    );
}

#[test]
fn test_opsec_strip_correctness() {
    // strip 後もプログラムが正しく動作すること
    if !can_run_x86_64() {
        eprintln!("skipping: x86_64 execution not available");
        return;
    }
    let source = r#"
        int helper(int x) { return x + 1; }
        int compute(int a, int b) { return helper(a) + b; }
        int main(void) { return compute(20, 21); }
    "#;
    let result = compile_and_run_with_opts(source, &["--fobfuscate", "--obf-level=3"]);
    assert_eq!(
        result, 42,
        "OPSEC strip correctness: expected 42, got {result}"
    );
}

#[test]
fn test_opsec_strip_disabled() {
    // --obf-no-strip で .globl が維持されることを確認
    let source = r#"
        int helper(int x) { return x + 1; }
        int main(void) { return helper(41); }
    "#;
    let (asm, _) =
        compile_to_asm_with_opts(source, &["--fobfuscate", "--obf-level=3", "--obf-no-strip"]);

    // main 以外の .globl が存在するべき（リネーム済みシンボル _f0 等）
    let other_globls: Vec<&str> = asm
        .lines()
        .filter(|line| {
            let trimmed = line.trim();
            trimmed.starts_with(".globl ") && trimmed != ".globl main"
        })
        .collect();
    assert!(
        !other_globls.is_empty(),
        "OPSEC strip disabled: .globl directives for renamed symbols should exist when strip is disabled"
    );
}

// === OPSEC Policy (fail-closed) テスト ===

/// テストヘルパー: コンパイラを実行し（-S モード）、Output を返す（失敗も許容）
fn run_compiler_raw(source: &str, extra_args: &[&str]) -> std::process::Output {
    let dir = TempDir::new().unwrap();
    let src_path = dir.path().join("test.c");

    std::fs::write(&src_path, source).unwrap();

    let mut cmd = Command::new(env!("CARGO_BIN_EXE_ferrugocc"));
    for arg in extra_args {
        cmd.arg(arg);
    }
    cmd.arg("-S").arg(&src_path);

    cmd.output().expect("failed to run compiler")
}

/// テストヘルパー: フルコンパイル（リンクまで）して Output を返す（失敗も許容）
fn run_compiler_full(source: &str, extra_args: &[&str]) -> std::process::Output {
    let dir = TempDir::new().unwrap();
    let src_path = dir.path().join("test.c");

    std::fs::write(&src_path, source).unwrap();

    let mut cmd = Command::new(env!("CARGO_BIN_EXE_ferrugocc"));
    for arg in extra_args {
        cmd.arg(arg);
    }
    // -S を付けない → フルコンパイル（リンクまで実行）
    cmd.arg(&src_path);

    cmd.output().expect("failed to run compiler")
}

#[test]
fn test_opsec_policy_warn_allows_compilation() {
    // warn ポリシーでは違反があってもコンパイルが成功する
    let source = r#"
        int printf(char *fmt, ...);
        int main(void) {
            printf("connect to 192.168.1.1\n");
            return 0;
        }
    "#;
    let output = run_compiler_raw(
        source,
        &["--fobfuscate", "--obf-level=2", "--opsec-policy=warn"],
    );
    assert!(
        output.status.success(),
        "OPSEC warn policy should allow compilation even with violations"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("OPSEC WARNING"),
        "OPSEC warn policy should emit warnings: {stderr}"
    );
}

#[test]
fn test_opsec_policy_deny_fails_on_ip() {
    // deny ポリシーでは IP アドレスが含まれるとコンパイル失敗
    let source = r#"
        int printf(char *fmt, ...);
        int main(void) {
            printf("server at 10.0.0.1 ready\n");
            return 0;
        }
    "#;
    let output = run_compiler_raw(
        source,
        &["--fobfuscate", "--obf-level=2", "--opsec-policy=deny"],
    );
    assert!(
        !output.status.success(),
        "OPSEC deny policy should fail compilation on IP address"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("OPSEC ERROR"),
        "OPSEC deny should emit ERROR tag: {stderr}"
    );
}

#[test]
fn test_opsec_policy_deny_fails_on_sensitive_keyword() {
    // deny ポリシーでは "password" が含まれるとコンパイル失敗
    let source = r#"
        int printf(char *fmt, ...);
        int main(void) {
            printf("enter password:\n");
            return 0;
        }
    "#;
    let output = run_compiler_raw(
        source,
        &["--fobfuscate", "--obf-level=2", "--opsec-policy=deny"],
    );
    assert!(
        !output.status.success(),
        "OPSEC deny policy should fail compilation on sensitive keyword 'password'"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("password"),
        "OPSEC deny should report the sensitive keyword: {stderr}"
    );
}

#[test]
fn test_opsec_policy_deny_passes_clean_code() {
    // deny ポリシーでも違反がなければコンパイル成功
    let source = r#"
        int main(void) {
            return 42;
        }
    "#;
    let output = run_compiler_raw(
        source,
        &["--fobfuscate", "--obf-level=2", "--opsec-policy=deny"],
    );
    assert!(
        output.status.success(),
        "OPSEC deny policy should pass clean code:\nstderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn test_opsec_policy_deny_disabled_by_no_warn() {
    // --obf-no-opsec-warn が deny より優先される（警告自体が無効）
    let source = r#"
        int printf(char *fmt, ...);
        int main(void) {
            printf("connect to 192.168.1.1\n");
            return 0;
        }
    "#;
    let output = run_compiler_raw(
        source,
        &[
            "--fobfuscate",
            "--obf-level=2",
            "--opsec-policy=deny",
            "--obf-no-opsec-warn",
        ],
    );
    assert!(
        output.status.success(),
        "OPSEC --obf-no-opsec-warn should override deny policy:\nstderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn test_opsec_policy_deny_multiple_violations() {
    // 複数カテゴリの違反が全て報告される
    let source = r#"
        int printf(char *fmt, ...);
        int main(void) {
            printf("server 10.0.0.1\n");
            printf("file at /home/user/.ssh/id_rsa\n");
            printf("visit https://example.com\n");
            printf("debug mode enabled\n");
            printf("enter password:\n");
            return 0;
        }
    "#;
    let output = run_compiler_raw(
        source,
        &["--fobfuscate", "--obf-level=2", "--opsec-policy=deny"],
    );
    assert!(
        !output.status.success(),
        "OPSEC deny should fail on multiple violations"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    // 全5カテゴリが報告されることを確認
    assert!(stderr.contains("IP address"), "should report IP: {stderr}");
    assert!(
        stderr.contains("file path"),
        "should report file path: {stderr}"
    );
    assert!(stderr.contains("URL"), "should report URL: {stderr}");
    assert!(
        stderr.contains("debug keyword"),
        "should report debug keyword: {stderr}"
    );
    assert!(
        stderr.contains("sensitive keyword"),
        "should report sensitive keyword: {stderr}"
    );
}

#[test]
fn test_opsec_default_policy_is_warn() {
    // --opsec-policy を省略した場合はデフォルト warn（後方互換）
    let source = r#"
        int printf(char *fmt, ...);
        int main(void) {
            printf("connect to 192.168.1.1\n");
            return 0;
        }
    "#;
    // --opsec-policy を指定しない
    let output = run_compiler_raw(source, &["--fobfuscate", "--obf-level=2"]);
    assert!(
        output.status.success(),
        "Default policy should be warn (compilation succeeds):\nstderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("OPSEC WARNING"),
        "Default policy should emit WARNING tag: {stderr}"
    );
}

#[test]
fn test_opsec_policy_invalid_value_rejected() {
    // 不正なポリシー値は clap がエラーとしてリジェクトする
    let source = r#"
        int main(void) { return 0; }
    "#;
    let output = run_compiler_raw(
        source,
        &["--fobfuscate", "--obf-level=2", "--opsec-policy=deni"],
    );
    assert!(
        !output.status.success(),
        "Invalid --opsec-policy value should be rejected by clap"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("invalid value") || stderr.contains("possible values"),
        "clap should report invalid value: {stderr}"
    );
}

#[test]
fn test_obf_no_opsec_overrides_opsec_audit() {
    // --obf-no-opsec は --opsec-audit より優先する
    // -S モードで検証（TACKY IR レベルの opsec_warn + opsec_policy が無効化されることを確認）
    let source = r#"
        int printf(char *fmt, ...);
        int main(void) {
            printf("connect to 192.168.1.1\n");
            return 0;
        }
    "#;
    let output = run_compiler_raw(
        source,
        &[
            "--fobfuscate",
            "--obf-level=3",
            "--obf-no-opsec",
            "--opsec-audit",
            "--opsec-policy=deny",
        ],
    );
    assert!(
        output.status.success(),
        "--obf-no-opsec should override --opsec-audit and --opsec-policy=deny:\nstderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    // -S では audit は実行されないが、opsec_warn + deny でも失敗しないことを確認
    assert!(
        !stderr.contains("OPSEC ERROR"),
        "--obf-no-opsec should suppress OPSEC errors: {stderr}"
    );
}

#[test]
fn test_obf_no_opsec_overrides_opsec_audit_full_link() {
    // --obf-no-opsec がフルコンパイル時にバイナリ監査も無効化する
    // macOS ではフルコンパイル（driver.rs の gcc 直呼び）が macOS fixup を経由しないためスキップ
    if cfg!(target_os = "macos") || !can_run_x86_64() {
        eprintln!("skipping: full-link test not supported on this platform");
        return;
    }
    let source = r#"
        int main(void) {
            return 42;
        }
    "#;
    let output = run_compiler_full(
        source,
        &[
            "--fobfuscate",
            "--obf-level=3",
            "--obf-no-opsec",
            "--opsec-audit",
        ],
    );
    assert!(
        output.status.success(),
        "--obf-no-opsec should override --opsec-audit in full compile:\nstderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !stderr.contains("binary audit"),
        "--obf-no-opsec should suppress binary audit: {stderr}"
    );
}

#[test]
fn test_opsec_audit_runs_on_full_compile() {
    // --opsec-audit がフルコンパイル（リンク後）で実際に実行される
    // macOS ではフルコンパイル（driver.rs の gcc 直呼び）が macOS fixup を経由しないためスキップ
    if cfg!(target_os = "macos") || !can_run_x86_64() {
        eprintln!("skipping: full-link test not supported on this platform");
        return;
    }
    let source = r#"
        int main(void) {
            return 42;
        }
    "#;
    let output = run_compiler_full(
        source,
        &[
            "--fobfuscate",
            "--obf-level=3",
            "--opsec-audit",
            "--opsec-policy=warn",
        ],
    );
    assert!(
        output.status.success(),
        "Clean code with --opsec-audit should pass:\nstderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("binary audit passed") || stderr.contains("OPSEC"),
        "Audit should produce output: {stderr}"
    );
}

#[test]
fn test_opsec_warn_utf8_no_panic() {
    // マルチバイト UTF-8 文字列を含むソースで OPSEC 警告が panic しないことを確認
    // FerrugoCC のレキサーは \x エスケープ未対応のため、UTF-8 バイトを直接ソースに埋め込む
    let source = "int printf(char *fmt, ...);\nint main(void) {\n    printf(\"password: \u{3053}\u{308c}\u{306f}\u{9577}\u{3044}\u{65e5}\u{672c}\u{8a9e}\u{306e}\u{6587}\u{5b57}\u{5217}\u{3067}\u{3059}\u{3002}\u{3068}\u{3066}\u{3082}\u{9577}\u{3044}\u{6587}\u{5b57}\u{5217}\u{306a}\u{306e}\u{3067}\u{5207}\u{308a}\u{8a70}\u{3081}\u{3089}\u{308c}\u{308b}\u{306f}\u{305a}\\n\");\n    return 0;\n}\n";
    let output = run_compiler_raw(
        source,
        &["--fobfuscate", "--obf-level=2", "--opsec-policy=warn"],
    );
    assert!(
        output.status.success(),
        "UTF-8 strings should not cause panic:\nstderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

/// Regression: cvtsi2sd memory dst (obfuscated IntToDouble)
///
/// Under heavy register pressure from obfuscation, both src and dst of
/// cvtsi2sd can be memory operands. The regalloc fixup must rewrite dst
/// to an XMM register (the x86 ISA requires it).
///
/// Bug: fixup pattern 1 (memory src → R10) fires and emits a new
/// Cvtsi2sd with R10 src, but the original memory dst is preserved.
/// Pattern 2 (memory dst → XMM15) never sees the rewritten instruction
/// because the fixup is single-pass.
#[test]
fn test_obfuscate_int_to_double_regalloc() {
    if !can_run_x86_64() {
        return;
    }
    // This exercises IntToDouble (cvtsi2sd) under obfuscation register
    // pressure. Multiple local variables and int↔double casts ensure the
    // regalloc can spill cvtsi2sd operands to memory.
    let source = r#"
int main(void) {
    int a = 10, b = 20, c = 30, d = 40;
    double da = (double)a;
    double db = (double)b;
    double dc = (double)c;
    double dd = (double)d;
    double sum = da + db + dc + dd;  /* 100.0 */
    int isum = (int)sum;
    /* round-trip: int → double → int should preserve value */
    if (isum != 100) return 1;

    /* more pressure: nested conversions */
    int e = 5, f = 7;
    double de = (double)e;
    double df = (double)f;
    double prod = de * df;   /* 35.0 */
    int iprod = (int)prod;
    if (iprod != 35) return 2;

    /* result: 100 - 35 - 23 = 42 */
    return isum - iprod - 23;
}
"#;
    assert_eq!(compile_and_run(source, true), 42);
}

/// Regression: function pointer call through struct member under obfuscation.
///
/// pdjson's push() calls json->alloc.realloc(json->stack, size) — a function
/// pointer stored in a struct member. Under obfuscation (inlining + register
/// pressure), the indirect call must preserve the correct function pointer.
#[test]
fn test_obfuscate_fn_ptr_struct_realloc() {
    if !can_run_x86_64() {
        return;
    }
    let source = r#"
void *realloc(void *, unsigned long);
void free(void *);

struct alloc {
    void *(*my_realloc)(void *, unsigned long);
    void (*my_free)(void *);
};

struct stream {
    int *stack;
    unsigned long stack_top;
    unsigned long stack_size;
    struct alloc alloc;
};

void init_stream(struct stream *s) {
    s->stack = 0;
    s->stack_top = 0;
    s->stack_size = 0;
    s->alloc.my_realloc = realloc;
    s->alloc.my_free = free;
}

int do_push(struct stream *s, int val) {
    if (s->stack_top >= s->stack_size) {
        unsigned long size = (s->stack_size + 4) * sizeof(int);
        int *new_stack = (int *)s->alloc.my_realloc(s->stack, size);
        if (new_stack == 0) return -1;
        s->stack_size += 4;
        s->stack = new_stack;
    }
    s->stack[s->stack_top] = val;
    s->stack_top += 1;
    return val;
}

int main(void) {
    struct stream s;
    init_stream(&s);
    int r = 0;
    r += do_push(&s, 10);
    r += do_push(&s, 15);
    r += do_push(&s, 17);
    /* 10 + 15 + 17 = 42 */
    s.alloc.my_free(s.stack);
    return r;
}
"#;
    assert_eq!(compile_and_run(source, true), 42);
}
