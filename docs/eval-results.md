# FerrugoCC Evaluation Results

> **Version**: dev/future-features branch (post-v0.3.0)
> **Baseline tag**: v0.3.0 (`110afd4`, 2026-03-22) — code expansion metrics measured at v0.3.0
> **Date**: 2026-03-28
> **Platform**: Linux x86_64 (GitHub Actions), macOS arm64 (Rosetta 2, dev)

## Test Suite

| Suite | Tests | Pass | Fail | Ignored |
|-------|-------|------|------|---------|
| corpus (normal + obfuscated) | 24 | 24 | 0 | 0 |
| meaning-preservation | 33 | 33 | 0 | 0 |
| obfuscation unit | 75 | 75 | 0 | 0 |
| multi-file | 3 | 3 | 0 | 0 |
| **Total** | **135** | **135** | **0** | **0** |

## Benchmark Correctness (Linux x86_64, CI)

20 benchmark programs × 11 obfuscation conditions = **220/220 pass** (100%).

Conditions: L0 (normal), L1–L4 (obfuscation levels), L3 minus individual passes (CFF, strings, arithmetic, inlining, outlining), L4 minus VM.

## Code Size (Assembly Lines)

| Corpus | Normal | L1 | L2 | L3 | L3/Normal |
|--------|--------|-----|-----|-----|-----------|
| jsmn | 1,506 | 3,361 | 8,418 | 14,694 | 9.7x |
| inih | 1,664 | 3,745 | 8,922 | 19,153 | 11.5x |
| sds | 5,695 | 11,136 | 25,946 | 61,585 | 10.8x |
| pdjson | 7,468 | 14,540 | 33,475 | 86,899 | 11.6x |
| kilo | 8,081 | 15,808 | 36,734 | 89,842 | 11.1x |
| sbase-cat | 1,568 | 2,672 | 5,552 | 12,784 | 8.1x |
| sbase-wc | 2,426 | 4,588 | 10,571 | 22,116 | 9.1x |
| sbase-printf | 2,538 | 4,505 | 9,973 | 22,915 | 9.0x |
| sbase-head | 1,366 | 2,371 | 5,107 | 12,593 | 9.2x |
| sbase-cut | 3,280 | 6,232 | 15,221 | 33,408 | 10.2x |
| sbase-uniq | 2,335 | 4,516 | 10,690 | 23,096 | 9.9x |
| **Average** | | | | | **10.1x** |

## Symbol Exposure (OPSEC)

| Corpus | Normal .globl | Obf .globl | Reduction |
|--------|---------------|------------|-----------|
| jsmn | 1 | 1 | — |
| inih | 8 | 1 | 87.5% |
| sds | 50 | 1 | 98.0% |
| pdjson | 24 | 1 | 95.8% |
| kilo | 45 | 1 | 97.8% |
| sbase-cat | 13 | 1 | 92.3% |
| sbase-wc | 23 | 1 | 95.7% |
| sbase-printf | 29 | 1 | 96.6% |
| sbase-head | 11 | 1 | 90.9% |
| sbase-cut | 17 | 1 | 94.1% |
| sbase-uniq | 13 | 1 | 92.3% |

OPSEC strip: all exported symbols reduced to `main` only (95%+ average reduction).

## String Exposure

| Corpus | Normal | Obfuscated | Encrypted |
|--------|--------|------------|-----------|
| jsmn | 1 | 0 | 100% |
| inih | 8 | 0 | 100% |
| sds | 21 | 0 | 100% |
| pdjson | 79 | 0 | 100% |
| kilo | 130 | 0 | 100% |
| sbase-cat | 18 | 0 | 100% |
| sbase-wc | 16 | 0 | 100% |
| sbase-printf | 35 | 0 | 100% |
| sbase-head | 15 | 0 | 100% |
| sbase-cut | 44 | 0 | 100% |
| sbase-uniq | 25 | 0 | 100% |

Level 3 string encryption: 100% of string literals encrypted across all corpora.

## Static Analysis Proxy Metrics (Linux x86_64, CI)

> Measured via `objdump -d`, `nm`, `strings` on benchmark binaries.
> These are proxy metrics for reverse-engineering difficulty, not decompilation results.

### Per-Level Averages (20 benchmarks)

