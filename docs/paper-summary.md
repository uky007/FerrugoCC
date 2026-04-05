# FerrugoCC — Paper Summary

> Hub document linking all evaluation resources.

## 1. Project Snapshot

- **Evaluation version**: main branch post-v0.3.0 (includes all v0.3.0 features + struct return >16B, builtin lowering, implicit array size, float independent type, multi-file compilation)
- **Baseline tag**: v0.3.0 (`110afd4`, 2026-03-22) — code expansion metrics measured here; correctness metrics reflect current main
- **Architecture**: x86_64 System V ABI (Linux glibc + macOS Rosetta)
- **Language**: Rust implementation (~12,000 lines), compiles a practical C subset
- **Scope**: C source → x86_64 assembly with 16 obfuscation passes
- **Multi-file**: multiple `.c` files compiled independently, linked via `gcc`
- **Preprocessing**: delegated to `gcc -E` (not self-implemented)
- **Linking**: delegated to `gcc` (assembler + linker)

## 2. Core Claim

FerrugoCC is an experimental C compiler with integrated multi-pass
obfuscation, implemented in Rust. It compiles a practical subset of C
(not full ISO C — see §3 and §7 for scope and limitations) to x86_64
assembly and applies up to 16 obfuscation transformations while
preserving program semantics. We evaluate correctness on 11 real-world
open-source C programs totaling ~5,150 lines, demonstrating meaning
preservation between normal and obfuscated compilation across 31 test
categories. At the default obfuscation level (Level 3), code size
increases by 10.1x on average, exported symbols are reduced by 95%+,
and all string literals are encrypted. These results suggest that
integrated source-level obfuscation can provide meaningful resistance
to static reverse engineering for the supported C subset, while
maintaining functional equivalence verified by automated testing.

## 3. Supported Coverage

→ Full details: [docs/coverage.md](coverage.md)

**Types**: short, int, long, char, float, double, void, pointers, arrays (multi-dim),
structs (nested, self-referential), unions, enums, va_list, function
pointers. Struct return by value for all sizes (≤16B via RAX+RDX, >16B
via hidden sret pointer).

**Approximate types**: `long double`→`double`.

**Statements**: if/else, while, do-while, for, switch/case, goto/labels,
break/continue, ternary, sizeof, casts, all assignment operators,
designated array/struct initializers, implicit array size inference.

**GCC extensions**: `__attribute__`, `__asm__`, `__extension__`,
`__builtin_va_*`, `__builtin_bswap*`, `__builtin_abs/ctz/clz/popcount`
(correctly lowered), `__builtin_expect` (hint, pass-through).

**16 obfuscation passes** at 4 configurable levels.

## 4. Corpus Set

→ Full details: [docs/paper-corpus-selection.md](paper-corpus-selection.md)

### Primary (Paper Main Table) — 5 corpora, ~4,200 lines

| Corpus | Lines | Domain | Role |
|--------|-------|--------|------|
| jsmn | ~500 | Parser | JSON tokenizer — goto, enum, string handling |
| pdjson | ~1,000 | Parser | Streaming JSON — union, fn-ptr in struct, bsearch |
| sds | ~1,200 | Library | Dynamic strings — va_list, realloc, pointer arith |
| kilo | ~1,300 | Application | Text editor — struct arrays, file I/O, 21 unit tests |
| sbase-wc | ~200 | CLI tool | POSIX wc — UTF-8 rune, bsearch callback, struct return |

### Supplemental (Appendix) — 6 corpora, ~950 lines

inih, sbase-cat, sbase-printf, sbase-head, sbase-cut, sbase-uniq.
All pass normal + obfuscated. Detailed in appendix.

## 5. Meaning Preservation

→ Test file: `tests/meaning_preservation.rs`

**Method**: For each test program, compile with and without `--fobfuscate`.
Run both binaries. Assert `exit_code` and `stdout` are identical.

**Categories**: 30 (arithmetic, strings, loops/arrays, struct/pointer,
function pointers, switch, recursion, globals, variadic, bitwise,
logical/ternary, pointer arithmetic, enum, long arithmetic, nested
structs, do-while/break, initializers, linked list, builtin abs,
builtin bits, large struct return direct, large struct return indirect,
float arithmetic, float static, float conversions, float ABI,
float printf, float compound, user-defined variadic,
user-defined variadic loop).

**CI integration**: All 30 tests run on every push (Linux x86_64).

## 6. Evaluation Summary

→ Full details: [docs/eval-results.md](eval-results.md)

