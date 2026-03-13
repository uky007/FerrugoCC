//! E2E テスト — 配列初期化子リスト（array initializer lists）の検証
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

        if trimmed.contains(".note.GNU-stack") {
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

// ── 基本テスト ──

/// 基本: int arr[3] = {1, 2, 3}; return arr[1]; → 終了コード 2
#[test]
fn array_init_basic() {
    if !can_run_x86_64() {
        return;
    }
    let source = r#"
int main(void) {
    int arr[3] = {1, 2, 3};
    return arr[1];
}
"#;
    assert_eq!(compile_and_run(source), 2);
}

/// 部分初期化: int arr[5] = {10, 20}; return arr[2]; → 終了コード 0（残りゼロ初期化）
#[test]
fn array_init_partial() {
    if !can_run_x86_64() {
        return;
    }
    let source = r#"
int main(void) {
    int arr[5] = {10, 20};
    return arr[2];
}
"#;
    assert_eq!(compile_and_run(source), 0);
}

/// 部分初期化: 明示的要素のアクセス
#[test]
fn array_init_partial_explicit() {
    if !can_run_x86_64() {
        return;
    }
    let source = r#"
int main(void) {
    int arr[5] = {10, 20};
    return arr[0];
}
"#;
    assert_eq!(compile_and_run(source), 10);
}

/// char 配列: char s[4] = {'a', 'b', 'c', 0}; printf("%s\n", s);
#[test]
fn array_init_char() {
    if !can_run_x86_64() {
        return;
    }
    let source = r#"
int printf(char *fmt, ...);
int main(void) {
    char s[4] = {97, 98, 99, 0};
    printf("%s\n", s);
    return 0;
}
"#;
    let (code, stdout) = compile_and_run_capture(source);
    assert_eq!(code, 0);
    assert_eq!(stdout, "abc\n");
}

/// long 配列
#[test]
fn array_init_long() {
    if !can_run_x86_64() {
        return;
    }
    let source = r#"
int main(void) {
    long arr[2] = {100, 200};
    long sum = arr[0] + arr[1];
    return sum == 300;
}
"#;
    assert_eq!(compile_and_run(source), 1);
}

/// グローバル配列: ファイルスコープ int arr[3] = {1, 2, 3};
#[test]
fn array_init_global() {
    if !can_run_x86_64() {
        return;
    }
    let source = r#"
int arr[3] = {1, 2, 3};
int main(void) {
    return arr[0] + arr[1] + arr[2];
}
"#;
    assert_eq!(compile_and_run(source), 6);
}

/// static ローカル配列
#[test]
fn array_init_static_local() {
    if !can_run_x86_64() {
        return;
    }
    let source = r#"
int main(void) {
    static int arr[3] = {10, 20, 30};
    return arr[2];
}
"#;
    assert_eq!(compile_and_run(source), 30);
}

/// 式初期化子: int x = 5; int arr[2] = {x, x + 1};
#[test]
fn array_init_with_expressions() {
    if !can_run_x86_64() {
        return;
    }
    let source = r#"
int main(void) {
    int x = 5;
    int arr[2] = {x, x + 1};
    return arr[0] + arr[1];
}
"#;
    assert_eq!(compile_and_run(source), 11);
}

// ── char s[] = "hello" (文字列リテラルによる char 配列初期化) ──

/// char s[] = "hello": サイズ推論 + null 終端
#[test]
fn char_array_from_string_basic() {
    if !can_run_x86_64() {
        return;
    }
    let source = r#"
int main(void) {
    char s[] = "hello";
    if (s[0] != 104) return 1;
    if (s[4] != 111) return 2;
    if (s[5] != 0) return 3;
    return 42;
}
"#;
    assert_eq!(compile_and_run(source), 42);
}

/// char s[10] = "hi": 明示サイズ + ゼロ埋め
#[test]
fn char_array_from_string_explicit_size() {
    if !can_run_x86_64() {
        return;
    }
    let source = r#"
int main(void) {
    char s[10] = "hi";
    if (s[0] != 104) return 1;
    if (s[1] != 105) return 2;
    if (s[2] != 0) return 3;
    if (s[9] != 0) return 4;
    return 42;
}
"#;
    assert_eq!(compile_and_run(source), 42);
}

/// グローバル char[] = "hello"
#[test]
fn char_array_from_string_global() {
    if !can_run_x86_64() {
        return;
    }
    let source = r#"
char greeting[] = "world";
int main(void) {
    if (greeting[0] != 119) return 1;
    if (greeting[4] != 100) return 2;
    if (greeting[5] != 0) return 3;
    return 42;
}
"#;
    assert_eq!(compile_and_run(source), 42);
}

/// static ローカル char[] = "abc"
#[test]
fn char_array_from_string_static() {
    if !can_run_x86_64() {
        return;
    }
    let source = r#"
int main(void) {
    static char s[] = "abc";
    if (s[0] != 97) return 1;
    if (s[2] != 99) return 2;
    if (s[3] != 0) return 3;
    return 42;
}
"#;
    assert_eq!(compile_and_run(source), 42);
}
