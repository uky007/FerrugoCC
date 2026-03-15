//! E2E テスト — typedef サポートの検証
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

        if let Some(rip_pos) = trimmed.find("(%rip)") {
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

// ── テストケース ──

/// 基本: typedef int myint; myint x = 42; return x; → 42
#[test]
fn typedef_basic_int() {
    if !can_run_x86_64() {
        return;
    }
    let source = r#"
typedef int myint;
int main(void) {
    myint x = 42;
    return x;
}
"#;
    assert_eq!(compile_and_run(source), 42);
}

/// ポインタ: typedef int *pint; int x = 10; pint p = &x; return *p; → 10
#[test]
fn typedef_pointer() {
    if !can_run_x86_64() {
        return;
    }
    let source = r#"
typedef int *pint;
int main(void) {
    int x = 10;
    pint p = &x;
    return *p;
}
"#;
    assert_eq!(compile_and_run(source), 10);
}

/// 構造体: typedef struct { int x; int y; } Point; → 7
#[test]
fn typedef_struct() {
    if !can_run_x86_64() {
        return;
    }
    let source = r#"
typedef struct point { int x; int y; } Point;
int main(void) {
    Point p;
    p.x = 3;
    p.y = 4;
    return p.x + p.y;
}
"#;
    assert_eq!(compile_and_run(source), 7);
}

/// 関数パラメータ: typedef long mylong; → 42
#[test]
fn typedef_function_param() {
    if !can_run_x86_64() {
        return;
    }
    let source = r#"
typedef long mylong;
mylong add(mylong a, mylong b) {
    return a + b;
}
int main(void) {
    return add(20, 22);
}
"#;
    assert_eq!(compile_and_run(source), 42);
}

/// 配列: typedef int arr3[3]; → 6
#[test]
fn typedef_array() {
    if !can_run_x86_64() {
        return;
    }
    let source = r#"
typedef int arr3[3];
int main(void) {
    arr3 a = {1, 2, 3};
    return a[0] + a[1] + a[2];
}
"#;
    assert_eq!(compile_and_run(source), 6);
}

/// ネスト: typedef int *pint; typedef pint *ppint; → 5
#[test]
fn typedef_nested() {
    if !can_run_x86_64() {
        return;
    }
    let source = r#"
typedef int *pint;
typedef pint *ppint;
int main(void) {
    int x = 5;
    pint p = &x;
    ppint pp = &p;
    return **pp;
}
"#;
    assert_eq!(compile_and_run(source), 5);
}

/// unsigned: typedef unsigned long usize_t; → 1
#[test]
fn typedef_unsigned() {
    if !can_run_x86_64() {
        return;
    }
    let source = r#"
typedef unsigned long usize_t;
int main(void) {
    usize_t x = 100;
    return x == 100;
}
"#;
    assert_eq!(compile_and_run(source), 1);
}

/// グローバル typedef 変数: typedef int myint; myint g = 30; → 30
#[test]
fn typedef_global_var() {
    if !can_run_x86_64() {
        return;
    }
    let source = r#"
typedef int myint;
myint g = 30;
int main(void) {
    return g;
}
"#;
    assert_eq!(compile_and_run(source), 30);
}

/// typedef int (*binop_t)(int, int) — fn ptr typedef → 42
#[test]
fn typedef_fn_ptr() {
    if !can_run_x86_64() {
        return;
    }
    let source = r#"
typedef int (*binop_t)(int, int);
int add(int a, int b) { return a + b; }
int main(void) {
    binop_t op = add;
    return op(30, 12);
}
"#;
    assert_eq!(compile_and_run(source), 42);
}

/// typedef int handler_t(int, int) — fn type typedef + pointer decl → 42
#[test]
fn typedef_fn_type() {
    if !can_run_x86_64() {
        return;
    }
    let source = r#"
typedef int handler_t(int, int);
int add(int a, int b) { return a + b; }
int main(void) {
    handler_t *op = add;
    return op(30, 12);
}
"#;
    assert_eq!(compile_and_run(source), 42);
}

/// typedef int *retptr_t(void) — fn type returning pointer → 42
#[test]
fn typedef_fn_returning_pointer() {
    if !can_run_x86_64() {
        return;
    }
    let source = r#"
typedef int *retptr_t(void);
int g_val = 42;
int *get_ptr(void) { return &g_val; }
int main(void) {
    retptr_t *fp = get_ptr;
    int *p = fp();
    return *p;
}
"#;
    assert_eq!(compile_and_run(source), 42);
}

/// typedef fn ptr 経由の apply パターン → 84
#[test]
fn typedef_fn_ptr_apply() {
    if !can_run_x86_64() {
        return;
    }
    let source = r#"
typedef int (*binop_t)(int, int);
int add(int a, int b) { return a + b; }
int sub(int a, int b) { return a - b; }
int apply(binop_t op, int x, int y) { return op(x, y); }
int main(void) {
    int r1 = apply(add, 30, 12);
    int r2 = apply(sub, 50, 8);
    return r1 + r2;
}
"#;
    assert_eq!(compile_and_run(source), 84);
}

/// ブロック内 typedef: → 99
#[test]
fn typedef_block_scope() {
    if !can_run_x86_64() {
        return;
    }
    let source = r#"
int main(void) {
    typedef int myint;
    myint x = 99;
    return x;
}
"#;
    assert_eq!(compile_and_run(source), 99);
}
