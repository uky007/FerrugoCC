#!/usr/bin/env bash
# decompile_eval.sh — Decompiler / reverse-engineering evaluation
#
# Evaluates how resistant obfuscated binaries are to static analysis.
# Uses objdump (always available) and optionally Ghidra (headless).
#
# Metrics:
#   1. Function recovery: objdump -t (symbol table) function count
#   2. Disassembly completeness: objdump -d instruction count, invalid instruction count
#   3. String recovery: strings count vs original
#   4. Symbol exposure: readable symbol names remaining
#   5. CFG complexity: basic block count (label count in disassembly)
#
# Usage: bash scripts/eval/decompile_eval.sh <results_dir>
#        Expects binaries in <results_dir>/binaries/{L0,L1,L2,L3,L4}/
#
# Output: <results_dir>/decompile_eval.csv
#         <results_dir>/decompile_summary.md

set -euo pipefail

RESULTS_DIR="${1:?Usage: decompile_eval.sh <results_dir>}"
OUTCSV="$RESULTS_DIR/decompile_eval.csv"
OUTSUMMARY="$RESULTS_DIR/decompile_summary.md"

echo "program,condition,func_symbols,total_instructions,invalid_instructions,disasm_coverage,strings_count,readable_symbols,basic_blocks" > "$OUTCSV"

# Helper: count function symbols in binary
count_func_symbols() {
    local bin="$1"
    # Count T/t (text/code) symbols that look like functions
    nm "$bin" 2>/dev/null | grep -cE ' [Tt] ' || echo 0
}

# Helper: analyze disassembly
analyze_disasm() {
    local bin="$1"
    local disasm
    disasm=$(objdump -d "$bin" 2>/dev/null)

    # Total instructions (lines with hex address + instruction)
    local total
    total=$(echo "$disasm" | grep -cE '^\s+[0-9a-f]+:' || echo 0)

    # Invalid/unknown instructions (objdump marks these)
    local invalid
    invalid=$(echo "$disasm" | grep -cE '\(bad\)|<unknown>' || echo 0)

    # Basic blocks (labels in disassembly output)
    local blocks
    blocks=$(echo "$disasm" | grep -cE '^[0-9a-f]+ <' || echo 0)

    echo "$total $invalid $blocks"
}

# Helper: count readable strings (≥4 chars)
count_strings() {
    local bin="$1"
    strings "$bin" 2>/dev/null | wc -l | tr -d ' '
}

# Helper: count readable (non-mangled) symbol names
count_readable_symbols() {
    local bin="$1"
    # Symbols that are NOT: _fN, _vN, _cN (OPSEC renamed), .L* (local labels),
    # or toolchain symbols (_start, __*, etc.)
    nm "$bin" 2>/dev/null | grep -E ' [TtDdBb] ' | awk '{print $3}' | \
        grep -cvE '^_f[0-9]+$|^_v[0-9]+$|^_c[0-9]+$|^\.L|^__|^_start$|^_init$|^_fini$|^frame_dummy$|^register_tm_clones$|^deregister_tm_clones$|^__do_global_dtors_aux$' \
        || echo 0
}

# Process all conditions
echo "=== Decompiler Evaluation ==="
TOTAL=0

