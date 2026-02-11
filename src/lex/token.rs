//! トークンの定義
//!
//! 字句解析（Lexer）が出力するトークンの型を定義する。
//! ソースコードの各要素（キーワード、識別子、リテラル、記号）は
//! `TokenKind` 列挙型のバリアントとして表現される。
//!
//! # 構造
//! - `TokenKind` — トークンの種類（値を持つものもある）
//! - `Span` — ソースコード中の位置情報（エラー報告用）
//! - `Token` — `TokenKind` と `Span` の組

/// トークンの種類。
///
/// Cソースコードの各字句要素に対応する。
/// リテラルや識別子はデータを保持し、キーワードや記号は単純なバリアント。
#[derive(Debug, Clone, PartialEq)]
pub enum TokenKind {
    // ── リテラル ──
    /// 整数リテラル（例: `42`, `0`）。値そのものを保持する。
    IntLiteral(i64),
    /// long 整数リテラル（例: `42L`, `100l`）（Chapter 11）。
    LongLiteral(i64),
    /// unsigned int リテラル（例: `42U`, `100u`）（Chapter 12）。
    UIntLiteral(u64),
    /// unsigned long リテラル（例: `42UL`, `100ul`）（Chapter 12）。
    ULongLiteral(u64),
    /// 浮動小数点リテラル（例: `3.14`, `.5`, `2e10`）（Chapter 13）。
    DoubleLiteral(f64),
    /// 文字リテラル（例: `'a'`, `'\n'`）（Chapter 16）。値は signed char。
    CharLiteral(i8),
    /// 文字列リテラル（例: `"hello"`）（Chapter 16）。エスケープ解決済みの内容。
    StringLiteral(String),

    // ── 識別子 ──
    /// 識別子（例: `main`, `foo`）。名前の文字列を保持する。
    Identifier(String),

    // ── キーワード ──
    /// `int` キーワード — 関数の戻り値の型
    KwInt,
    /// `void` キーワード — 引数なしを示す
    KwVoid,
    /// `return` キーワード — 関数からの復帰
    KwReturn,
    /// `if` キーワード — 条件分岐（Chapter 6）
    KwIf,
    /// `else` キーワード — 条件分岐の偽側（Chapter 6）
    KwElse,
    /// `while` キーワード — whileループ（Chapter 8）
    KwWhile,
    /// `do` キーワード — do-whileループ（Chapter 8）
    KwDo,
    /// `for` キーワード — forループ（Chapter 8）
    KwFor,
    /// `break` キーワード — ループ脱出（Chapter 8）
    KwBreak,
    /// `continue` キーワード — ループ継続（Chapter 8）
    KwContinue,
    /// `static` キーワード — 静的ストレージ/内部リンケージ（Chapter 10）
    KwStatic,
    /// `extern` キーワード — 外部リンケージ（Chapter 10）
    KwExtern,
    /// `long` キーワード — 64ビット整数型（Chapter 11）
    KwLong,
    /// `unsigned` キーワード — 符号なし型（Chapter 12）
    KwUnsigned,
    /// `signed` キーワード — 符号付き型（Chapter 12）
    KwSigned,
    /// `double` キーワード — 倍精度浮動小数点型（Chapter 13）
    KwDouble,
    /// `sizeof` キーワード — 型/式のバイトサイズ取得（Chapter 15）
    KwSizeof,
    /// `char` キーワード — 文字型（Chapter 16）
    KwChar,

    // ── 演算子 ──（Chapter 2 で追加）
    /// `-` — 単項マイナス / 二項減算
    Minus,
    /// `~` — ビット反転（ビット単位の補数）
    Tilde,
    /// `!` — 論理否定
    Bang,

    // ── 演算子 ──（Chapter 3 で追加）
    /// `+` — 二項加算
    Plus,
    /// `*` — 二項乗算
    Star,
    /// `/` — 二項除算
    Slash,
    /// `%` — 二項剰余
    Percent,

    // ── 演算子 ──（Chapter 4 で追加）
    /// `<` — 小なり
    Less,
    /// `<=` — 小なりイコール
    LessEqual,
    /// `>` — 大なり
    Greater,
    /// `>=` — 大なりイコール
    GreaterEqual,
    /// `==` — 等価
    EqualEqual,
    /// `!=` — 非等価
    NotEqual,
    /// `&` — アドレス演算子（Chapter 14）
    Ampersand,
    /// `&&` — 論理AND
    AndAnd,
    /// `||` — 論理OR
    OrOr,

    // ── 演算子 ──（Chapter 5 で追加）
    /// `=` — 代入
    Assign,

    // ── 演算子 ──（Chapter 6 で追加）
    /// `?` — 三項演算子の条件部
    Question,
    /// `:` — 三項演算子の区切り
    Colon,

    // ── 演算子 ──（Chapter 7 で追加）
    /// `+=` — 複合加算代入
    PlusAssign,
    /// `-=` — 複合減算代入
    MinusAssign,
    /// `*=` — 複合乗算代入
    StarAssign,
    /// `/=` — 複合除算代入
    SlashAssign,
    /// `%=` — 複合剰余代入
    PercentAssign,
    /// `++` — インクリメント
    PlusPlus,
    /// `--` — デクリメント
    MinusMinus,
    /// `,` — カンマ演算子
    Comma,

    // ── 区切り記号 ──
    /// `(`
    OpenParen,
    /// `)`
    CloseParen,
    /// `{`
    OpenBrace,
    /// `}`
    CloseBrace,
    /// `;`
    Semicolon,
    /// `[` — 配列添字（Chapter 15）
    OpenBracket,
    /// `]` — 配列添字（Chapter 15）
    CloseBracket,
}

/// ソースコード中の位置情報。
///
/// エラーメッセージで「何行目の何列目」と報告するために使う。
/// `offset` はソース先頭からのバイト位置。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Span {
    /// ソース先頭からのバイトオフセット
    pub offset: usize,
    /// トークンのバイト長
    pub len: usize,
    /// 行番号（1始まり）
    pub line: usize,
    /// 列番号（1始まり）
    pub column: usize,
}

/// 字句解析の出力単位。トークンの種類と位置情報を持つ。
#[derive(Debug, Clone, PartialEq)]
pub struct Token {
    pub kind: TokenKind,
    pub span: Span,
}
