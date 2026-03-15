//! E2E テスト — 関数ポインタ型情報の検証
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
        "gcc failed:\nasm:\n{}\nstderr: {}",
        std::fs::read_to_string(&asm_path).unwrap_or_default(),
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

/// 基本: 関数ポインタ経由の間接呼び出し → 42
#[test]
fn fn_ptr_basic_indirect_call() {
    if !can_run_x86_64() {
        return;
    }
    let source = r#"
int add(int a, int b) { return a + b; }
int main(void) {
    int (*op)(int, int) = add;
    return op(30, 12);
}
"#;
    assert_eq!(compile_and_run(source), 42);
}

/// apply パターン: 関数ポインタをパラメータとして渡す → 84
#[test]
fn fn_ptr_apply_pattern() {
    if !can_run_x86_64() {
        return;
    }
    let source = r#"
int add(int a, int b) { return a + b; }
int sub(int a, int b) { return a - b; }
int apply(int (*op)(int, int), int x, int y) { return op(x, y); }
int main(void) {
    int r1 = apply(add, 30, 12);
    int r2 = apply(sub, 50, 8);
    return r1 + r2;
}
"#;
    assert_eq!(compile_and_run(source), 84);
}

/// typedef 関数ポインタ → 42
#[test]
fn fn_ptr_typedef() {
    if !can_run_x86_64() {
        return;
    }
    let source = r#"
typedef int (*binop_t)(int, int);
int mul(int a, int b) { return a * b; }
int main(void) {
    binop_t op = mul;
    return op(6, 7);
}
"#;
    assert_eq!(compile_and_run(source), 42);
}

/// 関数ポインタ配列 → 42
#[test]
fn fn_ptr_array() {
    if !can_run_x86_64() {
        return;
    }
    let source = r#"
int add(int a, int b) { return a + b; }
int sub(int a, int b) { return a - b; }
int main(void) {
    int (*ops[2])(int, int);
    ops[0] = add;
    ops[1] = sub;
    int r1 = ops[0](20, 10);
    int r2 = ops[1](22, 10);
    return r1 + r2;
}
"#;
    assert_eq!(compile_and_run(source), 42);
}

/// 戻り値型の伝搬: long を返す関数ポインタ → 42
#[test]
fn fn_ptr_return_type_long() {
    if !can_run_x86_64() {
        return;
    }
    let source = r#"
long big(long x) { return x; }
int main(void) {
    long (*fp)(long) = big;
    long v = fp(42);
    return (int)v;
}
"#;
    assert_eq!(compile_and_run(source), 42);
}

/// pointer-to-array: int (*p)[3] = &arr; return (*p)[1]; → 20
#[test]
fn declarator_pointer_to_array() {
    if !can_run_x86_64() {
        return;
    }
    let source = r#"
int main(void) {
    int arr[3];
    arr[0] = 10;
    arr[1] = 20;
    arr[2] = 12;
    int (*p)[3] = &arr;
    return (*p)[1];
}
"#;
    assert_eq!(compile_and_run(source), 20);
}

/// pointer-to-array: 加算でアクセス → 42
#[test]
fn declarator_pointer_to_array_sum() {
    if !can_run_x86_64() {
        return;
    }
    let source = r#"
int main(void) {
    int arr[3];
    arr[0] = 10;
    arr[1] = 20;
    arr[2] = 12;
    int (*p)[3] = &arr;
    return (*p)[0] + (*p)[1] + (*p)[2];
}
"#;
    assert_eq!(compile_and_run(source), 42);
}

/// abstract declarator grouping: (int (*)[3]) cast → 42
#[test]
fn declarator_abstract_group_cast() {
    if !can_run_x86_64() {
        return;
    }
    let source = r#"
int main(void) {
    int arr[3];
    arr[0] = 10;
    arr[1] = 20;
    arr[2] = 12;
    int (*p)[3] = (int (*)[3])&arr;
    return (*p)[0] + (*p)[1] + (*p)[2];
}
"#;
    assert_eq!(compile_and_run(source), 42);
}

/// abstract declarator grouping: sizeof(int (*)[3]) → 8 (pointer size)
#[test]
fn declarator_abstract_group_sizeof() {
    if !can_run_x86_64() {
        return;
    }
    let source = r#"
int main(void) {
    return sizeof(int (*)[3]);
}
"#;
    assert_eq!(compile_and_run(source), 8);
}

/// abstract declarator: fn ptr cast (int (*)(int, int)) → 42
#[test]
fn declarator_abstract_fn_ptr_cast() {
    if !can_run_x86_64() {
        return;
    }
    let source = r#"
int add(int a, int b) { return a + b; }
int main(void) {
    int (*op)(int, int) = (int (*)(int, int))add;
    return op(30, 12);
}
"#;
    assert_eq!(compile_and_run(source), 42);
}

/// function returning pointer: int *f(void) → 42
#[test]
fn declarator_fn_returning_pointer() {
    if !can_run_x86_64() {
        return;
    }
    let source = r#"
int g_val = 42;
int *get_ptr(void) { return &g_val; }
int main(void) {
    int *p = get_ptr();
    return *p;
}
"#;
    assert_eq!(compile_and_run(source), 42);
}

/// 関数ポインタの切り替え → 42
#[test]
fn fn_ptr_reassign() {
    if !can_run_x86_64() {
        return;
    }
    let source = r#"
int inc(int x) { return x + 1; }
int dec(int x) { return x - 1; }
int main(void) {
    int (*fp)(int) = inc;
    int r1 = fp(20);
    fp = dec;
    int r2 = fp(22);
    return r1 + r2;
}
"#;
    assert_eq!(compile_and_run(source), 42);
}

/// コールバックパターン: 構造体メンバとしての関数ポインタ → 42
#[test]
fn fn_ptr_struct_member() {
    if !can_run_x86_64() {
        return;
    }
    let source = r#"
struct handler {
    int (*callback)(int, int);
};
int add(int a, int b) { return a + b; }
int main(void) {
    struct handler h;
    h.callback = add;
    return h.callback(30, 12);
}
"#;
    assert_eq!(compile_and_run(source), 42);
}
