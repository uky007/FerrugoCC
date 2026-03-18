//! E2E テスト — ビット演算子
//!
//! ビット演算 (&, |, ^, <<, >>) とその複合代入、難読化対応を検証。

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

fn compile_and_run(source: &str, obfuscate: bool) -> i32 {
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
        "compilation failed (obfuscate={obfuscate}):\nstdout: {}\nstderr: {}",
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
        "gcc failed (obfuscate={obfuscate}):\nstderr: {}",
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
            if symbols.contains(label) {
                new_line = format!("_{label}:");
            }
        } else {
            for sym in &symbols {
                if new_line.contains(&format!("call {sym}"))
                    || new_line.contains(&format!("call\t{sym}"))
                {
                    new_line = new_line.replace(&format!("call {sym}"), &format!("call _{sym}"));
                    new_line = new_line.replace(&format!("call\t{sym}"), &format!("call\t_{sym}"));
                }
            }
        }
        result.push(new_line);
    }
    result.join("\n") + "\n"
}

// ── 基本ビット演算 ──

#[test]
fn bitwise_and() {
    if !can_run_x86_64() {
        return;
    }
    let src = r#"
int main(void) {
    int a = 0xFF;
    int b = 0x0F;
    return a & b;  // 15
}
"#;
    assert_eq!(compile_and_run(src, false), 15);
}

#[test]
fn bitwise_or() {
    if !can_run_x86_64() {
        return;
    }
    let src = r#"
int main(void) {
    int a = 0xF0;
    int b = 0x0F;
    return (a | b) - 200;  // 0xFF - 200 = 55
}
"#;
    assert_eq!(compile_and_run(src, false), 55);
}

#[test]
fn bitwise_xor() {
    if !can_run_x86_64() {
        return;
    }
    let src = r#"
int main(void) {
    int a = 0xFF;
    int b = 0xAA;
    return a ^ b;  // 0x55 = 85
}
"#;
    assert_eq!(compile_and_run(src, false), 85);
}

#[test]
fn shift_left() {
    if !can_run_x86_64() {
        return;
    }
    let src = r#"
int main(void) {
    int a = 1;
    return a << 5;  // 32
}
"#;
    assert_eq!(compile_and_run(src, false), 32);
}

#[test]
fn shift_right_signed() {
    if !can_run_x86_64() {
        return;
    }
    let src = r#"
int main(void) {
    int a = 128;
    return a >> 2;  // 32
}
"#;
    assert_eq!(compile_and_run(src, false), 32);
}

#[test]
fn shift_right_unsigned() {
    if !can_run_x86_64() {
        return;
    }
    let src = r#"
int main(void) {
    unsigned int a = 256u;
    return a >> 3;  // 32
}
"#;
    assert_eq!(compile_and_run(src, false), 32);
}

// ── 複合代入 ──

#[test]
fn bitwise_compound_assign() {
    if !can_run_x86_64() {
        return;
    }
    let src = r#"
int main(void) {
    int x = 0xFF;
    x &= 0x0F;    // x = 15
    x |= 0x30;    // x = 63
    x ^= 0x0A;    // x = 53
    return x;
}
"#;
    assert_eq!(compile_and_run(src, false), 53);
}

#[test]
fn shift_compound_assign() {
    if !can_run_x86_64() {
        return;
    }
    let src = r#"
int main(void) {
    int x = 1;
    x <<= 6;   // x = 64
    x >>= 1;   // x = 32
    return x;
}
"#;
    assert_eq!(compile_and_run(src, false), 32);
}

// ── XOR swap (classic bitwise pattern) ──

#[test]
fn xor_swap() {
    if !can_run_x86_64() {
        return;
    }
    let src = r#"
int main(void) {
    int a = 42;
    int b = 17;
    a = a ^ b;
    b = b ^ a;
    a = a ^ b;
    // now a=17, b=42
    return a + b;  // 59
}
"#;
    assert_eq!(compile_and_run(src, false), 59);
}

// ── XOR 暗号化パターン (Mirai 風) ──

#[test]
fn xor_encrypt_decrypt() {
    if !can_run_x86_64() {
        return;
    }
    let src = r#"
int main(void) {
    int data = 42;
    int key = 0x5A;
    int encrypted = data ^ key;
    int decrypted = encrypted ^ key;
    return decrypted;  // 42
}
"#;
    assert_eq!(compile_and_run(src, false), 42);
}

// ── ビットマスク + シフト (IP アドレス風操作) ──

