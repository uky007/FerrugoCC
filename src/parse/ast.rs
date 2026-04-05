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
//! <variable_decl>  ::= <storage_class>? <type> <declarator> ("=" <initializer>)? ("," <declarator> ("=" <initializer>)?)* ";"
//! <struct_decl>    ::= "struct" <identifier> "{" <member_decl>* "}" ";"  ← Ch18: 構造体定義のみ
//! <member_decl>    ::= <type> <declarator> ";"                ← Ch18: 構造体メンバ
//! <declarator>     ::= "*"* <identifier> ("[" <int> "]")?      ← Ch15: 配列宣言子
//! <type>           ::= <type_specifier>+                       ← Ch12: 任意順の型指定子
//! <type_specifier> ::= "int" | "long" | "signed" | "unsigned" | "double"
//!                    | "char" | "void"                         ← Ch16, Ch17
//!                    | "struct" <identifier> ("{" <member_decl>* "}")?  ← Ch18: 構造体型
//! <storage_class>  ::= "static" | "extern"
//! <params>         ::= "void" | <type> <declarator> ("," <type> <declarator>)* ("," "...")?
//! <block_item>     ::= <statement> | <declaration>
//! <declaration>    ::= <storage_class>? <type> <declarator> ("=" <initializer>)? ("," <declarator> ("=" <initializer>)?)* ";"
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
///
/// Struct variant の等価比較は tag 名のみで行う（C セマンティクス:
/// 同一タグ名の struct は前方宣言と定義で同一型）。
#[derive(Debug, Clone, Eq)]
pub enum Type {
    /// `void` — 不完全型。関数の戻り値やキャスト先として使用（Chapter 17）
    Void,
    /// `char` / `signed char` — 1バイト符号付き整数（Chapter 16）
    Char,
    /// `unsigned char` — 1バイト符号なし整数（Chapter 16）
    UChar,
    /// `short` — 16ビット符号付き整数
    Short,
    /// `unsigned short` — 16ビット符号なし整数
    UShort,
    /// `int` — 32ビット符号付き整数
    Int,
    /// `long` — 64ビット符号付き整数
    Long,
    /// `unsigned int` — 32ビット符号なし整数（Chapter 12）
    UInt,
    /// `unsigned long` — 64ビット符号なし整数（Chapter 12）
    ULong,
    /// `float` — IEEE 754 単精度浮動小数点（32ビット）
    Float,
    /// `double` — IEEE 754 倍精度浮動小数点（Chapter 13）
    Double,
    /// ポインタ型（Chapter 14）。`int *` → `Pointer(Box::new(Int))`
    Pointer(Box<Type>),
    /// 配列型（Chapter 15）。`int[10]` → `Array(Box::new(Int), 10)`
    Array(Box<Type>, usize),
    /// 構造体/共用体型（Chapter 18）。tag 名とメンバリストを保持。
    /// members が空の場合は前方宣言（不完全型）。
    /// is_union: true の場合、全メンバが offset 0 に配置される。
    Struct {
        tag: String,
        members: Vec<MemberDecl>,
        is_union: bool,
    },
    /// `va_list` 型 — 可変長引数アクセス用の24バイト構造体（System V AMD64 ABI）
    VaList,
    /// 関数型 — typedef の基底型や関数ポインタの指す先として使用。
    /// 変数型としては不可（値を格納できない）。typedef 定義時はそのまま保持し、
    /// 変数宣言・パラメータ宣言で使われた時点で Pointer(Function) に decay する。
    ///
    /// - `param_types: None` — 引数未指定 `()`: 任意個の引数を受け付ける（K&R 互換）
    /// - `param_types: Some(vec![])` — 引数ゼロ `(void)`: 引数なしが確定
    /// - `param_types: Some(vec![Int, Double])` — 具体的なプロトタイプ
    Function {
        return_type: Box<Type>,
        param_types: Option<Vec<Type>>,
        is_variadic: bool,
    },
}

