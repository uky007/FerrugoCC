//! C言語の抽象構文木（AST）定義
//!
//! パーサーが構築する木構造を定義する。
//! 各ノードはCの文法要素に対応し、ソースの構造を忠実に表現する。
//!
//! # 現在サポートする文法（Chapter 18）
//! ```text
//! <program>        ::= <top_level_decl>*                      ← Ch10: 関数+変数
//! <top_level_decl> ::= <function_decl> | <variable_decl> | <struct_decl>
//! <function_decl>  ::= <storage_class>? <type> <declarator> "(" <params> ")" ( "{" <block>* "}" | ";" )
//! <variable_decl>  ::= <storage_class>? <type> <declarator> ("=" <initializer>)? ";"
//! <struct_decl>    ::= "struct" <identifier> "{" <member_decl>* "}" ";"  ← Ch18: 構造体定義のみ
//! <member_decl>    ::= <type> <declarator> ";"                ← Ch18: 構造体メンバ
//! <declarator>     ::= "*"* <identifier> ("[" <int> "]")?      ← Ch15: 配列宣言子
//! <type>           ::= <type_specifier>+                       ← Ch12: 任意順の型指定子
//! <type_specifier> ::= "int" | "long" | "signed" | "unsigned" | "double"
//!                    | "char" | "void"                         ← Ch16, Ch17
//!                    | "struct" <identifier> ("{" <member_decl>* "}")?  ← Ch18: 構造体型
//! <storage_class>  ::= "static" | "extern"
//! <params>         ::= "void" | <type> <declarator> ("," <type> <declarator>)*
//! <block_item>     ::= <statement> | <declaration>
//! <declaration>    ::= <storage_class>? <type> <declarator> ("=" <initializer>)? ";"
//! <initializer>    ::= <assignment> | "{" <assignment> ("," <assignment>)* ","? "}"  ← Ch18: 複合初期化子
//! <statement>      ::= "return" <exp> ";"
//!                    | <exp> ";"
//!                    | ";"
//!                    | "if" "(" <exp> ")" <statement> ("else" <statement>)?
//!                    | "{" <block_item>* "}"
//!                    | "while" "(" <exp> ")" <statement>
//!                    | "do" <statement> "while" "(" <exp> ")" ";"
//!                    | "for" "(" <for_init> <exp>? ";" <exp>? ")" <statement>
//!                    | "break" ";"
//!                    | "continue" ";"
//! <for_init>       ::= <declaration> | <exp>? ";"
//! <exp>            ::= <assignment> ("," <assignment>)*       ← Ch7: カンマ演算子
//! <assignment>     ::= <lvalue> <assign_op> <assignment>       ← Ch14: 左辺値を一般化
//!                    | <conditional>
//! <assign_op>      ::= "=" | "+=" | "-=" | "*=" | "/=" | "%="
//! <conditional>    ::= <logical_or> ("?" <exp> ":" <conditional>)?
//! <logical_or>     ::= <logical_and> ( "||" <logical_and> )*
//! <logical_and>    ::= <equality> ( "&&" <equality> )*
//! <equality>       ::= <relational> ( ("==" | "!=") <relational> )*
//! <relational>     ::= <additive> ( ("<" | "<=" | ">" | ">=") <additive> )*
//! <additive>       ::= <multiplicative> ( ("+" | "-") <multiplicative> )*
//! <multiplicative> ::= <cast> ( ("*" | "/" | "%") <cast> )*   ← Ch14: cast 挿入
//! <cast>           ::= "(" <type> <abstract_declarator> ")" <cast>  ← Ch14: キャスト式
//!                    | <unary>
//! <abstract_declarator> ::= "*"* ("[" <int> "]")?              ← Ch15: 配列サフィックス
//! <unary>          ::= <unary_op> <cast>                        ← Ch14: cast を呼ぶ
//!                    | "sizeof" <unary>                         ← Ch15: sizeof 式
//!                    | "sizeof" "(" <type> <abstract_declarator> ")"  ← Ch15: sizeof(型)
//!                    | <postfix>
//! <unary_op>       ::= "-" | "~" | "!" | "++" | "--" | "*" | "&"  ← Ch14: 間接参照・アドレス取得
//! <postfix>        ::= <primary> ("++" | "--" | "[" <exp> "]" | "." <id> | "->" <id>)*  ← Ch15, Ch18
//! <primary>        ::= <int> | <long> | <uint> | <ulong> | <double>
//!                    | <char> | <string>                       ← Ch16: 文字・文字列リテラル
//!                    | <identifier> ("(" <args>? ")")?         ← Ch9: 関数呼び出し
//!                    | "(" <exp> ")"
//! <int>            ::= [0-9]+                                  ← 整数リテラル
//! <long>           ::= [0-9]+ ("l" | "L")                     ← Ch11: long リテラル
//! <uint>           ::= [0-9]+ ("u" | "U")                     ← Ch12: unsigned int リテラル
//! <ulong>          ::= [0-9]+ ("ul" | "UL" | "lu" | "LU")    ← Ch12: unsigned long リテラル
//! <double>         ::= [0-9]*"."[0-9]+ | [0-9]+"."[0-9]* | ...  ← Ch13: 浮動小数点リテラル
//! <args>           ::= <assignment> ("," <assignment>)*        ← カンマ演算子と区別
//! ```