#[test]
fn bit_mask_and_shift() {
    if !can_run_x86_64() {
        return;
    }
    let src = r#"
int main(void) {
    // Pack: (10 << 24) | (0 << 16) | (0 << 8) | 1  = 0x0A000001
    int ip = (10 << 24) | (0 << 16) | (0 << 8) | 1;
    int octet1 = (ip >> 24) & 0xFF;  // 10
    int octet4 = ip & 0xFF;          // 1
    return octet1 + octet4;           // 11
}
"#;
    assert_eq!(compile_and_run(src, false), 11);
}

// ── Xorshift32 RNG (Mirai rand.c 風) ──

#[test]
fn xorshift32() {
    if !can_run_x86_64() {
        return;
    }
    let src = r#"
int main(void) {
    unsigned int state = 12345u;
    // One iteration of xorshift32
    state = state ^ (state << 13);
    state = state ^ (state >> 17);
    state = state ^ (state << 5);
    // Check it produces a non-trivial value
    return (state != 0u) ? 42 : 1;
}
"#;
    assert_eq!(compile_and_run(src, false), 42);
}

// ── const パース ──

#[test]
fn const_qualifier_parse() {
    if !can_run_x86_64() {
        return;
    }
    let src = r#"
int main(void) {
    const int x = 42;
    const int y = x + 1;
    return y - 1;  // 42
}
"#;
    assert_eq!(compile_and_run(src, false), 42);
}

#[test]
fn const_pointer_parse() {
    if !can_run_x86_64() {
        return;
    }
    let src = r#"
int main(void) {
    int x = 42;
    const int *p = &x;
    return *p;  // 42
}
"#;
    assert_eq!(compile_and_run(src, false), 42);
}

// ── 難読化でもビット演算が正しく動く ──

#[test]
fn bitwise_obfuscated() {
    if !can_run_x86_64() {
        return;
    }
    let src = r#"
int main(void) {
    int a = 0xFF;
    int b = 0x0F;
    int c = a & b;      // 15
    int d = c | 0x30;   // 63
    int e = d ^ 0x0A;   // 53
    int f = 1 << 5;     // 32
    int g = 128 >> 2;   // 32
    return e - f - g + 64;  // 53 - 32 - 32 + 64 = 53
}
"#;
    let normal = compile_and_run(src, false);
    let obfuscated = compile_and_run(src, true);
    assert_eq!(normal, 53);
    assert_eq!(obfuscated, 53);
}

// ── pointer -= on general lvalue (non-variable) ──

#[test]
fn pointer_subtract_assign_deref() {
    if !can_run_x86_64() {
        return;
    }
    let src = r#"
int main(void) {
    int arr[5];
    arr[0] = 10;
    arr[1] = 20;
    arr[2] = 30;
    arr[3] = 40;
    arr[4] = 42;
    int *p = &arr[4];
    int **pp = &p;
    *pp -= 2;
    return *p + arr[0];
}
"#;
    assert_eq!(compile_and_run(src, false), 40);
}

// ── __builtin_bswap{16,32,64} lowering ──

#[test]
fn bswap16_basic() {
    if !can_run_x86_64() {
        return;
    }
    let src = r#"
int main(void) {
    // 0x0102 -> 0x0201
    unsigned int x = __builtin_bswap16(0x0102);
    if (x != 0x0201) return 1;
    // 0xAABB -> 0xBBAA
    unsigned int y = __builtin_bswap16(0xAABB);
    if (y != 0xBBAA) return 2;
    return 42;
}
"#;
    assert_eq!(compile_and_run(src, false), 42);
}

#[test]
fn bswap32_basic() {
    if !can_run_x86_64() {
        return;
    }
    let src = r#"
int main(void) {
    unsigned int x = __builtin_bswap32(0x01020304);
    if (x != 0x04030201) return 1;
    // round-trip
    unsigned int rt = __builtin_bswap32(__builtin_bswap32(0xDEADBEEF));
    if (rt != 0xDEADBEEF) return 2;
    return 42;
}
"#;
    assert_eq!(compile_and_run(src, false), 42);
}

#[test]
fn bswap64_basic() {
    if !can_run_x86_64() {
        return;
    }
    let src = r#"
int main(void) {
    unsigned long x = __builtin_bswap64(0x0102030405060708UL);
    if (x != 0x0807060504030201UL) return 1;
    return 42;
}
"#;
    assert_eq!(compile_and_run(src, false), 42);
}
