#!/usr/bin/env bash
# bench_runtime.sh — Measure runtime overhead of obfuscation on kilo_unit
#
# Compiles kilo_unit.c at each obfuscation level, then runs each binary
# in a tight loop (N iterations) and measures wall-clock time.
#
# Usage: bash scripts/eval/bench_runtime.sh [N]
# Default N = 1000
#
# Output: printed to stdout as a table

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
ROOT_DIR="$(cd "$SCRIPT_DIR/../.." && pwd)"
COMPILER="$ROOT_DIR/target/release/ferrugocc"
SRC="$ROOT_DIR/corpus/kilo/kilo_unit.c"
TMPDIR=$(mktemp -d)
trap 'rm -rf "$TMPDIR"' EXIT

N="${1:-1000}"
CFLAGS="-D_FORTIFY_SOURCE=0 -D_DONT_USE_CTYPE_INLINE_"

# Build compiler if needed
if [ ! -x "$COMPILER" ]; then
    echo "Building release..."
    cargo build --release --manifest-path "$ROOT_DIR/Cargo.toml"
fi

CONDITIONS=(
    "L0"
    "L1 --fobfuscate --obf-level=1"
    "L2 --fobfuscate --obf-level=2"
    "L3 --fobfuscate --obf-level=3"
    "L4 --fobfuscate --obf-level=4"
)

echo "=== Runtime Benchmark: kilo_unit.c ($N iterations) ==="
echo ""
printf "%-8s %10s %10s %12s\n" "Level" "Total(ms)" "Per-iter" "Overhead"
printf "%-8s %10s %10s %12s\n" "-----" "---------" "--------" "--------"

L0_TOTAL=0

for cond_line in "${CONDITIONS[@]}"; do
    read -r COND_NAME COND_FLAGS <<< "$cond_line"
    COND_FLAGS="${COND_FLAGS:-}"

    ASM="$TMPDIR/${COND_NAME}.s"
    BIN="$TMPDIR/${COND_NAME}"

    # Compile
    # shellcheck disable=SC2086
    if ! FERRUGOCC_ASM_OUTPUT="$ASM" "$COMPILER" -S "$SRC" $COND_FLAGS -D_FORTIFY_SOURCE=0 -D_DONT_USE_CTYPE_INLINE_ 2>/dev/null; then
        echo "  FAIL compile $COND_NAME"
        continue
    fi

    # Assemble + link
    if ! gcc -o "$BIN" "$ASM" -no-pie 2>/dev/null; then
        echo "  FAIL link $COND_NAME"
        continue
    fi

    # Verify correctness first
    RESULT=$("$BIN" 2>/dev/null; echo $?)
    RESULT=$(echo "$RESULT" | tail -1)
    if [ "$RESULT" != "42" ]; then
        echo "  FAIL correctness $COND_NAME (got $RESULT, expected 42)"
        continue
    fi

    # Benchmark: run N times in a tight loop
    START_NS=$(date +%s%N)
    for ((i=0; i<N; i++)); do
        "$BIN" >/dev/null 2>&1 || true
    done
    END_NS=$(date +%s%N)

    ELAPSED_MS=$(( (END_NS - START_NS) / 1000000 ))
    PER_ITER_US=$(( (END_NS - START_NS) / N / 1000 ))

    if [ "$COND_NAME" = "L0" ]; then
        L0_TOTAL=$ELAPSED_MS
        OVERHEAD="1.0x"
    else
        if [ "$L0_TOTAL" -gt 0 ]; then
            # Use bc for floating point
            OVERHEAD=$(echo "scale=1; $ELAPSED_MS / $L0_TOTAL" | bc 2>/dev/null || echo "?")
            OVERHEAD="${OVERHEAD}x"
        else
            OVERHEAD="?"
        fi
    fi

    printf "%-8s %8d ms %6d us %12s\n" "$COND_NAME" "$ELAPSED_MS" "$PER_ITER_US" "$OVERHEAD"
done

echo ""
echo "Binary sizes:"
for cond_line in "${CONDITIONS[@]}"; do
    read -r COND_NAME _ <<< "$cond_line"
    BIN="$TMPDIR/${COND_NAME}"
    if [ -f "$BIN" ]; then
        SIZE=$(stat --format='%s' "$BIN" 2>/dev/null || stat -f '%z' "$BIN" 2>/dev/null || echo "?")
        TEXT=$(objdump -h "$BIN" 2>/dev/null | awk '/\.text/ { printf "%d", strtonum("0x" $3) }' || echo "?")
        printf "  %-8s total=%s  .text=%s\n" "$COND_NAME" "$SIZE" "$TEXT"
    fi
done
