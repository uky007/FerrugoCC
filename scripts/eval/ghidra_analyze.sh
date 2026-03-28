#!/usr/bin/env bash
# ghidra_analyze.sh — Ghidra headless decompiler analysis
#
# Requires: GHIDRA_HOME environment variable pointing to Ghidra installation
# Uses: analyzeHeadless for automated decompilation
#
# Metrics per binary:
#   - Decompile success/failure
#   - Number of recovered functions
#   - Decompiled C output line count (proxy for readability)
#   - Recovered function names (non-FUN_* count)
#
# Usage: bash scripts/eval/ghidra_analyze.sh <results_dir>
# Output: <results_dir>/ghidra_eval.csv
#
# If GHIDRA_HOME is not set, this script skips with a warning.

set -euo pipefail

RESULTS_DIR="${1:?Usage: ghidra_analyze.sh <results_dir>}"
OUTCSV="$RESULTS_DIR/ghidra_eval.csv"

if [ -z "${GHIDRA_HOME:-}" ]; then
    echo "  SKIP: GHIDRA_HOME not set (install Ghidra and set the env var)"
    echo "program,condition,decompile_ok,recovered_functions,named_functions,decompiled_lines" > "$OUTCSV"
    exit 0
fi

HEADLESS="$GHIDRA_HOME/support/analyzeHeadless"
if [ ! -x "$HEADLESS" ]; then
    echo "  SKIP: analyzeHeadless not found at $HEADLESS"
    echo "program,condition,decompile_ok,recovered_functions,named_functions,decompiled_lines" > "$OUTCSV"
    exit 0
fi

echo "program,condition,decompile_ok,recovered_functions,named_functions,decompiled_lines" > "$OUTCSV"

TMPDIR=$(mktemp -d)
trap 'rm -rf "$TMPDIR"' EXIT

# Ghidra post-analysis script to export decompiled C and function list
cat > "$TMPDIR/ExportDecompiled.java" << 'JAVAEOF'
import ghidra.app.script.GhidraScript;
import ghidra.app.decompiler.*;
import ghidra.program.model.listing.*;
import java.io.*;

public class ExportDecompiled extends GhidraScript {
    @Override
    public void run() throws Exception {
        DecompInterface decomp = new DecompInterface();
        decomp.openProgram(currentProgram);

        String outDir = getScriptArgs()[0];
        PrintWriter funcWriter = new PrintWriter(new File(outDir, "functions.txt"));
        PrintWriter cWriter = new PrintWriter(new File(outDir, "decompiled.c"));

        FunctionIterator funcs = currentProgram.getListing().getFunctions(true);
        int funcCount = 0;
        int namedCount = 0;
        while (funcs.hasNext()) {
            Function f = funcs.next();
            funcCount++;
            String name = f.getName();
            funcWriter.println(name);
            if (!name.startsWith("FUN_") && !name.startsWith("_")) {
                namedCount++;
            }

            DecompileResults res = decomp.decompileFunction(f, 30, monitor);
            if (res.decompileCompleted()) {
                String c = res.getDecompiledFunction().getC();
                cWriter.println("// === " + name + " ===");
                cWriter.println(c);
            }
        }

        funcWriter.close();
        cWriter.close();

        PrintWriter metaWriter = new PrintWriter(new File(outDir, "meta.txt"));
        metaWriter.println("functions=" + funcCount);
        metaWriter.println("named=" + namedCount);
        metaWriter.close();

        decomp.dispose();
    }
}
JAVAEOF

echo "=== Ghidra Decompiler Evaluation ==="
TOTAL=0

for cond_dir in "$RESULTS_DIR"/binaries/*/; do
    [ -d "$cond_dir" ] || continue
    COND_NAME="$(basename "$cond_dir")"

    for bin in "$cond_dir"/*; do
        [ -f "$bin" ] || continue
        [ -x "$bin" ] || continue
        PROG="$(basename "$bin")"

        WORK="$TMPDIR/work_${PROG}_${COND_NAME}"
        mkdir -p "$WORK"

        # Run Ghidra headless analysis
        DECOMPILE_OK=0
        FUNC_COUNT=0
        NAMED_COUNT=0
        DECOMP_LINES=0

        if "$HEADLESS" "$WORK" "proj" -import "$bin" \
            -postScript "$TMPDIR/ExportDecompiled.java" "$WORK" \
            -deleteProject -noanalysis -readOnly \
            2>/dev/null; then

            DECOMPILE_OK=1

            if [ -f "$WORK/meta.txt" ]; then
                FUNC_COUNT=$(grep 'functions=' "$WORK/meta.txt" | cut -d= -f2)
                NAMED_COUNT=$(grep 'named=' "$WORK/meta.txt" | cut -d= -f2)
            fi
            if [ -f "$WORK/decompiled.c" ]; then
                DECOMP_LINES=$(wc -l < "$WORK/decompiled.c" | tr -d ' ')
            fi
        fi

        echo "$PROG,$COND_NAME,$DECOMPILE_OK,$FUNC_COUNT,$NAMED_COUNT,$DECOMP_LINES" >> "$OUTCSV"
        TOTAL=$((TOTAL + 1))

        # Cleanup per-binary project
        rm -rf "$WORK"
    done
    echo "  $COND_NAME: done"
done

echo "  Total entries: $TOTAL"
echo "=== Ghidra evaluation complete ==="
