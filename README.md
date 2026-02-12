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

# 難読化コンパイル（最適化の代わりに難読化パスを適用）
cargo run -- --fobfuscate source.c
cargo run -- --fobfuscate -S source.c  # 難読化アセンブリ出力
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
| 14 | ポインタ (`int *`, `&`, `*`, ポインタ比較, null, キャスト) | 完了 |
| 15 | 配列とポインタ算術 (`int arr[10]`, `arr[i]`, `ptr + n`, `sizeof`) | 完了 |
| 16 | 文字と文字列 (`char`, `unsigned char`, 文字リテラル, 文字列リテラル) | 完了 |
| 17 | void 型と void ポインタ (`void`, `void *`, `malloc`/`free`) | 完了 |
| 18 | 構造体（`struct`、メンバアクセス、ポインタ経由アクセス） | 完了 |
| 19 | TACKY IR（三番地コード中間表現、最適化パス基盤） | 完了 |
| 20 | レジスタ割り当て（グラフ彩色、生存解析、Chaitin-Briggs） | 完了 |

### Chapter 20 の詳細

Chapter 20 では**グラフ彩色によるレジスタ割り当て**を実装した。
それまで全変数をスタックに配置し、毎回 load→operate→store の3命令パターンだったのを、
変数をレジスタに直接割り当てることで効率的なコードを生成する。

- **Pseudo レジスタ**: コード生成で変数を `Operand::Pseudo(name)` として出力し、後段で解決する
  ```
  // 旧: 3命令パターン（全変数スタック経由）
  movl -4(%rbp), %eax      // load left
  addl -8(%rbp), %eax      // operate
  movl %eax, -12(%rbp)     // store result

  // 新: 2命令パターン（変数がレジスタに割り当てられる）
  movl %ebx, %ecx          // left → dst
  addl %edx, %ecx          // right + dst
  ```
- **生存解析（Liveness Analysis）**: 後方データフロー解析で各命令時点の生存変数集合を計算
  - ラベル/ジャンプから CFG を構築し、不動点反復で `live_in`/`live_after` を求める
  - 暗黙的な use/def を追跡（`idiv` → AX,DX、`call` → 全 caller-saved レジスタ等）
- **干渉グラフ**: 同時に生存する変数間に辺を張る。整数グラフと XMM グラフを分離して独立に彩色
  - Mov 命令の src-dst 間には辺を張らない（将来の coalescing 最適化のため）
- **Chaitin-Briggs グラフ彩色**:
  - Simplify: degree < k のノードをスタックに push して除去
  - Potential Spill: degree が最大のノードを spill 候補としてマーク
  - Select: スタックから pop して、隣接ノードが使っていない色（レジスタ）を割り当て
  - 楽観的彩色: spill 候補でも色が見つかれば割り当て、見つからなければ実際に spill
- **レジスタ分類（System V AMD64 ABI）**:
  - 整数割り当て候補（12個）: AX, BX, CX, DX, SI, DI, R8, R9, R12, R13, R14, R15
  - Callee-saved（5個）: BX, R12, R13, R14, R15（関数呼び出しをまたいで保存が必要）
  - Scratch レジスタ: R10, R11（fixup 用）、XMM15（XMM の fixup 用）
  - XMM 割り当て候補（15個）: XMM0〜XMM14
- **Fixup パス**: レジスタ割り当て後に生じる無効なオペランド組み合わせを修正
  - `movl Stack, Stack` → scratch レジスタ（R10 or XMM15）経由の2命令に展開
  - `imul` のメモリ dst → R11 経由に展開
  - `movslq $imm, reg` → `movq $imm, reg` に変換（即値の符号拡張は不要）
  - 同一レジスタ間の `mov` を除去（noop 最適化）
- **プロローグ/エピローグ自動生成**: 使用した callee-saved レジスタのみ push/pop する
  ```asm
  push %rbp              # フレームポインタ保存
  movq %rsp, %rbp
  push %rbx              # callee-saved（使用した場合のみ）
  push %r12              # callee-saved（使用した場合のみ）
  subq $N, %rsp          # spill スロット確保（16バイトアラインメント調整済み）
  ```
- **スタックアラインメント**: `callee_saved_count * 8 + alloc_size ≡ 0 (mod 16)` を保証
  ```rust
  let callee_bytes = callee_count * 8;
  let total = callee_bytes + spill_bytes;
  let aligned_total = (total + 15) & !15;
  let alloc_size = aligned_total - callee_bytes;
  ```
