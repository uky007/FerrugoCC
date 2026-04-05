#!/usr/bin/env bash
# measure_size.sh — Collect binary sizes (only for passing binaries)
#
# Metrics per binary:
#   size_bytes      — total binary size
#   text_bytes      — .text section size (code only)
#   stripped_bytes  — size after strip (simulated, no modification)
#
# Usage: bash measure_size.sh <results_dir>

set -euo pipefail

RESULTS_DIR="${1:?Usage: measure_size.sh <results_dir>}"
PASSLIST="$RESULTS_DIR/pass_list.txt"

OUTFILE="$RESULTS_DIR/size.csv"
echo "program,condition,size_bytes,text_bytes,stripped_bytes" > "$OUTFILE"

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

        # Total binary size
        SIZE=$(stat --format='%s' "$bin" 2>/dev/null || stat -f '%z' "$bin" 2>/dev/null || echo 0)

        # .text section size
        TEXT_BYTES=$(objdump -h "$bin" 2>/dev/null | awk '/\.text/ { printf "%d", strtonum("0x" $3) }' || echo 0)
        [ -z "$TEXT_BYTES" ] && TEXT_BYTES=0

        # Stripped size (copy + strip + measure + remove)
        STRIPPED_BYTES=0
        TMPBIN=$(mktemp)
        if cp "$bin" "$TMPBIN" 2>/dev/null; then
            strip "$TMPBIN" 2>/dev/null || true
            STRIPPED_BYTES=$(stat --format='%s' "$TMPBIN" 2>/dev/null || stat -f '%z' "$TMPBIN" 2>/dev/null || echo 0)
            rm -f "$TMPBIN"
        fi

        echo "$PROG,$COND_NAME,$SIZE,$TEXT_BYTES,$STRIPPED_BYTES" >> "$OUTFILE"
        INCLUDED=$((INCLUDED + 1))
    done
done

echo "  Size data collected: $INCLUDED entries ($EXCLUDED excluded for incorrect results)"
