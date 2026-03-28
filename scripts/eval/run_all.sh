#!/usr/bin/env bash
# run_all.sh — Main evaluation entry point
# Compiles all benchmarks under all obfuscation conditions,
# then collects correctness, size, performance, and reverse-engineering metrics.
#
# Usage: bash scripts/eval/run_all.sh [results_dir]
# Default results_dir: results/YYYYMMDD

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
ROOT_DIR="$(cd "$SCRIPT_DIR/../.." && pwd)"
BENCHMARK_DIR="$ROOT_DIR/benchmark"
EXPECTED="$BENCHMARK_DIR/expected.txt"
DATE_TAG="$(date +%Y%m%d)"
RESULTS_DIR="${1:-$ROOT_DIR/results/$DATE_TAG}"

# --- Condition definitions ---
# Each line: NAME FLAGS
CONDITIONS=(
    "L0"
    "L1 --fobfuscate --obf-level=1"
    "L2 --fobfuscate --obf-level=2"
    "L3 --fobfuscate --obf-level=3"
    "L4 --fobfuscate --obf-level=4"
    "L3-no-cff --fobfuscate --obf-level=3 --obf-no-cff"
    "L3-no-str --fobfuscate --obf-level=3 --obf-no-strings"
    "L3-no-arith --fobfuscate --obf-level=3 --obf-no-arith-subst"
    "L3-no-inl --fobfuscate --obf-level=3 --obf-no-func-inline"
    "L3-no-outl --fobfuscate --obf-level=3 --obf-no-func-outline"
    "L4-no-vm --fobfuscate --obf-level=4 --obf-no-vm-virtualize"
)

# --- Collect benchmark program names ---
PROGRAMS=()
while IFS=: read -r name _code; do
    PROGRAMS+=("$name")
done < "$EXPECTED"

# --- Step 1: Build compiler (release) ---
echo "=== Building FerrugoCC (release) ==="
cargo build --release --manifest-path "$ROOT_DIR/Cargo.toml"
COMPILER="$ROOT_DIR/target/release/ferrugocc"

# --- Step 2: Create output directory ---
mkdir -p "$RESULTS_DIR"/{binaries,assembly}
echo "=== Results directory: $RESULTS_DIR ==="

# --- Step 3: Generate meta.json ---
cat > "$RESULTS_DIR/meta.json" <<METAEOF
{
    "date": "$DATE_TAG",
    "os": "$(uname -s)",
    "kernel": "$(uname -r)",
    "arch": "$(uname -m)",
    "rustc": "$(rustc --version 2>/dev/null || echo unknown)",
    "gcc": "$(gcc --version 2>/dev/null | head -1 || echo unknown)",
    "commit": "$(git -C "$ROOT_DIR" rev-parse --short HEAD 2>/dev/null || echo unknown)",
    "commit_full": "$(git -C "$ROOT_DIR" rev-parse HEAD 2>/dev/null || echo unknown)"
}
METAEOF
echo "=== meta.json generated ==="

# --- Step 4: Compile all conditions × all programs ---
echo "=== Compiling benchmarks ==="
COMPILE_FAILURES=0

for cond_line in "${CONDITIONS[@]}"; do
    # Split: first word is name, rest are flags
    read -r COND_NAME COND_FLAGS <<< "$cond_line"
    COND_FLAGS="${COND_FLAGS:-}"

    mkdir -p "$RESULTS_DIR/binaries/$COND_NAME"
    mkdir -p "$RESULTS_DIR/assembly/$COND_NAME"

    for prog in "${PROGRAMS[@]}"; do
        SRC="$BENCHMARK_DIR/${prog}.c"
        ASM_OUT="$RESULTS_DIR/assembly/$COND_NAME/${prog}.s"
        BIN_OUT="$RESULTS_DIR/binaries/$COND_NAME/${prog}"

        if [ ! -f "$SRC" ]; then
            echo "  SKIP $prog (source not found)"
            continue
        fi

        # Compile to assembly
        # shellcheck disable=SC2086
        if ! "$COMPILER" -S "$SRC" $COND_FLAGS 2>/dev/null; then
            echo "  FAIL compile $COND_NAME/$prog"
            COMPILE_FAILURES=$((COMPILE_FAILURES + 1))
            continue
        fi

        # Move generated .s to results
        GENERATED_ASM="$BENCHMARK_DIR/${prog}.s"
        if [ -f "$GENERATED_ASM" ]; then
            mv "$GENERATED_ASM" "$ASM_OUT"
        fi

        # Assemble + link with gcc
        if ! gcc -o "$BIN_OUT" "$ASM_OUT" -no-pie 2>/dev/null; then
            echo "  FAIL link $COND_NAME/$prog"
            COMPILE_FAILURES=$((COMPILE_FAILURES + 1))
            continue
        fi
    done
    echo "  $COND_NAME done"
done

if [ "$COMPILE_FAILURES" -gt 0 ]; then
    echo "WARNING: $COMPILE_FAILURES compilation/link failures"
fi

# --- Step 5: Run sub-scripts ---
echo "=== Collecting correctness ==="
bash "$SCRIPT_DIR/collect_correctness.sh" "$RESULTS_DIR" "$EXPECTED"

echo "=== Measuring binary size ==="
bash "$SCRIPT_DIR/measure_size.sh" "$RESULTS_DIR"

echo "=== Measuring compilation time ==="
bash "$SCRIPT_DIR/measure_compile_time.sh" "$RESULTS_DIR" || echo "WARNING: compile time measurement failed (non-fatal)"

echo "=== Measuring runtime performance (optional) ==="
bash "$SCRIPT_DIR/measure_perf.sh" "$RESULTS_DIR" || echo "WARNING: runtime performance measurement failed (non-fatal)"

echo "=== Collecting reverse-engineering metrics ==="
bash "$SCRIPT_DIR/collect_reverse_metrics.sh" "$RESULTS_DIR"

echo "=== Decompiler evaluation (objdump) ==="
bash "$SCRIPT_DIR/decompile_eval.sh" "$RESULTS_DIR"

echo "=== Ghidra decompiler evaluation (optional) ==="
bash "$SCRIPT_DIR/ghidra_analyze.sh" "$RESULTS_DIR"

# --- Step 6: Generate plots ---
echo "=== Generating plots ==="
if command -v python3 &>/dev/null; then
    python3 "$SCRIPT_DIR/plot.py" "$RESULTS_DIR" || echo "WARNING: plot.py failed"
else
    echo "WARNING: python3 not found, skipping plots"
fi

echo ""
echo "=== Evaluation complete ==="
echo "Results: $RESULTS_DIR"
echo "  meta.json"
echo "  correctness.csv"
echo "  size.csv (total, .text, stripped)"
echo "  compile_time.csv"
echo "  performance.csv"
echo "  reverse_metrics.csv"
echo "  decompile_eval.csv"
echo "  decompile_summary.md"
ls "$RESULTS_DIR"/fig_*.png 2>/dev/null && echo "  (plots generated)" || echo "  (no plots)"
