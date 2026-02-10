# FerrugoCC

Rust製のCコンパイラ。[Writing a C Compiler](https://nostarch.com/writing-c-compiler) (Nora Sandler) に沿って段階的に開発する学習プロジェクト。

"Ferrugo" はラテン語で「錆」を意味し、Rust で書いていることに由来する。

## ビルド・実行

```bash
cargo build

# フルコンパイル（Cソース → 実行ファイル）
cargo run -- source.c

# 各ステージで停止
cargo run -- --lex source.c      # 字句解析のみ
cargo run -- --parse source.c    # 構文解析まで
cargo run -- --validate source.c # 型検査まで
cargo run -- --codegen source.c  # コード生成まで
cargo run -- -S source.c         # アセンブリ出力（.s ファイル生成）
```

アセンブリから実行ファイルへの変換には、システムに `gcc` が必要。

## テスト

```bash
cargo test
```

## 実装の進捗

| Chapter | 内容 | 状態 |
|---------|------|------|
| 1 | 定数 return (`return 42;`) | 完了 |
| 2 | 単項演算子 (`-`, `~`, `!`) | 完了 |
| 3 | 二項算術演算子 (`+`, `-`, `*`, `/`, `%`) | 完了 |
| 4 | 関係・等価・論理演算子 (`<`, `<=`, `>`, `>=`, `==`, `!=`, `&&`, `\|\|`) | 完了 |
| 5 | ローカル変数・代入 (`int a = 5; a = 10;`) | 完了 |
| 6 | if文・三項演算子・複合文 (`if/else`, `?:`, `{}`) | 完了 |
| 7 | 複合代入・インクリメント/デクリメント・カンマ演算子 | 完了 |
| 8 | ループ文 (`while`, `do-while`, `for`) と `break`/`continue` | 完了 |
| 9 | 関数（宣言・定義・呼び出し・パラメータ） | 完了 |
| 10 | ファイルスコープ変数・ストレージクラス (`static`, `extern`) | 完了 |
| 11 | Long 整数 (`long`, 型検査パス導入, 暗黙的型変換) | 完了 |
| 12 | 符号なし整数 (`unsigned int`, `unsigned long`, 通常算術変換) | 完了 |
| 13 | 浮動小数点数 (`double`, SSE 命令, XMM レジスタ) | 完了 |

### Chapter 13 の詳細

Chapter 13 では以下の機能を追加した:

- **`double` 型**: IEEE 754 倍精度浮動小数点数に対応
  ```c
  double pi = 3.14159;
  int truncated = pi;      // 3（切り捨て）
  return truncated;
  ```
- **浮動小数点リテラル**: 小数点・指数表記に対応
  ```c
  double a = .5;       // 小数点から始まる
  double b = 42.;      // 小数点で終わる
  double c = 2e1;      // 指数表記（20.0）
  double d = 1.5e-3;   // 小数＋指数（0.0015）
  ```
- **SSE 命令による浮動小数点演算**: `addsd`, `subsd`, `mulsd`, `divsd`
  ```c
  double x = 3.0 + 2.0;   // addsd
  double y = x * 1.5;      // mulsd
  double z = 10.0 / 3.0;   // divsd
  ```
- **単項演算子**: 符号反転（`xorpd` で符号ビット反転）、論理否定（`comisd` で 0.0 比較）
  ```c
  double neg = -x;     // xorpd で符号ビット反転
  int a = !0.0;        // 1（ゼロは偽）
  int b = !3.14;       // 0（非ゼロは真）
  ```
- **比較演算**: `comisd` 命令 + unsigned 条件コード (`A`/`AE`/`B`/`BE`)
  ```c
  if (a < b) return 1;  // comisd + setb
  ```
- **型変換**: 整数 ↔ `double` の暗黙変換（`cvtsi2sd`, `cvttsd2si`）
  ```c
  int x = 42;
  double f = x;    // int → double (cvtsi2sd)
  int y = f;       // double → int (cvttsd2si, 切り捨て)
  ```
- **混合引数の関数呼出規約**: 整数（DI,SI,DX,CX,R8,R9）と double（XMM0〜XMM7）を独立カウント
  ```c
  int sum(int a, double b, int c, double d) {
      return a + b + c + d;
  }
  int main(void) { return sum(1, 2.5, 3, 4.5); } // 10
  ```
- **`double` 定数プール**: `.rodata` セクションにビットパターンを配置
- **複合代入・インクリメント/デクリメント**: `double` に対する `+=`, `-=`, `*=`, `/=`, `++`, `--`

### Chapter 12 の詳細

Chapter 12 では以下の機能を追加した:

- **`unsigned int`/`unsigned long` 型**: 符号なし整数型に対応
  ```c
  unsigned int a = 4294967295U;
  unsigned long b = 18446744073709551615UL;
  ```
- **符号なしリテラル**: `U`/`u` (unsigned int), `UL`/`ul`/`LU`/`lu` (unsigned long) サフィックス
  ```c
  42U     // unsigned int
  42UL    // unsigned long
  42LU    // unsigned long（逆順も可）
  ```
- **型指定子の任意順サポート**: `unsigned int`, `int unsigned`, `unsigned long int` 等
  ```c
  unsigned long int x = 0;  // OK
  long unsigned y = 0;      // OK（同じ型）
  ```
- **通常算術変換 (usual arithmetic conversions)**: 4 型の暗黙変換
  ```c
  int a = -1;
  unsigned int b = 1U;
  long c = a + b;  // a は unsigned int に変換される
  ```
- **符号なし除算/剰余**: `div` 命令（符号付きの `idiv` と区別）
  ```c
  unsigned int a = 4294967295U;
  return a / 2U;  // 2147483647（符号なし除算）
  ```
- **符号なし比較**: `seta`/`setae`/`setb`/`setbe` 条件コード
  ```c
  unsigned int a = 3000000000U;
  return a > 2000000000U;  // 1（符号なし比較）
  ```
- **ゼロ拡張**: `unsigned int` → `unsigned long` 変換に `movl`（上位32ビット自動クリア）
  ```c
  unsigned int a = 4294967295U;
  unsigned long b = a;  // ゼロ拡張: 0x00000000FFFFFFFF
  ```

### Chapter 11 の詳細

Chapter 11 では以下の機能を追加した:

- **`long` 型**: 64ビット符号付き整数に対応
  ```c
  long x = 2147483648L;  // i32 範囲外の値
  ```
- **型検査パス（Validate）の導入**: パースとコード生成の間に型検査を挿入
- **暗黙的型変換**: `int` ↔ `long` の自動変換（`Cast` ノード挿入）
  ```c
  long x = 42;       // int → long に暗黙変換
  int y = x;         // long → int に切り詰め
  ```
- **型に応じたコード生成**: `movl`/`addl` (32bit) vs `movq`/`addq` (64bit) の選択
- **符号拡張/切り詰め命令**: `movslq` (int→long), `movl` (long→int truncate)

### Chapter 10 の詳細

Chapter 10 では以下の機能を追加した:

- **グローバル変数**: ファイルスコープの変数宣言に対応（`.data`/`.bss` セクションに配置）
  ```c
  int x = 5;
  int main(void) { return x; } // 5
  ```
- **未初期化グローバル変数**: デフォルトで 0 に初期化される
  ```c
  int x;
  int main(void) { return x; } // 0
  ```
- **グローバル変数の共有**: 複数の関数からアクセス可能
  ```c
  int g = 100;
  int add_to_g(int n) { return g + n; }
  int main(void) { return add_to_g(23); } // 123
  ```
- **`static` ローカル変数**: 関数呼び出し間で値を保持する
  ```c
  int counter(void) { static int c = 0; c = c + 1; return c; }
  int main(void) { counter(); counter(); return counter(); } // 3
  ```
- **`static` 関数**: 内部リンケージ（`.globl` を出力しない）
  ```c
  static int helper(void) { return 42; }
  int main(void) { return helper(); } // 42
  ```
- **`extern` 宣言**: 外部リンケージの変数参照
  ```c
  int x = 10;
  int get(void) { extern int x; return x; }
  int main(void) { return get(); } // 10
  ```
- **ローカルによるグローバルのシャドーイング**:
  ```c
  int x = 1;
  int main(void) { int x = 2; return x; } // 2
  ```
- **バリデーション**: `extern` 変数に初期化子があるとエラー、ファイルスコープの非定数初期化子はエラー、重複定義エラーを検出

### Chapter 9 の詳細

Chapter 9 では以下の機能を追加した:

- **関数定義・呼び出し**: 複数関数のプログラムに対応
  ```c
  int add(int a, int b) { return a + b; }
  int main(void) { return add(2, 3); } // 5
  ```
- **関数プロトタイプ（前方宣言）**: 定義前に宣言できる
  ```c
  int add(int a, int b);
  int main(void) { return add(10, 20); }
  int add(int a, int b) { return a + b; } // 30
  ```
- **再帰**: 関数の再帰呼び出しに対応
  ```c
  int fib(int n) { if (n <= 1) return n; return fib(n - 1) + fib(n - 2); }
  int main(void) { return fib(10); } // 55
  ```
- **最大6引数のレジスタ渡し**: System V AMD64 ABI 準拠 (`%edi`, `%esi`, `%edx`, `%ecx`, `%r8d`, `%r9d`)
  ```c
  int sum6(int a, int b, int c, int d, int e, int f) { return a + b + c + d + e + f; }
  int main(void) { return sum6(1, 2, 3, 4, 5, 6); } // 21
  ```
- **7引数以上のスタック渡し**: 16バイトアラインメント対応
  ```c
  int sum8(int a, int b, int c, int d, int e, int f, int g, int h) {
      return a + b + c + d + e + f + g + h;
  }
  int main(void) { return sum8(1, 2, 3, 4, 5, 6, 7, 8); } // 36
  ```
- **バリデーション**: 重複定義エラー、パラメータ数不一致エラー、引数数不一致エラーを検出

### Chapter 8 の詳細

Chapter 8 では以下の機能を追加した:

- **while ループ**: `while (cond) stmt`
  ```c
  int a = 0; while (a < 5) a = a + 1; // a は 5
  ```
- **do-while ループ**: `do stmt while (cond);` (最低1回実行)
  ```c
  int a = 10; do { a++; } while (a < 5); // a は 11（条件が偽でも1回実行）
  ```
- **for ループ**: `for (init; cond; post) stmt` (初期化部に宣言も可)
  ```c
  int a = 0; for (int i = 0; i < 5; i++) a++; // a は 5
  ```
- **break**: ループから脱出
  ```c
  int a = 0; while (1) { a++; if (a == 3) break; } // a は 3
  ```
- **continue**: ループの次の反復へスキップ
  ```c
  int s = 0; for (int i = 0; i < 10; i++) { if (i % 2 == 0) continue; s += i; } // s は 25 (1+3+5+7+9)
  ```

ネストしたループでは `break`/`continue` は最も内側のループにのみ作用する。

### Chapter 7 の詳細

Chapter 7 では以下の機能を追加した:

- **複合代入演算子**: `+=`, `-=`, `*=`, `/=`, `%=`
  ```c
  int a = 5; a += 3; // a は 8
  ```
- **前置インクリメント/デクリメント**: `++a`, `--a` (新値を返す)
  ```c
  int a = 5; return ++a; // 6 を返す
  ```
- **後置インクリメント/デクリメント**: `a++`, `a--` (旧値を返す)
  ```c
  int a = 5; return a++; // 5 を返す（a は 6 になる）
  ```
- **カンマ演算子**: `expr1, expr2` (左辺を評価して捨て、右辺の値を返す)
  ```c
  return (1, 2, 3); // 3 を返す
  ```

## アーキテクチャ

```
ソースコード (.c)
    │
    ▼
┌──────────┐
│  Lexer    │  src/lex/        トークン列に分割
└────┬─────┘
     ▼
┌──────────┐
│  Parser   │  src/parse/      抽象構文木 (AST) を構築
└────┬─────┘
     ▼
┌──────────┐
│ Validate  │  src/typecheck/  型検査・暗黙的型変換の挿入
└────┬─────┘
     ▼
┌──────────┐
│ Codegen   │  src/codegen/    AST → アセンブリ AST に変換
└────┬─────┘
     ▼
┌──────────┐
│ Emitter   │  src/emit/       アセンブリ AST → .s テキスト出力
└────┬─────┘
     ▼
┌──────────┐
│  Driver   │  src/driver.rs   gcc を呼び出し .s → 実行ファイル
└──────────┘
```

ターゲットは x86-64 Linux (AT&T 構文)。

## ライセンス

学習目的のプロジェクト。
