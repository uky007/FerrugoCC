//! E2E テスト — 可変長引数関数の定義サポート（va_list / va_start / va_arg / va_end）
//!
//! C ソースをコンパイル → gcc リンク → 実行 → 終了コード検証。

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

        if let Some(label) = trimmed.strip_suffix(':') {
            if symbols.contains(label) {
                result.push(format!("_{label}:"));
                continue;
            }
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
        if trimmed.starts_with("leaq ") {
            if let Some(rip_pos) = trimmed.find("(%rip)") {
                let after_leaq = &trimmed[5..rip_pos];
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

/// テストヘルパー: コンパイル → リンク → 実行し、終了コードを返す。
fn compile_and_run(source: &str) -> i32 {
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

    run_output.status.code().unwrap_or(-1)
}

// ── テスト 1: 基本 int 可変長引数 ──

#[test]
fn va_def_basic_int_sum() {
    if !can_run_x86_64() { return; }
    let source = r#"
int my_sum(int count, ...) {
    va_list ap;
    va_start(ap, count);
    int sum = 0;
    for (int i = 0; i < count; i = i + 1) {
        sum = sum + va_arg(ap, int);
    }
    va_end(ap);
    return sum;
}

int main(void) {
    return my_sum(3, 10, 20, 12);
}
"#;
    assert_eq!(compile_and_run(source), 42);
}

// ── テスト 2: long 引数 ──

#[test]
fn va_def_long_args() {
    if !can_run_x86_64() { return; }
    let source = r#"
long my_sum_long(int count, ...) {
    va_list ap;
    va_start(ap, count);
    long sum = 0L;
    for (int i = 0; i < count; i = i + 1) {
        sum = sum + va_arg(ap, long);
    }
    va_end(ap);
    return sum;
}

int main(void) {
    long result = my_sum_long(3, 10L, 20L, 12L);
    return (int)result;
}
"#;
    assert_eq!(compile_and_run(source), 42);
}

// ── テスト 3: double 引数 ──

#[test]
fn va_def_double_args() {
    if !can_run_x86_64() { return; }
    let source = r#"
int check_double_sum(int count, ...) {
    va_list ap;
    va_start(ap, count);
    double sum = 0.0;
    for (int i = 0; i < count; i = i + 1) {
        sum = sum + va_arg(ap, double);
    }
    va_end(ap);
    if (sum > 41.9 && sum < 42.1)
        return 1;
    return 0;
}

int main(void) {
    return check_double_sum(3, 10.5, 20.5, 11.0);
}
"#;
    assert_eq!(compile_and_run(source), 1);
}

// ── テスト 4: 混合型（int と long） ──

#[test]
fn va_def_mixed_int_long() {
    if !can_run_x86_64() { return; }
    let source = r#"
long mixed_sum(int count, ...) {
    va_list ap;
    va_start(ap, count);
    long sum = 0L;
    for (int i = 0; i < count; i = i + 1) {
        sum = sum + va_arg(ap, long);
    }
    va_end(ap);
    return sum;
}

int main(void) {
    long result = mixed_sum(4, 10L, 20L, 5L, 7L);
    return (int)result;
}
"#;
    assert_eq!(compile_and_run(source), 42);
}

// ── テスト 5: 多数引数（7個以上、スタック渡し発生） ──

#[test]
fn va_def_many_args_stack_overflow() {
    if !can_run_x86_64() { return; }
    let source = r#"
int sum_many(int count, ...) {
    va_list ap;
    va_start(ap, count);
    int sum = 0;
    for (int i = 0; i < count; i = i + 1) {
        sum = sum + va_arg(ap, int);
    }
    va_end(ap);
    return sum;
}

int main(void) {
    return sum_many(8, 1, 2, 3, 4, 5, 6, 7, 14);
}
"#;
    assert_eq!(compile_and_run(source), 42);
}

// ── テスト 6: ポインタ引数 ──

#[test]
fn va_def_pointer_args() {
    if !can_run_x86_64() { return; }
    let source = r#"
int sum_ptrs(int count, ...) {
    va_list ap;
    va_start(ap, count);
    int sum = 0;
    for (int i = 0; i < count; i = i + 1) {
        int *p = va_arg(ap, int *);
        sum = sum + *p;
    }
    va_end(ap);
    return sum;
}

int main(void) {
    int a = 10;
    int b = 20;
    int c = 12;
    return sum_ptrs(3, &a, &b, &c);
}
"#;
    assert_eq!(compile_and_run(source), 42);
}

// ── テスト 7: 単一引数 ──

#[test]
fn va_def_single_arg() {
    if !can_run_x86_64() { return; }
    let source = r#"
int get_first(int count, ...) {
    va_list ap;
    va_start(ap, count);
    int val = va_arg(ap, int);
    va_end(ap);
    return val;
}

int main(void) {
    return get_first(1, 42);
}
"#;
    assert_eq!(compile_and_run(source), 42);
}
