# Paper Corpus Selection

## Selection Criteria

1. **Diversity**: Different C patterns, not just variations of the same thing
2. **Realism**: Real open-source projects, not toy programs
3. **Size range**: From minimal (~100 lines) to moderate (~1300 lines)
4. **Obfuscation coverage**: Both normal and obfuscated must pass

## Primary Corpus (Paper Main Table)

| # | Corpus | Lines | Role | Key C Patterns |
|---|--------|-------|------|---------------|
| 1 | jsmn | ~500 | JSON tokenizer | goto, enum expressions, pointer arithmetic |
| 2 | pdjson | ~1,000 | Streaming JSON parser | union, function pointers in struct, bsearch, forward-declared structs |
| 3 | sds | ~1,200 | Dynamic string library | va_list, realloc, pointer arithmetic, compound assignment |
| 4 | kilo | ~1,300 | Text editor | struct arrays, snprintf, switch/case, function pointers, file I/O |
| 5 | sbase-wc | ~200 | POSIX wc(1) | UTF-8 rune decoding, bsearch with function pointer comparator, struct return by value |

**Rationale**: 5 corpora spanning 3 application domains (parsers, libraries, CLI tools), covering the major C feature set that FerrugoCC supports. Total ~4,200 lines of real-world C.

### Why these 5

- **jsmn**: Minimal self-contained parser. Validates basic correctness (goto, enum, string handling).
- **pdjson**: Complex parser with union layout, function pointers in structs, and self-referential types. Stress-tests type system.
- **sds**: Library with heavy pointer arithmetic, variadic functions, and dynamic memory. Validates codegen correctness for pointer operations.
- **kilo**: Largest corpus, exercises editing operations (insert/delete/search), struct manipulation, and file I/O. Validates real application-scale compilation.
- **sbase-wc**: UTF-8 processing, bsearch with comparator callbacks, struct return by value. Validates ABI compliance and function pointer handling.

## Secondary Corpus (Appendix / Supplemental)

| Corpus | Lines | Role | Reason for Supplemental |
|--------|-------|------|------------------------|
| inih | ~200 | INI parser | Similar pattern coverage to jsmn (parser, callbacks) |
| sbase-cat | ~100 | POSIX cat(1) | I/O only, limited C pattern diversity |
| sbase-printf | ~200 | POSIX printf(1) | Format parsing, but strtonum/unescape are utility-focused |
| sbase-head | ~80 | POSIX head(1) | getline + line counting, subset of wc's coverage |
| sbase-cut | ~220 | POSIX cut(1) | Field cutting with linked list, overlaps with wc/head |
| sbase-uniq | ~150 | POSIX uniq(1) | Line dedup, subset of wc's pattern |

**Total supplemental**: ~950 lines, 6 corpora. All pass normal + obfuscated.

## Meaning-Preservation Mapping

### Primary categories (paper main text)
These directly correspond to patterns exercised by the primary corpus:

| Category | Primary Corpus |
|----------|---------------|
| struct_pointer | pdjson, kilo |
| function_pointer | pdjson, sds |
| recursion | jsmn (token parsing) |
| switch | kilo (syntax highlighting) |
| bitwise | jsmn (enum flags), kilo |
| pointer_arith | sds |
| loops_arrays | all |
| globals | sds, kilo |
| variadic | sds |
| large_struct_return | sbase-wc |

### Supplemental categories (appendix)
| Category | Notes |
|----------|-------|
| arithmetic | Basic — covered implicitly by all corpora |
| strings | Basic — strlen/strcmp in all corpora |
| logical | Short-circuit, ternary — implicit in control flow tests |
| enum | Covered by jsmn/kilo enum usage |
| long_arithmetic | Covered by pointer arithmetic tests |
| nested_struct | Covered by pdjson/kilo struct tests |
| do_while_break | Covered by kilo unit tests |
| initializers | Covered by designated init in corpus tests |
| linked_list | Covered by sbase-cut (supplemental) |
| builtin_abs | Utility — not primary evaluation target |
| builtin_bits | Utility — not primary evaluation target |

## Summary for Paper

**Main evaluation table**: 5 corpora, ~4,200 lines of C
- 100% normal correctness
- 100% obfuscated correctness
- 100% meaning preservation (normal stdout == obfuscated stdout)
- Level 3: 9.1x–11.6x code expansion
- 95%+ symbol reduction
- 100% string encryption

**Supplemental**: 6 additional corpora, ~950 lines
- All pass normal + obfuscated (mentioned in text, detailed in appendix)