/// 型（Chapter 11, 12, 14, 16, 17, 18）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Type {
    /// `void` — 不完全型。関数の戻り値やキャスト先として使用（Chapter 17）
    Void,
    /// `char` / `signed char` — 1バイト符号付き整数（Chapter 16）
    Char,
    /// `unsigned char` — 1バイト符号なし整数（Chapter 16）
    UChar,
    /// `int` — 32ビット符号付き整数
    Int,
    /// `long` — 64ビット符号付き整数
    Long,
    /// `unsigned int` — 32ビット符号なし整数（Chapter 12）
    UInt,
    /// `unsigned long` — 64ビット符号なし整数（Chapter 12）
    ULong,
    /// `double` — IEEE 754 倍精度浮動小数点（Chapter 13）
    Double,
    /// ポインタ型（Chapter 14）。`int *` → `Pointer(Box::new(Int))`
    Pointer(Box<Type>),
    /// 配列型（Chapter 15）。`int[10]` → `Array(Box::new(Int), 10)`
    Array(Box<Type>, usize),
    /// 構造体型（Chapter 18）。tag 名とメンバリストを保持。
    /// members が空の場合は前方宣言（不完全型）。
    Struct { tag: String, members: Vec<MemberDecl> },
}

/// 構造体メンバ宣言（Chapter 18）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemberDecl {
    pub name: String,
    pub member_type: Type,
}

impl Type {
    /// void 型かどうかを判定する（Chapter 17）。
    pub fn is_void(&self) -> bool {
        matches!(self, Type::Void)
    }

    /// 構造体型かどうかを判定する（Chapter 18）。
    pub fn is_struct(&self) -> bool {
        matches!(self, Type::Struct { .. })
    }

    /// 不完全型かどうかを判定する（Chapter 17, 18）。
    /// void は不完全型。前方宣言のみの構造体（members が空）も不完全型。
    pub fn is_incomplete(&self) -> bool {
        match self {
            Type::Void => true,
            Type::Struct { members, .. } => members.is_empty(),
            _ => false,
        }
    }

    /// 符号なし型かどうかを判定する。
    pub fn is_unsigned(&self) -> bool {
        matches!(self, Type::UInt | Type::ULong | Type::UChar)
    }

    /// 文字型かどうかを判定する（Chapter 16）。
    pub fn is_character(&self) -> bool {
        matches!(self, Type::Char | Type::UChar)
    }

    /// 浮動小数点型かどうかを判定する（Chapter 13）。
    pub fn is_double(&self) -> bool {
        matches!(self, Type::Double)
    }

    /// ポインタ型かどうかを判定する（Chapter 14）。
    pub fn is_pointer(&self) -> bool {
        matches!(self, Type::Pointer(_))
    }

    /// 配列型かどうかを判定する（Chapter 15）。
    pub fn is_array(&self) -> bool {
        matches!(self, Type::Array(_, _))
    }

