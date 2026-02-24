#!/usr/bin/env bash
# collect_correctness.sh — Run each binary and compare exit code to expected
#
# Usage: bash collect_correctness.sh <results_dir> <expected.txt>
# Output: correctness.csv + pass_list.txt (for downstream filtering)

set -euo pipefail

RESULTS_DIR="${1:?Usage: collect_correctness.sh <results_dir> <expected.txt>}"
EXPECTED="${2:?Usage: collect_correctness.sh <results_dir> <expected.txt>}"

OUTFILE="$RESULTS_DIR/correctness.csv"
PASSLIST="$RESULTS_DIR/pass_list.txt"
echo "program,condition,expected,actual,pass" > "$OUTFILE"
: > "$PASSLIST"

PASS=0
FAIL=0
SKIP=0

for cond_dir in "$RESULTS_DIR"/binaries/*/; do
    [ -d "$cond_dir" ] || continue
    COND_NAME="$(basename "$cond_dir")"

    for bin in "$cond_dir"/*; do
        [ -f "$bin" ] && [ -x "$bin" ] || continue
        PROG="$(basename "$bin")"

        # Look up expected code from expected.txt (grep-based, no declare -A)
        EXPECTED_CODE="$(grep "^${PROG}:" "$EXPECTED" 2>/dev/null | head -1 | cut -d: -f2)"

        if [ -z "$EXPECTED_CODE" ]; then
            SKIP=$((SKIP + 1))
            continue
        fi

        set +e
        "$bin" >/dev/null 2>&1
        ACTUAL=$?
        set -e

        if [ "$ACTUAL" -eq "$EXPECTED_CODE" ]; then
            PASSED="true"
            PASS=$((PASS + 1))
            echo "${PROG},${COND_NAME}" >> "$PASSLIST"
        else
            PASSED="false"
            FAIL=$((FAIL + 1))
        fi

        echo "$PROG,$COND_NAME,$EXPECTED_CODE,$ACTUAL,$PASSED" >> "$OUTFILE"
    done
done

echo "  Correctness: $PASS pass, $FAIL fail, $SKIP skip"