| Metric | Value | Source |
|--------|-------|--------|
| Paper evaluation tests | 130 | 3 suites: corpus 24 + meaning-preservation 31 + obfuscation 75 |
| Pass rate | 100% (0 failures, 0 ignored) | [eval-results.md](eval-results.md) §Test Suite |
| Corpora (total) | 11 (5 primary + 6 supplemental) | [paper-corpus-selection.md](paper-corpus-selection.md) |
| C code compiled | ~5,150 lines | Sum of corpus ORIGIN files |
| Level 3 code expansion | 10.1x average (8.1x–11.6x) | [eval-results.md](eval-results.md) §Code Size |
| Symbol reduction | 95%+ (all corpora → `main` only) | [eval-results.md](eval-results.md) §Symbol Exposure |
| String encryption | 100% (0 readable strings) | [eval-results.md](eval-results.md) §String Exposure |
| Meaning preservation | 31/31 categories pass | `cargo test --test meaning_preservation` |
| Obfuscation passes | 16 (4 configurable levels) | [coverage.md](coverage.md) §Obfuscation Passes |

## 7. Known Limitations

No language-feature limitations remain. All previously listed items are resolved:
- ~~`float` precision~~: `float` is now a fully independent IEEE 754 single-precision type (4 bytes, `movss`/`addss`/`comiss`, proper ABI)
- ~~Struct return >16 bytes~~: implemented (hidden sret pointer)
- ~~`__builtin_ctz/clz/popcount/abs`~~: correctly lowered
- ~~Designated struct initializers~~: supported
- ~~Implicit array size~~: supported

### Open Issues

_(None. All previously tracked issues have been resolved.)_

## 8. Reproducibility

### Reproducing code expansion metrics (v0.3.0 baseline)

```bash
git clone https://github.com/uky007/FerrugoCC.git
cd FerrugoCC
git checkout v0.3.0
cargo build --release
./scripts/evaluate.sh
```

### Reproducing correctness + meaning preservation (current main)

```bash
git clone https://github.com/uky007/FerrugoCC.git
cd FerrugoCC
# main branch includes all v0.3.0 + post-v0.3.0 improvements
cargo test                               # all tests
cargo test --test corpus                 # 24 corpus tests (normal + obfuscated)
cargo test --test meaning_preservation   # 30 meaning-preservation tests
cargo test --test obfuscation            # 75 obfuscation unit tests
```

### Prerequisites

- Rust 1.87+ (2024 edition)
- GCC or Clang (for `gcc -E` preprocessing and assembly/linking)
- x86_64 target (Linux native or macOS via Rosetta 2)

### Determinism

Obfuscation is deterministic: same source + same obfuscation level
produces identical output. No random seeds or time-dependent behavior.

## 9. Target Venue

**Computers & Security** (Elsevier), single-column, ~30 pages.
Core contribution: reproducible evaluation + benchmark + artifact.

## 10. Evaluation Data Summary (all collected 2026-03-28)

| Evaluation Axis | Tool | Key Result |
|----------------|------|-----------|
| Correctness | FerrugoCC tests | 220/220 benchmark, 402 tests |
| Code expansion | objdump | 7.6x instructions, 8.2x .text (L3) |
| Anti-disassembly | objdump | 40.4 invalid instructions (L3) |
| Runtime overhead | kilo_unit bench | 1.08x (L3) |
| Compile time | date +%s%N | 6.8x (L3), 445x (L4) |
| Decompiler resistance | Ghidra 12.0.4 | 14.3x functions, 31.2x lines |
| Symbolic execution | angr 9.2.207 | 5.1x SE time, 20/20 found (L3) |
| Symbol sanitization | nm | >95% reduction |
| String encryption | corpus tests | 0% plaintext recovery |
| Pass ablation | objdump | outlining/inlining 17% each |

## 11. Open Risks

- **O-LLVM/Tigress comparison not yet performed**: Qualitative comparison
  in related work; quantitative comparison on same benchmarks would
  strengthen positioning but is not required for initial submission.
- **angr solves all L3 benchmarks**: Honest result — small programs are
  tractable. Paper discusses scaling argument per Banescu et al.
- **No large-program runtime benchmark**: kilo_unit (1,100 lines) is the
  largest tested. Larger workloads may show measurable runtime overhead.

## Document Index

| Document | Content |
|----------|---------|
| [coverage.md](coverage.md) | C language support, GCC extensions, platform |
| [evaluation.md](evaluation.md) | Evaluation methodology and metrics |
| [eval-results.md](eval-results.md) | Machine-generated evaluation data |
| [benchmarks.md](benchmarks.md) | v0.3.0 baseline benchmark data |
| [paper-corpus-selection.md](paper-corpus-selection.md) | Corpus selection rationale |
| [paper-summary.md](paper-summary.md) | This document |