    /// ポインタまたは配列の指す先の型を返す（Chapter 15）。
    pub fn target_type(&self) -> Option<&Type> {
        match self {
            Type::Pointer(t) | Type::Array(t, _) => Some(t),
            _ => None,
        }
    }

    /// 型のバイトサイズを返す。
    /// void は不完全型のためサイズを持たない（呼び出し前にチェックすること）。
    pub fn size(&self) -> usize {
        match self {
            Type::Void => panic!("void has no size"),
            Type::Char | Type::UChar => 1,
            Type::Int | Type::UInt => 4,
            Type::Long | Type::ULong | Type::Double | Type::Pointer(_) => 8,
            Type::Array(elem, count) => elem.size() * count,
            Type::Struct { members, tag } => {
                if members.is_empty() {
                    panic!("incomplete struct '{}' has no size", tag);
                }
                struct_size(members)
            }
        }
    }

    /// 型のアラインメントを返す（Chapter 18）。
    pub fn alignment(&self) -> usize {
        match self {
            Type::Void => panic!("void has no alignment"),
            Type::Char | Type::UChar => 1,
            Type::Int | Type::UInt => 4,
            Type::Long | Type::ULong | Type::Double | Type::Pointer(_) => 8,
            Type::Array(elem, _) => elem.alignment(),
            Type::Struct { members, tag } => {
                if members.is_empty() {
                    panic!("incomplete struct '{}' has no alignment", tag);
                }
                struct_alignment(members)
            }
        }
    }
}

/// 構造体のアラインメントを計算する（最大メンバアラインメント）。
pub fn struct_alignment(members: &[MemberDecl]) -> usize {
    members.iter().map(|m| m.member_type.alignment()).max().unwrap_or(1)
}

/// 構造体の全体サイズを計算する（パディング込み）。
pub fn struct_size(members: &[MemberDecl]) -> usize {
    let mut offset = 0;
    for m in members {
        let align = m.member_type.alignment();
        // メンバのアラインメントに合わせる
        if offset % align != 0 {
            offset += align - (offset % align);
        }
        offset += m.member_type.size();
    }
    // 全体サイズを構造体アラインメントの倍数に切り上げ
    let struct_align = struct_alignment(members);
    if offset % struct_align != 0 {
        offset += struct_align - (offset % struct_align);
    }
    offset
}

/// 構造体メンバのオフセットと型を取得する。
pub fn struct_member_offset(members: &[MemberDecl], name: &str) -> Option<(usize, Type)> {
    let mut offset = 0;
    for m in members {
        let align = m.member_type.alignment();
        if offset % align != 0 {
            offset += align - (offset % align);
        }
        if m.name == name {
            return Some((offset, m.member_type.clone()));
        }
        offset += m.member_type.size();
    }
    None
}

/// ストレージクラス指定子（Chapter 10）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StorageClass {
    /// `static` — 静的ストレージ / 内部リンケージ
    Static,
    /// `extern` — 外部リンケージ
    Extern,
}

/// トップレベル宣言（Chapter 10）。関数宣言/定義または変数宣言。
#[derive(Debug, Clone, PartialEq)]
pub enum TopLevelDecl {
    /// 関数宣言/定義
    Function(FunctionDecl),
    /// 変数宣言
    Variable(Declaration),
}

/// プログラム全体。トップレベル宣言の列を持つ（Chapter 10）。
#[derive(Debug, Clone, PartialEq)]
pub struct Program {
    pub declarations: Vec<TopLevelDecl>,
}

/// 関数宣言/定義（Chapter 9, 10, 11）。
///
/// - `return_type`: 戻り値の型（Chapter 11）
/// - `params`: パラメータの (型, 名前) のリスト（Chapter 11）
/// - `body`: `Some(...)` なら関数定義、`None` なら前方宣言（プロトタイプ）
/// - `storage_class`: オプショナルのストレージクラス指定子（Chapter 10）
#[derive(Debug, Clone, PartialEq)]
pub struct FunctionDecl {
    pub name: String,
    pub return_type: Type,
    pub params: Vec<(Type, String)>,
    pub body: Option<Vec<BlockItem>>,
    pub storage_class: Option<StorageClass>,
}

