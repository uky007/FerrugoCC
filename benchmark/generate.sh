#!/usr/bin/env bash
# benchmark/generate.sh — 難読化ベンチマーク自動生成
#
# 10 個の C プログラムを Level 0(通常) + Level 1〜4 でコンパイルし、
# 正しい exit code を検証してバイナリサイズを集計する。
#
# 出力: benchmark/output/level_N/{basename}    (バイナリ)
#       benchmark/output/level_N/{basename}.s  (アセンブリ)

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
ROOT_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"
COMPILER="$ROOT_DIR/target/release/ferrugocc"
OUTPUT_DIR="$SCRIPT_DIR/output"
EXPECTED="$SCRIPT_DIR/expected.txt"

# ── プラットフォーム検出 ──
IS_MACOS=false
NEEDS_ARCH=false
if [[ "$(uname)" == "Darwin" ]]; then
    IS_MACOS=true
    if [[ "$(uname -m)" == "arm64" ]]; then
        NEEDS_ARCH=true
    fi
fi

# ── macOS 向けアセンブリ修正 ──
# FerrugoCC は Linux 向け x86_64 アセンブリを出力するため、
# macOS では .globl シンボルの _ プレフィックス付加等が必要。
fixup_asm_macos() {
    local asm_file="$1"
    local tmp_file="${asm_file}.fixup"

    # .globl で宣言されるシンボル名を収集
    local symbols=()
    while IFS= read -r sym; do
        symbols+=("$sym")
    done < <(grep '\.globl ' "$asm_file" | sed 's/.*\.globl[[:space:]]*//' | tr -d ' ')

    # 行ごとに変換
    while IFS= read -r line; do
        trimmed="${line#"${line%%[![:space:]]*}"}"

        # .note.GNU-stack 行を削除
        if [[ "$trimmed" == *".note.GNU-stack"* ]]; then
            continue
        fi

        # .section .rodata → .section __TEXT,__const
        if [[ "$trimmed" == ".section .rodata" ]]; then
            echo "    .section __TEXT,__const"
            continue
        fi

        # .align N → .p2align log2(N) (GNU as vs macOS as の解釈差異を吸収)
        if [[ "$trimmed" =~ ^\.align[[:space:]]+([0-9]+)$ ]]; then
            local align_val="${BASH_REMATCH[1]}"
            local p2=0
            local v=$align_val
            while [[ $v -gt 1 ]]; do
                v=$((v / 2))
                p2=$((p2 + 1))
            done
            echo "    .p2align $p2"
            continue
        fi

        # .globl sym → .globl _sym
        if [[ "$trimmed" == ".globl "* ]]; then
            local sym="${trimmed#.globl }"
            sym="${sym// /}"
            echo "    .globl _${sym}"
            continue
        fi

        # シンボルラベル定義: sym: → _sym:
        if [[ "$trimmed" == *":" && "$trimmed" != *" "* ]]; then
            local label="${trimmed%:}"
            local matched=false
            for sym in "${symbols[@]}"; do
                if [[ "$label" == "$sym" ]]; then
                    echo "_${label}:"
                    matched=true
                    break
                fi
            done
            if $matched; then continue; fi
        fi

        # call sym → call _sym (直接呼び出しのみ、call *%reg は対象外)
        if [[ "$trimmed" == "call "* && "$trimmed" != "call *"* ]]; then
            local target="${trimmed#call }"
            target="${target// /}"
            for sym in "${symbols[@]}"; do
                if [[ "$target" == "$sym" ]]; then
                    echo "    call _${target}"
                    target=""
                    break
                fi
            done
            if [[ -z "$target" ]]; then continue; fi
        fi

        # lea sym(%rip) → lea _sym(%rip) (間接呼び出し変換用)
        local did_lea=false
        for sym in "${symbols[@]}"; do
            if [[ "$trimmed" == *"${sym}(%rip)"* ]]; then
                echo "${line//${sym}(%rip)/_${sym}(%rip)}"
                did_lea=true
                break
            fi
        done
        if $did_lea; then continue; fi

        echo "$line"
    done < "$asm_file" > "$tmp_file"

    mv "$tmp_file" "$asm_file"
}

# ── gcc / 実行ラッパー ──
run_gcc() {
    if $NEEDS_ARCH; then
        arch -x86_64 gcc "$@"
    else
        gcc "$@"
    fi
}

run_binary() {
    if $NEEDS_ARCH; then
        arch -x86_64 "$@"
    else
        "$@"
    fi
}

# ── Step 1: コンパイラビルド ──
echo "=== Building FerrugoCC (release) ==="
cargo build --release --manifest-path "$ROOT_DIR/Cargo.toml"
echo ""

# ── Step 2: 出力ディレクトリ準備 ──
rm -rf "$OUTPUT_DIR"
for level in 0 1 2 3 4; do
    mkdir -p "$OUTPUT_DIR/level_${level}"
done

# ── Step 3 & 4: コンパイル + 検証 ──
pass=0
fail=0
total=0

# サイズ集計用の配列 (レベル別合計)
size_level_0=0
size_level_1=0
size_level_2=0
size_level_3=0
size_level_4=0

echo "=== Compiling & Verifying ==="
echo ""

