# FerrugoCC — Paper Summary

> Hub document linking all evaluation resources.

## 1. Project Snapshot

- **Evaluation version**: main branch post-v0.3.0 (includes all v0.3.0 features + struct return >16B, builtin lowering, implicit array size)
- **Baseline tag**: v0.3.0 (`110afd4`, 2026-03-22) — code expansion metrics measured here; correctness metrics reflect current main
- **Architecture**: x86_64 System V ABI (Linux glibc + macOS Rosetta)
- **Language**: Rust implementation (~12,000 lines), compiles a practical C subset
- **Scope**: C source → x86_64 assembly with 16 obfuscation passes
- **Preprocessing**: delegated to `gcc -E` (not self-implemented)
- **Linking**: delegated to `gcc` (assembler + linker)

## 2. Core Claim

FerrugoCC is an experimental C compiler with integrated multi-pass
obfuscation, implemented in Rust. It compiles a practical subset of C
(not full ISO C — see §3 and §7 for scope and limitations) to x86_64
assembly and applies up to 16 obfuscation transformations while
preserving program semantics. We evaluate correctness on 11 real-world
open-source C programs totaling ~5,150 lines, demonstrating meaning
preservation between normal and obfuscated compilation across 22 test
categories. At the default obfuscation level (Level 3), code size
increases by 10.1x on average, exported symbols are reduced by 95%+,
and all string literals are encrypted. These results suggest that
integrated source-level obfuscation can provide meaningful resistance
to static reverse engineering for the supported C subset, while
maintaining functional equivalence verified by automated testing.

## 3. Supported Coverage

→ Full details: [docs/coverage.md](coverage.md)

**Types**: int, long, char, double, void, pointers, arrays (multi-dim),
structs (nested, self-referential), unions, enums, va_list, function
pointers. Struct return by value for all sizes (≤16B via RAX+RDX, >16B
via hidden sret pointer).

**Approximate types**: `float`→`double`, `short`→`int`, `long double`→`double`.

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

**Categories**: 22 (arithmetic, strings, loops/arrays, struct/pointer,
function pointers, switch, recursion, globals, variadic, bitwise,
logical/ternary, pointer arithmetic, enum, long arithmetic, nested
structs, do-while/break, initializers, linked list, builtin abs,
builtin bits, large struct return direct, large struct return indirect).

**CI integration**: All 22 tests run on every push (Linux x86_64).

## 6. Evaluation Summary

→ Full details: [docs/eval-results.md](eval-results.md)

| Metric | Value | Source |
|--------|-------|--------|
| Paper evaluation tests | 121 | 3 suites: corpus 24 + meaning-preservation 22 + obfuscation 75 |
| Pass rate | 100% (0 failures, 0 ignored) | [eval-results.md](eval-results.md) §Test Suite |
| Corpora (total) | 11 (5 primary + 6 supplemental) | [paper-corpus-selection.md](paper-corpus-selection.md) |
| C code compiled | ~5,150 lines | Sum of corpus ORIGIN files |
| Level 3 code expansion | 10.1x average (8.1x–11.6x) | [eval-results.md](eval-results.md) §Code Size |
| Symbol reduction | 95%+ (all corpora → `main` only) | [eval-results.md](eval-results.md) §Symbol Exposure |
| String encryption | 100% (0 readable strings) | [eval-results.md](eval-results.md) §String Exposure |
| Meaning preservation | 22/22 categories pass | `cargo test --test meaning_preservation` |
| Obfuscation passes | 16 (4 configurable levels) | [coverage.md](coverage.md) §Obfuscation Passes |

## 7. Known Limitations

1. **`float` precision**: `float` is treated as `double` in the current
   implementation. All floating-point arithmetic uses 64-bit IEEE 754
   double-precision; single-precision 32-bit IEEE 754 (`float`) is not
   supported. None of the 11 evaluation corpora use `float`-specific
   precision, so this does not affect evaluation results. In the paper,
   this is stated as a scope decision: "FerrugoCC targets integer-heavy
   systems code; double-only floating point is sufficient for the
   evaluated workloads."

**Not limitations** (previously listed, now resolved):
- ~~Struct return >16 bytes~~: implemented (hidden sret pointer)
- ~~`__builtin_ctz/clz/popcount/abs`~~: correctly lowered
- ~~Designated struct initializers~~: supported
- ~~Implicit array size~~: supported

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
cargo test --test meaning_preservation   # 22 meaning-preservation tests
cargo test --test obfuscation            # 75 obfuscation unit tests
```

### Prerequisites

- Rust 1.87+ (2024 edition)
- GCC or Clang (for `gcc -E` preprocessing and assembly/linking)
- x86_64 target (Linux native or macOS via Rosetta 2)

### Determinism

Obfuscation is deterministic: same source + same level → same output.
No random seeds or time-dependent behavior.
No random seeds or time-dependent behavior in the obfuscation passes.

## 9. Open Risks

- **Decompiler evaluation not yet performed**: Ghidra/IDA analysis of
  obfuscated binaries would strengthen the reverse-engineering resistance
  claim. Currently measured by proxy (code size, symbol count, string
  exposure).
- **Runtime performance not benchmarked**: Obfuscation overhead on
  execution time is not measured. The corpora are too small for
  meaningful runtime benchmarks, but compilation time could be reported.
- **No comparison with OLLVM/Tigress**: Direct comparison with existing
  obfuscation tools would contextualize the results. This requires
  setting up those tools on the same corpora.

## Document Index

| Document | Content |
|----------|---------|
| [coverage.md](coverage.md) | C language support, GCC extensions, platform |
| [evaluation.md](evaluation.md) | Evaluation methodology and metrics |
| [eval-results.md](eval-results.md) | Machine-generated evaluation data |
| [benchmarks.md](benchmarks.md) | v0.3.0 baseline benchmark data |
| [paper-corpus-selection.md](paper-corpus-selection.md) | Corpus selection rationale |
| [paper-summary.md](paper-summary.md) | This document |
