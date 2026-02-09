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
┌─────────┐
│  Lexer   │  src/lex/     トークン列に分割
└────┬────┘
     ▼
┌─────────┐
│  Parser  │  src/parse/   抽象構文木 (AST) を構築
└────┬────┘
     ▼
┌─────────┐
│ Codegen  │  src/codegen/ AST → アセンブリ AST に変換
└────┬────┘
     ▼
┌─────────┐
│ Emitter  │  src/emit/    アセンブリ AST → .s テキスト出力
└────┬────┘
     ▼
┌─────────┐
│  Driver  │  src/driver/  gcc を呼び出し .s → 実行ファイル
└─────────┘
```

ターゲットは x86-64 Linux (AT&T 構文)。

## ライセンス

学習目的のプロジェクト。