for cond_dir in "$RESULTS_DIR"/binaries/*/; do
    [ -d "$cond_dir" ] || continue
    COND_NAME="$(basename "$cond_dir")"

    for bin in "$cond_dir"/*; do
        [ -f "$bin" ] || continue
        [ -x "$bin" ] || continue
        PROG="$(basename "$bin")"

        # Collect metrics
        FUNC_SYMS=$(count_func_symbols "$bin")
        read -r TOTAL_INSN INVALID_INSN BASIC_BLOCKS <<< "$(analyze_disasm "$bin")"
        STR_COUNT=$(count_strings "$bin")
        READABLE_SYMS=$(count_readable_symbols "$bin")

        # Disassembly coverage (percentage of valid instructions)
        if [ "$TOTAL_INSN" -gt 0 ]; then
            COVERAGE=$(echo "scale=1; ($TOTAL_INSN - $INVALID_INSN) * 100 / $TOTAL_INSN" | bc)
        else
            COVERAGE="0.0"
        fi

        echo "$PROG,$COND_NAME,$FUNC_SYMS,$TOTAL_INSN,$INVALID_INSN,$COVERAGE,$STR_COUNT,$READABLE_SYMS,$BASIC_BLOCKS" >> "$OUTCSV"
        TOTAL=$((TOTAL + 1))
    done
    echo "  $COND_NAME: done"
done

echo "  Total entries: $TOTAL"

# --- Generate summary markdown ---
echo "# Decompiler Evaluation Summary" > "$OUTSUMMARY"
echo "" >> "$OUTSUMMARY"
echo "> Generated: $(date -u +%Y-%m-%dT%H:%M:%SZ)" >> "$OUTSUMMARY"
echo "" >> "$OUTSUMMARY"
echo "## Metrics" >> "$OUTSUMMARY"
echo "" >> "$OUTSUMMARY"
echo "| Metric | Description |" >> "$OUTSUMMARY"
echo "|--------|-------------|" >> "$OUTSUMMARY"
echo "| func_symbols | Function symbols visible in symbol table (nm) |" >> "$OUTSUMMARY"
echo "| total_instructions | Total disassembled instructions (objdump -d) |" >> "$OUTSUMMARY"
echo "| invalid_instructions | Instructions marked as invalid/unknown |" >> "$OUTSUMMARY"
echo "| disasm_coverage | Percentage of valid instructions |" >> "$OUTSUMMARY"
echo "| strings_count | Readable strings in binary (strings) |" >> "$OUTSUMMARY"
echo "| readable_symbols | Non-obfuscated symbol names remaining |" >> "$OUTSUMMARY"
echo "| basic_blocks | Approximate basic block count (disasm labels) |" >> "$OUTSUMMARY"
echo "" >> "$OUTSUMMARY"
echo "## Raw Data" >> "$OUTSUMMARY"
echo "" >> "$OUTSUMMARY"
echo '```' >> "$OUTSUMMARY"
column -t -s, < "$OUTCSV" >> "$OUTSUMMARY"
echo '```' >> "$OUTSUMMARY"
echo "" >> "$OUTSUMMARY"

# --- Per-condition averages ---
echo "## Per-Condition Averages" >> "$OUTSUMMARY"
echo "" >> "$OUTSUMMARY"
echo "| Condition | Avg Functions | Avg Instructions | Avg Invalid | Avg Coverage | Avg Strings | Avg Readable Syms | Avg Blocks |" >> "$OUTSUMMARY"
echo "|-----------|---------------|------------------|-------------|--------------|-------------|-------------------|------------|" >> "$OUTSUMMARY"

# Use awk to compute per-condition averages
tail -n +2 "$OUTCSV" | awk -F, '
{
    cond = $2
    count[cond]++
    func[cond] += $3
    insn[cond] += $4
    inv[cond] += $5
    cov[cond] += $6
    str[cond] += $7
    sym[cond] += $8
    blk[cond] += $9
}
END {
    # Sort conditions
    n = asorti(count, sorted)
    for (i = 1; i <= n; i++) {
        c = sorted[i]
        n_c = count[c]
        printf "| %s | %.1f | %.0f | %.1f | %.1f%% | %.1f | %.1f | %.1f |\n",
            c, func[c]/n_c, insn[c]/n_c, inv[c]/n_c, cov[c]/n_c,
            str[c]/n_c, sym[c]/n_c, blk[c]/n_c
    }
}' >> "$OUTSUMMARY"

echo "" >> "$OUTSUMMARY"
echo "## Interpretation" >> "$OUTSUMMARY"
echo "" >> "$OUTSUMMARY"
echo "- **Function recovery**: Lower = better obfuscation (OPSEC strips symbols)" >> "$OUTSUMMARY"
echo "- **Invalid instructions**: Higher = anti-disassembly effective (confuses linear sweep)" >> "$OUTSUMMARY"
echo "- **Disasm coverage**: Lower = more disassembly confusion" >> "$OUTSUMMARY"
echo "- **String count**: Lower = string encryption effective" >> "$OUTSUMMARY"
echo "- **Readable symbols**: Lower = OPSEC sanitization effective" >> "$OUTSUMMARY"
echo "- **Basic blocks**: Higher = CFF/outlining increases apparent complexity" >> "$OUTSUMMARY"

echo ""
echo "=== Decompiler evaluation complete ==="
echo "  CSV: $OUTCSV"
echo "  Summary: $OUTSUMMARY"
