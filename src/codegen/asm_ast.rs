//! x86-64 アセンブリの抽象構文木（Assembly AST）
//!
//! TACKY IR から変換されるアセンブリレベルの中間表現。
//! 実際のアセンブリテキスト（AT&T 構文）に変換する前段として使う。
//!
//! # なぜアセンブリ AST を挟むのか
//! TACKY IR から直接テキストを生成することもできるが、
//! 中間表現を挟むことで以下のメリットがある:
//! - コード生成ロジック（TACKY → Asm）と出力フォーマット（テキスト変換）を分離
//! - 後段の最適化パスを挿入しやすい
//! - テストしやすい（構造体の比較で検証できる）
//!
//! # Chapter 11: AsmType による命令サイズの区別
//! `Longword` (32bit) と `Quadword` (64bit) で命令のサイズを区別する。
//!
//! # Chapter 14: ポインタ対応
//! - `Lea { src, dst }`: アドレスロード命令（`leaq`）。変数のアドレスを取得する。
//! - `Operand::Memory(Reg)`: レジスタ間接アドレッシング（`(%rax)` 等）。ポインタ経由のメモリアクセスに使用。
//!
//! # Chapter 15: 配列とポインタ算術
//! - `StaticInit::ZeroInit(usize)`: 配列のゼロ初期化（`.bss` セクションに `.zero N` で配置）。
//!
//! # Chapter 18: 構造体
//! - `Operand::MemoryOffset(Reg, i32)`: レジスタ+オフセット間接アドレッシング（`N(%rax)` 等）。
//!   構造体メンバへのアクセスで、ベースアドレスからメンバオフセット分ずらしてアクセスする。
//! - グローバル構造体変数は `ZeroInit(struct_size)` で `.bss` セクションに配置。
//!
//! # Chapter 19: TACKY IR 導入
//! コード生成の入力が C AST から TACKY IR に変更された。
//! アセンブリ AST 自体は変更なし（TACKY 命令を機械的にマッピングする）。
//!
//! # Chapter 20: レジスタ割り当て
//! - `Operand::Pseudo(String)`: 割り当て前の疑似レジスタ。コード生成で変数に使用され、
//!   レジスタ割り当てパス (`regalloc`) で `Register` または `Stack(spill)` に置換される。
//! - `Reg` に `Hash` derive を追加（干渉グラフのノードとして使用するため）。
//! - `BX`, `R10`-`R15`, `SP`, `BP`, `XMM8`-`XMM13` を追加。
//! - レジスタ分類定数: `GP_ALLOCATABLE`（12個）, `GP_CALLEE_SAVED`（5個）,
//!   `GP_CALLER_SAVED`（9個）, `XMM_ALLOCATABLE`（15個）, `XMM_ALL`（16個）。
//! - `R10`/`R11` は fixup パスの scratch レジスタ、`XMM15` は XMM の scratch として予約。

/// アセンブリ命令のサイズ（Chapter 11, 13, 16）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AsmType {
    /// 8ビット（char）（Chapter 16）
    Byte,
    /// 32ビット（int）
    Longword,
    /// 64ビット（long）
    Quadword,
    /// 64ビット浮動小数点（double）（Chapter 13）
    Double,
}

/// 静的変数/定数の初期値（Chapter 13, 15, 16）。
#[derive(Debug, Clone, PartialEq)]
pub enum StaticInit {
    /// 整数初期値
    IntInit(i64),
    /// 浮動小数点初期値
    DoubleInit(f64),
    /// ゼロ初期化（指定バイト数分）（Chapter 15: 配列のゼロ初期化）
    ZeroInit(usize),
    /// 文字列初期値（内容, バイト長 = len + 1）（Chapter 16）
    StringInit(String, usize),
    /// バイト配列初期値（難読化パスで暗号化された文字列リテラル）。
    /// `.data` セクションに `.byte 0xNN, ...` として配置される。
    ByteArrayInit(Vec<u8>),
}

/// 読み取り専用の静的定数（Chapter 13）。`.rodata` セクションに配置。
#[derive(Debug, Clone, PartialEq)]
pub struct AsmStaticConstant {
    pub name: String,
    pub alignment: usize,
    pub init: StaticInit,
}

