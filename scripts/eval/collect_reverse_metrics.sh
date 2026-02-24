#!/usr/bin/env bash
# collect_reverse_metrics.sh — Collect reverse-engineering difficulty metrics
# Only measures binaries that passed correctness verification.
#
# Metrics per binary:
#   nm_symbols    — number of defined symbols (nm --defined-only)
#   strings_count — number of readable strings (strings)
#   globl_count   — .globl directives in assembly
#   label_count   — labels in assembly (basic block approximation)
#   call_count    — call instructions in assembly
#
# Usage: bash collect_reverse_metrics.sh <results_dir>

set -euo pipefail

RESULTS_DIR="${1:?Usage: collect_reverse_metrics.sh <results_dir>}"
PASSLIST="$RESULTS_DIR/pass_list.txt"

OUTFILE="$RESULTS_DIR/reverse_metrics.csv"
echo "program,condition,nm_symbols,strings_count,globl_count,label_count,call_count" > "$OUTFILE"

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

        # Binary-level metrics
        NM_SYMS=$(nm --defined-only "$bin" 2>/dev/null | wc -l | tr -d ' ')
        STR_COUNT=$(strings "$bin" 2>/dev/null | wc -l | tr -d ' ')

        # Assembly-level metrics
        ASM_FILE="$RESULTS_DIR/assembly/$COND_NAME/${PROG}.s"
        if [ -f "$ASM_FILE" ]; then
            GLOBL_COUNT=$(grep -c '^\s*\.globl' "$ASM_FILE" 2>/dev/null || echo 0)
            LABEL_COUNT=$(grep -cE '^[A-Za-z_.][A-Za-z0-9_.]*:' "$ASM_FILE" 2>/dev/null || echo 0)
            CALL_COUNT=$(grep -cE '^\s*call\s' "$ASM_FILE" 2>/dev/null || echo 0)
        else
            GLOBL_COUNT=0
            LABEL_COUNT=0
            CALL_COUNT=0
        fi

        echo "$PROG,$COND_NAME,$NM_SYMS,$STR_COUNT,$GLOBL_COUNT,$LABEL_COUNT,$CALL_COUNT" >> "$OUTFILE"
        INCLUDED=$((INCLUDED + 1))
    done
done

echo "  Reverse metrics collected: $INCLUDED entries ($EXCLUDED excluded for incorrect results)"