- **アドレス取得対象の例外処理**: `&x` でアドレスを取られる変数、構造体、配列は
  レジスタ割り当ての対象外とし、従来通りスタックに配置する

### Chapter 19 の詳細

Chapter 19 では TACKY IR（三番地コード中間表現）を導入した:

- **TACKY IR**: C の AST と x86-64 アセンブリの中間に位置する三番地コード形式の IR
  ```
  // C: int r = a + b * c;
  // TACKY:
  tmp.0 = b * c
  r = a + tmp.0
  ```
- **コンパイルパイプライン変更**: `C AST → TACKY IR → Assembly AST` の2段階に分離
- **最適化パス基盤**: TACKY IR 上で定数畳み込み等の最適化を実行可能にする

### Chapter 18 の詳細

Chapter 18 では構造体を実装した:

- **構造体型**: `struct` の定義とメンバアクセス
  ```c
  struct Point { int x; int y; };
  struct Point p;
  p.x = 10;
  p.y = 20;
  return p.x + p.y;  // 30
  ```
- **ポインタ経由のメンバアクセス**: `ptr->member`（`(*ptr).member` の糖衣構文）
- **`MemoryOffset(Reg, i32)` オペランド**: レジスタ+オフセット間接アドレッシングで構造体メンバにアクセス
- **`CopyToOffset`/`CopyFromOffset`**: 構造体コピーのための TACKY 命令

### Chapter 17 の詳細

Chapter 17 では以下の機能を追加した:

- **`void` 型**: 関数の戻り値型・キャスト先として使用可能な不完全型
  ```c
  void do_nothing(void) { return; }
  int main(void) { do_nothing(); return 0; }
  ```
- **`void *` (void ポインタ)**: 任意のポインタ型と暗黙的に相互変換可能な汎用ポインタ
  ```c
  void *malloc(unsigned long size);
  void free(void *ptr);
  int main(void) {
      int *arr = malloc(5 * sizeof(int));  // void* → int* 暗黙変換
      arr[0] = 42;
      int result = arr[0];
      free(arr);                            // int* → void* 暗黙変換
      return result;  // 42
  }
  ```
- **`(void)expr` キャスト**: 式の値を捨てる（副作用のみ実行）
  ```c
  int x;
  int set_x(int i) { x = i; return 0; }
  int main(void) { (void) set_x(12); return x; }  // 12
  ```
- **void ポインタの暗黙変換**: 代入・関数引数・return・三項演算子で `void *` ↔ 任意のポインタ型
  ```c
  void *ptr = malloc(32);
  double *dp = ptr;      // void* → double* 暗黙変換
  void *ptr2 = dp;       // double* → void* 暗黙変換
  free(ptr2);
  ```
- **void ternary**: 両辺が void の三項演算子
  ```c
  void incr_i(void);
  void incr_j(void);
  int main(void) { 1 ? incr_i() : incr_j(); return 0; }
  ```
- **不完全型チェック**: 以下をコンパイルエラーとして検出
  - `void x;` — void 型の変数宣言
  - `sizeof(void)` — 不完全型の sizeof
  - `-(void)10` — void 式の算術演算
  - `void *p; p + 1;` — void ポインタの算術
  - `int foo(void) { return; }` — 非 void 関数での値なし return

### Chapter 15 の詳細

Chapter 15 では以下の機能を追加した:

- **配列型**: `int arr[10]`, `long arr[5]` 等の固定長配列宣言
  ```c
  int arr[5];
  arr[0] = 10;
  arr[4] = 20;
  return arr[0] + arr[4];  // 30
  ```
- **配列添字**: `arr[i]` はパーサーで `*(arr + i)` に脱糖（desugaring）
  ```c
  int arr[3];
  arr[0] = 100; arr[1] = 200; arr[2] = 300;
  return arr[2];  // 300
  ```
- **配列→ポインタ減衰（decay）**: 式中の配列は自動的にポインタに変換
  ```c
  int arr[5];
  int *p = arr;    // 配列 → 先頭要素へのポインタに暗黙変換
  ```
- **ポインタ算術**: ポインタ加減算は要素サイズ分スケーリングされる
  ```c
  int arr[10];
  int *p = arr;
  *(p + 3) = 42;   // arr[3] に 42 を代入（3 * sizeof(int) = 12 バイト進む）
  return arr[3];    // 42
  ```
- **ポインタ減算**: 同じ型のポインタ同士の差分は要素数で返される
  ```c
  int arr[10];
  int *p = &arr[7];
  int *q = &arr[2];
  long diff = p - q;
  return (int) diff;  // 5
  ```
