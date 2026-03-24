#!/bin/bash
# FerrugoCC Evaluation Script
# Collects all metrics for paper evaluation.
# Usage: ./scripts/evaluate.sh [--tag v0.3.0]
#
# Output: docs/eval-results.md (overwritten)

set -euo pipefail

TAG="${1#--tag }"
TAG="${TAG:-HEAD}"
OUT="docs/eval-results.md"
FERRUGOCC="target/release/ferrugocc"
CFLAGS="-D_FORTIFY_SOURCE=0 -D_DONT_USE_CTYPE_INLINE_"

echo "=== FerrugoCC Evaluation ==="
echo "Tag: $TAG"
echo "Date: $(date -u +%Y-%m-%dT%H:%M:%SZ)"

# Build release
echo "Building release..."
cargo build --release --quiet

# ── 1. Test Suite ──
echo ""
echo "=== 1. Test Suite ==="

echo "Running corpus tests..."
CORPUS_RESULT=$(cargo test --test corpus --release 2>&1 | grep "^test result:" | head -1)
CORPUS_RESULT="${CORPUS_RESULT:-running...}"
echo "  corpus: $CORPUS_RESULT"

echo "Running meaning-preservation tests..."
MP_RESULT=$(cargo test --test meaning_preservation --release 2>&1 | grep "^test result:" | head -1)
MP_RESULT="${MP_RESULT:-running...}"
echo "  meaning-preservation: $MP_RESULT"

echo "Running obfuscation tests..."
OBF_RESULT=$(cargo test --test obfuscation --release 2>&1 | grep "^test result:" | head -1)
OBF_RESULT="${OBF_RESULT:-running...}"
echo "  obfuscation: $OBF_RESULT"

# ── 2. Code Size Expansion ──
echo ""
echo "=== 2. Code Size Expansion ==="

CORPORA=(
    "corpus/jsmn/test_jsmn.c:jsmn"
    "corpus/inih/test_inih.c:inih"
    "corpus/sds/test_sds.c:sds"
    "corpus/pdjson/test_pdjson.c:pdjson"
    "corpus/kilo/kilo.c:kilo"
    "corpus/sbase-cat/test_sbase_cat.c:sbase-cat"
    "corpus/sbase-wc/test_sbase_wc.c:sbase-wc"
    "corpus/sbase-printf/test_sbase_printf.c:sbase-printf"
    "corpus/sbase-head/test_sbase_head.c:sbase-head"
    "corpus/sbase-cut/test_sbase_cut.c:sbase-cut"
    "corpus/sbase-uniq/test_sbase_uniq.c:sbase-uniq"
)

{
    echo "# FerrugoCC Evaluation Results"
    echo ""
    echo "> **Tag**: $TAG"
    echo "> **Date**: $(date -u +%Y-%m-%dT%H:%M:%SZ)"
    echo "> **Platform**: $(uname -s) $(uname -m)"
    echo ""
    echo "## Test Suite"
    echo ""
    echo "| Suite | Result |"
    echo "|-------|--------|"
    echo "| corpus | $CORPUS_RESULT |"
    echo "| meaning-preservation | $MP_RESULT |"
    echo "| obfuscation | $OBF_RESULT |"
    echo ""
    echo "## Code Size (Assembly Lines)"
    echo ""
    echo "| Corpus | Normal | L1 | L2 | L3 | L3/Normal |"
    echo "|--------|--------|-----|-----|-----|-----------|"
} > "$OUT"

for entry in "${CORPORA[@]}"; do
    src="${entry%%:*}"
    name="${entry##*:}"
    asm="${src%.c}.s"

    # Normal
    $FERRUGOCC -S $CFLAGS "$src" 2>/dev/null || true
    normal=$(wc -l < "$asm" 2>/dev/null || echo "0")
    normal=$(echo "$normal" | tr -d ' ')
    rm -f "$asm"

    # Level 1
    $FERRUGOCC --fobfuscate --obf-level=1 -S $CFLAGS "$src" 2>/dev/null || true
    l1=$(wc -l < "$asm" 2>/dev/null || echo "0")
    l1=$(echo "$l1" | tr -d ' ')
    rm -f "$asm"

    # Level 2
    $FERRUGOCC --fobfuscate --obf-level=2 -S $CFLAGS "$src" 2>/dev/null || true
    l2=$(wc -l < "$asm" 2>/dev/null || echo "0")
    l2=$(echo "$l2" | tr -d ' ')
    rm -f "$asm"

    # Level 3
    $FERRUGOCC --fobfuscate --obf-level=3 -S $CFLAGS "$src" 2>/dev/null || true
    l3=$(wc -l < "$asm" 2>/dev/null || echo "0")
    l3=$(echo "$l3" | tr -d ' ')
    rm -f "$asm"

    if [ "$normal" -gt 0 ] 2>/dev/null; then
        ratio=$(echo "scale=1; $l3 / $normal" | bc 2>/dev/null || echo "?")
        echo "  $name: normal=$normal L1=$l1 L2=$l2 L3=$l3 (${ratio}x)"
        echo "| $name | $normal | $l1 | $l2 | $l3 | ${ratio}x |" >> "$OUT"
    fi
done

# ── 3. Symbol Exposure ──
echo ""
echo "=== 3. Symbol Exposure ==="

{
    echo ""
    echo "## Symbol Exposure"
    echo ""
    echo "| Corpus | Normal .globl | Obf .globl |"
    echo "|--------|---------------|------------|"
} >> "$OUT"

for entry in "${CORPORA[@]}"; do
    src="${entry%%:*}"
    name="${entry##*:}"
    asm="${src%.c}.s"

    $FERRUGOCC -S $CFLAGS "$src" 2>/dev/null || true
    n_sym=$(grep -c "\.globl" "$asm" 2>/dev/null || echo 0)
    rm -f "$asm"

    $FERRUGOCC --fobfuscate -S $CFLAGS "$src" 2>/dev/null || true
    o_sym=$(grep -c "\.globl" "$asm" 2>/dev/null || echo 0)
    rm -f "$asm"

    echo "  $name: normal=$n_sym obf=$o_sym"
    echo "| $name | $n_sym | $o_sym |" >> "$OUT"
done

# ── 4. String Exposure ──
echo ""
echo "=== 4. String Exposure ==="

{
    echo ""
    echo "## String Exposure"
    echo ""
    echo "| Corpus | Normal | Obfuscated |"
    echo "|--------|--------|------------|"
} >> "$OUT"

for entry in "${CORPORA[@]}"; do
    src="${entry%%:*}"
    name="${entry##*:}"
    asm="${src%.c}.s"

    $FERRUGOCC -S $CFLAGS "$src" 2>/dev/null || true
    n_str=$(grep -c -e "\.asciz" -e "\.ascii" "$asm" 2>/dev/null || echo 0)
    n_str=$(echo "$n_str" | tr -d ' \n')
    rm -f "$asm"

    $FERRUGOCC --fobfuscate -S $CFLAGS "$src" 2>/dev/null || true
    o_str=$(grep -c -e "\.asciz" -e "\.ascii" "$asm" 2>/dev/null || echo 0)
    o_str=$(echo "$o_str" | tr -d ' \n')
    rm -f "$asm"

    echo "  $name: normal=$n_str obf=$o_str"
    echo "| $name | $n_str | $o_str |" >> "$OUT"
done

echo ""
echo "=== Done ==="
echo "Results written to $OUT"