/// アセンブリプログラム全体（Chapter 10: 静的変数対応, Chapter 13: 静的定数追加）
#[derive(Debug, Clone, PartialEq)]
pub struct AsmProgram {
    pub functions: Vec<AsmFunction>,
    pub static_vars: Vec<AsmStaticVar>,
    pub static_constants: Vec<AsmStaticConstant>,
}

/// アセンブリレベルの関数。名前と命令列を持つ。
#[derive(Debug, Clone, PartialEq)]
pub struct AsmFunction {
    pub name: String,
    pub instructions: Vec<Instruction>,
    pub global: bool,
}

/// 静的変数（Chapter 10, 11, 13）。グローバル変数および static ローカル変数。
#[derive(Debug, Clone, PartialEq)]
pub struct AsmStaticVar {
    pub name: String,
    pub global: bool,
    pub init: StaticInit,
    pub asm_type: AsmType,
}

/// 個々のアセンブリ命令。
///
/// 各バリアントが1つの x86-64 命令に対応する。
/// Chapter 11: サイズ依存命令に `asm_type` フィールドを追加。
#[derive(Debug, Clone, PartialEq)]
pub enum Instruction {
    /// `movl`/`movq` — 値の転送
    Mov { asm_type: AsmType, src: Operand, dst: Operand },

    /// 単項演算命令（neg/not）
    Unary { asm_type: AsmType, op: AsmUnaryOp, operand: Operand },

    /// `cmpl`/`cmpq` — 比較命令
    Cmp { asm_type: AsmType, src: Operand, dst: Operand },

    /// `setCC dst` — 条件付きバイト設定
    SetCC { condition: CondCode, operand: Operand },

    /// 二項演算命令（add/sub/imul）
    Binary { asm_type: AsmType, op: AsmBinaryOp, src: Operand, dst: Operand },

    /// `idivl`/`idivq` — 符号付き除算
    Idiv { asm_type: AsmType, operand: Operand },

    /// `divl`/`divq` — 符号なし除算（Chapter 12）
    Div { asm_type: AsmType, operand: Operand },

    /// `cdq` (Longword) / `cqo` (Quadword) — 符号拡張
    SignExtend(AsmType),

    /// `movslq src, dst` — int → long 符号拡張（Chapter 11）
    Movsx { src: Operand, dst: Operand },

    /// `movsbl`/`movsbq src, dst` — byte → int/long 符号拡張（Chapter 16）
    MovsxByte { asm_type: AsmType, src: Operand, dst: Operand },

    /// `movl src, dst` — 32→64 ゼロ拡張（Chapter 12）
    /// x86-64 で32ビット mov は上位32ビットを自動ゼロクリアする。
    MovZeroExtend { src: Operand, dst: Operand },

    /// `movzbl`/`movzbq src, dst` — byte → int/long ゼロ拡張（Chapter 16）
    MovZeroExtendByte { asm_type: AsmType, src: Operand, dst: Operand },

    /// `movl src, dst` — long → int 切り詰め（Chapter 11）
    /// 32ビット mov で上位32ビットを暗黙的にゼロクリア。
    Truncate { src: Operand, dst: Operand },

    /// `push operand` — スタックにプッシュ
    Push(Operand),

    /// `pop operand` — スタックからポップ
    Pop(Operand),

    /// `jmp label` — 無条件ジャンプ
    Jmp(String),

    /// `jCC label` — 条件ジャンプ
    JmpCC(CondCode, String),

    /// `label:` — ラベル定義
    Label(String),

    /// `subq $N, %rsp` — スタック領域の確保
    AllocateStack(usize),

    /// `addq $N, %rsp` — スタック領域の解放
    DeallocateStack(usize),

    /// `call <function>` — 関数呼び出し
    Call(String),

    /// `ret` — 関数からの復帰
    Ret,

    /// `cvtsi2sd` — 整数→double 変換（Chapter 13）
    Cvtsi2sd { asm_type: AsmType, src: Operand, dst: Operand },

    /// `cvttsd2si` — double→整数 変換（切り捨て）（Chapter 13）
    Cvttsd2si { asm_type: AsmType, src: Operand, dst: Operand },

    /// `leaq src, dst` — 実効アドレスのロード（Chapter 14）
    Lea { src: Operand, dst: Operand },
}

/// アセンブリレベルの単項演算子
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AsmUnaryOp {
    /// `negl`/`negq` — 符号反転
    Neg,
    /// `notl`/`notq` — ビット反転
    Not,
}

