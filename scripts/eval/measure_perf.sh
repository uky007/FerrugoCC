#!/usr/bin/env bash
# measure_perf.sh — Measure execution time for each binary
# Only measures binaries that passed correctness verification.
#
# Each binary is run N times with a 5-second timeout.
# Programs that require stdin or run interactively are skipped.
#
# Usage: bash measure_perf.sh <results_dir> [N]
# Default N = 5

set -euo pipefail

RESULTS_DIR="${1:?Usage: measure_perf.sh <results_dir> [N]}"
N="${2:-5}"
TIMEOUT_SEC=5
PASSLIST="$RESULTS_DIR/pass_list.txt"

OUTFILE="$RESULTS_DIR/performance.csv"
echo "program,condition,runs,avg_ms,min_ms,max_ms" > "$OUTFILE"

is_passing() {
    local prog="$1" cond="$2"
    if [ -f "$PASSLIST" ]; then
        grep -qx "${prog},${cond}" "$PASSLIST"
    else
        return 0
    fi
}

measure_one() {
    local bin="$1"
    local n="$2"
    local times=()
    local run=1
    while [ "$run" -le "$n" ]; do
        # Use bash built-in TIMEFORMAT with timeout
        local elapsed
        elapsed=$( { TIMEFORMAT='%R'; time timeout "$TIMEOUT_SEC" "$bin" </dev/null >/dev/null 2>&1; } 2>&1 ) || true
        if [ -n "$elapsed" ] && [ "$elapsed" != "" ]; then
            # Convert seconds to milliseconds
            local ms
            ms=$(echo "$elapsed * 1000" | bc 2>/dev/null || echo "0")
            times+=("$ms")
        fi
        run=$((run + 1))
    done

    if [ ${#times[@]} -eq 0 ]; then
        echo "0,0,0,0"
        return
    fi

    # Compute avg/min/max
    local sum=0 min=999999999 max=0 count=${#times[@]}
    for t in "${times[@]}"; do
        local ti=${t%.*}  # truncate to integer for comparison
        [ -z "$ti" ] && ti=0
        sum=$((sum + ti))
        [ "$ti" -lt "$min" ] && min=$ti
        [ "$ti" -gt "$max" ] && max=$ti
    done
    local avg=$((sum / count))
    echo "$count,$avg,$min,$max"
}

INCLUDED=0
EXCLUDED=0
SKIPPED=0

for cond_dir in "$RESULTS_DIR"/binaries/*/; do
    [ -d "$cond_dir" ] || continue
    COND_NAME="$(basename "$cond_dir")"

    for bin in "$cond_dir"/*; do
        [ -f "$bin" ] && [ -x "$bin" ] || continue
        PROG="$(basename "$bin")"

        if ! is_passing "$PROG" "$COND_NAME"; then
            EXCLUDED=$((EXCLUDED + 1))
            continue
        fi

        result=$(measure_one "$bin" "$N")
        IFS=',' read -r runs avg_ms min_ms max_ms <<< "$result"

        if [ "$runs" -eq 0 ]; then
            SKIPPED=$((SKIPPED + 1))
            continue
        fi

        echo "$PROG,$COND_NAME,$runs,$avg_ms,$min_ms,$max_ms" >> "$OUTFILE"
        INCLUDED=$((INCLUDED + 1))
    done
    echo "  $COND_NAME: done"
done

echo "  Performance: $INCLUDED measured, $EXCLUDED excluded (wrong results), $SKIPPED skipped (timeout/error)"
