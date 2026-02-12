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

        result.push(line.to_string());
    }

    result.join("\n") + "\n"
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
