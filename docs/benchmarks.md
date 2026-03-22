# FerrugoCC — Benchmark Results

## Code Size (Assembly Lines)

| Corpus | C Lines | Normal | Level 1 | Level 2 | Level 3 | L3/Normal |
|--------|---------|--------|---------|---------|---------|-----------|
| jsmn | ~500 | 1,506 | 3,361 | 8,418 | 14,694 | 9.8x |
| inih | ~200 | 1,664 | 3,745 | 8,922 | 19,153 | 11.5x |
| sds | ~1,200 | 5,695 | 11,136 | 25,946 | 61,585 | 10.8x |
| pdjson | ~1,000 | 7,468 | 14,540 | 33,475 | 86,899 | 11.6x |
| kilo | ~1,300 | 8,081 | 15,808 | 36,734 | 89,842 | 11.1x |
| sbase-cat | ~100 | 1,568 | 2,672 | 5,552 | 12,784 | 8.2x |
| sbase-wc | ~200 | 2,426 | 4,588 | 10,571 | 22,116 | 9.1x |
| sbase-printf | ~200 | 2,538 | 4,505 | 9,973 | 22,915 | 9.0x |
| sbase-head | ~80 | 1,366 | 2,371 | 5,107 | 12,593 | 9.2x |
| sbase-cut | ~220 | 3,280 | 6,232 | 15,221 | 33,408 | 10.2x |
| sbase-uniq | ~150 | 2,335 | 4,516 | 10,690 | 23,096 | 9.9x |

### Code Expansion Summary
| Level | Avg. Expansion | Passes Enabled |
|-------|---------------|----------------|
| Level 1 | ~2.0x | constant encoding, junk code, opaque predicates, lib obfuscation |
| Level 2 | ~4.5x | + arithmetic substitution, CFF |
| Level 3 | ~10.1x | + string encryption, anti-disasm, indirect calls, register shuffle, stack frame obf, instruction substitution, function inlining, function outlining |

## Test Suite Summary

| Test Suite | Tests | Pass | Fail | Ignored |
|-----------|-------|------|------|---------|
| corpus (normal) | 12 | 12 | 0 | 0 |
| corpus (obfuscated) | 12 | 12 | 0 | 0 |
| obfuscation unit | 75 | 75 | 0 | 0 |
| meaning-preservation | 20 | 20 | 0 | 0 |
| **Total** | **119** | **119** | **0** | **0** |

## Corpus Coverage

| Corpus | Source | License | Lines | Tests | Description |
|--------|--------|---------|-------|-------|-------------|
| jsmn | zserge/jsmn | MIT | ~500 | 1 | JSON tokenizer |
| inih | benhoyt/inih | BSD-3 | ~200 | 1 | INI parser |
| sds | antirez/sds | BSD-2 | ~1,200 | 1 | Dynamic string library |
| pdjson | skeeto/pdjson | Unlicense | ~1,000 | 8 | Streaming JSON parser |
| kilo | antirez/kilo | BSD-2 | ~1,300 | 23 | Terminal text editor |
| sbase-cat | suckless/sbase | MIT | ~100 | 4 | POSIX cat(1) |
| sbase-wc | suckless/sbase | MIT | ~200 | 5 | POSIX wc(1) with UTF-8 |
| sbase-printf | suckless/sbase | MIT | ~200 | 5 | POSIX printf(1) |
| sbase-head | suckless/sbase | MIT | ~80 | 5 | POSIX head(1) |
| sbase-cut | suckless/sbase | MIT | ~220 | 4 | POSIX cut(1) |
| sbase-uniq | suckless/sbase | MIT | ~150 | 5 | POSIX uniq(1) |

**Total**: ~5,150 lines of real-world C code, 62 test groups.

## Symbol Exposure (OPSEC)

| Corpus | Normal .globl | Obf .globl | Reduction |
|--------|---------------|------------|-----------|
| jsmn | 1 | 1 | — |
| inih | 8 | 1 | 87.5% |
| sds | 50 | 1 | 98.0% |
| pdjson | 24 | 1 | 95.8% |
| sbase-cat | 13 | 1 | 92.3% |
| sbase-wc | 23 | 1 | 95.7% |

OPSEC strip reduces all exported symbols to `main` only.

## String Literal Exposure

| Corpus | Normal | Obfuscated | Encrypted |
|--------|--------|------------|-----------|
| jsmn | 1 | 0 | 100% |
| inih | 8 | 0 | 100% |
| sds | 21 | 0 | 100% |
| pdjson | 79 | 0 | 100% |

Level 3 string encryption eliminates all readable string literals.
