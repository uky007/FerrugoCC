# FerrugoCC Evaluation Results

> **Version**: main branch (post-v0.3.0, includes struct return >16B, float independent type)
> **Baseline tag**: v0.3.0 (`110afd4`, 2026-03-22) — code expansion metrics measured at v0.3.0
> **Date**: 2026-03-27
> **Platform**: macOS arm64 (Rosetta 2), Linux x86_64 (CI)

## Test Suite

| Suite | Tests | Pass | Fail | Ignored |
|-------|-------|------|------|---------|
| corpus (normal + obfuscated) | 24 | 24 | 0 | 0 |
| meaning-preservation | 31 | 31 | 0 | 0 |
| obfuscation unit | 75 | 75 | 0 | 0 |
| **Total** | **130** | **130** | **0** | **0** |

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
