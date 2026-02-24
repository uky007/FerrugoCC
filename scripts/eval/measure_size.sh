#!/usr/bin/env bash
# measure_size.sh — Collect binary sizes (only for passing binaries)
#
# Usage: bash measure_size.sh <results_dir>

set -euo pipefail

RESULTS_DIR="${1:?Usage: measure_size.sh <results_dir>}"
PASSLIST="$RESULTS_DIR/pass_list.txt"

OUTFILE="$RESULTS_DIR/size.csv"
echo "program,condition,size_bytes" > "$OUTFILE"

is_passing() {
    local prog="$1" cond="$2"
    if [ -f "$PASSLIST" ]; then
        grep -qx "${prog},${cond}" "$PASSLIST"
    else
        return 0
    fi
}

INCLUDED=0
EXCLUDED=0

for cond_dir in "$RESULTS_DIR"/binaries/*/; do
    [ -d "$cond_dir" ] || continue
    COND_NAME="$(basename "$cond_dir")"

    for bin in "$cond_dir"/*; do
        [ -f "$bin" ] || continue
        PROG="$(basename "$bin")"

        if ! is_passing "$PROG" "$COND_NAME"; then
            EXCLUDED=$((EXCLUDED + 1))
            continue
        fi

        SIZE=$(stat --format='%s' "$bin" 2>/dev/null || stat -f '%z' "$bin" 2>/dev/null)
        echo "$PROG,$COND_NAME,$SIZE" >> "$OUTFILE"
        INCLUDED=$((INCLUDED + 1))
    done
done

echo "  Size data collected: $INCLUDED entries ($EXCLUDED excluded for incorrect results)"