/// Struct variant はタグ名のみで等価比較する（C の名前的同一性）。
impl PartialEq for Type {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Type::Void, Type::Void) => true,
            (Type::Char, Type::Char) => true,
            (Type::UChar, Type::UChar) => true,
            (Type::Short, Type::Short) => true,
            (Type::UShort, Type::UShort) => true,
            (Type::Int, Type::Int) => true,
            (Type::Long, Type::Long) => true,
            (Type::UInt, Type::UInt) => true,
            (Type::ULong, Type::ULong) => true,
            (Type::Float, Type::Float) => true,
            (Type::Double, Type::Double) => true,
            (Type::VaList, Type::VaList) => true,
            (Type::Pointer(a), Type::Pointer(b)) => a == b,
            (Type::Array(a, sa), Type::Array(b, sb)) => a == b && sa == sb,
            (Type::Struct { tag: ta, .. }, Type::Struct { tag: tb, .. }) => ta == tb, // nominal
            (
                Type::Function {
                    return_type: ra,
                    param_types: pa,
                    is_variadic: va,
                },
                Type::Function {
                    return_type: rb,
                    param_types: pb,
                    is_variadic: vb,
                },
            ) => ra == rb && pa == pb && va == vb,
            _ => false,
        }
    }
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
    /// void、前方宣言のみの構造体（members が空）が該当。
    /// 注意: Function は不完全型ではなく「非オブジェクト型」。
    /// 変数型として禁止したい場合は `is_object_type()` を使う。
    pub fn is_incomplete(&self) -> bool {
        match self {
            Type::Void => true,
            Type::Struct { members, .. } => members.is_empty(),
            _ => false,
        }
    }

    /// オブジェクト型かどうかを判定する。
    /// 変数の型として使用可能な型のみ true を返す。
    /// Function 型は typedef の基底型としては合法だが、変数型としては不可。
    pub fn is_object_type(&self) -> bool {
        !matches!(self, Type::Void | Type::Function { .. }) && !self.is_incomplete()
    }

    /// va_list 型かどうかを判定する。
    pub fn is_va_list(&self) -> bool {
        matches!(self, Type::VaList)
    }

    /// 符号なし型かどうかを判定する。
    pub fn is_unsigned(&self) -> bool {
        matches!(self, Type::UInt | Type::ULong | Type::UChar | Type::UShort)
    }

    /// 文字型かどうかを判定する（Chapter 16）。
    pub fn is_character(&self) -> bool {
        matches!(self, Type::Char | Type::UChar)
    }

    /// short 型かどうかを判定する。
    pub fn is_short(&self) -> bool {
        matches!(self, Type::Short | Type::UShort)
    }

    /// 浮動小数点型かどうかを判定する（Chapter 13）。
    pub fn is_float(&self) -> bool {
        matches!(self, Type::Float)
    }

    pub fn is_double(&self) -> bool {
        matches!(self, Type::Double)
    }

    /// float または double かどうか
    pub fn is_floating(&self) -> bool {
        matches!(self, Type::Float | Type::Double)
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
            Type::Function { .. } => panic!("function type has no size"),
            Type::Char | Type::UChar => 1,
            Type::Short | Type::UShort => 2,
            Type::Int | Type::UInt | Type::Float => 4,
            Type::Long | Type::ULong | Type::Double | Type::Pointer(_) => 8,
            Type::Array(elem, count) => elem.size() * count,
            Type::Struct {
                members,
                tag,
                is_union,
            } => {
                if members.is_empty() {
                    panic!("incomplete struct '{}' has no size", tag);
                }
                if *is_union {
                    union_size(members)
                } else {
                    struct_size(members)
                }
            }
            Type::VaList => 24,
        }
    }

    /// 型のアラインメントを返す（Chapter 18）。
    pub fn alignment(&self) -> usize {
        match self {
            Type::Void => panic!("void has no alignment"),
            Type::Function { .. } => panic!("function type has no alignment"),
            Type::Char | Type::UChar => 1,
            Type::Short | Type::UShort => 2,
            Type::Int | Type::UInt | Type::Float => 4,
            Type::Long | Type::ULong | Type::Double | Type::Pointer(_) => 8,
            Type::Array(elem, _) => elem.alignment(),
            Type::Struct { members, tag, .. } => {
                if members.is_empty() {
                    panic!("incomplete struct '{}' has no alignment", tag);
                }
                struct_alignment(members)
            }
            Type::VaList => 8,
        }
    }
}

