# FerrugoCC — Coverage & Status

> **Version**: v0.3.0 (2026-03-22)

## C Language Coverage

### Fully Supported Types
| Type | Notes |
|------|-------|
| `int`, `unsigned int` | 32-bit |
| `long`, `unsigned long` | 64-bit (LP64) |
| `char`, `unsigned char` | 8-bit |
| `double` | IEEE 754 double-precision |
| `void` | incomplete type, pointer target |
| `_Bool` | mapped to `int` |
| Pointers | all levels of indirection |
| Arrays | fixed-size, multi-dimensional |
| Structs | nested, self-referential |
| Unions | proper layout (offset 0, max size) |
| Enums | with constant expressions |
| `va_list` | System V AMD64 ABI (24-byte struct) |
| Function pointers | typedef, callbacks, struct members |

### Approximate / Limited Types
| Type | Treatment |
|------|-----------|
| `float` | treated as `double` |
| `long double` | treated as `double` |
| `short` | treated as `int` |
| `__uint128_t` | treated as `long` |

### Not Supported
- Variable-length arrays (VLA)
- `_Generic`, `_Atomic`, `_Thread_local`, `_Alignas`/`_Alignof`
- Flexible array members (`struct { int n; char data[]; }`)
- Compound literals in expression context (`(type){init}`)

### Statements & Expressions
| Feature | Status |
|---------|--------|
| if/else, while, do-while, for | ✓ |
| switch/case/default | ✓ |
| break, continue, goto/labels | ✓ |
| Ternary `? :` | ✓ |
| Comma operator | ✓ |
| sizeof (type and expression) | ✓ |
| Casts (all integer/pointer types) | ✓ |
| Compound assignment (`+=`, `-=`, etc.) | ✓ |
| Prefix/postfix increment/decrement | ✓ |
| Bitwise operators (`& \| ^ ~ << >>`) | ✓ |
| Logical operators (`&& \|\| !`) | ✓ (including short-circuit) |
| Designated array initializers (`[N] = val`) | ✓ |
| Designated struct initializers (`.field = val`) | ✓ |
| Local struct array compound init | ✓ |
| String literal array init (`char s[] = "..."`) | ✓ |

### Preprocessing
- External preprocessor: `gcc -E -P`
- `-D` / `-U` flags passed through
- Hex escape sequences (`\xNN`) in strings

### GCC Extension Tolerance
| Extension | Treatment |
|-----------|-----------|
| `__attribute__((...))` | balanced paren skip |
| `__asm__("...")` | balanced paren skip |
| `__extension__` | no-op |
| `__builtin_va_list` | → `va_list` |
| `__builtin_va_start/end/arg/copy` | correctly lowered |
| `__builtin_bswap{16,32,64}` | lowered to shift/mask/or |
| `__builtin_abs/labs` | lowered to conditional negate |
| `__builtin_popcount/popcountl` | lowered to bit-count loop |
| `__builtin_ctz/ctzl` | lowered to trailing-zero loop |
| `__builtin_clz/clzl` | lowered to leading-zero loop |
| `__builtin_expect` | returns first arg (hint ignored) |
| `_Nonnull`/`_Nullable` | skipped |
| `const`/`volatile`/`restrict` | parsed, semantically ignored |
| `inline`/`_Noreturn` | parsed, ignored |

## Corpus

### Tier 1 (Required — CI critical path)
| Corpus | Lines | Normal | Obfuscated | Description |
|--------|-------|--------|------------|-------------|
| jsmn | ~500 | ✓ | ✓ | JSON parser — goto, enum expressions |
| inih | ~200 | ✓ | ✓ | INI parser — function pointers, callbacks |
| sds | ~1200 | ✓ | ✓ | Dynamic strings — va_list, pointer arithmetic |
| pdjson | ~1000 | ✓ | ✓ | JSON parser — union, struct fn-ptrs, bsearch |

### Tier 2 (Regression — CI path)
| Corpus | Tests | Normal | Obfuscated | Description |
|--------|-------|--------|------------|-------------|
| kilo | 21 groups + 2 smoke | ✓ | ✓ | Text editor — editing, search, scroll |
| sbase-cat | 4 | ✓ | ✓ | pipe I/O, concat, variadic eprintf |
| sbase-wc | 5 | ✓ | ✓ | UTF-8 rune decoding, bsearch, counting |
| sbase-printf | 5 | ✓ | ✓ | Format parsing, strtonum, unescape |
| sbase-head | 5 | ✓ | ✓ | getline, line counting, dup2 |
| sbase-cut | 4 | ✓ | ✓ | Linked list, field cutting, memmem |
| sbase-uniq | 5 | ✓ | ✓ | Line dedup, memcmp, realloc |

### Meaning-Preservation Tests
22 categories verifying `normal.stdout == obfuscated.stdout`:
arithmetic, strings, loops/arrays, struct/pointer, function pointers,
switch/case, recursion, globals, variadic, bitwise, logical/ternary,
pointer arithmetic, enum, long arithmetic, nested structs, do-while/break,
initializers, linked list, builtin abs, builtin bits,
large struct return (direct), large struct return (indirect).

## Obfuscation Passes

| # | Pass | Level 1 | Level 2 | Level 3 | Level 4 |
|---|------|---------|---------|---------|---------|
| 1 | Constant encoding | ✓ | ✓ | ✓ | ✓ |
| 2 | Arithmetic substitution | | ✓ | ✓ | ✓ |
| 3 | Junk code insertion | ✓ | ✓ | ✓ | ✓ |
| 4 | Opaque predicates | ✓ | ✓ | ✓ | ✓ |
| 5 | Control flow flattening (CFF) | | ✓ | ✓ | ✓ |
| 6 | String encryption | | | ✓ | ✓ |
| 7 | Anti-disassembly | | | ✓ | ✓ |
| 8 | Indirect calls | | | ✓ | ✓ |
| 9 | Register shuffle | | | ✓ | ✓ |
| 10 | Stack frame obfuscation | | | ✓ | ✓ |
| 11 | Instruction substitution | | | ✓ | ✓ |
| 12 | Function inlining | | | ✓ | ✓ |
| 13 | Function outlining | | | ✓ | ✓ |
| 14 | VM virtualization | | | | ✓ |
| 15 | Library function obfuscation | ✓ | ✓ | ✓ | ✓ |
| 16 | OPSEC sanitization | | ✓ | ✓ | ✓ |

## Known Limitations

1. **`float` precision**: Treated as `double` — no single-precision IEEE 754. This does not affect any current corpus (none use `float`-specific precision).

## Platform Support

| Platform | Status |
|----------|--------|
| x86_64 Linux (glibc) | ✓ CI tested |
| x86_64 macOS (Apple Silicon via Rosetta) | ✓ development platform |
| ARM64 | Not supported (x86_64 output only) |
