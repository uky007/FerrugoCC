#!/bin/bash
# demo.sh -- Generate obfuscated binaries at all levels (offline, no network required)
#
# Usage: bash scripts/demo.sh
#
# Produces: demo_level0 through demo_level4 in demo_output/
# Each binary is functionally identical: ./demo_levelN s3cr3t! -> "Access granted"
#
# Supports: x86_64 Linux, x86_64 macOS, Apple Silicon macOS (via Rosetta 2)

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
ROOT_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"
DEMO_SRC="$ROOT_DIR/demo.c"
OUT_DIR="$ROOT_DIR/demo_output"

IS_MACOS=false
NEED_ROSETTA=false
if [ "$(uname -s)" = "Darwin" ]; then
    IS_MACOS=true
    if [ "$(uname -m)" != "x86_64" ]; then
        NEED_ROSETTA=true
    fi
fi

# Wrapper: run gcc (and binaries) through arch -x86_64 on Apple Silicon
run_gcc() {
    if [ "$NEED_ROSETTA" = true ]; then
        arch -x86_64 gcc "$@"
    else
        gcc "$@"
    fi
}

run_bin() {
    if [ "$NEED_ROSETTA" = true ]; then
        arch -x86_64 "$@"
    else
        "$@"
    fi
}

if [ ! -f "$DEMO_SRC" ]; then
    echo "Error: demo.c not found at $DEMO_SRC"
    exit 1
fi

echo "Building FerrugoCC..."
cargo build --manifest-path "$ROOT_DIR/Cargo.toml" --release 2>&1 | tail -1
COMPILER="$ROOT_DIR/target/release/ferrugocc"

if [ ! -x "$COMPILER" ]; then
    echo "Error: compiler binary not found at $COMPILER"
    exit 1
fi

mkdir -p "$OUT_DIR"

# On macOS, FerrugoCC emits Linux-style assembly that needs fixup.
# This mirrors the fixup_asm_for_macos() logic used in the test suite.
fixup_macos_asm() {
    local asmfile="$1"
    if [ "$IS_MACOS" = false ]; then
        return
    fi

    # Collect .globl symbols
    local symbols
    symbols=$(grep '\.globl ' "$asmfile" | sed 's/.*\.globl //' | tr -d ' ' || true)

    # Also collect all defined labels (non-local, i.e. not starting with .L)
    local defined_labels
    defined_labels=$(grep -E '^[a-zA-Z_][a-zA-Z0-9_]*:' "$asmfile" | sed 's/:.*//' || true)

    # Merge: all symbols that need _ prefix = globl + defined labels
    local all_internal
    all_internal=$(printf "%s\n%s" "$symbols" "$defined_labels" | sort -u | grep -v '^$' || true)

    local tmpfile="${asmfile}.tmp"
    > "$tmpfile"

    while IFS= read -r line; do
        trimmed=$(echo "$line" | sed 's/^[[:space:]]*//')

        # Remove .note.GNU-stack
        if echo "$trimmed" | grep -q '\.note\.GNU-stack'; then
            continue
        fi

        # .section .rodata -> .section __TEXT,__const
        if [ "$trimmed" = ".section .rodata" ]; then
            echo "    .section __TEXT,__const" >> "$tmpfile"
            continue
        fi

        # .globl sym -> .globl _sym
        if echo "$trimmed" | grep -q '^\.globl '; then
            sym=$(echo "$trimmed" | sed 's/\.globl //')
            echo "    .globl _${sym}" >> "$tmpfile"
            continue
        fi

        # sym: label -> _sym: (for globl symbols and defined labels)
        if echo "$trimmed" | grep -q ':$'; then
            label=$(echo "$trimmed" | sed 's/:$//')
            if echo "$all_internal" | grep -qx "$label"; then
                echo "_${label}:" >> "$tmpfile"
                continue
            fi
        fi

        # call sym -> call _sym (not .labels, not *reg)
        if echo "$trimmed" | grep -q '^call '; then
            target=$(echo "$trimmed" | sed 's/^call //')
            case "$target" in
                .* | \**)
                    echo "$line" >> "$tmpfile"
                    ;;
                *)
                    echo "    call _${target}" >> "$tmpfile"
                    ;;
            esac
            continue
        fi

        # leaq sym(%rip), %r10 for indirect calls:
        # - Locally defined symbols: leaq _sym(%rip), %r10
        # - External symbols (printf etc.): movq _sym@GOTPCREL(%rip), %r10
        if echo "$trimmed" | grep -qE '^leaq [a-zA-Z_].*\(%rip\)'; then
            # Extract the symbol name before (%rip)
            sym_name=$(echo "$trimmed" | sed -E 's/^leaq ([a-zA-Z_][a-zA-Z0-9_]*)\(%rip\).*/\1/')
            rest=$(echo "$trimmed" | sed -E "s/^leaq ${sym_name}\\(%rip\\)//" )
            if echo "$all_internal" | grep -qx "$sym_name"; then
                # Locally defined symbol
                echo "    leaq _${sym_name}(%rip)${rest}" >> "$tmpfile"
            else
                # External symbol: use GOT
                echo "    movq _${sym_name}@GOTPCREL(%rip)${rest}" >> "$tmpfile"
            fi
            continue
        fi

        # Other sym(%rip) references: add _ prefix to all internal symbols
        if echo "$trimmed" | grep -q '(%rip)'; then
            fixed="$line"
            for sym in $all_internal; do
                fixed=$(echo "$fixed" | sed "s/${sym}(%rip)/_${sym}(%rip)/g")
            done
            echo "$fixed" >> "$tmpfile"
            continue
        fi

        echo "$line" >> "$tmpfile"
    done < "$asmfile"

    mv "$tmpfile" "$asmfile"
}

