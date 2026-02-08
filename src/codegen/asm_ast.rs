//! x86-64 アセンブリの抽象構文木（Assembly AST）
//!
//! C の AST から変換されるアセンブリレベルの中間表現。
//! 実際のアセンブリテキスト（AT&T 構文）に変換する前段として使う。
//!
//! # なぜアセンブリ AST を挟むのか
//! C の AST から直接テキストを生成することもできるが、
//! 中間表現を挟むことで以下のメリットがある:
//! - コード生成ロジック（意味変換）と出力フォーマット（テキスト変換）を分離
//! - 後段の最適化パスを挿入しやすい
//! - テストしやすい（構造体の比較で検証できる）
//!
//! # x86-64 レジスタ（Chapter 1-3 で使用するもの）
//! - `%eax` — 32ビット汎用レジスタ。関数の戻り値や算術演算の結果を格納する。
//!   Cの `return` 文はこのレジスタに値を置いてから `ret` する。
//! - `%al` — `%eax` の下位8ビット。`sete` 等の SetCC 命令で使用。
//! - `%ecx` — 二項演算の一時レジスタ。除算の除数格納にも使用。
//! - `%edx` — `cdq`/`idivl` で使用。符号拡張と剰余格納。

/// アセンブリプログラム全体
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AsmProgram {
    pub function: AsmFunction,
}

/// アセンブリレベルの関数。名前と命令列を持つ。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AsmFunction {
    pub name: String,
    pub instructions: Vec<Instruction>,
}

/// 個々のアセンブリ命令。
///
/// 各バリアントが1つの x86-64 命令に対応する。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Instruction {
    /// `movl src, dst` — 値の転送（32ビット）
    ///
    /// 即値をレジスタにロードする、またはレジスタ間のコピーに使う。
    /// 例: `movl $42, %eax` → EAX に 42 を格納
    Mov { src: Operand, dst: Operand },

    /// 単項演算命令（Chapter 2）
    ///
    /// `negl %eax` (Neg) や `notl %eax` (Not) を表す。
    /// オペランドを直接書き換える（破壊的操作）。
    Unary { op: AsmUnaryOp, operand: Operand },

    /// `cmpl src, dst` — 比較命令（Chapter 2: 論理否定 `!` で使用）
    ///
    /// src と dst を比較し、結果をフラグレジスタに設定する。
    /// 値そのものは変更しない。
    /// 例: `cmpl $0, %eax` → EAX が 0 かどうかをフラグに反映
    Cmp { src: Operand, dst: Operand },

    /// `setCC dst` — 条件付きバイト設定（Chapter 2: 論理否定 `!` で使用）
    ///
    /// フラグレジスタの状態に基づき、dst の下位8ビットに 0 または 1 を設定する。
    /// 例: `sete %al` → ZF（ゼロフラグ）が立っていれば AL=1、そうでなければ AL=0
    SetCC { condition: CondCode, operand: Operand },

    /// 二項演算命令（Chapter 3）
    ///
    /// `addl`, `subl`, `imull` を表す。
    /// 例: `addl %ecx, %eax` → EAX = EAX + ECX
    Binary { op: AsmBinaryOp, src: Operand, dst: Operand },

    /// `idivl operand` — 符号付き除算（Chapter 3）
    ///
    /// `%edx:%eax` を被除数、operand を除数として除算する。
    /// 商 → `%eax`、余り → `%edx`
    Idiv(Operand),

    /// `cdq` — EAX を EDX:EAX に符号拡張（Chapter 3）
    ///
    /// `idivl` の前に呼び出して被除数を64ビットに拡張する。
    Cdq,

    /// `push operand` — スタックにプッシュ（Chapter 3）
    ///
    /// 二項演算で左辺の結果を一時退避するために使用。
    Push(Operand),

    /// `pop operand` — スタックからポップ（Chapter 3）
    ///
    /// 退避した左辺の結果を復帰する。
    Pop(Operand),

    /// `jmp label` — 無条件ジャンプ（Chapter 4: 論理演算子で使用）
    Jmp(String),

    /// `jCC label` — 条件ジャンプ（Chapter 4: 論理演算子で使用）
    ///
    /// フラグレジスタの状態に基づき、条件が成立すればジャンプする。
    JmpCC(CondCode, String),

    /// `label:` — ラベル定義（Chapter 4: 論理演算子で使用）
    ///
    /// ジャンプ先のターゲットとなるラベルを定義する。
    Label(String),

    /// `subq $N, %rsp` — スタック領域の確保（Chapter 5）
    ///
    /// ローカル変数用のスタック領域を確保する。
    AllocateStack(usize),

    /// `ret` — 関数からの復帰
    ///
    /// コールスタックからリターンアドレスを取り出してジャンプする。
    /// `%eax` の値が呼び出し元に戻り値として渡される（x86-64 ABI）。
    Ret,
}

/// アセンブリレベルの単項演算子（Chapter 2）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AsmUnaryOp {
    /// `negl` — 2の補数による符号反転。C の `-x` に対応。
    /// 内部動作: operand = 0 - operand
    Neg,
    /// `notl` — ビット反転。C の `~x` に対応。
    /// 内部動作: 各ビットを反転（0↔1）
    Not,
}

/// アセンブリレベルの二項演算子（Chapter 3）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AsmBinaryOp {
    /// `addl` — 加算。C の `a + b` に対応。
    Add,
    /// `subl` — 減算。C の `a - b` に対応。
    Sub,
    /// `imull` — 符号付き乗算。C の `a * b` に対応。
    Mult,
}

/// 条件コード（Chapter 2-4）
///
/// `SetCC` や `JmpCC` 命令でどのフラグ条件を使うかを指定する。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CondCode {
    /// Equal — ZF (ゼロフラグ) が 1 のとき真
    E,
    /// Not Equal — ZF が 0 のとき真
    NE,
    /// Less Than — SF ≠ OF のとき真（符号付き比較）
    L,
    /// Less or Equal — ZF=1 または SF≠OF のとき真
    LE,
    /// Greater Than — ZF=0 かつ SF=OF のとき真
    G,
    /// Greater or Equal — SF=OF のとき真
    GE,
}

/// オペランド（命令の引数）
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Operand {
    /// 即値（定数）。例: `$42`
    Imm(i64),
    /// レジスタ。例: `%eax`
    Register(Reg),
    /// スタック上の変数（Chapter 5）。例: `-4(%rbp)`
    ///
    /// オフセットは `%rbp` からの相対位置（負の値）。
    Stack(i32),
}

/// レジスタ名
///
/// AT&T 構文では命令に応じて `%eax`（32ビット）や `%al`（8ビット）として出力する。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reg {
    /// EAX レジスタ — 戻り値・算術演算用
    AX,
    /// ECX レジスタ — 二項演算の一時格納・除算の除数用（Chapter 3）
    CX,
    /// EDX レジスタ — `cdq`/`idivl` で使用（Chapter 3）
    DX,
}
