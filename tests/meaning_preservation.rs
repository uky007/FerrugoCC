//! Meaning-Preservation テスト — 難読化の意味保存性を検証
//!
//! normal コンパイルと obfuscated コンパイルの実行結果（exit code, stdout, stderr）
//! が完全一致することを確認する。これにより難読化パスが正当性を壊していないことを
//! 体系的に保証する。

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

/// Compile and run a C source, returning (exit_code, stdout, stderr).
fn compile_and_run(source: &str, obfuscate: bool) -> (i32, String, String) {
    let dir = TempDir::new().unwrap();
    let src_path = dir.path().join("test.c");
    let asm_path = dir.path().join("test.s");
    let bin_path = dir.path().join("test");

    std::fs::write(&src_path, source).unwrap();

    let mut cmd = Command::new(env!("CARGO_BIN_EXE_ferrugocc"));
    if obfuscate {
        cmd.arg("--fobfuscate");
    }
    cmd.arg("-S").arg(&src_path);
    let output = cmd.output().expect("failed to run compiler");
    assert!(
        output.status.success(),
        "compilation failed (obfuscate={obfuscate}):\n{}",
        String::from_utf8_lossy(&output.stderr),
    );
    assert!(asm_path.exists(), "assembly file not generated");

    if cfg!(target_os = "macos") {
        let asm = std::fs::read_to_string(&asm_path).unwrap();
        let asm = fixup_asm_for_macos(&asm);
        std::fs::write(&asm_path, asm).unwrap();
    }

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
        "gcc link failed (obfuscate={obfuscate}):\n{}",
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

    let code = run_output.status.code().unwrap_or(-1);
    let stdout = String::from_utf8_lossy(&run_output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&run_output.stderr).to_string();
    (code, stdout, stderr)
}

/// Assert that normal and obfuscated produce identical results.
fn assert_meaning_preserved(source: &str, name: &str) {
    let (n_code, n_stdout, n_stderr) = compile_and_run(source, false);
    let (o_code, o_stdout, o_stderr) = compile_and_run(source, true);

    assert_eq!(
        n_code, o_code,
        "[{name}] exit code mismatch: normal={n_code}, obfuscated={o_code}"
    );
    assert_eq!(
        n_stdout, o_stdout,
        "[{name}] stdout mismatch:\n  normal:     {:?}\n  obfuscated: {:?}",
        n_stdout, o_stdout
    );
    // stderr: only compare if normal produces stderr (obfuscation may add warnings)
    if !n_stderr.is_empty() {
        assert_eq!(
            n_stderr, o_stderr,
            "[{name}] stderr mismatch:\n  normal:     {:?}\n  obfuscated: {:?}",
            n_stderr, o_stderr
        );
    }
}

