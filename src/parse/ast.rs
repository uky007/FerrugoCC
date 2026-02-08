//! C言語の抽象構文木（AST）定義
//!
//! パーサーが構築する木構造を定義する。
//! 各ノードはCの文法要素に対応し、ソースの構造を忠実に表現する。
//!
//! # 現在サポートする文法（Chapter 7）
//! ```text
//! <program>        ::= <function>
//! <function>       ::= "int" <identifier> "(" "void" ")" "{" <block_item>* "}"
//! <block_item>     ::= <statement> | <declaration>
//! <declaration>    ::= "int" <identifier> ("=" <assignment>)? ";"
//! <statement>      ::= "return" <exp> ";"
//!                    | <exp> ";"
//!                    | ";"
//!                    | "if" "(" <exp> ")" <statement> ("else" <statement>)?
//!                    | "{" <block_item>* "}"
//! <exp>            ::= <assignment> ("," <assignment>)*       ← Ch7: カンマ演算子
//! <assignment>     ::= <identifier> <assign_op> <assignment>  ← Ch7: 複合代入
//!                    | <conditional>
//! <assign_op>      ::= "=" | "+=" | "-=" | "*=" | "/=" | "%="
//! <conditional>    ::= <logical_or> ("?" <exp> ":" <conditional>)?
//! <logical_or>     ::= <logical_and> ( "||" <logical_and> )*
//! <logical_and>    ::= <equality> ( "&&" <equality> )*
//! <equality>       ::= <relational> ( ("==" | "!=") <relational> )*
//! <relational>     ::= <additive> ( ("<" | "<=" | ">" | ">=") <additive> )*
//! <additive>       ::= <multiplicative> ( ("+" | "-") <multiplicative> )*
//! <multiplicative> ::= <unary> ( ("*" | "/" | "%") <unary> )*
//! <unary>          ::= <unary_op> <unary> | <postfix>         ← Ch7: postfix呼び出し
//! <unary_op>       ::= "-" | "~" | "!" | "++" | "--"          ← Ch7: 前置++/--
//! <postfix>        ::= <primary> ("++" | "--")*                ← Ch7: 後置++/--
//! <primary>        ::= <int> | <identifier> | "(" <exp> ")"
//! ```

/// プログラム全体。現時点では関数1つだけを持つ。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Program {
    pub function: Function,
}

/// 関数定義。名前と本体（ブロック要素のリスト）を持つ。
///
/// Chapter 5 で `body` が単一文から `Vec<BlockItem>` に変更された。
/// 戻り値の型は常に `int`、引数は常に `void`。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Function {
    pub name: String,
    pub body: Vec<BlockItem>,
}

/// ブロック要素（Chapter 5 で追加）。
///
/// 関数本体は文と宣言の列からなる。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BlockItem {
    /// 文（return文、式文、空文）
    Statement(Statement),
    /// 変数宣言
    Declaration(Declaration),
}

/// 変数宣言（Chapter 5 で追加）。
///
/// `int <name>;` または `int <name> = <expr>;`
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Declaration {
    pub name: String,
    pub init: Option<Expr>,
}

/// 文（Statement）。
///
/// Chapter 5 で式文と空文が追加された。
/// Chapter 6 で if文と複合文が追加された。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Statement {
    /// `return <expr>;` — 式の値を返して関数を終了する
    Return(Expr),
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
}

/// 式（Expression）。
///
/// 式は値を持つ構文要素。定数、単項演算、二項演算、変数参照、代入がある。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Expr {
    /// 整数定数（例: `42`）
    Constant(i64),
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
    /// 代入式（Chapter 5）。変数名と右辺の式を持つ（右結合）。
    ///
    /// 例: `a = 5` → `Assign("a", Constant(5))`
    /// 代入は式として値を返す（代入された値）。
    Assign(String, Box<Expr>),
    /// 三項演算子（Chapter 6）。`cond ? then_expr : else_expr`（右結合）。
    Conditional {
        condition: Box<Expr>,
        then_expr: Box<Expr>,
        else_expr: Box<Expr>,
    },
    /// 複合代入式（Chapter 7）。`a += 5` → `CompoundAssign(Add, "a", Constant(5))`
    CompoundAssign(BinaryOp, String, Box<Expr>),
    /// 後置インクリメント（Chapter 7）。`a++` — 旧値を返す。
    PostfixIncrement(String),
    /// 後置デクリメント（Chapter 7）。`a--` — 旧値を返す。
    PostfixDecrement(String),
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
