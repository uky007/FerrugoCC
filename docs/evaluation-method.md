# FerrugoCC 評価方法（Evaluation Methodology）

## 1. 概要

FerrugoCC の難読化機能を以下の 4 軸で定量評価する。

1. **正しさ（Correctness）** — 難読化後のバイナリが期待する exit code を返すか
2. **サイズコスト（Size Overhead）** — 難読化によるバイナリサイズの増加率
3. **実行時間コスト（Performance Overhead）** — 難読化による実行時間の増加率
4. **逆解析耐性（Reverse-Engineering Resistance）** — シンボル数・文字列数・ラベル数等の変化

## 2. 対象環境

- **プラットフォーム**: x86_64 Linux
- **コンパイラ**: FerrugoCC (Rust, release build)
- **リンカ**: gcc (system)
- **計時ツール**: GNU time (`/usr/bin/time -f '%e'`)
- **バイナリ解析**: `nm --defined-only`, `strings`, アセンブリファイル解析

環境情報は `results/YYYYMMDD/meta.json` に自動記録される（OS, kernel, rustc, gcc, commit hash）。

## 3. ベンチマークセット

`benchmark/` 配下の 20 本の C プログラム。
各プログラムは `main()` の `return` 値（exit code）で正しさを検証する。
期待値は `benchmark/expected.txt` に `program_name:exit_code` 形式で記録。

カテゴリ配分:
- 定数返却、算術、条件分岐、ループ、再帰、ポインタ/配列、文字列、構造体
- switch/enum、行列演算、リンクリスト、文字列検索、ハッシュ計算
- 多次元配列、関数ディスパッチ、ネスト構造体

## 4. 評価条件（11 条件）

| 条件 | フラグ | 目的 |
|------|--------|------|
| L0 | (なし) | ベースライン |
| L1 | `--fobfuscate --obf-level=1` | 軽量難読化 |
| L2 | `--fobfuscate --obf-level=2` | 標準難読化 |
| L3 | `--fobfuscate --obf-level=3` | 全パス（VM 除く） |
| L4 | `--fobfuscate --obf-level=4` | 最大（VM 含む） |
| L3-no-cff | `--fobfuscate --obf-level=3 --obf-no-cff` | CFF の寄与 |
| L3-no-str | `--fobfuscate --obf-level=3 --obf-no-strings` | 文字列暗号化の寄与 |
| L3-no-arith | `--fobfuscate --obf-level=3 --obf-no-arith-subst` | 算術置換の寄与 |
| L3-no-inl | `--fobfuscate --obf-level=3 --obf-no-func-inline` | インライン展開の寄与 |
| L3-no-outl | `--fobfuscate --obf-level=3 --obf-no-func-outline` | アウトライン化の寄与 |
| L4-no-vm | `--fobfuscate --obf-level=4 --obf-no-vm-virtualize` | VM 仮想化の寄与 |

## 5. 計測手順

### 5.1 自動実行

```bash
bash scripts/eval/run_all.sh
```

以下が自動で実行される:

1. FerrugoCC の release ビルド
2. 11 条件 × 20 プログラムのアセンブリ生成 + gcc リンク
3. 正しさ検証 → `correctness.csv` + `pass_list.txt`
4. バイナリサイズ計測 → `size.csv`（正しいバイナリのみ）
5. 実行時間計測（各 10 回） → `performance.csv`（正しいバイナリのみ）
6. 逆解析指標収集 → `reverse_metrics.csv`（正しいバイナリのみ）
7. グラフ生成 → `fig_*.png`

### 5.2 正しさフィルタリング

正しさ検証で失敗した (program, condition) ペアは、後続のサイズ・性能・逆解析指標
の計測から**自動的に除外**される。これにより、誤動作バイナリの指標が結果に混入する
ことを防ぐ。

### 5.3 実行時間計測

- 各バイナリを N 回（デフォルト 10）実行
- GNU time (`/usr/bin/time -f '%e'`) で壁時計時間を計測
- 全 N 回の個別値を `performance.csv` に記録
- グラフでは平均値 + 標準偏差のエラーバーを表示

### 5.4 逆解析指標

| 指標 | ソース | 意味 |
|------|--------|------|
| `nm_symbols` | `nm --defined-only` | 定義済みシンボル数 |
| `strings_count` | `strings` | 可読文字列数 |
| `globl_count` | `.s` ファイル | `.globl` ディレクティブ数 |
| `label_count` | `.s` ファイル | ラベル数（基本ブロック近似） |
| `call_count` | `.s` ファイル | `call` 命令数 |

## 6. 出力形式

全 CSV は先頭行にカラム名を含む。

```
results/YYYYMMDD/
  meta.json              — 環境情報
  correctness.csv        — program,condition,expected,actual,pass
  pass_list.txt          — 正しさ検証通過ペア一覧
  size.csv               — program,condition,size_bytes
  performance.csv        — program,condition,run,time_sec
  reverse_metrics.csv    — program,condition,nm_symbols,strings_count,...
  binaries/{cond}/{prog} — コンパイル済みバイナリ
  assembly/{cond}/{prog}.s — アセンブリファイル
  fig_*.png              — 可視化グラフ
```

## 7. 再現性

- 難読化は決定的（内部カウンタベース、RNG 不使用）だが、アセンブリ出力は
  コンパイラの内部状態（ハッシュマップ走査順等）により実行間で変動しうる
- 正しさ（exit code）は同一 commit であれば一致する
- バイナリサイズは微小な変動がありうるため、複数回の計測から代表値を使用する
- 実行時間は測定環境に依存するため、同一マシン・同一負荷条件での比較を前提とする
- 環境差異は `meta.json` で追跡可能

## 8. 前提条件

- x86_64 Linux
- Bash 4+ (`collect_correctness.sh` は Bash 3 でも動作）
- Rust toolchain (rustc, cargo)
- gcc
- GNU time (`/usr/bin/time`)
- `nm`, `strings` (binutils)
- Python 3 + matplotlib（グラフ生成時のみ）
