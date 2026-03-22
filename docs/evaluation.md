# FerrugoCC — Evaluation Methodology

> **Baseline version**: v0.3.0 (tag `v0.3.0`, 2026-03-22)

## 1. Correctness Evaluation

### 1.1 Corpus Pass Rate
```bash
# Run all corpus tests (normal + obfuscated)
cargo test --test corpus

# Expected: all pass, 0 ignored
```

| Metric | Value |
|--------|-------|
| Tier 1 corpora | 4 (jsmn, inih, sds, pdjson) |
| Tier 2 corpora | 7 (kilo, sbase-cat/wc/printf/head/cut/uniq) |
| Total test groups | 12 normal + 12 obfuscated = 24 |
| Pass rate target | 24/24 (100%) |

### 1.2 Meaning-Preservation
```bash
# Run meaning-preservation tests (normal == obfuscated stdout + exit code)
cargo test --test meaning_preservation

# Expected: all 20 categories pass
```

Verifies that for each test program:
- `normal.exit_code == obfuscated.exit_code`
- `normal.stdout == obfuscated.stdout`

| Category | C Patterns Exercised |
|----------|---------------------|
| arithmetic | integer ops, printf |
| strings | strlen, strcmp |
| loops_arrays | for, array indexing |
| struct_pointer | member access via pointer |
| function_pointer | typedef callbacks |
| switch | switch/case, string returns |
| recursion | fibonacci |
| globals | static variable mutation |
| variadic | printf formats, snprintf |
| bitwise | AND/OR/XOR/shift, flags |
| logical | short-circuit &&/\|\|, ternary |
| pointer_arith | offset, difference, cast |
| enum | constants, bitflags |
| long_arithmetic | 64-bit math, unsigned overflow |
| nested_struct | struct-in-struct, array-of-struct |
| do_while_break | do-while + continue + break |
| initializers | implicit array, designated init |
| linked_list | malloc/free, struct-with-pointer |
| builtin_abs | __builtin_abs, __builtin_labs |
| builtin_bits | popcount, ctz, clz |

### 1.3 Obfuscation Unit Tests
```bash
# Run obfuscation-specific tests
cargo test --test obfuscation

# Expected: 75 pass, 0 ignored
```

## 2. Obfuscation Strength Evaluation

### 2.1 Code Size Increase
```bash
# Compare assembly line counts: normal vs obfuscated
ferrugocc -S source.c                    # normal
ferrugocc --fobfuscate -S source.c       # obfuscated (level 3)

wc -l source.s  # compare
```

### 2.2 Control Flow Complexity
Measure per-function:
- **Basic block count**: number of blocks in CFG
- **Edge count**: number of CFG edges
- **Cyclomatic complexity**: edges - nodes + 2

```bash
# Generate assembly and count labels (proxy for basic blocks)
ferrugocc -S source.c
grep -c "^\.L" source.s           # normal
ferrugocc --fobfuscate -S source.c
grep -c "^\.L" source.s           # obfuscated
```

### 2.3 Symbol Exposure
```bash
# Count externally visible symbols
nm -g binary_normal | wc -l
nm -g binary_obfuscated | wc -l
# OPSEC strip should reduce visible symbols to minimum (main + libc)
```

### 2.4 String Exposure
```bash
# Count string literals in binary
strings binary_normal | wc -l
strings binary_obfuscated | wc -l
# String encryption should reduce readable strings
```

### 2.5 Obfuscation Level Comparison
| Metric | Level 0 (normal) | Level 1 | Level 2 | Level 3 | Level 4 |
|--------|-----------------|---------|---------|---------|---------|
| ASM lines | baseline | ~2x | ~5x | ~10x | ~15x |
| Basic blocks | baseline | ~1.5x | ~3x | ~5x | ~8x |
| Symbols | all visible | all visible | renamed | renamed + stripped | renamed + stripped |
| Strings | plain | plain | plain | encrypted | encrypted |
| CFF | no | no | yes | yes | yes |

## 3. Reverse-Engineering Resistance

### 3.1 Decompiler Analysis
Test with:
- **Ghidra** (NSA): decompile obfuscated binary, measure function recovery
- **IDA Pro / Hex-Rays**: if available
- **RetDec**: open-source decompiler

Metrics:
- Function identification rate (# recovered / # actual)
- Control flow graph accuracy (structural similarity to original)
- String cross-reference availability

### 3.2 Comparison with Existing Tools
Compare FerrugoCC obfuscation against:
- **OLLVM**: LLVM-based obfuscator (CFF, bogus control flow, substitution)
- **Tigress**: source-to-source C obfuscator
- No obfuscation baseline

## 4. Performance Overhead

### 4.1 Compilation Time
```bash
time ferrugocc -S source.c                      # normal
time ferrugocc --fobfuscate -S source.c          # obfuscated
time ferrugocc --fobfuscate --obf-level=1 -S source.c  # level 1
```

### 4.2 Binary Size
```bash
ls -la binary_normal binary_obfuscated
```

### 4.3 Runtime Performance
```bash
# For corpora with measurable workloads
time ./binary_normal < input.txt
time ./binary_obfuscated < input.txt
```

## 5. Reproducibility

### 5.1 Environment
```bash
# Required
rustc --version     # Rust toolchain
gcc --version       # GNU C compiler (for preprocessing + linking)
uname -a            # OS and architecture

# Optional (for reverse-engineering evaluation)
ghidra --version    # Ghidra decompiler
```

### 5.2 Full Test Suite
```bash
# Complete reproducibility check
cargo test                    # all tests
cargo clippy --all-targets -- -D warnings  # no warnings
cargo fmt --check             # consistent formatting
```

### 5.3 Corpus Provenance
Each corpus directory contains an `ORIGIN` file documenting:
- Source URL
- License
- Retrieval date
- Local patches applied