/// ブロック要素（Chapter 5 で追加）。
///
/// 関数本体は文と宣言の列からなる。
#[derive(Debug, Clone, PartialEq)]
pub enum BlockItem {
    /// 文（return文、式文、空文）
    Statement(Statement),
    /// 変数宣言
    Declaration(Declaration),
}

/// 変数宣言（Chapter 5, 10, 11 で拡張）。
///
/// `int <name>;` または `long <name> = <expr>;`
/// Chapter 10: オプショナルのストレージクラス指定子を追加。
/// Chapter 11: 変数の型フィールドを追加。
#[derive(Debug, Clone, PartialEq)]
pub struct Declaration {
    pub name: String,
    pub var_type: Type,
    pub init: Option<Expr>,
    pub storage_class: Option<StorageClass>,
}

/// 文（Statement）。
///
/// Chapter 5 で式文と空文が追加された。
/// Chapter 6 で if文と複合文が追加された。
/// Chapter 8 で while, do-while, for, break, continue が追加された。
#[derive(Debug, Clone, PartialEq)]
pub enum Statement {
    /// `return <expr>;` or `return;` — 式の値を返して関数を終了する（Chapter 17: void 関数で値なし return）
    Return(Option<Expr>),
    /// `<expr>;` — 式文（副作用のために式を評価する）
    Expression(Expr),
    /// `;` — 空文（何もしない）
    Null,
    /// `if (<cond>) <then> [else <else>]` — 条件分岐（Chapter 6）
    If {
        condition: Expr,
        then_branch: Box<Statement>,
        else_branch: Option<Box<Statement>>,
    },
    /// `{ <block_item>* }` — 複合文（Chapter 6）。スコープを導入する。
    Compound(Vec<BlockItem>),
    /// `while (<cond>) <body>` — whileループ（Chapter 8）
    While {
        condition: Expr,
        body: Box<Statement>,
    },
    /// `do <body> while (<cond>);` — do-whileループ（Chapter 8）
    DoWhile {
        body: Box<Statement>,
        condition: Expr,
    },
    /// `for (<init> <cond>? ; <post>?) <body>` — forループ（Chapter 8）
    For {
        init: ForInit,
        condition: Option<Expr>,
        post: Option<Expr>,
        body: Box<Statement>,
    },
    /// `break;` — ループ脱出（Chapter 8）
    Break,
    /// `continue;` — ループ継続（Chapter 8）
    Continue,
}

/// forループの初期化部（Chapter 8）。
#[derive(Debug, Clone, PartialEq)]
pub enum ForInit {
    /// 宣言（例: `int i = 0;`）
    Declaration(Declaration),
    /// 式（例: `i = 0;`）または空（`;`）
    Expression(Option<Expr>),
}

