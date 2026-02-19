//! E2E テスト — switch/case/default + enum サポートの検証
//!
//! C ソースをコンパイル → gcc リンク → 実行 → 終了コード検証。

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

        if trimmed.starts_with("call ") {
            let target = trimmed.strip_prefix("call ").unwrap().trim();
            if !target.starts_with('.') && !target.starts_with('*') {
                result.push(format!("    call _{target}"));
                continue;
            }
        }

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

        if let Some(rip_pos) = trimmed.find("(%rip)") {
            let before_rip = &trimmed[..rip_pos];
            let sym_start = before_rip.rfind(|c: char| c == ' ' || c == ',' || c == '\t')
                .map(|i| i + 1)
                .unwrap_or(0);
            let sym_name = &before_rip[sym_start..];
            if !sym_name.is_empty() && symbols.contains(sym_name) {
                let mut fixed = String::new();
                fixed.push_str(&trimmed[..sym_start]);
                fixed.push('_');
                fixed.push_str(&trimmed[sym_start..]);
                result.push(format!("    {fixed}"));
                continue;
            }
        }

        result.push(line.to_string());
    }

    result.join("\n") + "\n"
}

fn compile_and_run(source: &str) -> i32 {
    let dir = TempDir::new().unwrap();
    let src_path = dir.path().join("test.c");
    let asm_path = dir.path().join("test.s");
    let bin_path = dir.path().join("test");

    std::fs::write(&src_path, source).unwrap();

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

// ══════════════════════════════════════
// Enum テスト
// ══════════════════════════════════════

/// 基本 enum: RED=0, GREEN=1, BLUE=2
#[test]
fn enum_basic() {
    if !can_run_x86_64() { return; }
    let source = r#"
enum Color { RED, GREEN, BLUE };
int main(void) {
    return BLUE;
}
"#;
    assert_eq!(compile_and_run(source), 2);
}

/// 明示値 + 自動インクリメント
#[test]
fn enum_explicit_values() {
    if !can_run_x86_64() { return; }
    let source = r#"
enum Nums { A = 10, B, C = 20, D };
int main(void) {
    return A + B + C + D;
}
"#;
    // A=10, B=11, C=20, D=21 → 62
    assert_eq!(compile_and_run(source), 62);
}

/// 負値 enum
#[test]
fn enum_negative() {
    if !can_run_x86_64() { return; }
    let source = r#"
enum Sign { NEG = -1, ZERO = 0, POS = 1 };
int main(void) {
    int x = NEG + ZERO + POS;
    return x;
}
"#;
    assert_eq!(compile_and_run(source), 0);
}

/// enum 変数宣言
#[test]
fn enum_variable() {
    if !can_run_x86_64() { return; }
    let source = r#"
enum Dir { UP, DOWN, LEFT, RIGHT };
int main(void) {
    int d = RIGHT;
    return d;
}
"#;
    assert_eq!(compile_and_run(source), 3);
}

/// typedef enum
#[test]
fn enum_typedef() {
    if !can_run_x86_64() { return; }
    let source = r#"
typedef enum { OFF, ON } State;
int main(void) {
    State s = ON;
    return s;
}
"#;
    assert_eq!(compile_and_run(source), 1);
}

/// 無名 enum
#[test]
fn enum_anonymous() {
    if !can_run_x86_64() { return; }
    let source = r#"
enum { X = 5, Y = 10 };
int main(void) {
    return X + Y;
}
"#;
    assert_eq!(compile_and_run(source), 15);
}

/// enum の算術演算
#[test]
fn enum_arithmetic() {
    if !can_run_x86_64() { return; }
    let source = r#"
enum { A = 3, B = 7 };
int main(void) {
    return A * B - A;
}
"#;
    // 3*7-3 = 18
    assert_eq!(compile_and_run(source), 18);
}

// ══════════════════════════════════════
// Switch テスト
// ══════════════════════════════════════

/// 基本 switch + break
#[test]
fn switch_basic_break() {
    if !can_run_x86_64() { return; }
    let source = r#"
int main(void) {
    int x = 2;
    int r = 0;
    switch (x) {
        case 1: r = 10; break;
        case 2: r = 20; break;
        case 3: r = 30; break;
    }
    return r;
}
"#;
    assert_eq!(compile_and_run(source), 20);
}

/// fall-through（break なし）
#[test]
fn switch_fallthrough() {
    if !can_run_x86_64() { return; }
    let source = r#"
int main(void) {
    int x = 1;
    int r = 0;
    switch (x) {
        case 1: r = r + 1;
        case 2: r = r + 2;
        case 3: r = r + 4;
    }
    return r;
}
"#;
    // case 1 → r=1, falls to case 2 → r=3, falls to case 3 → r=7
    assert_eq!(compile_and_run(source), 7);
}

/// default ケース
#[test]
fn switch_default() {
    if !can_run_x86_64() { return; }
    let source = r#"
int main(void) {
    int x = 99;
    int r = 0;
    switch (x) {
        case 1: r = 10; break;
        case 2: r = 20; break;
        default: r = 42; break;
    }
    return r;
}
"#;
    assert_eq!(compile_and_run(source), 42);
}

/// default なし（マッチしない場合）
#[test]
fn switch_no_match() {
    if !can_run_x86_64() { return; }
    let source = r#"
int main(void) {
    int x = 99;
    int r = 5;
    switch (x) {
        case 1: r = 10; break;
        case 2: r = 20; break;
    }
    return r;
}
"#;
    assert_eq!(compile_and_run(source), 5);
}

/// switch + enum 定数の case ラベル
#[test]
fn switch_with_enum() {
    if !can_run_x86_64() { return; }
    let source = r#"
enum Color { RED, GREEN, BLUE };
int main(void) {
    int c = GREEN;
    int r = 0;
    switch (c) {
        case RED:   r = 1; break;
        case GREEN: r = 2; break;
        case BLUE:  r = 3; break;
    }
    return r;
}
"#;
    assert_eq!(compile_and_run(source), 2);
}

/// ネストした switch
#[test]
fn switch_nested() {
    if !can_run_x86_64() { return; }
    let source = r#"
int main(void) {
    int a = 1;
    int b = 2;
    int r = 0;
    switch (a) {
        case 1:
            switch (b) {
                case 1: r = 11; break;
                case 2: r = 12; break;
            }
            break;
        case 2:
            r = 20;
            break;
    }
    return r;
}
"#;
    assert_eq!(compile_and_run(source), 12);
}

/// switch 内 break vs ループ内 continue
#[test]
fn switch_break_loop_continue() {
    if !can_run_x86_64() { return; }
    let source = r#"
int main(void) {
    int sum = 0;
    int i;
    for (i = 0; i < 5; i = i + 1) {
        switch (i) {
            case 2: continue;
            default: break;
        }
        sum = sum + i;
    }
    return sum;
}
"#;
    // i=0: default→break, sum+=0 → sum=0
    // i=1: default→break, sum+=1 → sum=1
    // i=2: case 2→continue (skip sum+=)
    // i=3: default→break, sum+=3 → sum=4
    // i=4: default→break, sum+=4 → sum=8
    assert_eq!(compile_and_run(source), 8);
}

/// case に負値
#[test]
fn switch_negative_case() {
    if !can_run_x86_64() { return; }
    let source = r#"
int main(void) {
    int x = -1;
    int r = 0;
    switch (x) {
        case -1: r = 10; break;
        case 0:  r = 20; break;
        case 1:  r = 30; break;
    }
    return r;
}
"#;
    assert_eq!(compile_and_run(source), 10);
}

/// default が先頭にある場合
#[test]
fn switch_default_first() {
    if !can_run_x86_64() { return; }
    let source = r#"
int main(void) {
    int x = 2;
    int r = 0;
    switch (x) {
        default: r = 99; break;
        case 1:  r = 10; break;
        case 2:  r = 20; break;
    }
    return r;
}
"#;
    assert_eq!(compile_and_run(source), 20);
}

/// 空の switch body
#[test]
fn switch_empty_body() {
    if !can_run_x86_64() { return; }
    let source = r#"
int main(void) {
    int x = 1;
    switch (x) {
    }
    return 42;
}
"#;
    assert_eq!(compile_and_run(source), 42);
}

/// switch in while loop with break
#[test]
fn switch_in_while_loop() {
    if !can_run_x86_64() { return; }
    let source = r#"
int main(void) {
    int i = 0;
    int r = 0;
    while (i < 10) {
        switch (i) {
            case 5: r = 50; break;
            default: break;
        }
        if (r == 50) break;
        i = i + 1;
    }
    return r;
}
"#;
    assert_eq!(compile_and_run(source), 50);
}