- **ポインタ比較の拡張**: `<`, `<=`, `>`, `>=` を同じポインタ型同士で許可
  ```c
  int arr[5];
  int *p = &arr[1];
  int *q = &arr[3];
  return p < q;  // 1
  ```
- **ポインタ増分/減分**: `++`, `--`, `+=`, `-=` が要素サイズ分移動
  ```c
  int arr[5];
  arr[0] = 1; arr[1] = 2; arr[2] = 3;
  int *p = arr;
  p++;
  return *p;  // 2
  ```
- **`sizeof` 演算子**: 型チェック時に定数（`unsigned long`）に解決
  ```c
  int arr[10];
  return (int) sizeof(arr);   // 40（int は 4 バイト × 10）
  return (int) sizeof(int);   // 4
  return (int) sizeof(long);  // 8
  ```
- **配列パラメータ**: 関数パラメータの `int arr[]` は `int *arr` に変換
  ```c
  int first(int arr[]) { return arr[0]; }
  int main(void) {
      int a[3];
      a[0] = 99;
      return first(a);  // 99
  }
  ```
- **グローバル配列**: `.bss` セクションにゼロ初期化で配置
  ```c
  int arr[3];
  int main(void) {
      arr[0] = 5; arr[1] = 10; arr[2] = 15;
      return arr[0] + arr[1] + arr[2];  // 30
  }
  ```
- **制限事項**: 配列の初期化子リスト（`int arr[3] = {1, 2, 3}`）は未対応（Chapter 18 で実装予定）

### Chapter 14 の詳細

Chapter 14 では以下の機能を追加した:

- **ポインタ型**: `int *`, `double *`, `int **` 等の多段ポインタに対応
  ```c
  int x = 3;
  int *ptr = &x;
  return *ptr;       // 3
  ```
- **アドレス演算子 (`&`)**: 変数のアドレスを取得
  ```c
  int x = 10;
  int *p = &x;       // x のアドレスを p に格納
  ```
- **間接参照演算子 (`*`)**: ポインタ経由の読み書き
  ```c
  int x = 0;
  int *ptr = &x;
  *ptr = 42;          // ポインタ経由で x に書き込み
  return x;            // 42
  ```
- **ポインタの関数引数・戻り値**: ポインタを関数間で受け渡し
  ```c
  int *return_pointer(int *in) { return in; }
  int main(void) {
      int x = 10;
      int *p = return_pointer(&x);
      return *p;       // 10
  }
  ```
- **ポインタ比較**: `==` / `!=` で同じポインタ型同士を比較
  ```c
  int a = 0, b = 0;
  int *a_ptr = &a;
  int *b_ptr = &b;
  return a_ptr == b_ptr;  // 0（異なるアドレス）
  ```
- **null ポインタ**: 整数定数 0 をポインタに代入可能。条件式で真偽判定
  ```c
  int *p = 0;
  if (p) return 1;
  return 0;            // 0（null は偽）
  ```
- **ポインタ ↔ 整数キャスト**: 明示的キャストで相互変換
  ```c
  long l = 128;
  int *a = (int *) l;
  int *b = (int *) 128l;
  return a == b;       // 1
  ```
- **宣言子パーサー**: `int *x`, `int **pp` 等のポインタ宣言構文を解析
- **キャスト式パーサー**: `(int *)expr` と `(expr)` の区別（型キーワードによる先読み）
- **左辺値の一般化**: `*ptr = val` 形式の代入に対応（`Assign(Box<Expr>, ...)` に変更）
- **コード生成**: `Lea` (アドレスロード)、`Memory(Reg)` (レジスタ間接アドレッシング)

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
│  Lexer    │  src/lex/           トークン列に分割
└────┬─────┘
     ▼
┌──────────┐
│  Parser   │  src/parse/         抽象構文木 (AST) を構築
└────┬─────┘
     ▼
┌──────────┐
│ Validate  │  src/typecheck/     型検査・暗黙的型変換の挿入
└────┬─────┘
     ▼
┌──────────┐
│ TACKY Gen │  src/tacky/         C AST → TACKY IR（三番地コード）に変換
└────┬─────┘
     ▼
┌──────────┐
│ Optimize  │  src/tacky/         TACKY IR 最適化パス（デフォルト）
│    or     │  optimize.rs        定数畳み込み・コピー伝播・不要コード除去
│ Obfuscate │  obfuscate.rs       難読化パス（--fobfuscate 指定時）
└────┬─────┘                      定数間接化・ジャンクコード・不透明述語・CFF
     ▼