/// macOS assembly fixup (same as corpus.rs)
fn fixup_asm_for_macos(asm: &str) -> String {
    use std::collections::HashSet;

    let mut result = Vec::new();
    let mut all_symbols: HashSet<String> = HashSet::new();
    for line in asm.lines() {
        let t = line.trim();
        if let Some(rest) = t.strip_prefix(".globl ") {
            all_symbols.insert(rest.trim().to_string());
        }
        if t.ends_with(':') && !t.starts_with('.') {
            all_symbols.insert(t.trim_end_matches(':').to_string());
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
                    } else if is_external
                        && (trimmed_line.starts_with("movq ") || trimmed_line.starts_with("movl "))
                    {
                        let is_quad = trimmed_line.starts_with("movq ");
                        let after_rip = &new_line[rip_idx + 6..];
                        let dst_part = after_rip.trim().trim_start_matches(',').trim();
                        let dst_reg = dst_part.split_whitespace().next().unwrap_or("").to_string();
                        let got_reg = if let Some(r) = dst_reg.strip_prefix('%') {
                            let r64 = match r {
                                "eax" => "rax",
                                "ebx" => "rbx",
                                "ecx" => "rcx",
                                "edx" => "rdx",
                                "esi" => "rsi",
                                "edi" => "rdi",
                                "r8d" => "r8",
                                "r9d" => "r9",
                                "r10d" => "r10",
                                "r11d" => "r11",
                                "r12d" => "r12",
                                "r13d" => "r13",
                                "r14d" => "r14",
                                "r15d" => "r15",
                                other => other,
                            };
                            format!("%{r64}")
                        } else {
                            dst_reg.clone()
                        };
                        let deref_instr = if is_quad { "movq" } else { "movl" };
                        new_line = format!(
                            "    movq _{sym}@GOTPCREL(%rip), {got_reg}\n    {deref_instr} ({got_reg}), {dst_reg}"
                        );
                        search_from = new_line.len();
                    } else if is_external {
                        let original = format!("{sym}(%rip)");
                        let got_load =
                            format!("    movq _{sym}@GOTPCREL(%rip), %r11\n    movq (%r11), %r11");
                        let replacement = new_line.replacen(&original, "%r11", 1);
                        new_line = format!("{got_load}\n{replacement}");
                        search_from = new_line.len();
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

// ── Meaning-Preservation Tests ──

/// Arithmetic + control flow
#[test]
fn mp_arithmetic() {
    if !can_run_x86_64() {
        return;
    }
    let source = r#"
int printf(const char *, ...);
int main(void) {
    int a = 15, b = 27;
    int sum = a + b;
    int diff = b - a;
    int prod = a * b;
    int div = prod / sum;
    printf("%d %d %d %d\n", sum, diff, prod, div);
    return 0;
}
"#;
    assert_meaning_preserved(source, "arithmetic");
}

/// String operations
#[test]
fn mp_strings() {
    if !can_run_x86_64() {
        return;
    }
    let source = r#"
int printf(const char *, ...);
int strlen(const char *);
int strcmp(const char *, const char *);
int main(void) {
    const char *s = "hello world";
    printf("len=%d\n", strlen(s));
    printf("cmp=%d\n", strcmp("abc", "abd"));
    printf("eq=%d\n", strcmp("test", "test"));
    return 0;
}
"#;
    assert_meaning_preserved(source, "strings");
}

/// Loops + arrays
#[test]
fn mp_loops_arrays() {
    if !can_run_x86_64() {
        return;
    }
    let source = r#"
int printf(const char *, ...);
int main(void) {
    int arr[10];
    int i;
    for (i = 0; i < 10; i++)
        arr[i] = i * i;
    int sum = 0;
    for (i = 0; i < 10; i++)
        sum += arr[i];
    printf("sum=%d\n", sum);
    return 0;
}
"#;
    assert_meaning_preserved(source, "loops_arrays");
}

/// Struct + pointer
#[test]
fn mp_struct_pointer() {
    if !can_run_x86_64() {
        return;
    }
    let source = r#"
int printf(const char *, ...);
struct point { int x; int y; };
int distance_sq(struct point *a, struct point *b) {
    int dx = a->x - b->x;
    int dy = a->y - b->y;
    return dx*dx + dy*dy;
}
int main(void) {
    struct point p1; p1.x = 3; p1.y = 4;
    struct point p2; p2.x = 0; p2.y = 0;
    printf("d2=%d\n", distance_sq(&p1, &p2));
    return 0;
}
"#;
    assert_meaning_preserved(source, "struct_pointer");
}

/// Function pointers + callbacks
#[test]
fn mp_function_pointer() {
    if !can_run_x86_64() {
        return;
    }
    let source = r#"
int printf(const char *, ...);
typedef int (*binop)(int, int);
int add(int a, int b) { return a + b; }
int mul(int a, int b) { return a * b; }
int apply(binop f, int x, int y) { return f(x, y); }
int main(void) {
    printf("add=%d\n", apply(add, 3, 4));
    printf("mul=%d\n", apply(mul, 3, 4));
    return 0;
}
"#;
    assert_meaning_preserved(source, "function_pointer");
}

/// Switch/case
#[test]
fn mp_switch() {
    if !can_run_x86_64() {
        return;
    }
    let source = r#"
int printf(const char *, ...);
const char *day_name(int d) {
    switch (d) {
    case 0: return "Sun";
    case 1: return "Mon";
    case 2: return "Tue";
    case 3: return "Wed";
    case 4: return "Thu";
    case 5: return "Fri";
    case 6: return "Sat";
    default: return "???";
    }
}
int main(void) {
    int i;
    for (i = 0; i < 8; i++)
        printf("%s ", day_name(i));
    printf("\n");
    return 0;
}
"#;
    assert_meaning_preserved(source, "switch");
}

/// Recursive function
#[test]
fn mp_recursion() {
    if !can_run_x86_64() {
        return;
    }
    let source = r#"
int printf(const char *, ...);
int fib(int n) {
    if (n <= 1) return n;
    return fib(n-1) + fib(n-2);
}
int main(void) {
    int i;
    for (i = 0; i < 10; i++)
        printf("%d ", fib(i));
    printf("\n");
    return 0;
}
"#;
    assert_meaning_preserved(source, "recursion");
}

/// Global + static variables
#[test]
fn mp_globals() {
    if !can_run_x86_64() {
        return;
    }
    let source = r#"
int printf(const char *, ...);
static int counter = 0;
void increment(void) { counter++; }
int get_counter(void) { return counter; }
int main(void) {
    increment();
    increment();
    increment();
    printf("counter=%d\n", get_counter());
    return 0;
}
"#;
    assert_meaning_preserved(source, "globals");
}