/// アセンブリレベルの二項演算子
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AsmBinaryOp {
    /// `addl`/`addq`/`addsd` — 加算
    Add,
    /// `subl`/`subq`/`subsd` — 減算
    Sub,
    /// `imull`/`imulq`/`mulsd` — 乗算
    Mult,
    /// `divsd` — SSE 除算（Chapter 13）
    DivDouble,
    /// `xorpd` — XOR（符号ビット反転用）（Chapter 13）
    Xor,
}

/// 条件コード
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CondCode {
    E, NE, L, LE, G, GE,
    /// `a` — 符号なし大なり（Chapter 12）
    A,
    /// `ae` — 符号なし大なりイコール（Chapter 12）
    AE,
    /// `b` — 符号なし小なり（Chapter 12）
    B,
    /// `be` — 符号なし小なりイコール（Chapter 12）
    BE,
}

/// オペランド（命令の引数）
#[derive(Debug, Clone, PartialEq)]
pub enum Operand {
    /// 即値（定数）。例: `$42`
    Imm(i64),
    /// レジスタ。例: `%eax`
    Register(Reg),
    /// 割り当て前の疑似レジスタ（Chapter 20）。レジスタ割り当てパスで解決される。
    Pseudo(String),
    /// スタック上の変数。例: `-4(%rbp)`
    Stack(i32),
    /// グローバル/静的変数。例: `x(%rip)`
    Data(String),
    /// レジスタ間接アドレッシング（Chapter 14）。例: `(%rax)`
    Memory(Reg),
    /// レジスタ + オフセット間接アドレッシング（Chapter 18）。例: `8(%rax)`
    MemoryOffset(Reg, i32),
}

/// レジスタ名
///
/// Chapter 20: Hash derive 追加（干渉グラフのノードとして使用）。
/// BX, R10-R15, SP, BP, XMM8-13 追加（レジスタ割り当て用）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Reg {
    AX, BX, CX, DX, DI, SI,
    R8, R9, R10, R11,
    R12, R13, R14, R15,
    SP, BP,
    /// XMM レジスタ（Chapter 13: SSE 浮動小数点演算）
    XMM0, XMM1, XMM2, XMM3, XMM4, XMM5, XMM6, XMM7,
    XMM8, XMM9, XMM10, XMM11, XMM12, XMM13,
    XMM14, XMM15,
}

/// 整数レジスタ割り当て候補（12個）。R10/R11 は fixup 用 scratch として予約。
pub const GP_ALLOCATABLE: [Reg; 12] = [
    Reg::AX, Reg::BX, Reg::CX, Reg::DX, Reg::SI, Reg::DI,
    Reg::R8, Reg::R9, Reg::R12, Reg::R13, Reg::R14, Reg::R15,
];

/// Callee-saved 整数レジスタ（関数呼び出しをまたいで保存が必要）
pub const GP_CALLEE_SAVED: [Reg; 5] = [Reg::BX, Reg::R12, Reg::R13, Reg::R14, Reg::R15];

/// Caller-saved 整数レジスタ（Call 命令で破壊される）
pub const GP_CALLER_SAVED: [Reg; 9] = [
    Reg::AX, Reg::CX, Reg::DX, Reg::SI, Reg::DI,
    Reg::R8, Reg::R9, Reg::R10, Reg::R11,
];

/// XMM 割り当て候補（15個）。XMM15 は fixup 用 scratch として予約。
pub const XMM_ALLOCATABLE: [Reg; 15] = [
    Reg::XMM0, Reg::XMM1, Reg::XMM2, Reg::XMM3,
    Reg::XMM4, Reg::XMM5, Reg::XMM6, Reg::XMM7,
    Reg::XMM8, Reg::XMM9, Reg::XMM10, Reg::XMM11,
    Reg::XMM12, Reg::XMM13, Reg::XMM14,
];

/// 全 XMM レジスタ（Call 命令で破壊される caller-saved）
pub const XMM_ALL: [Reg; 16] = [
    Reg::XMM0, Reg::XMM1, Reg::XMM2, Reg::XMM3,
    Reg::XMM4, Reg::XMM5, Reg::XMM6, Reg::XMM7,
    Reg::XMM8, Reg::XMM9, Reg::XMM10, Reg::XMM11,
    Reg::XMM12, Reg::XMM13, Reg::XMM14, Reg::XMM15,
];