| Level | Avg Function Symbols | Avg Instructions | Avg Invalid Insns | Avg Readable Symbols |
|-------|---------------------|-----------------|-------------------|---------------------|
| L0 (normal) | 11.0 | 220 | 0.0 | 9.6 |
| L1 | 11.0 | 357 | 0.0 | 9.6 |
| L2 | 11.0 | 710 | 0.0 | 9.6 |
| **L3** | **39.7** | **1,679** | **40.4** | **9.8** |
| L4 | 73.8 | 15,427 | 207.0 | 11.7 |

### Key Observations

- **Code expansion**: L0→L3 instruction count increases 7.6x on average
- **Anti-disassembly**: L3 produces 40.4 invalid instructions per binary (linear sweep confusion); L4 produces 207.0
- **Function proliferation**: L3 has 3.6x more function symbols than L0 (outlining splits functions); L4 has 6.7x (VM dispatch adds handler functions)
- **Symbol sanitization**: Readable symbols remain ~10 across all levels (toolchain-only: `_start`, `main`, `frame_dummy`, etc.). All user-defined symbols are OPSEC-renamed at L2+.
- **Instruction expansion by pass** (L3 minus one pass):
  - Without CFF: 1,512 insns (CFF adds ~10%)
  - Without arithmetic substitution: 1,511 insns (~10% from arith)
  - Without outlining: 1,392 insns (outlining adds ~17%)
  - Without inlining: 1,400 insns (inlining adds ~17%)

## Binary Size (Linux x86_64, CI)

> Measured via `stat` (total), `objdump -h` (.text section), `strip` (stripped).

| Level | Avg Total | Avg .text (Code) | Avg Stripped | Code Expansion |
|-------|----------|-----------------|-------------|----------------|
| L0 (normal) | 15.7 KB | 665 B | 14.3 KB | 1.0x |
| L1 | 15.7 KB | 1.1 KB | 14.3 KB | 1.7x |
| L2 | 16.2 KB | 2.4 KB | 14.8 KB | 3.6x |
| **L3** | **20.3 KB** | **5.4 KB** | **17.9 KB** | **8.2x** |
| L4 | 99.1 KB | 59.4 KB | 90.4 KB | 89.4x |

L3 code section expands 8.2x (consistent with 7.6x instruction count expansion).
L4 VM virtualization causes 89.4x code expansion — the VM dispatch loop and bytecode tables dominate.

## Compilation Time (Linux x86_64, CI)

> Measured via `date +%s%N` around `ferrugocc -S` invocation.

| Level | Avg Compile Time | Range | Overhead vs L0 |
|-------|-----------------|-------|----------------|
| L0 | 9 ms | 7–11 ms | 1.0x |
| L1 | 12 ms | 7–23 ms | 1.3x |
| L2 | 35 ms | 8–104 ms | 3.9x |
| **L3** | **61 ms** | **8–438 ms** | **6.8x** |
| L4 | 4,006 ms | 8–45,888 ms | 445x |

L3 compilation overhead is modest (6.8x). L4 VM virtualization is expensive (445x) due to bytecode generation and dispatch table construction.

## Runtime Performance (Linux x86_64, CI)

> Measured via bash TIMEFORMAT with 5-second timeout, 5 runs per binary.

| Level | Avg Runtime |
|-------|------------|
| L0 | ~2 ms |
| L1 | ~2 ms |
| L2 | ~2 ms |
| L3 | ~2 ms |
| L4 | ~2 ms |

All levels produce identical ~2ms runtime on the benchmark suite. The benchmarks are too small (~50–300 lines of C, single-function workloads) to produce measurable execution time differences. This indicates that **for small programs, obfuscation runtime overhead is below measurement threshold**. Larger workloads (e.g., kilo at 1,300 lines with I/O loops) would be needed to quantify runtime overhead.

### Note on strings_count

Raw `strings` output increases with obfuscation level (L0: 64.3, L3: 98.2, L4: 433.3) because encrypted string ciphertext, VM bytecode, and junk code produce printable byte sequences. This does NOT indicate increased string exposure — original plaintext literals are encrypted at L3+. The correct interpretation is "original literal recovery rate = 0%" (verified by corpus string encryption tests).