/// 構造体のアラインメントを計算する（最大メンバアラインメント）。
pub fn struct_alignment(members: &[MemberDecl]) -> usize {
    members
        .iter()
        .map(|m| m.member_type.alignment())
        .max()
        .unwrap_or(1)
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

/// 共用体の全体サイズを計算する（最大メンバサイズ、アラインメント切り上げ）。
pub fn union_size(members: &[MemberDecl]) -> usize {
    let max_member = members
        .iter()
        .map(|m| m.member_type.size())
        .max()
        .unwrap_or(0);
    let align = struct_alignment(members);
    if max_member % align != 0 {
        max_member + align - (max_member % align)
    } else {
        max_member
    }
}

/// 構造体/共用体メンバのオフセットと型を取得する。
/// is_union が true の場合、全メンバが offset 0。
pub fn struct_member_offset_ex(
    members: &[MemberDecl],
    name: &str,
    is_union: bool,
) -> Option<(usize, Type)> {
    if is_union {
        // union: 全メンバ offset 0
        for m in members {
            if m.name == name {
                return Some((0, m.member_type.clone()));
            }
        }
        None
    } else {
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
}

/// 構造体が flexible array member (C99) を持つか判定する。
/// 最後のメンバが `Array(_, 0)` であれば FAM とみなす。
pub fn has_fam(members: &[MemberDecl]) -> bool {
    members
        .last()
        .is_some_and(|m| matches!(&m.member_type, Type::Array(_, 0)))
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
    /// typedef 宣言（パース時に型名テーブルに登録済み。コード生成不要）
    Typedef { name: String, underlying_type: Type },
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
/// - `is_variadic`: 可変長引数関数かどうか（`...` を含む）
/// - `has_prototype`: プロトタイプの有無。`(void)` や `(int x)` は true、`()` は false。
///   `()` は「引数未指定」（K&R 互換）で、任意の引数で呼べる。
#[derive(Debug, Clone, PartialEq)]
pub struct FunctionDecl {
    pub name: String,
    pub return_type: Type,
    pub params: Vec<(Type, String)>,
    pub body: Option<Vec<BlockItem>>,
    pub storage_class: Option<StorageClass>,
    pub is_variadic: bool,
    pub has_prototype: bool,
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
    /// ブロック内 typedef 宣言（パース時に型名テーブルに登録済み。コード生成不要）
    Typedef { name: String, underlying_type: Type },
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
        init: Box<ForInit>,
        condition: Option<Expr>,
        post: Option<Expr>,
        body: Box<Statement>,
    },
    /// `break;` — ループ脱出（Chapter 8）
    Break,
    /// `continue;` — ループ継続（Chapter 8）
    Continue,
    /// `switch (expr) body` — 多分岐文
    Switch { expr: Expr, body: Box<Statement> },
    /// `case value: body` — switch のケースラベル
    Case { value: i64, body: Box<Statement> },
    /// `default: body` — switch のデフォルトラベル
    Default(Box<Statement>),
    /// `goto label;` — ラベルへの無条件ジャンプ
    Goto(String),
    /// `label: stmt` — ラベル付き文
    Label { name: String, body: Box<Statement> },
}

/// forループの初期化部（Chapter 8）。
#[derive(Debug, Clone, PartialEq)]
pub enum ForInit {
    /// 宣言（例: `int i = 0;` or `int i = 0, j = 1;`）
    Declaration(Vec<Declaration>),
    /// 式（例: `i = 0;`）または空（`;`）
    Expression(Option<Expr>),
}

/// 式（Expression）。
///
/// 式は値を持つ構文要素。定数、単項演算、二項演算、変数参照、代入がある。
#[derive(Debug, Clone, PartialEq)]
#[allow(clippy::enum_variant_names)]
pub enum Expr {
    /// 整数定数（例: `42`）
    Constant(i64),
    /// long 整数定数（例: `42L`）（Chapter 11）
    ConstantLong(i64),
    /// unsigned int 定数（例: `42U`）（Chapter 12）
    ConstantUInt(u64),
    /// unsigned long 定数（例: `42UL`）（Chapter 12）
    ConstantULong(u64),
    /// 倍精度浮動小数点定数（例: `3.14`）（Chapter 13）
    ConstantDouble(f64),
    /// 単精度浮動小数点定数（例: `3.14f`）
    ConstantFloat(f32),
    /// 型変換（Chapter 11, 12）。型検査パスが挿入する。
    /// `source_type` はコード生成で符号付き/なし拡張を区別するために使う。
    Cast {
        target_type: Type,
        source_type: Type,
        expr: Box<Expr>,
    },
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
    /// 間接呼び出し（式経由）。`expr(a, b)` — 関数ポインタ配列やメンバ経由の呼び出し。
    /// 例: `ops[0](30, 20)`, `s.callback(x)`
    CallExpr(Box<Expr>, Vec<Expr>),
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
    /// Compound literal — `(type){ init_list }`
    CompoundLiteral { target_type: Type, init: Box<Expr> },
    /// `va_start(ap)` — va_list の初期化
    VaStart(Box<Expr>),
    /// `va_arg(ap, type)` — va_list から次の引数を取得
    VaArg { ap: Box<Expr>, arg_type: Type },
    /// `va_end(ap)` — va_list の終了処理（no-op）
    VaEnd(Box<Expr>),
    /// `va_copy(dst, src)` — va_list のコピー
    VaCopy(Box<Expr>, Box<Expr>),
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
    // ── ビット演算 ──
    /// `&` — ビットAND
    BitwiseAnd,
    /// `|` — ビットOR
    BitwiseOr,
    /// `^` — ビットXOR
    BitwiseXor,
    /// `<<` — 左シフト
    ShiftLeft,
    /// `>>` — 右シフト
    ShiftRight,
    /// `,` — カンマ演算子（Chapter 7）。左辺を評価して捨て、右辺の値を返す。
    Comma,
}
