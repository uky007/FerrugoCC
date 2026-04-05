//! Multi-file compilation tests

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

/// Multi-file 用 macOS fixup。
/// 単一ファイル版と異なり、外部シンボルも単純に `_` prefix する
/// （@GOTPCREL は不要 — 同じバイナリ内のシンボル同士）。
fn fixup_asm_for_macos(asm: &str) -> String {
    let mut result = Vec::new();
    for line in asm.lines() {
        let trimmed = line.trim();
        if trimmed.contains(".note.GNU-stack") || trimmed.contains(".ferrugo_sig") || trimmed.starts_with(".byte 0x46,0x45") || trimmed.starts_with(".byte 0x01,0x00") {
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
            // call sym → call _sym
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
            // sym(%rip) → _sym(%rip)
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
                    let original = format!("{sym}(%rip)");
                    let replacement = format!("_{sym}(%rip)");
                    if let Some(pos) = new_line[search_from..].find(&original) {
                        let abs_pos = search_from + pos;
                        new_line = format!(
                            "{}{}{}",
                            &new_line[..abs_pos],
                            replacement,
                            &new_line[abs_pos + original.len()..]
                        );
                        search_from = abs_pos + replacement.len();
                    } else {
                        search_from = rip_idx + 6;
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

fn compile_multi_and_run(files: &[(&str, &str)], obfuscate: bool) -> (i32, String) {
    let dir = TempDir::new().unwrap();

    // Write source files
    let mut src_paths = Vec::new();
    for (name, content) in files {
        let path = dir.path().join(name);
        std::fs::write(&path, content).unwrap();
        src_paths.push(path);
    }

    // Compile ALL .c to .s in one invocation (so preserve_globals is set for multi-file)
    {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_ferrugocc"));
        if obfuscate {
            cmd.arg("--fobfuscate");
        }
        cmd.arg("-S");
        for src_path in &src_paths {
            cmd.arg(src_path);
        }
        let output = cmd.output().expect("failed to run compiler");
        assert!(
            output.status.success(),
            "compilation failed (obf={obfuscate}):\n{}",
            String::from_utf8_lossy(&output.stderr),
        );

    }

    // macOS fixup for all .s files
    if cfg!(target_os = "macos") {
        for src_path in &src_paths {
            let asm_path = src_path.with_extension("s");
            if asm_path.exists() {
                let asm = std::fs::read_to_string(&asm_path).unwrap();
                let asm = fixup_asm_for_macos(&asm);
                std::fs::write(&asm_path, asm).unwrap();
            }
        }
    }

    // Assemble each .s to .o
    let mut obj_paths = Vec::new();
    for src_path in &src_paths {
        let asm_path = src_path.with_extension("s");
        let obj_path = src_path.with_extension("o");
        let gcc = if cfg!(target_arch = "x86_64") {
            Command::new("gcc")
                .arg("-c")
                .arg(&asm_path)
                .arg("-o")
                .arg(&obj_path)
                .output()
                .expect("failed to run gcc -c")
        } else {
            Command::new("arch")
                .args(["-x86_64", "gcc"])
                .arg("-c")
                .arg(&asm_path)
                .arg("-o")
                .arg(&obj_path)
                .output()
                .expect("failed to run arch -x86_64 gcc -c")
        };
        assert!(
            gcc.status.success(),
            "gcc -c failed for {}:\n{}",
            asm_path.display(),
            String::from_utf8_lossy(&gcc.stderr),
        );
        obj_paths.push(obj_path);
    }

    // Link all .o files
    let bin_path = dir.path().join("test");
    let gcc = if cfg!(target_arch = "x86_64") {
        let mut cmd = Command::new("gcc");
        for obj in &obj_paths {
            cmd.arg(obj);
        }
        cmd.arg("-o").arg(&bin_path);
        if cfg!(target_os = "linux") {
            cmd.arg("-no-pie");
        }
        cmd.output().expect("failed to run gcc link")
    } else {
        let mut cmd = Command::new("arch");
        cmd.args(["-x86_64", "gcc"]);
        for obj in &obj_paths {
            cmd.arg(obj);
        }
        cmd.arg("-o").arg(&bin_path);
        cmd.output().expect("failed to run arch -x86_64 gcc link")
    };
    assert!(
        gcc.status.success(),
        "gcc link failed:\n{}",
        String::from_utf8_lossy(&gcc.stderr),
    );

    // Run
    let run = if cfg!(target_arch = "x86_64") {
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

    let code = run.status.code().unwrap_or(-1);
    let stdout = String::from_utf8_lossy(&run.stdout).to_string();
    (code, stdout)
}

#[test]
fn multi_file_basic() {
    if !can_run_x86_64() {
        return;
    }
    let lib_c = r#"
int add(int a, int b) { return a + b; }
int mul(int a, int b) { return a * b; }
"#;
    let main_c = r#"
int add(int, int);
int mul(int, int);
int main(void) {
    return add(10, mul(4, 8));
}
"#;
    let (n_code, _) = compile_multi_and_run(&[("lib.c", lib_c), ("main.c", main_c)], false);
    assert_eq!(n_code, 42, "normal: expected 42, got {n_code}");

    let (o_code, _) = compile_multi_and_run(&[("lib.c", lib_c), ("main.c", main_c)], true);
    assert_eq!(o_code, 42, "obfuscated: expected 42, got {o_code}");
}

#[test]
fn multi_file_global_var() {
    if !can_run_x86_64() {
        return;
    }
    let data_c = r#"
int counter = 0;
void increment(void) { counter += 1; }
int get_counter(void) { return counter; }
"#;
    let main_c = r#"
int printf(const char *, ...);
void increment(void);
int get_counter(void);
int main(void) {
    increment();
    increment();
    increment();
    int r = get_counter();
    printf("counter=%d\n", r);
    return r;
}
"#;
    let (n_code, n_out) = compile_multi_and_run(&[("data.c", data_c), ("main.c", main_c)], false);
    assert_eq!(n_code, 3);
    assert_eq!(n_out, "counter=3\n");

    let (o_code, o_out) = compile_multi_and_run(&[("data.c", data_c), ("main.c", main_c)], true);
    assert_eq!(o_code, n_code, "exit code mismatch");
    assert_eq!(o_out, n_out, "stdout mismatch");
}

#[test]
fn multi_file_three_files() {
    if !can_run_x86_64() {
        return;
    }
    let math_c = r#"
int square(int x) { return x * x; }
"#;
    let util_c = r#"
int square(int);
int sum_of_squares(int a, int b) { return square(a) + square(b); }
"#;
    let main_c = r#"
int printf(const char *, ...);
int sum_of_squares(int, int);
int main(void) {
    int r = sum_of_squares(3, 4);
    printf("sos=%d\n", r);
    return r;
}
"#;
    let files = &[("math.c", math_c), ("util.c", util_c), ("main.c", main_c)];
    let (n_code, n_out) = compile_multi_and_run(files, false);
    assert_eq!(n_code, 25);
    assert_eq!(n_out, "sos=25\n");

    let (o_code, o_out) = compile_multi_and_run(files, true);
    assert_eq!(o_code, n_code, "exit code mismatch");
    assert_eq!(o_out, n_out, "stdout mismatch");
}

/// 個別の `-S --fobfuscate` (単一ファイル) で global シンボルが保持されることを検証。
/// preserve_globals が compile_only 時に有効でないとシンボルが消えてリンクエラーになる。
/// (ドライバの -c と同じ preserve_globals パスを通る)
#[test]
fn single_file_obfuscate_preserves_symbols() {
    if !can_run_x86_64() {
        return;
    }
    let lib_c = r#"
int add(int a, int b) { return a + b; }
"#;
    let main_c = r#"
int add(int, int);
int main(void) {
    return add(40, 2);
}
"#;
    let dir = TempDir::new().unwrap();
    let lib_path = dir.path().join("lib.c");
    let main_path = dir.path().join("main.c");
    std::fs::write(&lib_path, lib_c).unwrap();
    std::fs::write(&main_path, main_c).unwrap();

    // Compile each file INDIVIDUALLY with -S -c --fobfuscate (single file per invocation)
    for src in [&lib_path, &main_path] {
        let output = Command::new(env!("CARGO_BIN_EXE_ferrugocc"))
            .arg("--fobfuscate")
            .arg("-c")
            .arg("-S")
            .arg(src)
            .output()
            .expect("failed to run compiler");
        assert!(
            output.status.success(),
            "-S -c --fobfuscate failed for {}:\n{}",
            src.display(),
            String::from_utf8_lossy(&output.stderr),
        );
    }

    // macOS fixup for all .s files
    if cfg!(target_os = "macos") {
        for src in [&lib_path, &main_path] {
            let asm_path = src.with_extension("s");
            if asm_path.exists() {
                let asm = std::fs::read_to_string(&asm_path).unwrap();
                let asm = fixup_asm_for_macos(&asm);
                std::fs::write(&asm_path, asm).unwrap();
            }
        }
    }

    // Assemble each .s to .o
    let mut obj_paths = Vec::new();
    for src in [&lib_path, &main_path] {
        let asm_path = src.with_extension("s");
        let obj_path = src.with_extension("o");
        let gcc = if cfg!(target_arch = "x86_64") {
            Command::new("gcc")
                .arg("-c")
                .arg(&asm_path)
                .arg("-o")
                .arg(&obj_path)
                .output()
                .expect("failed to run gcc -c")
        } else {
            Command::new("arch")
                .args(["-x86_64", "gcc"])
                .arg("-c")
                .arg(&asm_path)
                .arg("-o")
                .arg(&obj_path)
                .output()
                .expect("failed to run arch -x86_64 gcc -c")
        };
        assert!(
            gcc.status.success(),
            "gcc -c failed for {}:\n{}",
            asm_path.display(),
            String::from_utf8_lossy(&gcc.stderr),
        );
        obj_paths.push(obj_path);
    }

    // Link the two .o files
    let bin_path = dir.path().join("test");
    let link = if cfg!(target_arch = "x86_64") {
        let mut cmd = Command::new("gcc");
        for obj in &obj_paths {
            cmd.arg(obj);
        }
        cmd.arg("-o").arg(&bin_path);
        if cfg!(target_os = "linux") {
            cmd.arg("-no-pie");
        }
        cmd.output().expect("failed to link")
    } else {
        let mut cmd = Command::new("arch");
        cmd.args(["-x86_64", "gcc"]);
        for obj in &obj_paths {
            cmd.arg(obj);
        }
        cmd.arg("-o").arg(&bin_path);
        cmd.output().expect("failed to link")
    };
    assert!(
        link.status.success(),
        "link failed (symbols likely renamed by OPSEC):\n{}",
        String::from_utf8_lossy(&link.stderr),
    );

    // Run and verify
    let run = if cfg!(target_arch = "x86_64") {
        Command::new(&bin_path).output().expect("failed to run")
    } else {
        Command::new("arch")
            .arg("-x86_64")
            .arg(&bin_path)
            .output()
            .expect("failed to run")
    };
    assert_eq!(run.status.code().unwrap_or(-1), 42);
}
