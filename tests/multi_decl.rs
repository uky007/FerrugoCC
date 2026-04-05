//! E2E テスト — カンマ区切り複数宣言（comma-separated multiple declarations）の検証
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

        if trimmed.starts_with("call ") {
            let target = trimmed.strip_prefix("call ").unwrap().trim();
            if !target.starts_with('.') && !target.starts_with('*') {
                result.push(format!("    call _{target}"));
                continue;
            }
        }

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

        // General RIP-relative symbol references: movl/addl/etc symbol(%rip), ...
        if let Some(rip_pos) = trimmed.find("(%rip)") {
            // Find the symbol name before (%rip): scan backwards for whitespace or comma
            let before_rip = &trimmed[..rip_pos];
            let sym_start = before_rip
                .rfind([' ', ',', '\t'])
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

    run_output.status.code().unwrap_or(-1)
}

// ── テストケース ──

/// 基本: int a = 1, b = 2, c = 3; return a + b + c; → 終了コード 6
#[test]
fn multi_decl_basic() {
    if !can_run_x86_64() {
        return;
    }
    let source = r#"
int main(void) {
    int a = 1, b = 2, c = 3;
    return a + b + c;
}
"#;
    assert_eq!(compile_and_run(source), 6);
}

/// ポインタ混在: int x = 10, *p = &x; return *p; → 終了コード 10
#[test]
fn multi_decl_pointer_mix() {
    if !can_run_x86_64() {
        return;
    }
    let source = r#"
int main(void) {
    int x = 10, *p = &x;
    return *p;
}
"#;
    assert_eq!(compile_and_run(source), 10);
}

/// 初期化なし混在: int a = 5, b, c = 3; b = 7; return a + b + c; → 終了コード 15
#[test]
fn multi_decl_partial_init() {
    if !can_run_x86_64() {
        return;
    }
    let source = r#"
int main(void) {
    int a = 5, b, c = 3;
    b = 7;
    return a + b + c;
}
"#;
    assert_eq!(compile_and_run(source), 15);
}

/// グローバル: int g1 = 10, g2 = 20; int main() { return g1 + g2; } → 終了コード 30
#[test]
fn multi_decl_global() {
    if !can_run_x86_64() {
        return;
    }
    let source = r#"
int g1 = 10, g2 = 20;
int main(void) {
    return g1 + g2;
}
"#;
    assert_eq!(compile_and_run(source), 30);
}

/// for ループ: for (int i = 0, sum = 0; ...) → 終了コード 10
#[test]
fn multi_decl_for_loop() {
    if !can_run_x86_64() {
        return;
    }
    let source = r#"
int main(void) {
    int result;
    for (int i = 0, sum = 0; i < 5; i = i + 1) {
        sum = sum + i;
        result = sum;
    }
    return result;
}
"#;
    assert_eq!(compile_and_run(source), 10);
}

/// long 型混在: long a = 100, b = 200; return (a + b) == 300; → 終了コード 1
#[test]
fn multi_decl_long() {
    if !can_run_x86_64() {
        return;
    }
    let source = r#"
int main(void) {
    long a = 100, b = 200;
    return (a + b) == 300;
}
"#;
    assert_eq!(compile_and_run(source), 1);
}

/// 配列混在: int x = 1, arr[3] = {2, 3, 4}; return x + arr[0] + arr[1] + arr[2]; → 終了コード 10
#[test]
fn multi_decl_array_mix() {
    if !can_run_x86_64() {
        return;
    }
    let source = r#"
int main(void) {
    int x = 1, arr[3] = {2, 3, 4};
    return x + arr[0] + arr[1] + arr[2];
}
"#;
    assert_eq!(compile_and_run(source), 10);
}

/// static: static int a = 10, b = 20; return a + b; → 終了コード 30
#[test]
fn multi_decl_static() {
    if !can_run_x86_64() {
        return;
    }
    let source = r#"
int main(void) {
    static int a = 10, b = 20;
    return a + b;
}
"#;
    assert_eq!(compile_and_run(source), 30);
}