echo ""
echo "=== FerrugoCC Obfuscation Demo ==="
echo ""

# FerrugoCC outputs .s next to the input file (no -o flag),
# so we copy demo.c into the output directory for each level.

compile_level() {
    local level=$1
    local src="$OUT_DIR/demo_level${level}.c"
    local asm="$OUT_DIR/demo_level${level}.s"
    local bin="$OUT_DIR/demo_level${level}"

    cp "$DEMO_SRC" "$src"

    if [ "$level" -eq 0 ]; then
        "$COMPILER" -S "$src"
    else
        "$COMPILER" --fobfuscate --obf-level="$level" -S "$src"
    fi

    fixup_macos_asm "$asm"
    run_gcc "$asm" -o "$bin"
    local size
    size=$(wc -c < "$bin" | tr -d ' ')
    echo "  Size: $size bytes"
}

for level in 0 1 2 3 4; do
    echo "[Level $level] Compiling..."
    compile_level "$level"
done

echo ""
echo "=== Correctness Verification ==="
echo ""

PASS=0
FAIL=0
for level in 0 1 2 3 4; do
    BIN="$OUT_DIR/demo_level${level}"

    # Test correct password
    OUTPUT=$(run_bin "$BIN" "s3cr3t!" 2>&1 || true)
    if echo "$OUTPUT" | grep -q "Access granted"; then
        echo "[Level $level] Correct password: PASS"
        PASS=$((PASS + 1))
    else
        echo "[Level $level] Correct password: FAIL (got: $OUTPUT)"
        FAIL=$((FAIL + 1))
    fi

    # Test wrong password
    OUTPUT=$(run_bin "$BIN" "wrong" 2>&1 || true)
    if echo "$OUTPUT" | grep -q "Access denied"; then
        echo "[Level $level] Wrong password:   PASS"
        PASS=$((PASS + 1))
    else
        echo "[Level $level] Wrong password:   FAIL (got: $OUTPUT)"
        FAIL=$((FAIL + 1))
    fi
done

echo ""
echo "=== Summary ==="
echo ""

printf "%-10s %10s %10s %10s\n" "Level" "Size" "ASM Lines" "Ratio"
printf "%-10s %10s %10s %10s\n" "-----" "----" "---------" "-----"
for level in 0 1 2 3 4; do
    SIZE=$(wc -c < "$OUT_DIR/demo_level${level}" | tr -d ' ')
    ASM_LINES=$(wc -l < "$OUT_DIR/demo_level${level}.s" | tr -d ' ')
    if [ "$level" -eq 0 ]; then
        BASE_SIZE=$SIZE
    fi
    RATIO=$(echo "scale=2; $SIZE / $BASE_SIZE" | bc)
    printf "%-10s %10s %10s %10sx\n" "Level $level" "$SIZE" "$ASM_LINES" "$RATIO"
done

echo ""
echo "Results: $PASS passed, $FAIL failed"
echo "Output directory: $OUT_DIR"
echo ""
echo "For defensive research & education. Reproducible via scripts/demo.sh"
