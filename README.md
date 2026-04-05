# FerrugoCC

[![CI](https://github.com/uky007/FerrugoCC/actions/workflows/ci.yml/badge.svg)](https://github.com/uky007/FerrugoCC/actions/workflows/ci.yml)
[![crates.io](https://img.shields.io/crates/v/ferrugocc.svg)](https://crates.io/crates/ferrugocc)
[![docs.rs](https://docs.rs/ferrugocc/badge.svg)](https://docs.rs/ferrugocc)
[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](https://opensource.org/licenses/MIT)

An experimental C compiler and obfuscating compiler written in Rust.

Compiles a practical subset of C to x86_64 assembly (System V ABI), with an optional 16-pass obfuscation pipeline. Developed following [Writing a C Compiler](https://nostarch.com/writing-c-compiler) by Nora Sandler, then extended with real-world corpus support and obfuscation.

> **Status: v0.3.0** — Experimental compiler, not production-ready. 121 paper-evaluation tests pass, including 24 corpus run tests across 11 real-world C projects (normal + obfuscated). See [Supported Scope](#supported-scope) and [Known Limitations](#known-limitations).

## Requirements

- **Rust** 2024 edition (1.85+)
- **gcc** — used for preprocessing (`gcc -E -P`) and assembling/linking
- **Target**: x86_64 only (Linux and macOS)
- macOS on Apple Silicon: requires Rosetta 2 (`arch -x86_64`)

## Install

From crates.io:

```bash
cargo install ferrugocc
```

From the repository checkout:

```bash
cargo install --path .
```

## Supported Scope

**Language features:**
- Types: `int`, `long`, `unsigned`, `double`, `char`, `void`, `_Bool`, pointers, arrays (multi-dimensional), structs (nested, self-referential), unions (proper layout), enums, `va_list`, function pointers (typedef, callbacks, struct members)
- Struct return by value: ≤16 bytes via RAX+RDX, >16 bytes via hidden sret pointer (System V ABI)
- Control flow: `if`/`else`, `while`, `do`/`while`, `for`, `switch`/`case`, `goto`/`label`, ternary `?:`
- Functions: declarations, definitions, variadic (`va_list`/`va_arg`/`va_copy`), function pointers
- Declarations: `typedef`, `enum`, `struct`, `union`, `static`, `extern`, file-scope initializers, designated array initializers (`[N] = val`), designated struct initializers (`.field = val`), implicit array size (`int arr[] = {1,2,3}`)
- Operators: arithmetic, bitwise (`& | ^ << >>`), logical (with short-circuit), comparison, compound assignment
- Builtins: `__builtin_va_*`, `__builtin_bswap{16,32,64}`, `__builtin_abs/labs`, `__builtin_popcount/ctz/clz` (correctly lowered)
- GCC extensions: `__attribute__`, `__asm__`, `__extension__`, `_Nonnull`/`_Nullable` (tolerance-parsed)
- Preprocessor: delegated to `gcc -E` (handles `#include`, `#define`, `#ifdef`, etc.)

**Obfuscation (16 passes):** constant encoding, arithmetic substitution, junk code, opaque predicates, control flow flattening, string encryption, VM virtualization, library function obfuscation, OPSEC sanitization, anti-disassembly, indirect calls, register shuffle, stack frame obfuscation, instruction substitution, function inlining, function outlining

**Tested corpora (11 projects, ~5,150 lines):**
- **Tier 1**: [jsmn](https://github.com/zserge/jsmn) (JSON tokenizer), [inih](https://github.com/benhoyt/inih) (INI parser), [sds](https://github.com/antirez/sds) (dynamic strings), [pdjson](https://github.com/skeeto/pdjson) (streaming JSON parser)
- **Tier 2**: [kilo](https://github.com/antirez/kilo) (text editor, 21 unit test groups), sbase-cat, sbase-wc, sbase-printf, sbase-head, sbase-cut, sbase-uniq (from [sbase](https://git.suckless.org/sbase/))

## Known Limitations

- `float` treated as `double` (no single-precision IEEE 754)
- No self-hosted preprocessor — requires `gcc` on PATH
- No multi-file compilation (single translation unit only)
- No `__attribute__` semantics (syntax is tolerance-skipped)
- No VLA, `_Generic`, `_Atomic`, `_Thread_local`, flexible array members
- See [docs/coverage.md](docs/coverage.md) for complete coverage details

## Corpus Licensing

The `corpus/` directory contains third-party C projects used for regression testing:
- **jsmn**: MIT — [zserge/jsmn](https://github.com/zserge/jsmn)
- **inih**: BSD-3-Clause — [benhoyt/inih](https://github.com/benhoyt/inih)
- **sds**: BSD-2-Clause — [antirez/sds](https://github.com/antirez/sds)
- **pdjson**: Unlicense — [skeeto/pdjson](https://github.com/skeeto/pdjson)
- **kilo**: BSD-2-Clause — [antirez/kilo](https://github.com/antirez/kilo)
- **sbase** (cat, wc, printf, head, cut, uniq): MIT — [suckless/sbase](https://git.suckless.org/sbase/)

Each directory contains an `ORIGIN` file with provenance details.

## Build & Run

```bash
cargo build

# Full compilation (C source -> executable)
cargo run -- source.c

# Stop at each stage
cargo run -- --lex source.c      # Lexing only
cargo run -- --parse source.c    # Through parsing
cargo run -- --validate source.c # Through type checking
cargo run -- --codegen source.c  # Through code generation
cargo run -- -S source.c         # Assembly output (.s file)

# Obfuscation compilation (applies obfuscation passes instead of optimization)
cargo run -- --fobfuscate source.c
cargo run -- --fobfuscate -S source.c  # Obfuscated assembly output

# Obfuscation level (1=light, 2=standard, 3=full, 4=maximum)
cargo run -- --fobfuscate --obf-level=1 source.c  # Constant encoding + junk + predicates only
cargo run -- --fobfuscate --obf-level=4 source.c  # All passes + high frequency

# Per-pass control
cargo run -- --fobfuscate --obf-no-cff source.c              # Disable CFF
cargo run -- --fobfuscate --obf-no-strings source.c           # Disable string encryption
cargo run -- --fobfuscate --obf-no-anti-disasm source.c       # Disable anti-disassembly
cargo run -- --fobfuscate --obf-no-indirect-calls source.c    # Disable indirect calls
cargo run -- --fobfuscate --obf-no-arith-subst source.c      # Disable arithmetic substitution
cargo run -- --fobfuscate --obf-no-reg-shuffle source.c      # Disable register shuffle
cargo run -- --fobfuscate --obf-no-stack-frame source.c     # Disable stack frame obfuscation
cargo run -- --fobfuscate --obf-no-instr-subst source.c    # Disable instruction substitution
cargo run -- --fobfuscate --obf-no-func-inline source.c   # Disable function inlining
cargo run -- --fobfuscate --obf-no-func-outline source.c  # Disable function outlining
cargo run -- --fobfuscate --obf-no-vm-virtualize source.c # Disable VM virtualization
cargo run -- --fobfuscate --obf-no-opsec source.c        # Disable OPSEC sanitization
cargo run -- --fobfuscate --obf-no-opsec-warn source.c   # Disable OPSEC string leak warnings
cargo run -- --fobfuscate --obf-no-strip source.c        # Disable symbol strip (.globl suppression + binary strip)

# OPSEC policy control (only "warn" and "deny" are accepted; invalid values are rejected)
cargo run -- --fobfuscate --opsec-policy=warn source.c   # Warn on violations (default)
cargo run -- --fobfuscate --opsec-policy=deny source.c   # Fail compilation on violations
cargo run -- --fobfuscate --opsec-audit source.c         # Audit final binary with strings/nm

# Frequency parameters
cargo run -- --fobfuscate --obf-junk-freq=2 source.c          # Insert junk every 2 instructions
cargo run -- --fobfuscate --obf-pred-freq=3 source.c           # Apply opaque predicate every 3rd
cargo run -- --fobfuscate --obf-arith-freq=2 source.c          # Apply arithmetic substitution every 2nd
cargo run -- --fobfuscate --obf-reg-shuffle-freq=3 source.c    # Insert register shuffle every 3 instructions
cargo run -- --fobfuscate --obf-stack-padding=8 source.c       # 8 fake stack slots
cargo run -- --fobfuscate --obf-stack-fake-freq=4 source.c     # Insert fake stack ops every 4 instructions
cargo run -- --fobfuscate --obf-instr-subst-freq=2 source.c    # Attempt instruction substitution every 2
cargo run -- --fobfuscate --obf-inline-freq=2 source.c         # Inline every 2nd eligible call
cargo run -- --fobfuscate --obf-outline-min-block=3 source.c   # Minimum outlined block size of 3
```

Requires `gcc` on the system for assembling and linking.

## Tests

```bash
cargo test
```

## Benchmark Suite

A benchmark suite of 20 C programs for quantitative evaluation of obfuscation effectiveness.

### Quick Run (5 levels)

```bash
bash benchmark/generate.sh
```

Generates 100 binaries (20 programs x 5 levels) + 100 assembly files,
with automatic correctness verification via exit codes and size aggregation.

### Full Evaluation (11 conditions + metrics + plots)

```bash
bash scripts/eval/run_all.sh
```

Runs all 20 benchmarks under 11 conditions (L0-L4 + 6 ablation), collecting
correctness, binary size, execution time, and reverse-engineering metrics.
Generates CSV files and visualization plots in `results/YYYYMMDD/`.
See [Evaluation Infrastructure](#evaluation-infrastructure) for details.

### Benchmark Programs

| # | File | Category | Expected exit code |
|---|------|----------|:--:|
| 01 | `constant_return.c` | Constant return | 42 |
| 02 | `arithmetic.c` | Arithmetic + type conversions | 30 |
| 03 | `conditional.c` | if/else chains | 77 |
| 04 | `loop_sum.c` | for loop summation | 55 |
| 05 | `nested_loops.c` | Nested loops (bubble sort) | 101 |
| 06 | `function_calls.c` | Multiple functions + recursion | 120 |
| 07 | `pointers.c` | Pointer arithmetic + arrays | 90 |
| 08 | `strings.c` | String literal operations | 44 |
| 09 | `structs.c` | Structs + pointers | 46 |
| 10 | `mixed_complex.c` | Combined features | 37 |
| 11 | `deep_recursion.c` | Fibonacci + mutual recursion | 89 |
| 12 | `branch_heavy.c` | Multi-stage if-else chains | 63 |
| 13 | `switch_table.c` | switch/case + enum | 55 |
| 14 | `matrix_ops.c` | 3x3 matrix multiply + trace | 30 |
| 15 | `linked_list.c` | Array-based linked list traversal | 15 |
| 16 | `string_search.c` | Hand-written strstr + counting | 3 |
| 17 | `bitwise_ops.c` | Hash, modular exponentiation, digit sum | 42 |
| 18 | `multi_array.c` | Flattened 2D array transpose + sum | 45 |
| 19 | `indirect_calls.c` | Switch-based function dispatch | 60 |
| 20 | `struct_chain.c` | Nested structs + pointer access | 77 |

Output: `benchmark/output/level_N/<name>` (binaries), `benchmark/output/level_N/<name>.s` (assembly)

Intended for quantitative evaluation with deobfuscators (D-810, SATURN, etc.).

## Evaluation Infrastructure

`scripts/eval/` contains a complete evaluation pipeline for research papers.
Designed for **x86_64 Linux**; requires Bash 4+, `gcc`, GNU `time`, `nm`, `strings`, Python 3 + matplotlib.
See [docs/evaluation-method.md](docs/evaluation-method.md) for full methodology.

### Scripts

| Script | Purpose | Output |
|--------|---------|--------|
| `run_all.sh` | Main entry: build, compile all conditions, run sub-scripts, plot | `results/YYYYMMDD/` |
| `collect_correctness.sh` | Run binaries, compare exit codes to expected | `correctness.csv` |
| `measure_size.sh` | Collect binary sizes | `size.csv` |
| `measure_perf.sh` | Wall-clock timing (N runs, default 10) | `performance.csv` |
| `collect_reverse_metrics.sh` | nm symbols, strings, .globl/label/call counts | `reverse_metrics.csv` |
| `plot.py` | Generate 4 figures from CSV data (matplotlib) | `fig_*.png` |

### Evaluation Conditions (11)

| Condition | Flags |
|-----------|-------|
| L0 | (no `--fobfuscate`) |
| L1 | `--fobfuscate --obf-level=1` |
| L2 | `--fobfuscate --obf-level=2` |
| L3 | `--fobfuscate --obf-level=3` |
| L4 | `--fobfuscate --obf-level=4` |
| L3-no-cff | `--fobfuscate --obf-level=3 --obf-no-cff` |
| L3-no-str | `--fobfuscate --obf-level=3 --obf-no-strings` |
| L3-no-arith | `--fobfuscate --obf-level=3 --obf-no-arith-subst` |
| L3-no-inl | `--fobfuscate --obf-level=3 --obf-no-func-inline` |
| L3-no-outl | `--fobfuscate --obf-level=3 --obf-no-func-outline` |
| L4-no-vm | `--fobfuscate --obf-level=4 --obf-no-vm-virtualize` |

### Output Directory Structure

```
results/YYYYMMDD/
  meta.json              # Environment info (OS, kernel, rustc, gcc, commit)
  correctness.csv        # program,condition,expected,actual,pass
  size.csv               # program,condition,size_bytes
  performance.csv        # program,condition,run,time_sec
  reverse_metrics.csv    # program,condition,nm_symbols,strings_count,...
  binaries/{cond}/{prog} # Compiled binaries
  assembly/{cond}/{prog}.s
  fig_size_overhead.png
  fig_perf_overhead.png
  fig_reverse_metrics.png
  fig_ablation.png
```

### Generated Figures

1. **`fig_size_overhead.png`** — Binary size ratio vs L0 (L0-L4, grouped bar)
2. **`fig_perf_overhead.png`** — Execution time ratio vs L0 (L0-L4, bar + error bars)
3. **`fig_reverse_metrics.png`** — Symbols, strings, labels normalized to L0
4. **`fig_ablation.png`** — Ablation: L3 vs L3-no-X (size + reverse metrics)

## Implementation Progress

| Chapter | Feature | Status |
|---------|---------|--------|
| 1 | Constant return (`return 42;`) | Done |
| 2 | Unary operators (`-`, `~`, `!`) | Done |
| 3 | Binary arithmetic (`+`, `-`, `*`, `/`, `%`) | Done |
| 4 | Relational, equality & logical operators (`<`, `<=`, `>`, `>=`, `==`, `!=`, `&&`, `\|\|`) | Done |
| 5 | Local variables & assignment (`int a = 5; a = 10;`) | Done |
| 6 | if/else, ternary, compound statements (`if/else`, `?:`, `{}`) | Done |
| 7 | Compound assignment, increment/decrement, comma operator | Done |
| 8 | Loop statements (`while`, `do-while`, `for`) with `break`/`continue` | Done |
| 9 | Functions (declaration, definition, calls, parameters, variadic `...`) | Done |
| 10 | File-scope variables & storage classes (`static`, `extern`) | Done |
| 11 | Long integers (`long`, type checking pass, implicit conversions) | Done |
| 12 | Unsigned integers (`unsigned int`, `unsigned long`, usual arithmetic conversions) | Done |
| 13 | Floating-point (`double`, SSE instructions, XMM registers) | Done |
| 14 | Pointers (`int *`, `&`, `*`, pointer comparison, null, casts) | Done |
| 15 | Arrays & pointer arithmetic (`int arr[10]`, `arr[i]`, `ptr + n`, `sizeof`) | Done |
| 16 | Characters & strings (`char`, `unsigned char`, char/string literals) | Done |
| 17 | void type & void pointers (`void`, `void *`, `malloc`/`free`) | Done |
| 18 | Structs (`struct`, member access, pointer-to-struct access) | Done |
| 19 | TACKY IR (three-address code intermediate representation, optimization pass infrastructure) | Done |
| 20 | Register allocation (graph coloring, liveness analysis, Chaitin-Briggs) | Done |

## Code Obfuscation (`--fobfuscate`)

After completing all 20 chapters, code obfuscation passes were implemented as an additional feature.
The `--fobfuscate` flag applies obfuscation passes instead of optimization.
Consists of 11 TACKY IR-level passes and 5 ASM-level passes (16 total).

### Obfuscation Levels

`--obf-level=N` controls obfuscation intensity:

| Level | Active Passes | VM | Use Case |
|-------|---------------|:--:|----------|
| 1 | Constant encoding, junk code, opaque predicates | No | Light: basic obfuscation |
| 2 | Level 1 + CFF, arithmetic substitution | No | Standard: adds control flow flattening |
| 3 | Level 2 + inlining, outlining, string encryption, anti-disasm, indirect calls, register shuffle, stack frame obf, instruction substitution, OPSEC (rename + strip) | No | Full: all passes except VM (default) |
| 4 | All 16 passes (+ VM virtualization), high frequency | Yes | Maximum: VM virtualization + all passes at high frequency |

### TACKY IR Level (11 passes)

- **Pass 12 -- Function Inlining**: Embeds callee function bodies at call sites, destroying the call graph.
  Renames variables/labels with `_inline_{N}_{name}`, converts `Return` to `Copy + Jump`.
  Eligibility: body <= 50 instructions, non-recursive, non-main, non-Struct return, no GetAddress on parameters.
  `--obf-inline-freq=N` controls frequency (default: every 3rd eligible call)
- **Pass 1 -- Constant Encoding**: Replaces immediate values with runtime computations
  ```c
  // Before: x = 42;
  // After:  tmp_a = 6; tmp_b = 7; x = tmp_a * tmp_b;  // 6 * 7 = 42
  // Zero:   tmp = 7; x = tmp - tmp;                     // a - a = 0
  ```
- **Pass 2 -- Arithmetic Substitution**: Expands Add/Subtract into mathematically equivalent multi-step computations,
  making expression recovery by decompilers (Hex-Rays, Ghidra) difficult. 4 patterns rotated:
  | # | Target | Transform | Principle |
  |---|--------|-----------|-----------|
  | 0 | Add | `a+b` -> `(a+K)+(b-K)` | Affine transform |
  | 1 | Add | `a+b` -> `3(a+b)-2a-2b` | Coefficient expansion |
  | 2 | Sub | `a-b` -> `(a+K)-(b+K)` | Affine transform |
  | 3 | Sub | `a-b` -> `3a-3b-(2a-2b)` | Coefficient expansion |
- **Pass 3 -- Junk Code Insertion**: Inserts 3 dead computation instructions every N instructions (default 4)
- **Pass 4 -- Opaque Predicates**: Wraps value-producing instructions with always-true conditional branches every Nth time (default 5).
  Rotates 4 mathematical identities to prevent pattern-matching removal:
  | # | Identity | Principle |
  |---|----------|-----------|
  | 0 | `x*(x+1) % 2 == 0` | Product of consecutive integers is even |
  | 1 | `!(x^2 + 1 > 0)` -> 0 | x^2+1 is always positive |
  | 2 | `(x+1)^2 - x^2 - 1 - 2x == 0` | Algebraic identity |
  | 3 | `(x^3 - x) % 3 == 0` | Product of 3 consecutive integers divisible by 3 |
- **Pass 13 -- Function Outlining**: Extracts straight-line code blocks (Copy/Binary/Unary only)
  into new functions `_obf_outlined_{N}`, flooding the binary with decoy functions.
  Validates: inputs <= 6, no Double/Struct/Array I/O, intermediate variables unused outside the block (scans entire function body, including loop back-edges).
  `--obf-outline-min-block=N` sets minimum block size (default 4)
- **Pass 14 -- VM Virtualization (VM-Based Code Virtualization)**: Converts each TACKY instruction of eligible functions
  into individual handlers, with bytecode arrays and handler tables in `.data` section.
  Same category as VMProtect/Themida commercial protectors. Applied before CFF so the VM dispatch loop
  itself gets flattened, achieving double indirection.
  Eligibility: non-main, no Double types, no float conversions, no struct ops, body >= 2 instructions.
  `--obf-no-vm-virtualize` to disable (enabled only at Level 4)
  ```
  // Before: Copy(a, dst); Binary(Add, dst, b, result); Return(result);
  // After:
  //   .data: bytecode[] = {0,1,2,...}  handler_table[] = {&h0, &h1, &h2,...}
  //   dispatch: fetch bytecode[pc] -> load handler_table[idx] -> jmp *handler
  //   handler_0: Copy(a, dst); jmp dispatch
  //   handler_1: Binary(Add, dst, b, result); jmp dispatch
  //   handler_2: Return(result)  // direct return
  ```
- **Pass 15 -- Library Function Obfuscation**: Replaces calls to known library functions (`strlen`, `strcmp`,
  `strcpy`, `memcpy`, `memset`, `memcmp`, `strncmp`, `strncpy`, `strchr`, `strcat`) with equivalent custom
  TACKY IR implementations (`_obf_strlen`, etc.).
  Applied before all other passes so the custom implementations get fully obfuscated by the entire pipeline,
  defeating IDA Pro's FLIRT signature matching.
  `--obf-no-lib-obfuscate` to disable (enabled at all levels)
- **Pass 5 -- Control Flow Flattening (CFF)**: Transforms basic blocks into a jump-table + state-encoded
  dispatch loop, destroying CFG recovery in IDA Pro etc.
  - **Jump table**: Places block label array (`PointerArrayInit`) in `.data`, dispatches via `JumpIndirect` (`jmp *%rax`)
  - **State encoding**: Encodes state variable with affine transform (`encoded = index * A + B`, default A=37, B=0xCAFE).
    Decodes at dispatch (`index = (encoded - 0xCAFE) / 37`) before indexing the jump table
  ```asm
  # Generated dispatch loop
  subl $51966, %eax        # decoded = (state - 0xCAFE) / 37
  cdq
  movl $37, %r10d
  idivl %r10d
  leaq .Lobf_jt_N(%rip), %rbx  # Jump table base address
  imulq $8, %rax
  addq %rbx, %rax
  movq (%rax), %rax        # Load jump target address
  jmp *%rax                # Indirect jump
  ```
- **Pass 6 -- String Encryption**: Encrypts string literals with additive cipher (key=0x5A) and stores as `ByteArrayInit` in `.data`.
  Inserts unrolled decryption code (Load -> Subtract(key) -> Store) at the beginning of main()
- **Pass 16 -- OPSEC Sanitization**: Operational security hardening applied as the final TACKY pass:
  1. **String Leak Detection**: Scans string literals for IP addresses, URLs, file paths, debug keywords, and credential keywords.
     `--opsec-policy=warn` (default) emits `[OPSEC WARNING]` to stderr; `--opsec-policy=deny` emits `[OPSEC ERROR]` and fails compilation
  2. **Symbol Renaming**: Renames all internal functions to `_f{N}`, global variables to `_v{N}`, and static constants to `_c{N}`.
     Preserves `main`, external functions (e.g. `printf`), and `.L` labels
  3. **Symbol Strip**: Suppresses `.globl` directives for all symbols except `main` (internal linkage), and runs `strip` on the final binary to remove the symbol table entirely.
     `--obf-no-strip` to disable, `--obf-no-opsec` to disable all OPSEC features (including `--opsec-policy` and `--opsec-audit`)
  4. **Binary Audit** (`--opsec-audit`): Post-link audit using `strings` and `nm` to scan the final binary for leaked IP addresses, URLs, file paths, debug keywords, and credential keywords. Also flags user-defined symbols visible via `nm` (toolchain-derived symbols like `frame_dummy` are filtered out). Respects `--opsec-policy` for fail/warn behavior. When `--opsec-policy=deny`, the `strings` command must be available or compilation fails (fail-closed). Only `"warn"` and `"deny"` are accepted as policy values; invalid values are rejected at argument parsing

### ASM Level (5 passes, applied after register allocation + fixup)

- **Pass 7 -- Anti-Disassembly**: Inserts `0xE8` (x86 `call rel32` opcode) as `.byte` after unconditional jumps.
  Linear sweep disassemblers interpret this as a 5-byte instruction, corrupting instruction boundary detection
  ```asm
  jmp .Lobf_6
  .byte 0xe8        # <- Disassembler tries to interpret as call rel32
  .Lobf_6:
  ```
- **Pass 8 -- Indirect Calls**: Converts `call func` to `lea func(%rip), %r10; call *%r10`, hindering static call graph recovery
- **Pass 9 -- Register Shuffle**: Inserts dead `movq` instructions every N instructions, creating false dependencies via R10/R11 scratch registers.
  3 patterns rotated: Dead copy, Copy chain, Round-trip
- **Pass 10 -- Stack Frame Obfuscation**: Extends the stack frame with fake slots and inserts fake store/load operations,
  causing decompilers to generate fake local variables and polluting data flow analysis
- **Pass 11 -- Instruction Substitution**: Replaces x86-64 instructions with semantically equivalent but pattern-different sequences.
  4 patterns: Add<->Sub immediate swap, Neg expansion (`not+add $1`), Mov immediate split (`mov (N+K); sub K`)

### Pass Ordering

TACKY IR pass ordering is intentionally designed:
1. **Library Function Obfuscation** first -- custom implementations get all subsequent passes applied
2. **Function Inlining** -- inlined code gets all subsequent passes applied
3. **Constant Encoding** -- constants added by later passes need not be encoded
4. **Arithmetic Substitution** -- further complicates expressions from constant encoding
5. **Junk Code** -- doesn't alter control flow, safe before CFF
6. **Opaque Predicates** -- adds branches that CFF will flatten
7. **Function Outlining** -- extracts already-obfuscated code into decoy functions
8. **VM Virtualization** -- converts functions to bytecode+VM interpreter; before CFF for double indirection
9. **CFF** -- flattens all functions including VM dispatch loops
10. **String Encryption** -- applied late so decryption code isn't destroyed by other passes
11. **OPSEC Sanitization** -- applied last: renames symbols after all passes complete, then strips `.globl`

ASM-level passes are applied after register allocation (order: Stack Frame Obf -> Register Shuffle -> Instruction Substitution -> Anti-Disassembly -> Indirect Calls).

## Architecture

```
Source Code (.c)
    |
    v
+----------+
|  Lexer    |  src/lex/           Tokenize
+----+-----+
     v
+----------+
|  Parser   |  src/parse/         Build AST
+----+-----+
     v
+----------+
| Validate  |  src/typecheck/     Type checking & implicit cast insertion
+----+-----+
     v
+----------+
| TACKY Gen |  src/tacky/         C AST -> TACKY IR (three-address code)
+----+-----+
     v
+----------+
| Optimize  |  src/tacky/         TACKY IR optimization passes (default)
|    or     |  optimize.rs        Algebraic simplification, constant folding, unreachable code elimination,
|          |                      copy propagation, CSE, liveness-based DCE
| Obfuscate |  obfuscate.rs       TACKY obfuscation passes (--fobfuscate)
+----+-----+                      Inlining, constant encoding, arith subst, junk code, opaque predicates,
     v                            outlining, VM virtualization, CFF, string encryption
+----------+
| Codegen   |  src/codegen/       TACKY IR -> Asm(Pseudo)
|          |  generator.rs
+----+-----+
     v
+----------+
| RegAlloc  |  src/codegen/       Liveness analysis -> interference graph -> coalescing -> graph coloring
|          |  regalloc.rs         Pseudo -> Register/Stack(spill)
+----+-----+
     v
+----------+
| Fixup     |  src/codegen/       Fix invalid operand combinations + prologue/epilogue generation
|          |  regalloc.rs
+----+-----+
     v
+----------+
| ASM Obf   |  src/codegen/       ASM-level obfuscation (--fobfuscate)
|          |  mod.rs              Stack frame obf, register shuffle, instruction substitution, anti-disasm, indirect calls
+----+-----+
     v
+----------+
| Emitter   |  src/emit/          Assembly AST -> .s text output
+----+-----+
     v
+----------+
|  Driver   |  src/driver.rs      Invoke gcc: .s -> executable
+----------+
```

Primary target: **x86-64 Linux** (AT&T syntax). macOS (x86_64/Rosetta 2) is best-effort and not guaranteed.

---

## License

MIT License. See [LICENSE](LICENSE).