for src in "$SCRIPT_DIR"/*.c; do
    basename_c="$(basename "$src")"
    name="${basename_c%.c}"

    # 期待する exit code を取得
    expected_code="$(grep "^${name}:" "$EXPECTED" | cut -d: -f2)"
    if [[ -z "$expected_code" ]]; then
        echo "WARNING: no expected exit code for ${name}, skipping"
        continue
    fi

    echo "--- ${name} (expected=${expected_code}) ---"

    for level in 0 1 2 3 4; do
        outdir="$OUTPUT_DIR/level_${level}"
        asm_dst="$outdir/${name}.s"
        bin_dst="$outdir/${name}"

        # コンパイル (-S でアセンブリ出力)
        # .s ファイルはソースと同じディレクトリに生成される
        asm_tmp="${src%.c}.s"

        # コンパイル → アセンブル → リンク → 実行 の各段階で失敗を許容
        set +e
        if [[ "$level" -eq 0 ]]; then
            "$COMPILER" -S "$src" 2>/dev/null
        else
            "$COMPILER" --fobfuscate --obf-level="$level" -S "$src" 2>/dev/null
        fi
        compile_rc=$?
        set -e

        if [[ "$compile_rc" -ne 0 ]] || [[ ! -f "$asm_tmp" ]]; then
            printf "  Level %d: FAIL (compiler error)\n" "$level"
            fail=$((fail + 1))
            total=$((total + 1))
            continue
        fi

        # .s を出力ディレクトリに移動
        mv "$asm_tmp" "$asm_dst"

        # macOS 向け修正
        if $IS_MACOS; then
            fixup_asm_macos "$asm_dst"
        fi

        # gcc でバイナリ化
        set +e
        run_gcc "$asm_dst" -o "$bin_dst" 2>/dev/null
        gcc_rc=$?
        set -e

        if [[ "$gcc_rc" -ne 0 ]] || [[ ! -f "$bin_dst" ]]; then
            printf "  Level %d: FAIL (linker error)\n" "$level"
            fail=$((fail + 1))
            total=$((total + 1))
            continue
        fi

        # 実行して exit code 検証
        set +e
        run_binary "$bin_dst" > /dev/null 2>&1
        actual=$?
        set -e

        total=$((total + 1))

        if [[ "$actual" -eq "$expected_code" ]]; then
            printf "  Level %d: OK  (exit=%d, size=%s)\n" "$level" "$actual" "$(wc -c < "$bin_dst" | tr -d ' ')"
            pass=$((pass + 1))
        else
            printf "  Level %d: FAIL (expected=%d, got=%d)\n" "$level" "$expected_code" "$actual"
            fail=$((fail + 1))
        fi

        # サイズ集計
        bin_size=$(wc -c < "$bin_dst" | tr -d ' ')
        case $level in
            0) size_level_0=$((size_level_0 + bin_size)) ;;
            1) size_level_1=$((size_level_1 + bin_size)) ;;
            2) size_level_2=$((size_level_2 + bin_size)) ;;
            3) size_level_3=$((size_level_3 + bin_size)) ;;
            4) size_level_4=$((size_level_4 + bin_size)) ;;
        esac
    done
    echo ""
done

# ── Step 5: サマリー出力 ──
echo "========================================="
echo "           BENCHMARK SUMMARY"
echo "========================================="
echo ""
printf "Results: %d / %d passed" "$pass" "$total"
if [[ "$fail" -gt 0 ]]; then
    printf " (%d FAILED)" "$fail"
fi
echo ""
echo ""

echo "Binary size by level (total across all programs):"
printf "  Level 0 (normal):  %'10d bytes\n" "$size_level_0"
printf "  Level 1 (light):   %'10d bytes\n" "$size_level_1"
printf "  Level 2 (standard):%'10d bytes\n" "$size_level_2"
printf "  Level 3 (full):    %'10d bytes\n" "$size_level_3"
printf "  Level 4 (maximum): %'10d bytes\n" "$size_level_4"
echo ""

echo "Per-program binary sizes:"
printf "%-20s %10s %10s %10s %10s %10s\n" "Program" "Level0" "Level1" "Level2" "Level3" "Level4"
printf "%-20s %10s %10s %10s %10s %10s\n" "-------" "------" "------" "------" "------" "------"
for src in "$SCRIPT_DIR"/*.c; do
    name="$(basename "$src" .c)"
    expected_code="$(grep "^${name}:" "$EXPECTED" | cut -d: -f2)"
    if [[ -z "$expected_code" ]]; then continue; fi
    s0=$(wc -c < "$OUTPUT_DIR/level_0/${name}" | tr -d ' ')
    s1=$(wc -c < "$OUTPUT_DIR/level_1/${name}" | tr -d ' ')
    s2=$(wc -c < "$OUTPUT_DIR/level_2/${name}" | tr -d ' ')
    s3=$(wc -c < "$OUTPUT_DIR/level_3/${name}" | tr -d ' ')
    s4=$(wc -c < "$OUTPUT_DIR/level_4/${name}" | tr -d ' ')
    printf "%-20s %10d %10d %10d %10d %10d\n" "$name" "$s0" "$s1" "$s2" "$s3" "$s4"
done
echo ""

echo "Assembly files: $OUTPUT_DIR/level_N/<name>.s"
echo "Binaries:       $OUTPUT_DIR/level_N/<name>"
echo ""

if [[ "$fail" -eq 0 ]]; then
    echo "All $total binaries verified successfully!"
else
    echo "WARNING: $fail / $total binaries failed verification!"
    exit 1
fi