/// 式（Expression）。
///
/// 式は値を持つ構文要素。定数、単項演算、二項演算、変数参照、代入がある。
#[derive(Debug, Clone, PartialEq)]
pub enum Expr {
    /// 整数定数（例: `42`）
    Constant(i64),
    /// long 整数定数（例: `42L`）（Chapter 11）
    ConstantLong(i64),
    /// unsigned int 定数（例: `42U`）（Chapter 12）
    ConstantUInt(u64),
    /// unsigned long 定数（例: `42UL`）（Chapter 12）
    ConstantULong(u64),
    /// 浮動小数点定数（例: `3.14`）（Chapter 13）
    ConstantDouble(f64),
    /// 型変換（Chapter 11, 12）。型検査パスが挿入する。
    /// `source_type` はコード生成で符号付き/なし拡張を区別するために使う。
    Cast { target_type: Type, source_type: Type, expr: Box<Expr> },
    /// 単項演算（Chapter 2）。演算子と被演算子を持つ。
    ///
    /// 例: `-5` → `Unary(Negate, Constant(5))`
    /// 例: `~(-3)` → `Unary(Complement, Unary(Negate, Constant(3)))`
    Unary(UnaryOp, Box<Expr>),
    /// 二項演算（Chapter 3）。左辺、演算子、右辺を持つ。
    ///
    /// 例: `1 + 2` → `Binary(Add, Constant(1), Constant(2))`
    Binary(BinaryOp, Box<Expr>, Box<Expr>),
    /// 変数参照（Chapter 5）。変数名を持つ。
    ///
    /// 例: `a` → `Var("a")`
    Var(String),
    /// 代入式（Chapter 5, 14で一般化）。左辺値と右辺の式を持つ（右結合）。
    ///
    /// 例: `a = 5` → `Assign(Box::new(Var("a")), Constant(5))`
    /// 例: `*ptr = 5` → `Assign(Box::new(Dereref(ptr)), Constant(5))`
    Assign(Box<Expr>, Box<Expr>),
    /// 三項演算子（Chapter 6）。`cond ? then_expr : else_expr`（右結合）。
    Conditional {
        condition: Box<Expr>,
        then_expr: Box<Expr>,
        else_expr: Box<Expr>,
    },
    /// 複合代入式（Chapter 7, 14で一般化）。`a += 5` → `CompoundAssign(Add, Box::new(Var("a")), Constant(5))`
    CompoundAssign(BinaryOp, Box<Expr>, Box<Expr>),
    /// 後置インクリメント（Chapter 7, 14で一般化）。`a++` — 旧値を返す。
    PostfixIncrement(Box<Expr>),
    /// 後置デクリメント（Chapter 7, 14で一般化）。`a--` — 旧値を返す。
    PostfixDecrement(Box<Expr>),
    /// 関数呼び出し（Chapter 9）。`foo(a, b)` → `FunctionCall("foo", vec![a, b])`
    FunctionCall(String, Vec<Expr>),
    /// 間接参照（Chapter 14）。`*expr` — ポインタの指す先の値を取得。左辺値にもなる。
    Dereref(Box<Expr>),
    /// アドレス取得（Chapter 14）。`&expr` — 式のアドレスを取得。
    AddrOf(Box<Expr>),
    /// sizeof(型)（Chapter 15）。型チェッカーで ConstantULong に置換される。
    SizeOfType(Type),
    /// sizeof 式（Chapter 15）。型チェッカーで ConstantULong に置換される。
    SizeOfExpr(Box<Expr>),
    /// 文字列リテラル（Chapter 16）。コード生成で .rodata ラベルに変換される。
    StringLiteral(String),
    /// メンバアクセス（Chapter 18）。`expr.member`。
    /// `ptr->member` はパーサーで `Dot(Dereref(ptr), member)` に脱糖される。
    Dot(Box<Expr>, String),
    /// 複合初期化子（Chapter 18）。`{ expr, expr, ... }`
    CompoundInit(Vec<Expr>),
}

/// 単項演算子の種類（Chapter 2）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnaryOp {
    /// `-` — 算術否定（2の補数で符号反転）
    Negate,
    /// `~` — ビット反転（全ビットを反転する）
    Complement,
    /// `!` — 論理否定（0 なら 1、非0 なら 0）
    Not,
    /// `++var` — 前置インクリメント（Chapter 7）。新値を返す。
    PreIncrement,
    /// `--var` — 前置デクリメント（Chapter 7）。新値を返す。
    PreDecrement,
}

/// 二項演算子の種類（Chapter 3-4）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BinaryOp {
    /// `+` — 加算
    Add,
    /// `-` — 減算
    Subtract,
    /// `*` — 乗算
    Multiply,
    /// `/` — 除算（整数除算）
    Divide,
    /// `%` — 剰余
    Remainder,
    // ── Chapter 4 で追加 ──
    /// `<` — 小なり
    LessThan,
    /// `<=` — 小なりイコール
    LessEqual,
    /// `>` — 大なり
    GreaterThan,
    /// `>=` — 大なりイコール
    GreaterEqual,
    /// `==` — 等価
    Equal,
    /// `!=` — 非等価
    NotEqual,
    /// `&&` — 論理AND（短絡評価）
    LogicalAnd,
    /// `||` — 論理OR（短絡評価）
    LogicalOr,
    /// `,` — カンマ演算子（Chapter 7）。左辺を評価して捨て、右辺の値を返す。
    Comma,
}
