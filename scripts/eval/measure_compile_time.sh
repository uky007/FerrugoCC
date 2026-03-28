#!/usr/bin/env bash
# measure_compile_time.sh — Measure compilation time per condition
#
# Measures how long FerrugoCC takes to compile each benchmark at each
# obfuscation level. This captures the compilation overhead of obfuscation.
#
# Usage: bash measure_compile_time.sh <results_dir>

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
ROOT_DIR="$(cd "$SCRIPT_DIR/../.." && pwd)"
RESULTS_DIR="${1:?Usage: measure_compile_time.sh <results_dir>}"
BENCHMARK_DIR="$ROOT_DIR/benchmark"
EXPECTED="$BENCHMARK_DIR/expected.txt"
COMPILER="$ROOT_DIR/target/release/ferrugocc"

OUTFILE="$RESULTS_DIR/compile_time.csv"
echo "program,condition,compile_ms" > "$OUTFILE"

CONDITIONS=(
    "L0"
    "L1 --fobfuscate --obf-level=1"
    "L2 --fobfuscate --obf-level=2"
    "L3 --fobfuscate --obf-level=3"
    "L4 --fobfuscate --obf-level=4"
)

PROGRAMS=()
while IFS=: read -r name _code; do
    PROGRAMS+=("$name")
done < "$EXPECTED"

TOTAL=0

for cond_line in "${CONDITIONS[@]}"; do
    read -r COND_NAME COND_FLAGS <<< "$cond_line"
    COND_FLAGS="${COND_FLAGS:-}"

    for prog in "${PROGRAMS[@]}"; do
        SRC="$BENCHMARK_DIR/${prog}.c"
        [ -f "$SRC" ] || continue

        # Measure compilation time (compile to assembly only, -S)
        ASM_OUT=$(mktemp --suffix=.s)
        ELAPSED_MS=0

        START_NS=$(date +%s%N 2>/dev/null || echo 0)
        # shellcheck disable=SC2086
        if FERRUGOCC_ASM_OUTPUT="$ASM_OUT" "$COMPILER" -S "$SRC" $COND_FLAGS 2>/dev/null; then
            END_NS=$(date +%s%N 2>/dev/null || echo 0)
            if [ "$START_NS" != "0" ] && [ "$END_NS" != "0" ]; then
                ELAPSED_MS=$(( (END_NS - START_NS) / 1000000 ))
            fi
        fi
        rm -f "$ASM_OUT"

        echo "$prog,$COND_NAME,$ELAPSED_MS" >> "$OUTFILE"
        TOTAL=$((TOTAL + 1))
    done
    echo "  $COND_NAME: done"
done

echo "  Compile time measured: $TOTAL entries"

# Summary
echo "" >> "$OUTFILE"
echo "# Per-condition averages:" >> "$OUTFILE"
for cond_line in "${CONDITIONS[@]}"; do
    read -r COND_NAME _ <<< "$cond_line"
    AVG=$(awk -F, -v c="$COND_NAME" 'NR>1 && $2==c && $3>0 { sum+=$3; n++ } END { if(n>0) printf "%.0f", sum/n; else print "0" }' "$OUTFILE")
    echo "# $COND_NAME: avg ${AVG}ms" >> "$OUTFILE"
done
