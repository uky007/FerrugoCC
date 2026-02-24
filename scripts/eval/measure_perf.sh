#!/usr/bin/env bash
# measure_perf.sh — Measure execution time for each binary (N runs)
# Only measures binaries that passed correctness verification.
#
# Usage: bash measure_perf.sh <results_dir> [N]
# Default N = 10

set -euo pipefail

RESULTS_DIR="${1:?Usage: measure_perf.sh <results_dir> [N]}"
N="${2:-10}"
PASSLIST="$RESULTS_DIR/pass_list.txt"

OUTFILE="$RESULTS_DIR/performance.csv"
echo "program,condition,run,time_sec" > "$OUTFILE"

is_passing() {
    local prog="$1" cond="$2"
    if [ -f "$PASSLIST" ]; then
        grep -qx "${prog},${cond}" "$PASSLIST"
    else
        return 0
    fi
}

# Check for GNU time
if /usr/bin/time --version 2>&1 | grep -q GNU 2>/dev/null; then
    TIME_CMD="/usr/bin/time"
elif command -v gtime &>/dev/null; then
    TIME_CMD="gtime"
else
    echo "WARNING: GNU time not found; falling back to bash TIMEFORMAT"
    TIME_CMD=""
fi

INCLUDED=0
EXCLUDED=0

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

        run=1
        while [ "$run" -le "$N" ]; do
            if [ -n "$TIME_CMD" ]; then
                ELAPSED=$( { $TIME_CMD -f '%e' "$bin" >/dev/null 2>&1; } 2>&1 )
            else
                ELAPSED=$( { TIMEFORMAT='%R'; time "$bin" >/dev/null 2>&1; } 2>&1 )
            fi
            echo "$PROG,$COND_NAME,$run,$ELAPSED" >> "$OUTFILE"
            run=$((run + 1))
        done
        INCLUDED=$((INCLUDED + 1))
    done
done

echo "  Performance data collected: $INCLUDED programs ($EXCLUDED excluded), $N runs each"
