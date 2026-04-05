#!/usr/bin/env bash
# ghidra_bench.sh — Run Ghidra headless decompiler on benchmark binaries
#
# Compiles each benchmark at L0 and L3, builds macOS x86_64 binaries,
# then runs Ghidra headless analysis on each.
#
# Usage: bash scripts/eval/ghidra_bench.sh
# Output: results in /tmp/ghidra_eval/

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
ROOT_DIR="$(cd "$SCRIPT_DIR/../.." && pwd)"
GHIDRA_HOME="$ROOT_DIR/tools/ghidra_12.0.4_PUBLIC"
COMPILER="$ROOT_DIR/target/release/ferrugocc"
BENCHMARK_DIR="$ROOT_DIR/benchmark"
EXPECTED="$BENCHMARK_DIR/expected.txt"
RESULTS_DIR="/tmp/ghidra_eval"
SCRIPT_PATH="$ROOT_DIR/scripts/eval/ghidra_scripts"

mkdir -p "$RESULTS_DIR"

# Build compiler if needed
if [ ! -x "$COMPILER" ]; then
    echo "Building release..."
    cargo build --release --manifest-path "$ROOT_DIR/Cargo.toml"
fi

CONDITIONS=("L0" "L3 --fobfuscate --obf-level=3")

OUTCSV="$RESULTS_DIR/ghidra_results.csv"
echo "program,condition,functions,decompiled,named,lines,gotos,binary_size" > "$OUTCSV"

fixup_macos_asm() {
    local asm="$1"
    # Remove GNU-stack
    sed -i '' '/.note.GNU-stack/d' "$asm"
    # Fix section
    sed -i '' 's/\.section \.rodata/.section __TEXT,__const/' "$asm"
    # Prefix all non-local symbols
    sed -i '' 's/\.globl \(.*\)/.globl _\1/' "$asm"
    sed -i '' '/^[a-zA-Z_][a-zA-Z0-9_]*:$/s/^\(.*\):$/_\1:/' "$asm"
    sed -i '' 's/call \([a-zA-Z_][a-zA-Z0-9_]*\)/call _\1/' "$asm"
    # Prefix symbols in (%rip) - use perl for reliability
    perl -pi -e 's/(?<=[^._a-zA-Z0-9])([a-zA-Z_][a-zA-Z0-9_]*)\(%rip\)/_$1(%rip)/g unless /^\s*\./' "$asm"
}

TOTAL=0
FAILED=0

for cond_line in "${CONDITIONS[@]}"; do
    read -r COND_NAME COND_FLAGS <<< "$cond_line"
    COND_FLAGS="${COND_FLAGS:-}"

    echo "=== $COND_NAME ==="
    mkdir -p "$RESULTS_DIR/$COND_NAME"

    while IFS=: read -r prog expected_code; do
        SRC="$BENCHMARK_DIR/${prog}.c"
        [ -f "$SRC" ] || continue

        ASM="$RESULTS_DIR/$COND_NAME/${prog}.s"
        BIN="$RESULTS_DIR/$COND_NAME/${prog}"

        # Compile to assembly
        # shellcheck disable=SC2086
        if ! FERRUGOCC_ASM_OUTPUT="$ASM" "$COMPILER" -S "$SRC" $COND_FLAGS 2>/dev/null; then
            echo "  SKIP $prog (compile failed)"
            FAILED=$((FAILED + 1))
            continue
        fi

        # macOS fixup
        fixup_macos_asm "$ASM"

        # Assemble + link
        if ! arch -x86_64 gcc "$ASM" -o "$BIN" 2>/dev/null; then
            echo "  SKIP $prog (link failed)"
            FAILED=$((FAILED + 1))
            continue
        fi

        # Verify correctness
        ACTUAL=$(arch -x86_64 "$BIN" 2>/dev/null; echo $?)
        ACTUAL=$(echo "$ACTUAL" | tail -1)
        if [ "$ACTUAL" != "$expected_code" ]; then
            echo "  SKIP $prog (wrong result: got $ACTUAL, expected $expected_code)"
            FAILED=$((FAILED + 1))
            continue
        fi

        # Run Ghidra headless
        GHIDRA_OUT="$RESULTS_DIR/$COND_NAME/${prog}_ghidra"
        GHIDRA_PROJ=$(mktemp -d)

        "$GHIDRA_HOME/support/analyzeHeadless" "$GHIDRA_PROJ" "proj" \
            -import "$BIN" \
            -scriptPath "$SCRIPT_PATH" \
            -postScript ExportDecomp.java "$GHIDRA_OUT" \
            2>/dev/null

        # Parse results
        if [ -f "$GHIDRA_OUT/meta.txt" ]; then
            FUNCS=$(grep 'functions=' "$GHIDRA_OUT/meta.txt" | cut -d= -f2)
            DECOMP=$(grep 'decompiled=' "$GHIDRA_OUT/meta.txt" | cut -d= -f2)
            NAMED=$(grep 'named=' "$GHIDRA_OUT/meta.txt" | cut -d= -f2)
            LINES=$(grep 'lines=' "$GHIDRA_OUT/meta.txt" | cut -d= -f2)
            GOTOS=$(grep 'gotos=' "$GHIDRA_OUT/meta.txt" | cut -d= -f2)
        else
            FUNCS=0; DECOMP=0; NAMED=0; LINES=0; GOTOS=0
        fi

        BIN_SIZE=$(stat -f '%z' "$BIN" 2>/dev/null || echo 0)

        echo "$prog,$COND_NAME,$FUNCS,$DECOMP,$NAMED,$LINES,$GOTOS,$BIN_SIZE" >> "$OUTCSV"
        echo "  $prog: funcs=$FUNCS decomp=$DECOMP lines=$LINES gotos=$GOTOS"
        TOTAL=$((TOTAL + 1))

        # Cleanup Ghidra project
        rm -rf "$GHIDRA_PROJ"
    done < "$EXPECTED"
done

echo ""
echo "=== Summary ==="
echo "Total: $TOTAL evaluated, $FAILED skipped"
echo "Results: $OUTCSV"

# Print comparison table
echo ""
echo "=== L0 vs L3 Comparison ==="
python3 -c "
import csv
from collections import defaultdict
data = defaultdict(dict)
with open('$OUTCSV') as f:
    for row in csv.DictReader(f):
        data[row['program']][row['condition']] = row
print(f'{'Program':20s} {'L0 funcs':>9s} {'L3 funcs':>9s} {'L0 lines':>9s} {'L3 lines':>9s} {'L0 gotos':>9s} {'L3 gotos':>9s}')
print('-' * 80)
for prog in sorted(data):
    l0 = data[prog].get('L0', {})
    l3 = data[prog].get('L3', {})
    print(f'{prog:20s} {l0.get(\"functions\",\"?\"):>9s} {l3.get(\"functions\",\"?\"):>9s} {l0.get(\"lines\",\"?\"):>9s} {l3.get(\"lines\",\"?\"):>9s} {l0.get(\"gotos\",\"?\"):>9s} {l3.get(\"gotos\",\"?\"):>9s}')
" 2>/dev/null || true
