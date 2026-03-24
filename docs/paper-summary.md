# FerrugoCC — Paper Summary

> Hub document linking all evaluation resources.

## 1. Project Snapshot

- **Evaluation baseline**: v0.3.0 (tag `v0.3.0`, commit `110afd4`, 2026-03-22)
- **Current development**: main branch (post-v0.3.0, adds struct return >16B)
- **Architecture**: x86_64 System V ABI (Linux glibc + macOS Rosetta)
- **Language**: Rust implementation (~12,000 lines), compiles a practical C subset
- **Scope**: C source → x86_64 assembly with 16 obfuscation passes
- **Preprocessing**: delegated to `gcc -E` (not self-implemented)
- **Linking**: delegated to `gcc` (assembler + linker)

## 2. Core Claim

FerrugoCC is an experimental C compiler with integrated multi-pass
obfuscation, implemented in Rust. It compiles a practical subset of C
to x86_64 assembly and applies up to 16 obfuscation transformations
(control flow flattening, string encryption, opaque predicates, function
outlining/inlining, VM virtualization, etc.) while preserving program
semantics. We evaluate correctness on 11 real-world open-source C
programs totaling ~5,150 lines, demonstrating 100% meaning preservation
between normal and obfuscated compilation. At the default obfuscation
level, code size increases by 10.1x on average, exported symbols are
reduced by 95%+, and all string literals are encrypted — making reverse
engineering substantially more difficult while maintaining functional
equivalence.

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

| Metric | Value |
|--------|-------|
| Total tests | 121 |
| Pass rate | 100% (0 failures, 0 ignored) |
| Corpora (total) | 11 (5 primary + 6 supplemental) |
| C code compiled | ~5,150 lines |
| Level 3 code expansion | 10.1x average (8.1x–11.6x) |
| Symbol reduction | 95%+ (all corpora → `main` only) |
| String encryption | 100% (0 readable strings in obfuscated binaries) |
| Meaning preservation | 22/22 categories pass |
| Obfuscation passes | 16 (4 configurable levels) |

## 7. Known Limitations

1. **`float` precision**: `float` is treated as `double`. No corpus uses
   `float`-specific precision, so this does not affect evaluation results.
   In the paper, this is stated as a design decision for the current
   prototype ("double-only floating point").

**Not limitations** (previously listed, now resolved):
- ~~Struct return >16 bytes~~: implemented (hidden sret pointer)
- ~~`__builtin_ctz/clz/popcount/abs`~~: correctly lowered
- ~~Designated struct initializers~~: supported
- ~~Implicit array size~~: supported

## 8. Reproducibility

```bash
# Clone and build
git clone https://github.com/uky007/FerrugoCC.git
cd FerrugoCC
git checkout v0.3.0  # evaluation baseline

# Prerequisites
rustc --version       # Rust 1.87+
gcc --version         # GCC or Clang (for preprocessing + linking)

# Run all tests
cargo test

# Run evaluation script
./scripts/evaluate.sh

# Compile with obfuscation
cargo run -- --fobfuscate --obf-level=3 -S source.c

# Obfuscation levels: 1 (light) to 4 (maximum)
# Default: level 3 (all passes except VM virtualization)
```

Obfuscation is deterministic: same source + same level → same output.
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
