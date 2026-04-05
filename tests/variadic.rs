//! E2E テスト — 可変長引数関数（variadic functions）の検証
//!
//! C ソースをコンパイル → gcc リンク → 実行 → 終了コード / stdout 検証。

use std::process::Command;
use tempfile::TempDir;

/// x86_64 バイナリを実行可能か判定（ARM64 macOS で Rosetta が使えるか）
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

/// macOS 向けにアセンブリを修正する。
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

        if trimmed.contains(".note.GNU-stack")
            || trimmed.contains(".ferrugo_sig")
            || trimmed.starts_with(".byte 0x46,0x45")
            || trimmed.starts_with(".byte 0x01,0x00")
        {
            continue;
        }

        if trimmed == ".section .rodata" {
            result.push("    .section __TEXT,__const".to_string());
            continue;
        }

        if let Some(rest) = trimmed.strip_prefix(".globl ") {
            let sym = rest.trim();
            result.push(format!("    .globl _{sym}"));
            continue;
        }

        if let Some(label) = trimmed.strip_suffix(':')
            && symbols.contains(label)
        {
            result.push(format!("_{label}:"));
            continue;
        }

        // call 命令: ローカルラベル以外は全て _ プレフィックス
        if trimmed.starts_with("call ") {
            let target = trimmed.strip_prefix("call ").unwrap().trim();
            if !target.starts_with('.') && !target.starts_with('*') {
                result.push(format!("    call _{target}"));
                continue;
            }
        }

        // leaq 命令
        if trimmed.starts_with("leaq ")
            && let Some(rip_pos) = trimmed.find("(%rip)")
        {
            let after_leaq = &trimmed[5..rip_pos];
            if symbols.contains(after_leaq) {
                let suffix = &trimmed[rip_pos..];
                result.push(format!("    leaq _{after_leaq}{suffix}"));
                continue;
            }
        }

        result.push(line.to_string());
    }

    result.join("\n") + "\n"
}

/// テストヘルパー: コンパイル → リンク → 実行し、(終了コード, stdout) を返す。
fn compile_and_run_capture(source: &str) -> (i32, String) {
    let dir = TempDir::new().unwrap();
    let src_path = dir.path().join("test.c");
    let asm_path = dir.path().join("test.s");
    let bin_path = dir.path().join("test");

    std::fs::write(&src_path, source).unwrap();

    // Step 1: FerrugoCC で .s を生成
    let output = Command::new(env!("CARGO_BIN_EXE_ferrugocc"))
        .arg("-S")
        .arg(&src_path)
        .output()
        .expect("failed to run compiler");
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
        "gcc failed:\nstderr: {}",
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

    let code = run_output.status.code().unwrap_or(-1);
    let stdout = String::from_utf8_lossy(&run_output.stdout).to_string();
    (code, stdout)
}

/// 終了コードのみ検証するヘルパー
fn compile_and_run(source: &str) -> i32 {
    compile_and_run_capture(source).0
}

// ── 基本テスト: printf 宣言と呼び出し ──

/// printf の基本呼び出し（文字列のみ）
#[test]
fn variadic_printf_basic() {
    if !can_run_x86_64() {
        return;
    }
    let source = r#"
int printf(char *fmt, ...);
int main(void) {
    printf("hello\n");
    return 0;
}
"#;
    let (code, stdout) = compile_and_run_capture(source);
    assert_eq!(code, 0);
    assert_eq!(stdout, "hello\n");
}

/// printf に整数引数を渡す
#[test]
fn variadic_printf_int_arg() {
    if !can_run_x86_64() {
        return;
    }
    let source = r#"
int printf(char *fmt, ...);
int main(void) {
    printf("%d\n", 42);
    return 0;
}
"#;
    let (code, stdout) = compile_and_run_capture(source);
    assert_eq!(code, 0);
    assert_eq!(stdout, "42\n");
}

/// printf に複数の整数引数を渡す
#[test]
fn variadic_printf_multiple_int_args() {
    if !can_run_x86_64() {
        return;
    }
    let source = r#"
int printf(char *fmt, ...);
int main(void) {
    printf("%d %d %d\n", 1, 2, 3);
    return 0;
}
"#;
    let (code, stdout) = compile_and_run_capture(source);
    assert_eq!(code, 0);
    assert_eq!(stdout, "1 2 3\n");
}

/// printf に double 引数を渡す（%al ABI 検証）
#[test]
fn variadic_printf_double_arg() {
    if !can_run_x86_64() {
        return;
    }
    let source = r#"
int printf(char *fmt, ...);
int main(void) {
    printf("%.2f\n", 3.14);
    return 0;
}
"#;
    let (code, stdout) = compile_and_run_capture(source);
    assert_eq!(code, 0);
    assert_eq!(stdout, "3.14\n");
}

/// printf に int と double の混合引数
#[test]
fn variadic_printf_mixed_args() {
    if !can_run_x86_64() {
        return;
    }
    let source = r#"
int printf(char *fmt, ...);
int main(void) {
    printf("%d %.1f\n", 42, 3.14);
    return 0;
}
"#;
    let (code, stdout) = compile_and_run_capture(source);
    assert_eq!(code, 0);
    assert_eq!(stdout, "42 3.1\n");
}

/// printf の戻り値を使う（文字数を返す）
#[test]
fn variadic_printf_return_value() {
    if !can_run_x86_64() {
        return;
    }
    let source = r#"
int printf(char *fmt, ...);
int main(void) {
    int n = printf("hi\n");
    return n;
}
"#;
    let code = compile_and_run(source);
    assert_eq!(code, 3); // "hi\n" = 3 characters
}

/// 7個以上の引数（スタック渡しの検証）
#[test]
fn variadic_printf_many_args() {
    if !can_run_x86_64() {
        return;
    }
    let source = r#"
int printf(char *fmt, ...);
int main(void) {
    printf("%d %d %d %d %d %d %d\n", 1, 2, 3, 4, 5, 6, 7);
    return 0;
}
"#;
    let (code, stdout) = compile_and_run_capture(source);
    assert_eq!(code, 0);
    assert_eq!(stdout, "1 2 3 4 5 6 7\n");
}

/// 文字列引数を渡す
#[test]
fn variadic_printf_string_arg() {
    if !can_run_x86_64() {
        return;
    }
    let source = r#"
int printf(char *fmt, ...);
int main(void) {
    printf("%s %s\n", "hello", "world");
    return 0;
}
"#;
    let (code, stdout) = compile_and_run_capture(source);
    assert_eq!(code, 0);
    assert_eq!(stdout, "hello world\n");
}
