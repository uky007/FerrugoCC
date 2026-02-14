//! TACKY IR（三アドレスコード中間表現）のデータ構造定義
//!
//! C の AST から変換される中間表現。無制限の仮想変数を持ち、
//! 最適化パスを適用した後にアセンブリ AST に変換される。
//!
//! # 特徴
//! - 三アドレスコード: 各命令は最大3つのオペランドを持つ
//! - 無制限の仮想変数: `tmp.0`, `tmp.1`, ... で一時変数を生成
//! - 制御フロー: `Jump`, `JumpIfZero`, `JumpIfNotZero`, `Label` で表現
//! - 間接ジャンプ: `JumpIndirect` — 難読化の CFF ジャンプテーブル用。
//!   `possible_targets` で生存解析に正しい CFG 後続ブロック情報を提供
//! - 型情報: `var_types` マップで全変数の型を追跡（最適化パスで使用）

use std::collections::HashMap;
use crate::parse::ast::Type;

/// TACKY プログラム全体
#[derive(Debug, Clone)]
pub struct TackyProgram {
    pub functions: Vec<TackyFunction>,
    pub static_vars: Vec<TackyStaticVar>,
    pub static_constants: Vec<TackyStaticConstant>,
}

/// TACKY 関数定義
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct TackyFunction {
    pub name: String,
    pub global: bool,
    pub params: Vec<String>,
    pub body: Vec<TackyInstruction>,
    pub return_type: Type,
    /// 全変数（パラメータ + ローカル + 一時変数）の型マッピング
    pub var_types: HashMap<String, Type>,
}

/// TACKY 三アドレスコード命令
#[derive(Debug, Clone, PartialEq)]
#[allow(dead_code)]
pub enum TackyInstruction {
    /// `return val`
    Return(TackyVal),
    /// `return` (void)
    ReturnVoid,
    /// `dst = op src`
    Unary { op: TackyUnaryOp, src: TackyVal, dst: TackyVal },
    /// `dst = left op right`
    Binary { op: TackyBinaryOp, left: TackyVal, right: TackyVal, dst: TackyVal },
    /// `dst = src`
    Copy { src: TackyVal, dst: TackyVal },
    /// `goto target`
    Jump(String),
    /// `if condition == 0 goto target`
    JumpIfZero { condition: TackyVal, target: String },
    /// `if condition != 0 goto target`
    JumpIfNotZero { condition: TackyVal, target: String },
    /// `target:`
    Label(String),
    /// `dst = name(args...)`
    FunCall { name: String, args: Vec<TackyVal>, dst: TackyVal, dst_type: Type },
    /// `dst = sign_extend(src)`
    SignExtend { src: TackyVal, dst: TackyVal },
    /// `dst = zero_extend(src)`
    ZeroExtend { src: TackyVal, dst: TackyVal },
    /// `dst = truncate(src)`
    Truncate { src: TackyVal, dst: TackyVal },
    /// `dst = int_to_double(src)`
    IntToDouble { src: TackyVal, dst: TackyVal },
    /// `dst = double_to_int(src)`
    DoubleToInt { src: TackyVal, dst: TackyVal },
    /// `dst = uint_to_double(src)`
    UIntToDouble { src: TackyVal, dst: TackyVal },
    /// `dst = double_to_uint(src)`
    DoubleToUInt { src: TackyVal, dst: TackyVal },
    /// `dst = &src`
    GetAddress { src: TackyVal, dst: TackyVal },
    /// `dst = *src_ptr`
    Load { src_ptr: TackyVal, dst: TackyVal },
    /// `*dst_ptr = src`
    Store { src: TackyVal, dst_ptr: TackyVal },
    /// `dst = ptr + index * scale`
    AddPtr { ptr: TackyVal, index: TackyVal, scale: usize, dst: TackyVal },
    /// `dst[offset..] = src` (構造体メンバへのコピー)
    CopyToOffset { src: TackyVal, dst: String, offset: usize },
    /// `dst = src[offset..]` (構造体メンバからのコピー)
    CopyFromOffset { src: String, offset: usize, dst: TackyVal },
    /// `dst = memcpy(src, size)` (構造体全体のコピー)
    CopyStruct { src: TackyVal, dst: TackyVal, size: usize },
    /// `jmp *target` — 間接ジャンプ（難読化の CFF ジャンプテーブル用）
    /// `possible_targets` はジャンプテーブルの全エントリ。
    /// コード生成時は `jmp *target` のみ出力するが、レジスタ割り当ての
    /// 生存解析で正しい後続ブロック情報を提供するために使う。
    JumpIndirect { target: TackyVal, possible_targets: Vec<String> },
}

/// TACKY 値（定数または変数）
#[derive(Debug, Clone, PartialEq)]
pub enum TackyVal {
    Constant(TackyConst),
    Var(String),
}

/// TACKY 定数
#[derive(Debug, Clone, PartialEq)]
#[allow(dead_code)]
pub enum TackyConst {
    Int(i32),
    Long(i64),
    UInt(u32),
    ULong(u64),
    Double(f64),
    Char(i8),
    UChar(u8),
}

/// TACKY 単項演算子
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TackyUnaryOp {
    /// `~` — ビット反転
    Complement,
    /// `-` — 算術否定
    Negate,
    /// `!` — 論理否定
    Not,
}

/// TACKY 二項演算子
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TackyBinaryOp {
    // 整数演算
    Add,
    Subtract,
    Multiply,
    Divide,
    Remainder,
    // 比較
    Equal,
    NotEqual,
    LessThan,
    LessOrEqual,
    GreaterThan,
    GreaterOrEqual,
    // Double 演算
    AddDouble,
    SubDouble,
    MulDouble,
    DivDouble,
}

/// 静的変数
#[derive(Debug, Clone)]
pub struct TackyStaticVar {
    pub name: String,
    pub global: bool,
    pub var_type: Type,
    pub init: TackyStaticInit,
}

/// 静的初期値
#[derive(Debug, Clone, PartialEq)]
pub enum TackyStaticInit {
    IntInit(i64),
    DoubleInit(f64),
    ZeroInit(usize),
    StringInit(String, usize),
    /// バイト配列初期値（難読化パスの文字列暗号化で使用）。
    /// StringInit を加算暗号化した結果を `.data` セクションに配置する。
    ByteArrayInit(Vec<u8>),
    /// ポインタ配列初期値（難読化の CFF ジャンプテーブル用）。
    /// ラベルアドレスの配列を `.data` セクションに `.quad label0, label1, ...` として配置する。
    PointerArrayInit(Vec<String>),
}

/// 読み取り専用の静的定数
#[derive(Debug, Clone)]
pub struct TackyStaticConstant {
    pub name: String,
    pub alignment: usize,
    pub init: TackyStaticInit,
}
