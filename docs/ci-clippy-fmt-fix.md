# CI ビルドエラー: cargo fmt / cargo clippy の未対応

## 概要

GitHub Actions CI が `cargo fmt --all --check` および `cargo clippy --all-targets -- -D warnings`
の 2 ステップで失敗していた。根本原因は **既存コード全体がこれら 2 つの lint に未対応だったこと**。

Pass 16（OPSEC 衛生化）の追加作業中に発見・修正した。

- 発見日: 2025 年
- 影響範囲: ソースコード全体（src/ + tests/）
- CI 定義: `.github/workflows/ci.yml`

---

## CI の構成

```yaml
jobs:
  fmt:     cargo fmt --all --check
  clippy:  cargo clippy --all-targets -- -D warnings
  test:    cargo test
```

3 ジョブのうち `test` のみ通過し、`fmt` と `clippy` が失敗していた。

---

## 問題 1: cargo fmt（フォーマット違反）

### 症状

`cargo fmt --all --check` がプロジェクト全体で大量の差分を検出。

主な違反パターン:

| パターン | 件数 | 例 |
|---------|------|----|
| enum variant の1行記述 | 多数 | `Mov { asm_type: AsmType, src: Operand, dst: Operand },` → 複数行に展開 |
| enum 定数の1行列挙 | 多数 | `E, NE, L, LE, G, GE,` → 各行に分割 |
| 配列定数の1行列挙 | 多数 | `[Reg::AX, Reg::BX, ...]` → 各行に分割 |
| if 文のブレース省略 | 多数 | `if !can_run_x86_64() { return; }` → 複数行に展開 |
| assert マクロの整形 | 多数 | 長い assert_eq! を複数行に展開 |

### 影響ファイル

ソース側: `asm_ast.rs`, `generator.rs`, `mod.rs`, `regalloc.rs`, `driver.rs`,
`emitter.rs`, `lexer.rs`, `lex/mod.rs`, `main.rs`, `obfuscation.rs`, `ast.rs`,
`parser.rs`, `tacky/mod.rs`, `obfuscate.rs`, `optimize.rs`, `tacky_ast.rs`,
`tacky_gen.rs`, `checker.rs`

テスト側: `multi_decl.rs`, `obfuscation.rs`, `switch_enum.rs`, `typedef.rs`, `va_def.rs`

### 修正

```bash
cargo fmt --all
```

全ファイルを rustfmt のデフォルトルールで自動整形した。

---

## 問題 2: cargo clippy（lint 警告 66 件）

### 症状

`cargo clippy --all-targets -- -D warnings` が 66 件のエラー（`-D warnings` により警告がエラー昇格）。

### 警告の内訳

| clippy lint | 件数 | 説明 | 修正方法 |
|-------------|------|------|---------|
| `collapsible_if` | 39 | ネストされた `if` を1つに統合可能 | let-chains 構文 (`if let ... && cond`) に書き換え |
| `collapsible_else_if` | 5 | `else { if ... }` を `else if` に統合可能 | `else if` に書き換え |
| `map_or` simplification | 4 | `.map_or(false, ...)` を `.is_some_and(...)` に簡略化可能 | `.is_some_and()` に書き換え |
| `or_insert_with` | 3 | `.or_insert_with(HashSet::new)` → `.or_default()` | `.or_default()` に書き換え |
| `manual_pattern_char_comparison` | 3 | `\|c\| c == ' ' \|\| c == ','` → `[' ', ',']` | 文字配列に書き換え |
| `redundant_closure` | 3 | `\|e\| func(e)` → `func` | クロージャを関数参照に置換 |
| `is_multiple_of` | 3 | `x % n != 0` → `!x.is_multiple_of(n)` | メソッド呼び出しに書き換え |
| `collapsible_match` | 2 | ネストされた `if let` をパターンに統合可能 | パターン統合 |
| `enum_variant_names` | 2 | 全 variant が同じ接尾辞 (`Init`) を持つ | `#[allow(clippy::enum_variant_names)]` を付与 |
| `unnecessary_cast` | 2 | `i as i32` で `i` が既に `i32` | キャストを除去 |
| `redundant_field_names` | 2 | `src: src` → `src` | フィールド名省略記法に書き換え |
| `too_many_arguments` | 2 | 関数の引数が 7 個超 (10, 11 個) | `#[allow(clippy::too_many_arguments)]` を付与 |
| `type_complexity` | 1 | 型が複雑すぎる | `#[allow(clippy::type_complexity)]` を付与 |
| `approx_constant` | 1 | テスト中の `3.14` が `PI` の近似値と検出 | `3.15` に変更（テスト用の任意値） |
| その他 | 各 1 | 未使用 `enumerate`, 空 `else`, 不要 `return`, ループ変数 | 個別に修正 |

### 主な修正例

#### collapsible_if（let-chains 構文）

```rust
// 修正前
if let Some(label) = trimmed.strip_suffix(':') {
    if symbols.contains(label) {
        result.push(format!("_{label}:"));
        continue;
    }
}

// 修正後
if let Some(label) = trimmed.strip_suffix(':')
    && symbols.contains(label)
{
    result.push(format!("_{label}:"));
    continue;
}
```

#### map_or → is_some_and

```rust
// 修正前
alias_map.get(&from).map_or(false, |set| set.contains(&to))

// 修正後
alias_map.get(&from).is_some_and(|set| set.contains(&to))
```

#### 構造的に変更不可能な lint

以下は `#[allow(...)]` で抑制した:

- `enum_variant_names`: `TackyStaticInit`, `StaticInit`, `CompileError` — variant 名を変えるとプロジェクト全体に影響
- `too_many_arguments`: `generate_binary_instruction` (10 引数), `generate_function_call` (11 引数) — リファクタリングは別課題
- `type_complexity`: `collect_switch_labels` の戻り値型 — 型エイリアス導入は別課題

---

## 教訓

### 1. CI は最初から通しておく

`cargo fmt` と `cargo clippy` は CI に入っていたが、既存コードが対応していなかったため、
**CI が最初から壊れていた**。新機能追加時に初めて CI を実行して気づくと、
自分の変更分と既存問題の切り分けが困難になる。

### 2. cargo fmt は差分フィルタリングできない

`cargo fmt --check` はプロジェクト全体をチェックするため、
「自分の変更したファイルだけチェック」はできない。
結果として、既存のフォーマット違反があると新規コードだけ直しても CI は通らない。

### 3. clippy の let-chains は Rust Edition 2024 以降で安定化

`collapsible_if` の修正に使う let-chains 構文 (`if let ... && cond`) は
Rust 1.87.0 / Edition 2024 で安定化された。
古い Rust ツールチェーンでは `#[allow(clippy::collapsible_if)]` で抑制する必要がある。

### 4. 既存コードの clippy 警告は一括修正が効率的

66 件の警告を個別に手作業で直すのは非効率。
`collapsible_if` (39 件) のように同種の警告が大量にある場合は、
パターンを理解した上で一括修正するのが効率的。
