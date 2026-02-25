# FerrugoCC: Obfuscation / OPSEC Survey (2026-02-25)

このドキュメントは、FerrugoCC の次期実装候補を「コード難読化」と「開発者OPSEC事故防止」の観点で整理したもの。
クロ向けに、すぐ実装着手できる粒度で優先順位と具体タスクを記載する。

## 1. 現状の要点（FerrugoCC）

- 難読化は TACKY 11 + ASM 5 の計16パスを実装済み。
- OPSEC 系は `opsec_warn`（警告）と `strip`（可能なら実行）がある。
- CLI は `--fobfuscate`、`--obf-level`、各種 `--obf-no-*` / `--obf-*-freq` を提供。

関連実装:
- `src/main.rs`（CLIフラグ）
- `src/obfuscation.rs`（レベル別プリセット）
- `src/driver.rs`（最終 `strip` 実行）

## 2. サーベイで見えた重要点

### 2.1 難読化パスの「効くが、単独では破られやすい」傾向

- O-MVLL の CFF, Indirect Call, Indirect Branch はいずれも有効だが、公開攻撃やオーバーヘッド課題が明示されている。
- CFF は高耐性だが `Public Attacks: Yes`。
- Indirect Branch は CFG 隠蔽に効くが `Overhead: High`。
- Control-Flow Breaking は `Resilience: High` / `Overhead: Low` で、追加候補として費用対効果が高い。

### 2.2 OPSEC は「warn-only」より fail-closed が有効

- GitHub Push Protection は secret push をブロックできる。
- Gitleaks は pre-commit / CI 両方に組み込み可能。
- 事故防止は「警告」より「CI失敗」が効果的。

### 2.3 研究・公開向けには provenance と再現性が強い武器

- `SOURCE_DATE_EPOCH`、`-ffile-prefix-map`、`--build-id=none` は再現性向上に有効。
- SLSA でも provenance 配布が要求される。

## 3. 推奨ロードマップ（実装優先度）

## P0（最優先）: OPSEC fail-closed 化 ✅ 実装済み (2026-02-25)

1. ✅ `--opsec-policy=warn|deny` を追加
- `warn`: 現状互換（`[OPSEC WARNING]` を stderr に出力、コンパイル続行）
- `deny`: 機密パターン検出時はコンパイル失敗（`[OPSEC ERROR]` + `CompileError::OpsecViolation`）
- `clap::ValueEnum` による型安全な引数パース（不正値は clap がリジェクト、fail-open を防止）

2. ✅ `--opsec-audit` を追加
- 生成バイナリに対して `strings`, `nm` を実行
- 5カテゴリ（IP, パス, URL, デバッグ語, 資格情報語）の禁止パターン検知
- `--opsec-policy` に従い warn/deny を切り替え
- `nm` でユーザー定義シンボル残存もフラグ（informational、ツールチェイン由来シンボルは除外）
- `deny` 時に `strings` コマンドが未導入ならコンパイル失敗（fail-closed）
- `warn` 時に `strings` 未導入なら `skipped` を出力して早期リターン（`passed` と混在しない）

3. ✅ CI に secret gate を追加
- `.github/workflows/ci.yml` に gitleaks job を追加

実装詳細:
- `OpsecPolicy` enum (`Warn`/`Deny`, `clap::ValueEnum` derive) を `src/obfuscation.rs` に追加
- `CompileError::OpsecViolation` を `src/error.rs` に追加
- `obfuscate()` の戻り値を `Result<TackyProgram>` 化（`src/tacky/obfuscate.rs`）
- `opsec_audit_binary()` を `src/driver.rs` に追加（リンク後バイナリ監査）
- 11件の E2E テストを `tests/obfuscation.rs` に追加（不正値リジェクト・`--obf-no-opsec` 優先順位・フルリンク監査を含む）
- `--obf-no-opsec-warn` が deny ポリシーより優先（検出自体を無効化）
- `--obf-no-opsec` が `--opsec-policy` / `--opsec-audit` を含む全 OPSEC 機能を上書き無効化

期待効果:
- 開発者のうっかり機密混入をリリース前に止められる。

## P1（高優先）: 難読化の実効性を上げる追加機能

1. Indirect Branch pass（選択適用）
- すべての分岐ではなく、重要関数・条件分岐に限定適用。
- `--obf-no-indirect-branch` と確率/頻度パラメータを追加。

2. Control-Flow Breaking pass（関数先頭の逆アセンブル攪乱）
- 低オーバーヘッドで静的解析耐性を底上げ。
- 既存 Anti-Disassembly と干渉しない順序設計が必要。