┌──────────┐
│ Codegen   │  src/codegen/       TACKY IR → Asm(Pseudo) に変換
│          │  generator.rs
└────┬─────┘
     ▼
┌──────────┐
│ RegAlloc  │  src/codegen/       生存解析 → 干渉グラフ → グラフ彩色
│          │  regalloc.rs         Pseudo → Register/Stack(spill) に置換
└────┬─────┘
     ▼
┌──────────┐
│ Fixup     │  src/codegen/       無効オペランド修正 + プロローグ/エピローグ生成
│          │  regalloc.rs
└────┬─────┘
     ▼
┌──────────┐
│ Emitter   │  src/emit/          アセンブリ AST → .s テキスト出力
└────┬─────┘
     ▼
┌──────────┐
│  Driver   │  src/driver.rs      gcc を呼び出し .s → 実行ファイル
└──────────┘
```

ターゲットは x86-64 Linux (AT&T 構文)。

## Future Work

書籍 "Writing a C Compiler" の全20章を完了。今後の改善候補:

### 最適化

- [ ] **Coalescing（コピー合体）**: `Mov a, b` で a と b が干渉しなければ合体し、Mov を除去する。George/Briggs 基準で安全性を判定（Chapter 20 Step 7 として計画済み）
- [ ] **TACKY IR 上の最適化パス強化**: 現在の定数畳み込みに加え、コピー伝播・不要コード除去・共通部分式除去など
- [ ] **ポインタ経由の書き込みを考慮した定数伝播**: `*p = 100; return x;` で `x` の値が更新されないバグの修正（TACKY オプティマイザの既知問題）

### 言語機能の拡充

- [ ] **配列初期化子リスト**: `int arr[3] = {1, 2, 3};`（Chapter 15 の制限事項）
- [ ] **カンマ区切り複数宣言**: `int a = 1, b = 2, c = 3;`
- [ ] **switch 文**: `switch`/`case`/`default`
- [ ] **enum 型**: `enum Color { RED, GREEN, BLUE };`
- [ ] **typedef**: 型エイリアスの定義
- [ ] **プリプロセッサ**: `#include`, `#define`, `#ifdef` 等

### コード難読化（Anti-Reverse Engineering）

コンパイラレベルでの難読化変換。元のソースコードと等価だが、逆コンパイル・逆アセンブル時に解析を困難にする。
`--fobfuscate` フラグで有効化し、TACKY IR 上の変換パスとして実装。最適化パスの代わりに4つの難読化パスを順に適用する。

- [x] **定数の間接化（Constant Encoding）**: 即値をランタイム計算に置換（`42` → `6 * 7`, `0` → `a - a` 等）。Double は精度問題のためスキップ
- [x] **ジャンクコード挿入**: 4命令ごとに実行結果に影響しない dead computation（3命令）を挿入し、逆コンパイラの解析量を増やす
- [x] **不透明述語（Opaque Predicates）**: `x*(x+1) % 2 == 0`（常に真）の条件分岐で値生成命令を囲み、制御フローを複雑化する
  ```c
  // 元: return x + 1;
  // 難読化後（等価）:
  if ((y * (y + 1)) % 2 == 0) { return x + 1; } else { fake_code; }  // else は到達不能
  ```
- [x] **制御フロー平坦化（Control Flow Flattening）**: 関数内の基本ブロックを state 変数 + dispatch ループに変換し、元の制御構造を隠蔽する
  ```c
  // 元: if (a) { ... } else { ... }; return r;
  // 変換後: state=0; dispatch: switch(state) { case 0: ... state=1; goto dispatch; ... }
  ```

未実装の候補:
- [ ] **文字列暗号化**: 文字列リテラルを暗号化して `.rodata` に配置し、使用時にランタイムで復号する
- [ ] **関数のインライン展開/アウトライン化**: 関数境界を攪乱し、元の関数構造の復元を困難にする
- [ ] **レジスタ割り当ての撹乱**: 不要なレジスタ間 mov や register rotation を挿入し、データフロー解析を妨害する

### コード品質

- [ ] **コンパイラ警告の解消**: 既存コード（tacky, parse, lex モジュール）に残る未使用変数・インポートの整理（現在20件）
- [ ] **E2E テストスイートの充実**: 各章の機能を網羅する統合テストの追加

## ライセンス

学習目的のプロジェクト。