3. 引数ランダム化 + ダミー引数（RndArgs系）
- 関数シグネチャベース解析への耐性向上。
- 非対応条件（varargs, 関数ポインタ呼び出し）を明示して段階導入。

期待効果:
- 既存 CFF/VM と異なる軸の攪乱を追加し、単一解除手法への依存を減らす。

## P2（中優先）: 評価/公開の強化

1. `--obf-seed` 導入
- 難読化を「再現可能」と「多様体生成」の両方で制御可能にする。

2. provenance 拡張
- 既存 `results/*/meta.json` に以下を追加:
  - obfuscation config hash
  - seed
  - input/output digest
  - build options

3. 再現ビルドモード
- `SOURCE_DATE_EPOCH` 対応
- `-ffile-prefix-map` の適用
- 必要に応じて linker build-id を制御

## 4. FerrugoCC 向け具体タスク（クロ向け）

1. CLI/設定の拡張
- `src/main.rs`: `--opsec-policy`, `--opsec-audit`, `--obf-seed`, `--obf-no-indirect-branch` 追加
- `src/obfuscation.rs`: config フィールド追加（policy/seed/indirect_branch）

2. ドライバ層の監査
- `src/driver.rs`:
  - コンパイル後に `opsec_audit(output_path)` 実行
  - deny ポリシー時の exit code を明確化

3. パス実装
- `src/tacky/obfuscate.rs` または `src/codegen/*`:
  - Indirect Branch 追加
  - Control-Flow Breaking 追加（必要なら ASM層で開始）

4. CI
- `.github/workflows/ci.yml` に gitleaks ジョブ追加
- （任意）`.pre-commit-config.yaml` に gitleaks hook 追加

5. テスト
- `tests/obfuscation.rs`:
  - 新パス有効時の意味保存テスト
  - `opsec_policy=deny` の失敗系テスト
  - `opsec_audit` の検知テスト

## 5. 実装時の注意

- すべての関数に重いパスを掛けると性能劣化が大きい。選択適用を前提にする。
- CFF/VM など既存重パスと新パスの順序依存を先に固定する。
- OPSEC は「検知精度」と「誤検知コスト」のトレードオフがあるため、`warn` と `deny` 両モードを持つ。

## 6. 推論（Surveyからの判断）

- 推論: FerrugoCC は難読化パス数よりも「運用ミス防止」の改善余地が大きい。
  - 根拠: 現状に `warn` と `strip` はあるが、CI強制失敗やバイナリ監査が未実装。
- 推論: 追加難読化は CFF系の強化より「別系統（indirect branch / control-flow breaking / call signature攪乱）」の方が費用対効果が高い。
  - 根拠: 公開資料で CFF 単体攻撃事例が継続して報告され、複合防御が推奨されるため。

## 7. 参考資料（確認日: 2026-02-25）

### Obfuscation
- O-MVLL Pass一覧: https://obfuscator.re/omvll/passes/
- O-MVLL Control-Flow Flattening: https://obfuscator.re/omvll/passes/control-flow-flattening/
- O-MVLL Indirect Branch: https://obfuscator.re/omvll/passes/indirect-branch/
- O-MVLL Indirect Call: https://obfuscator.re/omvll/passes/indirect-call/
- O-MVLL Control-Flow Breaking: https://obfuscator.re/omvll/passes/control-flow-breaking/
- O-MVLL Basic Block Duplicate: https://obfuscator.re/omvll/passes/basic-block-duplicate/
- Tigress Transformations: https://tigress.cs.arizona.edu/transformPage/index.html
- Tigress RndArgs: https://tigress.cs.arizona.edu/transformPage/docs/randomizeArguments/index.html
- Tigress EncodeExternal: https://tigress.cs.arizona.edu/transformPage/docs/encodeExternal/index.html
- MBA-Blast (USENIX Security 2021): https://www.usenix.org/conference/usenixsecurity21/presentation/liu-binbin

### OPSEC / Supply-chain / Reproducibility
- GitHub Push Protection: https://docs.github.com/en/code-security/concepts/secret-security/about-push-protection
- Gitleaks: https://github.com/gitleaks/gitleaks
- Gitleaks Action: https://github.com/gitleaks/gitleaks-action
- SOURCE_DATE_EPOCH: https://reproducible-builds.org/specs/source-date-epoch/
- GCC `-ffile-prefix-map`: https://gcc.gnu.org/onlinedocs/gcc-15.2.0/gcc/Overall-Options.html
- GNU ld `--build-id`: https://sourceware.org/binutils/docs/ld/Options.html
- SLSA Provenance requirements: https://slsa.dev/spec/v1.0/requirements
