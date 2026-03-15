//! 再帰下降パーサー
//!
//! トークン列を先頭から順に消費しながら AST を構築する。
//! 「再帰下降 (recursive descent)」とは、文法の各非終端記号に対して
//! 1つの関数を用意し、必要に応じて再帰呼び出しする手法。
//!
//! # パーサーの構造
//! - `Parser` 構造体がトークン列と現在位置を保持
//! - `peek()` で先読み、`advance()` で消費、`expect()` で特定トークンを要求
//! - 各 `parse_*` メソッドが文法規則に対応
//!
//! # 対応する文法（Chapter 18）
//! ```text
//! <program>        ::= <top_level_decl>*
//! <top_level_decl> ::= <function_decl> | <variable_decl> | <struct_decl>
//! <function_decl>  ::= <storage_class>? <type> <declarator> "(" <params> ")" ( "{" <block>* "}" | ";" )
//! <variable_decl>  ::= <storage_class>? <type> <declarator> ("=" <initializer>)? ("," <declarator> ("=" <initializer>)?)* ";"
//! <struct_decl>    ::= "struct" <identifier> ("{" <member_decl>* "}")? ";"  ← Ch18
//! <member_decl>    ::= <type> <declarator> ";"                ← Ch18: 構造体メンバ
//! <declarator>     ::= "*"* <identifier> ("[" <int> "]")?      ← Ch15: 配列宣言子
//! <type>           ::= <type_specifier>+
//! <type_specifier> ::= "int" | "long" | "signed" | "unsigned" | "double"
//!                    | "char" | "void"                         ← Ch16, Ch17
//!                    | "struct" <identifier> ("{" <member_decl>* "}")?  ← Ch18
//! <storage_class>  ::= "static" | "extern"
//! <params>         ::= "void" | <type> <declarator> ("," <type> <declarator>)* ("," "...")?
//! <block_item>     ::= <statement> | <declaration>
//! <declaration>    ::= <storage_class>? <type> <declarator> ("=" <initializer>)? ("," <declarator> ("=" <initializer>)?)* ";"
//! <initializer>    ::= <assignment> | "{" <assignment> ("," <assignment>)* ","? "}"  ← Ch18
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
//! <exp>            ::= <assignment> ("," <assignment>)*
//! <assignment>     ::= <lvalue> <assign_op> <assignment> | <conditional>
//! <assign_op>      ::= "=" | "+=" | "-=" | "*=" | "/=" | "%=" | "&=" | "|=" | "^=" | "<<=" | ">>="
//! <conditional>    ::= <logical_or> ("?" <exp> ":" <conditional>)?
//! <logical_or>     ::= <logical_and> ( "||" <logical_and> )*
//! <logical_and>    ::= <bitwise_or> ( "&&" <bitwise_or> )*
//! <bitwise_or>     ::= <bitwise_xor> ( "|" <bitwise_xor> )*
//! <bitwise_xor>    ::= <bitwise_and> ( "^" <bitwise_and> )*
//! <bitwise_and>    ::= <equality> ( "&" <equality> )*
//! <equality>       ::= <relational> ( ("==" | "!=") <relational> )*
//! <relational>     ::= <shift> ( ("<" | "<=" | ">" | ">=") <shift> )*
//! <shift>          ::= <additive> ( ("<<" | ">>") <additive> )*
//! <additive>       ::= <multiplicative> ( ("+" | "-") <multiplicative> )*
//! <multiplicative> ::= <cast> ( ("*" | "/" | "%") <cast> )*
//! <cast>           ::= "(" <type> <abstract_declarator> ")" <cast>
//!                    | <unary>
//! <abstract_declarator> ::= "*"* ("[" <int> "]")?              ← Ch15: 配列サフィックス
//! <unary>          ::= <unary_op> <cast> | "sizeof" <unary>    ← Ch15: sizeof
//!                    | "sizeof" "(" <type> <abstract_declarator> ")"
//!                    | <postfix>
//! <unary_op>       ::= "-" | "~" | "!" | "++" | "--" | "*" | "&"
//! <postfix>        ::= <primary> ("++" | "--" | "[" <exp> "]" | "." <id> | "->" <id>)*  ← Ch15, Ch18
//! <primary>        ::= <int> | <long> | <uint> | <ulong> | <double>
//!                    | <char> | <string>                       ← Ch16
//!                    | <identifier> ("(" <args>? ")")?
//!                    | "(" <exp> ")"
//! <args>           ::= <assignment> ("," <assignment>)*
//! ```

use super::ast::{
    BinaryOp, BlockItem, Declaration, Expr, ForInit, FunctionDecl, MemberDecl, Program, Statement,
    StorageClass, TopLevelDecl, Type, UnaryOp,
};
use crate::error::{CompileError, Result};
use crate::lex::{Token, TokenKind};
use std::collections::HashMap;

/// トークン列を構文解析して AST に変換する。
///
/// すべてのトークンを消費しきれなかった場合（余分なトークンがある場合）はエラー。
pub fn parse(tokens: &[Token]) -> Result<Program> {
    let mut parser = Parser::new(tokens);
    let program = parser.parse_program()?;
    // トークンが残っていればエラー
    if parser.pos < parser.tokens.len() {
        return Err(CompileError::ParseError(format!(
            "unexpected token after end of program: {:?}",
            parser.tokens[parser.pos].kind
        )));
    }
    Ok(program)
}

/// 宣言子の構文木ノード。C の宣言子を内側から外側へ読んで型を構築する。
///
/// 例: `int (*ops[2])(int, int)` のパース結果:
///   Function { inner: Pointer(Array(Name("ops"), 2)), params: [Int, Int], variadic: false }
/// apply(tree, Int) の結果:
///   Array(Pointer(Function { ret: Int, params: [Int, Int] }), 2)
enum DeclaratorNode {
    /// 終端: 識別子名（空文字列 = 無名パラメータ）
    Name(String),
    /// ポインタ: `*inner`
    Pointer(Box<DeclaratorNode>),
    /// 配列: `inner[size]`
    Array(Box<DeclaratorNode>, usize),
    /// 関数: `inner(params)` — グループ化宣言子 `(*)` の後のみ
    Function {
        inner: Box<DeclaratorNode>,
        param_types: Option<Vec<Type>>,
        is_variadic: bool,
    },
}

/// パーサーの内部状態。トークン列への参照と現在の読み取り位置を保持する。
struct Parser<'a> {
    tokens: &'a [Token],
    pos: usize,
    /// 構造体タグテーブル（Chapter 18）。タグ名 → メンバリスト。
    struct_tags: HashMap<String, Vec<MemberDecl>>,
    /// typedef 名テーブル。typedef 名 → 基底型。
    typedef_names: HashMap<String, Type>,
    /// enum 定数テーブル。定数名 → 値。
    enum_constants: HashMap<String, i64>,
    /// 直前にパースした型が enum 定義（`enum { ... }`）かどうか。
    /// `enum { ... };` のような定義のみ構文に対応するために使う。
    last_parsed_enum_def: bool,
    /// 匿名 struct/union 用の連番カウンタ。
    anon_counter: usize,
}

impl<'a> Parser<'a> {
    fn new(tokens: &'a [Token]) -> Self {
        Self {
            tokens,
            pos: 0,
            struct_tags: HashMap::new(),
            typedef_names: HashMap::new(),
            enum_constants: HashMap::new(),
            last_parsed_enum_def: false,
            anon_counter: 0,
        }
    }

    /// 現在位置のトークンを消費せずに参照する（先読み）。
    /// 式のパースで「次のトークンが何か」に応じて分岐する際に使う。
    fn peek(&self) -> Result<&'a Token> {
        self.tokens
            .get(self.pos)
            .ok_or_else(|| CompileError::ParseError("unexpected end of input".to_string()))
    }

    /// 現在位置のトークンを消費して返す。位置を1つ進める。
    fn advance(&mut self) -> Result<&'a Token> {
        let token = self
            .tokens
            .get(self.pos)
            .ok_or_else(|| CompileError::ParseError("unexpected end of input".to_string()))?;
        self.pos += 1;
        Ok(token)
    }

    /// 現在位置のトークンが `expected` と一致することを確認して消費する。
    /// 一致しなければパースエラーを返す。
    fn expect(&mut self, expected: &TokenKind) -> Result<&'a Token> {
        let token = self.advance()?;
        if &token.kind != expected {
            return Err(CompileError::ParseError(format!(
                "expected {:?}, got {:?} at line {}, column {}",
                expected, token.kind, token.span.line, token.span.column
            )));
        }
        Ok(token)
    }

    /// オプショナルのストレージクラス指定子をパースする（Chapter 10）。
    fn parse_storage_class(&mut self) -> Result<Option<StorageClass>> {
        if self.pos >= self.tokens.len() {
            return Ok(None);
        }
        match &self.peek()?.kind {
            TokenKind::KwStatic => {
                self.advance()?;
                Ok(Some(StorageClass::Static))
            }
            TokenKind::KwExtern => {
                self.advance()?;
                Ok(Some(StorageClass::Extern))
            }
            _ => Ok(None),
        }
    }

    /// 型キーワードの先読み判定（Chapter 14）。
    ///
    /// 現在位置のトークンが型指定子キーワードかどうかを返す。
    /// キャスト式 `(type)expr` と括弧式 `(expr)` の区別に使う。
    #[allow(dead_code)]
    fn is_type_keyword(&self) -> bool {
        self.is_type_token_at(self.pos)
    }

    /// 指定位置のトークンが型キーワード・修飾子・関数指定子・typedef 名かどうかを判定する。
    fn is_type_token_at(&self, pos: usize) -> bool {
        if pos >= self.tokens.len() {
            return false;
        }
        match &self.tokens[pos].kind {
            TokenKind::KwInt
            | TokenKind::KwLong
            | TokenKind::KwShort
            | TokenKind::KwUnsigned
            | TokenKind::KwSigned
            | TokenKind::KwDouble
            | TokenKind::KwChar
            | TokenKind::KwVoid
            | TokenKind::KwStruct
            | TokenKind::KwUnion
            | TokenKind::KwEnum
            | TokenKind::KwConst
            | TokenKind::KwVolatile
            | TokenKind::KwRestrict
            | TokenKind::KwInline
            | TokenKind::KwNoreturn => true,
            TokenKind::Identifier(name) => {
                name == "va_list" || self.typedef_names.contains_key(name)
            }
            _ => false,
        }
    }

    /// 型指定子をパースする（Chapter 11, 12）。
    ///
    /// フラグ方式でキーワードを任意順に収集し、型に解決する。
    /// 有効な組み合わせ:
    /// - `int` / `signed` / `signed int` → Int
    /// - `long` / `long int` / `int long` / `signed long` / `signed long int` → Long
    /// - `unsigned` / `unsigned int` → UInt
    /// - `unsigned long` / `unsigned long int` → ULong
    fn parse_type_specifier(&mut self) -> Result<Type> {
        let mut has_int = false;
        let mut has_long = false;
        let mut has_unsigned = false;
        let mut has_signed = false;
        let mut has_double = false;
        let mut has_char = false;
        let mut has_short = false;
        let mut has_void = false;
        let mut count = 0;

        loop {
            if self.pos >= self.tokens.len() {
                break;
            }
            match &self.peek()?.kind {
                // Skip qualifiers and function specifiers (parse-only)
                TokenKind::KwConst
                | TokenKind::KwVolatile
                | TokenKind::KwRestrict
                | TokenKind::KwInline
                | TokenKind::KwNoreturn => {
                    self.advance()?;
                    continue;
                }
                TokenKind::KwVoid => {
                    if has_void {
                        return Err(CompileError::ParseError(
                            "duplicate 'void' type specifier".to_string(),
                        ));
                    }
                    if has_int || has_long || has_unsigned || has_signed || has_double || has_char {
                        return Err(CompileError::ParseError(
                            "cannot combine 'void' with other type specifiers".to_string(),
                        ));
                    }
                    has_void = true;
                    self.advance()?;
                    count += 1;
                }
                TokenKind::KwInt => {
                    if has_int {
                        return Err(CompileError::ParseError(
                            "duplicate 'int' type specifier".to_string(),
                        ));
                    }
                    if has_double {
                        return Err(CompileError::ParseError(
                            "cannot combine 'double' with other type specifiers".to_string(),
                        ));
                    }
                    if has_char {
                        return Err(CompileError::ParseError(
                            "cannot combine 'char' with 'int'".to_string(),
                        ));
                    }
                    if has_void {
                        return Err(CompileError::ParseError(
                            "cannot combine 'void' with other type specifiers".to_string(),
                        ));
                    }
                    has_int = true;
                    self.advance()?;
                    count += 1;
                }
                TokenKind::KwLong => {
                    // long long は long と同義として許容（LP64）
                    // long double も許容（Double で近似）
                    if has_short {
                        return Err(CompileError::ParseError(
                            "cannot combine 'long' and 'short'".to_string(),
                        ));
                    }
                    if has_char {
                        return Err(CompileError::ParseError(
                            "cannot combine 'char' with 'long'".to_string(),
                        ));
                    }
                    if has_void {
                        return Err(CompileError::ParseError(
                            "cannot combine 'void' with other type specifiers".to_string(),
                        ));
                    }
                    has_long = true;
                    self.advance()?;
                    count += 1;
                }
                TokenKind::KwShort => {
                    if has_short {
                        return Err(CompileError::ParseError(
                            "duplicate 'short' type specifier".to_string(),
                        ));
                    }
                    if has_long || has_double || has_char || has_void {
                        return Err(CompileError::ParseError(
                            "cannot combine 'short' with other type specifiers".to_string(),
                        ));
                    }
                    has_short = true;
                    self.advance()?;
                    count += 1;
                }
                TokenKind::KwUnsigned => {
                    if has_unsigned {
                        return Err(CompileError::ParseError(
                            "duplicate 'unsigned' type specifier".to_string(),
                        ));
                    }
                    if has_signed {
                        return Err(CompileError::ParseError(
                            "cannot combine 'signed' and 'unsigned'".to_string(),
                        ));
                    }
                    if has_double {
                        return Err(CompileError::ParseError(
                            "cannot combine 'double' with other type specifiers".to_string(),
                        ));
                    }
                    if has_void {
                        return Err(CompileError::ParseError(
                            "cannot combine 'void' with other type specifiers".to_string(),
                        ));
                    }
                    has_unsigned = true;
                    self.advance()?;
                    count += 1;
                }
                TokenKind::KwSigned => {
                    if has_signed {
                        return Err(CompileError::ParseError(
                            "duplicate 'signed' type specifier".to_string(),
                        ));
                    }
                    if has_unsigned {
                        return Err(CompileError::ParseError(
                            "cannot combine 'signed' and 'unsigned'".to_string(),
                        ));
                    }
                    if has_double {
                        return Err(CompileError::ParseError(
                            "cannot combine 'double' with other type specifiers".to_string(),
                        ));
                    }
                    if has_void {
                        return Err(CompileError::ParseError(
                            "cannot combine 'void' with other type specifiers".to_string(),
                        ));
                    }
                    has_signed = true;
                    self.advance()?;
                    count += 1;
                }
                TokenKind::KwDouble => {
                    if has_double {
                        return Err(CompileError::ParseError(
                            "duplicate 'double' type specifier".to_string(),
                        ));
                    }
                    if has_int || has_unsigned || has_signed || has_char || has_void {
                        return Err(CompileError::ParseError(
                            "cannot combine 'double' with other type specifiers".to_string(),
                        ));
                    }
                    // `long double` は Double として近似（parse-only）
                    has_double = true;
                    self.advance()?;
                    count += 1;
                }
                TokenKind::KwChar => {
                    if has_char {
                        return Err(CompileError::ParseError(
                            "duplicate 'char' type specifier".to_string(),
                        ));
                    }
                    if has_int || has_long || has_double || has_void {
                        return Err(CompileError::ParseError(
                            "cannot combine 'char' with other type specifiers".to_string(),
                        ));
                    }
                    has_char = true;
                    self.advance()?;
                    count += 1;
                }
                TokenKind::KwStruct | TokenKind::KwUnion => {
                    if count > 0 {
                        return Err(CompileError::ParseError(
                            "cannot combine 'struct'/'union' with other type specifiers"
                                .to_string(),
                        ));
                    }
                    return self.parse_struct_type();
                }
                TokenKind::KwEnum => {
                    if count > 0 {
                        return Err(CompileError::ParseError(
                            "cannot combine 'enum' with other type specifiers".to_string(),
                        ));
                    }
                    return self.parse_enum_type();
                }
                TokenKind::Identifier(name) if count == 0 => {
                    // va_list を型として認識
                    if name == "va_list" {
                        self.advance()?;
                        return Ok(Type::VaList);
                    }
                    // GCC 組み込み型（parse-only: Long で近似）
                    if name == "__uint128_t" || name == "__int128_t" || name == "__int128" {
                        self.advance()?;
                        return Ok(Type::Long);
                    }
                    // float → Double として近似（parse-only）
                    if name == "float" {
                        self.advance()?;
                        return Ok(Type::Double);
                    }
                    // 他の型キーワードが未出現の場合のみ typedef 名として認識
                    if let Some(ty) = self.typedef_names.get(name).cloned() {
                        self.advance()?;
                        return Ok(ty);
                    }
                    break;
                }
                _ => break,
            }
        }

        if count == 0 {
            let token = self.advance()?;
            return Err(CompileError::ParseError(format!(
                "expected type specifier, got {:?} at line {}:{}",
                token.kind, token.span.line, token.span.column
            )));
        }

        // 型の解決
        if has_void {
            Ok(Type::Void)
        } else if has_double {
            Ok(Type::Double)
        } else if has_char {
            if has_unsigned {
                Ok(Type::UChar)
            } else {
                // char, signed char
                Ok(Type::Char)
            }
        } else if has_unsigned {
            if has_long {
                Ok(Type::ULong)
            } else if has_short {
                // unsigned short → treat as UInt (16-bit semantics not yet supported)
                Ok(Type::UInt)
            } else {
                Ok(Type::UInt)
            }
        } else if has_long {
            Ok(Type::Long)
        } else if has_short {
            // short / short int / signed short → treat as Int (16-bit semantics not yet)
            Ok(Type::Int)
        } else {
            // has_int or has_signed (or both)
            Ok(Type::Int)
        }
    }

    /// 構造体型のパース（Chapter 18）。
    ///
    /// `struct tag { members }` または `struct tag` を解析する。
    fn parse_struct_type(&mut self) -> Result<Type> {
        self.advance()?; // consume 'struct'

        // タグ名を取得（匿名 struct/union は合成名を生成）
        let tag = match &self.peek()?.kind {
            TokenKind::Identifier(name) => {
                let name = name.clone();
                self.advance()?;
                name
            }
            TokenKind::OpenBrace => {
                // 匿名 struct/union: `struct { ... }` or `union { ... }`
                let id = self.anon_counter;
                self.anon_counter += 1;
                format!("__anon_{id}")
            }
            _ => {
                return Err(CompileError::ParseError(
                    "expected struct tag name".to_string(),
                ));
            }
        };

        // '{' があれば構造体定義
        if self.pos < self.tokens.len() && self.peek()?.kind == TokenKind::OpenBrace {
            self.advance()?; // consume '{'

            let mut members = Vec::new();
            while self.peek()?.kind != TokenKind::CloseBrace {
                let member_base = self.parse_type_specifier()?;
                // 構造体メンバは tolerant パーサを使用（システムヘッダの
                // 複雑な配列サイズ式・関数ポインタ・無名メンバに対応）
                let (member_type, member_name) =
                    self.parse_param_declarator(member_base.clone())?;
                // ビットフィールド `:width` をスキップ
                if self.pos < self.tokens.len() && self.peek()?.kind == TokenKind::Colon {
                    self.advance()?; // consume ':'
                    self.advance()?; // consume width (IntLiteral)
                }
                members.push(MemberDecl {
                    name: member_name,
                    member_type: member_type.clone(),
                });
                // カンマ区切りメンバ: `int a:1, b:2, c:3;`
                while self.pos < self.tokens.len() && self.peek()?.kind == TokenKind::Comma {
                    self.advance()?; // consume ','
                    let (extra_type, extra_name) =
                        self.parse_param_declarator(member_base.clone())?;
                    if self.pos < self.tokens.len() && self.peek()?.kind == TokenKind::Colon {
                        self.advance()?;
                        self.advance()?;
                    }
                    members.push(MemberDecl {
                        name: extra_name,
                        member_type: extra_type,
                    });
                }
                self.expect(&TokenKind::Semicolon)?;
            }
            self.expect(&TokenKind::CloseBrace)?;

            // タグテーブルに登録
            self.struct_tags.insert(tag.clone(), members.clone());

            Ok(Type::Struct { tag, members })
        } else {
            // 前方参照: タグテーブルからメンバ情報を取得
            let members = self.struct_tags.get(&tag).cloned().unwrap_or_default();
            Ok(Type::Struct { tag, members })
        }
    }

    /// enum 定数の初期値式をパースし、定数値として返す。
    ///
    /// 対応パターン: リテラル、`-lit`、`lit << lit`、`lit | lit`、
    /// `(expr)`、既存 enum 定数名。
    fn parse_enum_const_expr(&mut self) -> Result<i64> {
        self.parse_enum_const_ternary()
    }

    /// enum 定数式: 三項演算子 (`?:`)
    fn parse_enum_const_ternary(&mut self) -> Result<i64> {
        let cond = self.parse_enum_const_bitor()?;
        if self.pos < self.tokens.len() && self.peek()?.kind == TokenKind::Question {
            self.advance()?; // consume '?'
            let then_val = self.parse_enum_const_expr()?;
            self.expect(&TokenKind::Colon)?;
            let else_val = self.parse_enum_const_ternary()?;
            Ok(if cond != 0 { then_val } else { else_val })
        } else {
            Ok(cond)
        }
    }

    /// enum 定数式: ビット OR (`|`)
    fn parse_enum_const_bitor(&mut self) -> Result<i64> {
        let mut val = self.parse_enum_const_bitand()?;
        while self.pos < self.tokens.len() && self.peek()?.kind == TokenKind::Pipe {
            self.advance()?;
            let rhs = self.parse_enum_const_bitand()?;
            val |= rhs;
        }
        Ok(val)
    }

    /// enum 定数式: ビット AND (`&`)
    fn parse_enum_const_bitand(&mut self) -> Result<i64> {
        let mut val = self.parse_enum_const_relational()?;
        while self.pos < self.tokens.len() && self.peek()?.kind == TokenKind::Ampersand {
            self.advance()?;
            let rhs = self.parse_enum_const_relational()?;
            val &= rhs;
        }
        Ok(val)
    }

    /// enum 定数式: 比較 (`<`, `>`, `<=`, `>=`)
    fn parse_enum_const_relational(&mut self) -> Result<i64> {
        let mut val = self.parse_enum_const_additive()?;
        while self.pos < self.tokens.len() {
            match self.peek()?.kind {
                TokenKind::Less => {
                    self.advance()?;
                    let rhs = self.parse_enum_const_additive()?;
                    val = if val < rhs { 1 } else { 0 };
                }
                TokenKind::Greater => {
                    self.advance()?;
                    let rhs = self.parse_enum_const_additive()?;
                    val = if val > rhs { 1 } else { 0 };
                }
                TokenKind::LessEqual => {
                    self.advance()?;
                    let rhs = self.parse_enum_const_additive()?;
                    val = if val <= rhs { 1 } else { 0 };
                }
                TokenKind::GreaterEqual => {
                    self.advance()?;
                    let rhs = self.parse_enum_const_additive()?;
                    val = if val >= rhs { 1 } else { 0 };
                }
                _ => break,
            }
        }
        Ok(val)
    }

    /// enum 定数式: 加減算 (`+`, `-`)
    fn parse_enum_const_additive(&mut self) -> Result<i64> {
        let mut val = self.parse_enum_const_shift()?;
        while self.pos < self.tokens.len() {
            match self.peek()?.kind {
                TokenKind::Plus => {
                    self.advance()?;
                    let rhs = self.parse_enum_const_shift()?;
                    val += rhs;
                }
                TokenKind::Minus => {
                    self.advance()?;
                    let rhs = self.parse_enum_const_shift()?;
                    val -= rhs;
                }
                _ => break,
            }
        }
        Ok(val)
    }

    /// enum 定数式: シフト (`<<`, `>>`)
    fn parse_enum_const_shift(&mut self) -> Result<i64> {
        let mut val = self.parse_enum_const_unary()?;
        while self.pos < self.tokens.len() {
            match self.peek()?.kind {
                TokenKind::ShiftLeft => {
                    self.advance()?;
                    let rhs = self.parse_enum_const_unary()?;
                    val <<= rhs;
                }
                TokenKind::ShiftRight => {
                    self.advance()?;
                    let rhs = self.parse_enum_const_unary()?;
                    val >>= rhs;
                }
                _ => break,
            }
        }
        Ok(val)
    }

    /// enum 定数式: 単項 (`-`, `~`)
    fn parse_enum_const_unary(&mut self) -> Result<i64> {
        if self.pos < self.tokens.len() && self.peek()?.kind == TokenKind::Minus {
            self.advance()?;
            let val = self.parse_enum_const_primary()?;
            return Ok(-val);
        }
        if self.pos < self.tokens.len() && self.peek()?.kind == TokenKind::Tilde {
            self.advance()?;
            let val = self.parse_enum_const_primary()?;
            return Ok(!val);
        }
        self.parse_enum_const_primary()
    }

    /// enum 定数式: プライマリ (リテラル、enum 定数名、括弧)
    fn parse_enum_const_primary(&mut self) -> Result<i64> {
        if self.pos < self.tokens.len() && self.peek()?.kind == TokenKind::OpenParen {
            self.advance()?; // consume '('
            let val = self.parse_enum_const_expr()?;
            self.expect(&TokenKind::CloseParen)?;
            return Ok(val);
        }
        let tok = self.advance()?;
        match &tok.kind {
            TokenKind::IntLiteral(v) => Ok(*v),
            TokenKind::LongLiteral(v) => Ok(*v),
            TokenKind::UIntLiteral(v) => Ok(*v as i64),
            TokenKind::ULongLiteral(v) => Ok(*v as i64),
            TokenKind::Identifier(name) => {
                if let Some(&val) = self.enum_constants.get(name) {
                    Ok(val)
                } else {
                    Err(CompileError::ParseError(format!(
                        "unknown enum constant '{name}' in enum initializer"
                    )))
                }
            }
            other => Err(CompileError::ParseError(format!(
                "expected constant expression in enum initializer, got {:?}",
                other
            ))),
        }
    }

    /// enum 型のパース。
    ///
    /// `enum [tag] { NAME [= value], ... }` または `enum tag` を解析する。
    /// enum は int として扱い、各定数を `enum_constants` テーブルに登録する。
    fn parse_enum_type(&mut self) -> Result<Type> {
        self.advance()?; // consume 'enum'
        self.last_parsed_enum_def = false;

        // オプショナルのタグ名
        let _tag = if self.pos < self.tokens.len() {
            if let TokenKind::Identifier(name) = &self.peek()?.kind {
                let name = name.clone();
                self.advance()?;
                Some(name)
            } else {
                None
            }
        } else {
            None
        };

        // '{' があれば定数リストをパース
        if self.pos < self.tokens.len() && self.peek()?.kind == TokenKind::OpenBrace {
            self.advance()?; // consume '{'
            let mut next_value: i64 = 0;

            while self.peek()?.kind != TokenKind::CloseBrace {
                let const_token = self.advance()?;
                let const_name = match &const_token.kind {
                    TokenKind::Identifier(name) => name.clone(),
                    other => {
                        return Err(CompileError::ParseError(format!(
                            "expected enum constant name, got {:?}",
                            other
                        )));
                    }
                };

                // 明示値: `NAME = const_expr`
                if self.pos < self.tokens.len() && self.peek()?.kind == TokenKind::Assign {
                    self.advance()?; // consume '='
                    next_value = self.parse_enum_const_expr()?;
                }

                self.enum_constants.insert(const_name, next_value);
                next_value += 1;

                // トレーリングカンマ許容
                if self.peek()?.kind == TokenKind::Comma {
                    self.advance()?;
                } else {
                    break;
                }
            }
            self.expect(&TokenKind::CloseBrace)?;
            self.last_parsed_enum_def = true;
        }

        Ok(Type::Int)
    }

    // ────────────────────────────────────────────
    // Declarator tree — C の宣言子を構文木として表現
    // ────────────────────────────────────────────

    /// 宣言子の構文木。C の宣言子は名前から外側へ読んで型を構築する。
    ///
    /// 例: `int (*ops[2])(int, int)` →
    ///   tree = Function { inner: Pointer(Array(Name("ops"), 2)), params, variadic }
    ///   apply(tree, Int) → Array(Pointer(Function { ret: Int, params: [Int, Int] }), 2)
    ///
    /// Name: 終端（識別子）
    /// Pointer: `*` プレフィクス
    /// Array: `[N]` サフィクス
    /// Function: `(params)` サフィクス（グループ後のみ）
    /// 宣言子ツリーを構築する。
    ///
    /// `allow_empty_name`: true なら名前なしを許容（パラメータ宣言子、抽象宣言子用）
    fn parse_declarator_tree(&mut self, allow_empty_name: bool) -> Result<DeclaratorNode> {
        // ポインタプレフィクス: * をカウント（const/volatile/restrict/_Nonnull 等をスキップ）
        let mut stars = 0;
        while self.pos < self.tokens.len()
            && matches!(self.peek()?.kind, TokenKind::Star | TokenKind::Caret)
        {
            self.advance()?;
            stars += 1;
            while self.pos < self.tokens.len()
                && matches!(
                    self.peek()?.kind,
                    TokenKind::KwConst | TokenKind::KwVolatile | TokenKind::KwRestrict
                )
            {
                self.advance()?;
            }
            // Clang nullability: _Nonnull, _Nullable, _Null_unspecified
            while self.pos < self.tokens.len() {
                if let TokenKind::Identifier(attr) = &self.peek()?.kind
                    && (attr.starts_with("_N") || attr.starts_with("_null"))
                {
                    self.advance()?;
                    continue;
                }
                break;
            }
        }

        let decl = self.parse_direct_declarator_tree(allow_empty_name)?;

        // ポインタでラップ（内側から）
        let mut result = decl;
        for _ in 0..stars {
            result = DeclaratorNode::Pointer(Box::new(result));
        }
        Ok(result)
    }

    /// 直接宣言子ツリーを構築する。
    ///
    /// ベースケース: 識別子 or `( declarator_tree )`
    /// サフィクス: `[N]` (常時), `(params)` (グループ後のみ)
    fn parse_direct_declarator_tree(&mut self, allow_empty_name: bool) -> Result<DeclaratorNode> {
        let mut from_group = false;

        let mut decl = if self.pos < self.tokens.len()
            && self.peek()?.kind == TokenKind::OpenParen
            && self.pos + 1 < self.tokens.len()
            && matches!(
                self.tokens[self.pos + 1].kind,
                TokenKind::Star | TokenKind::Caret
            ) {
            // グループ化宣言子: ( declarator_tree )
            from_group = true;
            self.advance()?; // consume '('
            let inner = self.parse_declarator_tree(allow_empty_name)?;
            self.expect(&TokenKind::CloseParen)?;
            inner
        } else if self.pos < self.tokens.len()
            && let TokenKind::Identifier(name) = &self.peek()?.kind
        {
            let name = name.clone();
            self.advance()?;
            DeclaratorNode::Name(name)
        } else if allow_empty_name {
            DeclaratorNode::Name(String::new())
        } else {
            let token = self.peek()?;
            return Err(CompileError::ParseError(format!(
                "expected identifier in declarator, got {:?} at line {}, column {}",
                token.kind, token.span.line, token.span.column
            )));
        };

        // サフィクスループ
        while self.pos < self.tokens.len() {
            match self.peek()?.kind {
                // 配列サフィクス: [N] — 常時許可
                TokenKind::OpenBracket => {
                    self.advance()?; // consume '['
                    let size = if self.peek()?.kind == TokenKind::CloseBracket {
                        0
                    } else {
                        let size_expr = self.parse_conditional()?;
                        Self::eval_const_expr(&size_expr)? as usize
                    };
                    self.expect(&TokenKind::CloseBracket)?;
                    decl = DeclaratorNode::Array(Box::new(decl), size);
                }
                // 関数サフィクス: (params) — グループ後のみ
                TokenKind::OpenParen if from_group => {
                    if let Some((param_types, is_variadic)) = self.try_parse_fn_ptr_params() {
                        decl = DeclaratorNode::Function {
                            inner: Box::new(decl),
                            param_types,
                            is_variadic,
                        };
                    } else {
                        self.skip_balanced_parens()?;
                        decl = DeclaratorNode::Function {
                            inner: Box::new(decl),
                            param_types: None,
                            is_variadic: false,
                        };
                    }
                    // 関数後に更に配列/関数サフィクスは実用上出ないが、
                    // ループは継続可能（C 文法的には valid）
                }
                _ => break,
            }
        }

        Ok(decl)
    }

    /// 宣言子ツリーをベース型に適用し、最終的な (Type, name) を返す。
    fn apply_declarator(decl: DeclaratorNode, base_type: Type) -> (Type, String) {
        match decl {
            DeclaratorNode::Name(name) => (base_type, name),
            DeclaratorNode::Pointer(inner) => {
                Self::apply_declarator(*inner, Type::Pointer(Box::new(base_type)))
            }
            DeclaratorNode::Array(inner, size) => {
                Self::apply_declarator(*inner, Type::Array(Box::new(base_type), size))
            }
            DeclaratorNode::Function {
                inner,
                param_types,
                is_variadic,
            } => {
                let fn_type = Type::Function {
                    return_type: Box::new(base_type),
                    param_types,
                    is_variadic,
                };
                Self::apply_declarator(*inner, fn_type)
            }
        }
    }

    /// 宣言子のパース。DeclaratorNode ツリーを構築し、ベース型に適用する。
    fn parse_declarator(&mut self, base_type: Type) -> Result<(Type, String)> {
        let tree = self.parse_declarator_tree(false)?;
        Ok(Self::apply_declarator(tree, base_type))
    }

    /// パラメータ宣言子のパース。名前付き・名前なし両対応。
    fn parse_param_declarator(&mut self, base_type: Type) -> Result<(Type, String)> {
        let tree = self.parse_declarator_tree(true)?;
        Ok(Self::apply_declarator(tree, base_type))
    }

    /// 抽象宣言子のパース。キャスト `(int *)` 等で使用。識別子なしで型だけを返す。
    fn parse_abstract_declarator(&mut self, base_type: Type) -> Result<Type> {
        let tree = self.parse_declarator_tree(true)?;
        let (ty, _) = Self::apply_declarator(tree, base_type);
        Ok(ty)
    }

    /// typedef 宣言をパースし、typedef 名テーブルに登録する。
    /// `typedef <type> <declarator> ("," <declarator>)* ";"`
    fn parse_typedef(&mut self) -> Result<Vec<(String, Type)>> {
        self.advance()?; // consume 'typedef'
        let base_type = self.parse_type_specifier()?;

        let mut results = Vec::new();

        let (resolved_type, name) = self.parse_declarator(base_type.clone())?;

        // typedef T name(params); — 関数型 typedef
        // 例: `typedef int handler_t(void *, const char *);`
        //   → handler_t = Function { return_type: resolved_type, params, variadic }
        // ポインタ化は使用箇所（変数宣言・パラメータ）で行う。ここでは関数型のまま保持。
        let resolved_type = self.try_resolve_fn_type_typedef(resolved_type)?;

        self.typedef_names
            .insert(name.clone(), resolved_type.clone());
        results.push((name, resolved_type));

        while self.peek()?.kind == TokenKind::Comma {
            self.advance()?;
            let (resolved_type, name) = self.parse_declarator(base_type.clone())?;
            // Finding 3: 2個目以降の declarator にも関数型チェックを適用
            let resolved_type = self.try_resolve_fn_type_typedef(resolved_type)?;
            self.typedef_names
                .insert(name.clone(), resolved_type.clone());
            results.push((name, resolved_type));
        }

        self.expect(&TokenKind::Semicolon)?;
        Ok(results)
    }

    /// typedef 宣言子の後に `(params)` が続く場合、関数型に解決する。
    /// `typedef int handler_t(void *, const char *);`
    ///   → `Function { return_type: Int, param_types: Some([Pointer(Void), Pointer(Char)]), ... }`
    fn try_resolve_fn_type_typedef(&mut self, resolved_type: Type) -> Result<Type> {
        if self.pos < self.tokens.len() && self.peek()?.kind == TokenKind::OpenParen {
            if let Some((param_types, is_variadic)) = self.try_parse_fn_ptr_params() {
                Ok(Type::Function {
                    return_type: Box::new(resolved_type),
                    param_types,
                    is_variadic,
                })
            } else {
                // フォールバック: パース失敗時はスキップして引数未指定の関数型
                self.skip_balanced_parens()?;
                Ok(Type::Function {
                    return_type: Box::new(resolved_type),
                    param_types: None,
                    is_variadic: false,
                })
            }
        } else {
            Ok(resolved_type)
        }
    }

    /// 関数ポインタ/関数型のパラメータリストをパースする。
    /// `(` は呼び出し前に特定済み（未消費）。`(` から `)` まで消費する。
    /// パース失敗時は位置を復元して None を返す（フォールバック用）。
    ///
    /// 返値: `(param_types, is_variadic)`
    /// - `param_types: None` — 引数未指定 `()` (K&R 互換)
    /// - `param_types: Some(vec![])` — 引数ゼロ `(void)`
    /// - `param_types: Some(vec![...])` — 具体的なプロトタイプ
    fn try_parse_fn_ptr_params(&mut self) -> Option<(Option<Vec<Type>>, bool)> {
        let save_pos = self.pos;
        match self.parse_fn_ptr_params_inner() {
            Ok(result) => Some(result),
            Err(_) => {
                self.pos = save_pos;
                None
            }
        }
    }

    /// 関数ポインタ/関数型のパラメータリストをパースする内部実装。
    fn parse_fn_ptr_params_inner(&mut self) -> Result<(Option<Vec<Type>>, bool)> {
        self.expect(&TokenKind::OpenParen)?;

        // `()` — 引数未指定（K&R 互換）: None で表現
        if self.pos < self.tokens.len() && self.peek()?.kind == TokenKind::CloseParen {
            self.advance()?;
            return Ok((None, false));
        }
        // `(void)` — 引数ゼロ: Some(vec![]) で表現
        if self.pos < self.tokens.len()
            && self.peek()?.kind == TokenKind::KwVoid
            && self.pos + 1 < self.tokens.len()
            && self.tokens[self.pos + 1].kind == TokenKind::CloseParen
        {
            self.advance()?; // consume 'void'
            self.advance()?; // consume ')'
            return Ok((Some(Vec::new()), false));
        }

        let mut param_types = Vec::new();
        let mut is_variadic = false;

        let param_base = self.parse_type_specifier()?;
        let (param_type, _) = self.parse_param_declarator(param_base)?;
        param_types.push(Self::adjust_param_type(param_type));

        while self.pos < self.tokens.len() && self.peek()?.kind == TokenKind::Comma {
            self.advance()?; // consume ','
            if self.pos < self.tokens.len() && self.peek()?.kind == TokenKind::Ellipsis {
                self.advance()?;
                is_variadic = true;
                break;
            }
            let param_base = self.parse_type_specifier()?;
            let (param_type, _) = self.parse_param_declarator(param_base)?;
            param_types.push(Self::adjust_param_type(param_type));
        }

        self.expect(&TokenKind::CloseParen)?;
        Ok((Some(param_types), is_variadic))
    }

    /// バランスの取れた括弧をスキップする（フォールバック用）。
    /// `(` が次のトークンであること前提。`(` から `)` まで消費する。
    fn skip_balanced_parens(&mut self) -> Result<()> {
        self.expect(&TokenKind::OpenParen)?;
        let mut depth = 1;
        while depth > 0 && self.pos < self.tokens.len() {
            let tok = self.advance()?;
            match tok.kind {
                TokenKind::OpenParen => depth += 1,
                TokenKind::CloseParen => depth -= 1,
                _ => {}
            }
        }
        Ok(())
    }

    /// パラメータ型の調整（C 6.7.6.3p7,8）。
    /// - 配列型 → ポインタ型
    /// - 関数型 → 関数ポインタ型
    fn adjust_param_type(ty: Type) -> Type {
        match ty {
            Type::Array(elem, _) => Type::Pointer(elem),
            Type::Function { .. } => Type::Pointer(Box::new(ty)),
            other => other,
        }
    }

    /// `<program> ::= <top_level_decl>*`
    fn parse_program(&mut self) -> Result<Program> {
        let mut declarations = Vec::new();
        while self.pos < self.tokens.len() {
            declarations.extend(self.parse_top_level_decl()?);
        }
        Ok(Program { declarations })
    }

    /// `<top_level_decl> ::= <function_decl> | <variable_decl>`
    ///
    /// トップレベルでの関数 vs 変数の区別:
    /// `[static|extern]? int <id>` の後に `(` → 関数、`=` or `;` → 変数。
    /// カンマ区切り変数宣言をサポート: `int a = 1, b = 2;` → 複数の TopLevelDecl。
    fn parse_top_level_decl(&mut self) -> Result<Vec<TopLevelDecl>> {
        // ファイルスコープの空文 `;` をスキップ（前処理出力に出現する）
        while self.pos < self.tokens.len() && self.peek()?.kind == TokenKind::Semicolon {
            self.advance()?;
        }
        if self.pos >= self.tokens.len() {
            return Ok(vec![]);
        }

        // typedef 宣言のチェック
        if self.peek()?.kind == TokenKind::KwTypedef {
            let results = self.parse_typedef()?;
            return Ok(results
                .into_iter()
                .map(|(name, ty)| TopLevelDecl::Typedef {
                    name,
                    underlying_type: ty,
                })
                .collect());
        }

        let storage_class = self.parse_storage_class()?;
        let base_type = self.parse_type_specifier()?;

        // Chapter 18: `struct tag { ... };` — 構造体定義のみ（変数宣言なし）
        // enum { ... }; — enum 定義のみ（変数宣言なし）
        if (base_type.is_struct() || self.last_parsed_enum_def)
            && self.pos < self.tokens.len()
            && self.peek()?.kind == TokenKind::Semicolon
        {
            self.last_parsed_enum_def = false;
            self.advance()?; // consume ';'
            return Ok(vec![TopLevelDecl::Variable(Declaration {
                name: String::new(),
                var_type: base_type,
                init: None,
                storage_class,
            })]);
        }
        self.last_parsed_enum_def = false;

        // 複雑な宣言子（関数ポインタを返す関数等）の回復:
        // `(*` or `(^` で始まる場合のみ（`void(*signal(...))(int);` 等の複雑パターン）
        let try_recovery = self.pos + 1 < self.tokens.len()
            && self.peek()?.kind == TokenKind::OpenParen
            && matches!(
                self.tokens[self.pos + 1].kind,
                TokenKind::Star | TokenKind::Caret
            );
        let saved_pos = self.pos;
        let (decl_type, name) = match self.parse_declarator(base_type.clone()) {
            Ok(result) => result,
            Err(e) => {
                if try_recovery {
                    // 回復: `;` までスキップ
                    // 関数名を抽出: `(*name(...))(...)` パターンから名前を取得
                    self.pos = saved_pos;
                    let mut recovered_name = String::new();
                    // Skip `(` and `*`/`^`
                    let scan = self.pos + 2;
                    if scan < self.tokens.len() {
                        // skip optional _Nullable / _Nonnull
                        let mut name_pos = scan;
                        while name_pos < self.tokens.len() {
                            if let TokenKind::Identifier(ref id) = self.tokens[name_pos].kind {
                                if id == "_Nullable" || id == "_Nonnull" {
                                    name_pos += 1;
                                    continue;
                                }
                                recovered_name = id.clone();
                            }
                            break;
                        }
                    }
                    while self.pos < self.tokens.len() && self.peek()?.kind != TokenKind::Semicolon
                    {
                        self.advance()?;
                    }
                    if self.pos < self.tokens.len() {
                        self.advance()?; // consume ';'
                    }
                    // 関数として登録（引数なし可変長、戻り値は base_type のポインタ）
                    if !recovered_name.is_empty() {
                        return Ok(vec![TopLevelDecl::Function(FunctionDecl {
                            name: recovered_name,
                            params: vec![],
                            body: None,
                            return_type: Type::Pointer(Box::new(base_type)),
                            storage_class,
                            is_variadic: true,
                            has_prototype: false,
                        })]);
                    }
                    return Ok(vec![TopLevelDecl::Variable(Declaration {
                        name: String::new(),
                        var_type: base_type,
                        init: None,
                        storage_class,
                    })]);
                }
                return Err(e);
            }
        };

        // `(` → 関数、`=`/`;` → 変数
        if self.peek()?.kind == TokenKind::OpenParen {
            // 関数宣言/定義
            self.expect(&TokenKind::OpenParen)?;

            let mut is_variadic = false;
            let mut has_prototype = true;
            let params = if self.peek()?.kind == TokenKind::CloseParen {
                // `()` — 引数未指定（K&R 互換）
                has_prototype = false;
                Vec::new()
            } else if self.peek()?.kind == TokenKind::KwVoid
                && self.pos + 1 < self.tokens.len()
                && self.tokens[self.pos + 1].kind == TokenKind::CloseParen
            {
                // `(void)` — 引数ゼロ
                self.advance()?;
                Vec::new()
            } else {
                let mut params = Vec::new();
                let param_base = self.parse_type_specifier()?;
                let (param_type, param_name) = self.parse_param_declarator(param_base)?;
                // 配列/関数パラメータ → ポインタに調整（C 6.7.6.3p7,8）
                params.push((Self::adjust_param_type(param_type), param_name));
                while self.peek()?.kind == TokenKind::Comma {
                    self.advance()?;
                    // `, ...` → 可変長引数
                    if self.peek()?.kind == TokenKind::Ellipsis {
                        self.advance()?;
                        is_variadic = true;
                        break;
                    }
                    let param_base = self.parse_type_specifier()?;
                    let (param_type, param_name) = self.parse_param_declarator(param_base)?;
                    params.push((Self::adjust_param_type(param_type), param_name));
                }
                params
            };

            self.expect(&TokenKind::CloseParen)?;

            let body = if self.peek()?.kind == TokenKind::OpenBrace {
                self.expect(&TokenKind::OpenBrace)?;
                let mut items = Vec::new();
                while self.peek()?.kind != TokenKind::CloseBrace {
                    items.extend(self.parse_block_item()?);
                }
                self.expect(&TokenKind::CloseBrace)?;
                Some(items)
            } else {
                self.expect(&TokenKind::Semicolon)?;
                None
            };

            Ok(vec![TopLevelDecl::Function(FunctionDecl {
                name,
                return_type: decl_type,
                params,
                body,
                storage_class,
                is_variadic,
                has_prototype,
            })])
        } else {
            // 変数宣言 — カンマ区切り対応
            let mut declarations = Vec::new();

            // 最初の変数
            let init = if self.peek()?.kind == TokenKind::Assign {
                self.advance()?;
                if self.pos < self.tokens.len() && self.peek()?.kind == TokenKind::OpenBrace {
                    Some(self.parse_compound_init()?)
                } else {
                    Some(self.parse_assignment()?)
                }
            } else {
                None
            };
            declarations.push(TopLevelDecl::Variable(Declaration {
                name,
                var_type: decl_type,
                init,
                storage_class,
            }));

            // カンマ区切りの追加変数
            while self.peek()?.kind == TokenKind::Comma {
                self.advance()?; // consume ','
                let (var_type, name) = self.parse_declarator(base_type.clone())?;
                let init = if self.peek()?.kind == TokenKind::Assign {
                    self.advance()?;
                    if self.pos < self.tokens.len() && self.peek()?.kind == TokenKind::OpenBrace {
                        Some(self.parse_compound_init()?)
                    } else {
                        Some(self.parse_assignment()?)
                    }
                } else {
                    None
                };
                declarations.push(TopLevelDecl::Variable(Declaration {
                    name,
                    var_type,
                    init,
                    storage_class,
                }));
            }

            self.expect(&TokenKind::Semicolon)?;
            Ok(declarations)
        }
    }

    /// `<block_item> ::= <statement> | <declaration>`
    ///
    /// 型キーワードまたはストレージクラスで始まれば宣言、それ以外は文。
    /// カンマ区切り宣言は複数の `BlockItem::Declaration` に展開される。
    fn parse_block_item(&mut self) -> Result<Vec<BlockItem>> {
        match &self.peek()?.kind {
            TokenKind::KwInt
            | TokenKind::KwLong
            | TokenKind::KwShort
            | TokenKind::KwUnsigned
            | TokenKind::KwSigned
            | TokenKind::KwDouble
            | TokenKind::KwChar
            | TokenKind::KwVoid
            | TokenKind::KwStatic
            | TokenKind::KwExtern
            | TokenKind::KwStruct
            | TokenKind::KwUnion
            | TokenKind::KwEnum
            | TokenKind::KwConst
            | TokenKind::KwVolatile
            | TokenKind::KwRestrict
            | TokenKind::KwInline
            | TokenKind::KwNoreturn => {
                let decls = self.parse_declaration()?;
                Ok(decls.into_iter().map(BlockItem::Declaration).collect())
            }
            TokenKind::KwTypedef => {
                // ブロック内 typedef
                let results = self.parse_typedef()?;
                Ok(results
                    .into_iter()
                    .map(|(name, ty)| BlockItem::Typedef {
                        name,
                        underlying_type: ty,
                    })
                    .collect())
            }
            TokenKind::Identifier(name)
                if name == "va_list" || self.typedef_names.contains_key(name) =>
            {
                // typedef 名または va_list で始まる宣言
                let decls = self.parse_declaration()?;
                Ok(decls.into_iter().map(BlockItem::Declaration).collect())
            }
            _ => Ok(vec![BlockItem::Statement(self.parse_statement()?)]),
        }
    }

    /// `<declaration> ::= <storage_class>? <type_specifier> <declarator> ("=" <initializer>)? ("," <declarator> ("=" <initializer>)?)* ";"`
    ///
    /// カンマ区切り複数宣言をサポート。例: `int a = 1, b = 2, *p;`
    /// `base_type.clone()` で各宣言子に同じベース型を渡し、`parse_declarator()` が
    /// ポインタ・配列を宣言子ごとに付加する。
    fn parse_declaration(&mut self) -> Result<Vec<Declaration>> {
        let storage_class = self.parse_storage_class()?;
        let base_type = self.parse_type_specifier()?;

        // Chapter 18: `struct tag { ... };` — 構造体定義のみ（変数宣言なし）
        // enum { ... }; — enum 定義のみ（変数宣言なし）
        if (base_type.is_struct() || self.last_parsed_enum_def)
            && self.pos < self.tokens.len()
            && self.peek()?.kind == TokenKind::Semicolon
        {
            self.last_parsed_enum_def = false;
            self.advance()?; // consume ';'
            // ダミー宣言を返す（コード生成では無視される）
            return Ok(vec![Declaration {
                name: String::new(),
                var_type: base_type,
                init: None,
                storage_class,
            }]);
        }
        self.last_parsed_enum_def = false;

        let mut declarations = Vec::new();

        // 最初の宣言子
        let (var_type, name) = self.parse_declarator(base_type.clone())?;
        let init = if self.peek()?.kind == TokenKind::Assign {
            self.advance()?; // consume '='
            // Chapter 18: 複合初期化子 `{ expr, expr, ... }`
            if self.pos < self.tokens.len() && self.peek()?.kind == TokenKind::OpenBrace {
                Some(self.parse_compound_init()?)
            } else {
                Some(self.parse_assignment()?)
            }
        } else {
            None
        };
        declarations.push(Declaration {
            name,
            var_type,
            init,
            storage_class,
        });

        // カンマ区切りの追加宣言子
        while self.peek()?.kind == TokenKind::Comma {
            self.advance()?; // consume ','
            let (var_type, name) = self.parse_declarator(base_type.clone())?;
            let init = if self.peek()?.kind == TokenKind::Assign {
                self.advance()?; // consume '='
                if self.pos < self.tokens.len() && self.peek()?.kind == TokenKind::OpenBrace {
                    Some(self.parse_compound_init()?)
                } else {
                    Some(self.parse_assignment()?)
                }
            } else {
                None
            };
            declarations.push(Declaration {
                name,
                var_type,
                init,
                storage_class,
            });
        }

        self.expect(&TokenKind::Semicolon)?;
        Ok(declarations)
    }

    /// 複合初期化子のパース（Chapter 18）。
    ///
    /// `{ expr, expr, ... }` をパースする。末尾カンマを許容する。
    fn parse_compound_init(&mut self) -> Result<Expr> {
        self.expect(&TokenKind::OpenBrace)?;
        let mut inits = Vec::new();
        while self.peek()?.kind != TokenKind::CloseBrace {
            if self.peek()?.kind == TokenKind::OpenBrace {
                // ネスト初期化子: `{ { ... }, { ... } }`
                inits.push(self.parse_compound_init()?);
            } else {
                inits.push(self.parse_assignment()?);
            }
            if self.peek()?.kind == TokenKind::Comma {
                self.advance()?; // consume ','
            } else {
                break;
            }
        }
        self.expect(&TokenKind::CloseBrace)?;
        Ok(Expr::CompoundInit(inits))
    }

    /// `<statement> ::= "return" <exp> ";" | <exp> ";" | ";"
    ///                | "if" "(" <exp> ")" <statement> ("else" <statement>)?
    ///                | "{" <block_item>* "}"`
    fn parse_statement(&mut self) -> Result<Statement> {
        match &self.peek()?.kind {
            TokenKind::KwReturn => {
                self.advance()?;
                // Chapter 17: `return;` (no expression) for void functions
                if self.peek()?.kind == TokenKind::Semicolon {
                    self.advance()?;
                    Ok(Statement::Return(None))
                } else {
                    let expr = self.parse_expr()?;
                    self.expect(&TokenKind::Semicolon)?;
                    Ok(Statement::Return(Some(expr)))
                }
            }
            TokenKind::Semicolon => {
                self.advance()?;
                Ok(Statement::Null)
            }
            TokenKind::KwIf => {
                self.advance()?; // consume 'if'
                self.expect(&TokenKind::OpenParen)?;
                let condition = self.parse_expr()?;
                self.expect(&TokenKind::CloseParen)?;
                let then_branch = Box::new(self.parse_statement()?);
                // ダングリング else: 貪欲マッチ
                let else_branch =
                    if self.pos < self.tokens.len() && self.peek()?.kind == TokenKind::KwElse {
                        self.advance()?; // consume 'else'
                        Some(Box::new(self.parse_statement()?))
                    } else {
                        None
                    };
                Ok(Statement::If {
                    condition,
                    then_branch,
                    else_branch,
                })
            }
            TokenKind::OpenBrace => {
                self.advance()?; // consume '{'
                let mut items = Vec::new();
                while self.peek()?.kind != TokenKind::CloseBrace {
                    items.extend(self.parse_block_item()?);
                }
                self.expect(&TokenKind::CloseBrace)?;
                Ok(Statement::Compound(items))
            }
            // Chapter 8: while
            TokenKind::KwWhile => {
                self.advance()?;
                self.expect(&TokenKind::OpenParen)?;
                let condition = self.parse_expr()?;
                self.expect(&TokenKind::CloseParen)?;
                let body = Box::new(self.parse_statement()?);
                Ok(Statement::While { condition, body })
            }
            // Chapter 8: do-while
            TokenKind::KwDo => {
                self.advance()?;
                let body = Box::new(self.parse_statement()?);
                self.expect(&TokenKind::KwWhile)?;
                self.expect(&TokenKind::OpenParen)?;
                let condition = self.parse_expr()?;
                self.expect(&TokenKind::CloseParen)?;
                self.expect(&TokenKind::Semicolon)?;
                Ok(Statement::DoWhile { body, condition })
            }
            // Chapter 8: for
            TokenKind::KwFor => {
                self.advance()?;
                self.expect(&TokenKind::OpenParen)?;

                // for-init: 宣言 or 式文 or 空文
                let init = match &self.peek()?.kind {
                    TokenKind::KwInt
                    | TokenKind::KwLong
                    | TokenKind::KwUnsigned
                    | TokenKind::KwSigned
                    | TokenKind::KwDouble
                    | TokenKind::KwChar
                    | TokenKind::KwVoid
                    | TokenKind::KwStatic
                    | TokenKind::KwExtern
                    | TokenKind::KwStruct
                    | TokenKind::KwEnum => {
                        // parse_declaration() returns Vec<Declaration> for comma-separated decls
                        ForInit::Declaration(self.parse_declaration()?)
                    }
                    TokenKind::Identifier(name)
                        if name == "va_list" || self.typedef_names.contains_key(name) =>
                    {
                        // typedef 名または va_list で始まる宣言
                        ForInit::Declaration(self.parse_declaration()?)
                    }
                    TokenKind::Semicolon => {
                        self.advance()?;
                        ForInit::Expression(None)
                    }
                    _ => {
                        let expr = self.parse_expr()?;
                        self.expect(&TokenKind::Semicolon)?;
                        ForInit::Expression(Some(expr))
                    }
                };

                // condition（省略可能）
                let condition = if self.peek()?.kind != TokenKind::Semicolon {
                    Some(self.parse_expr()?)
                } else {
                    None
                };
                self.expect(&TokenKind::Semicolon)?;

                // post（省略可能）
                let post = if self.peek()?.kind != TokenKind::CloseParen {
                    Some(self.parse_expr()?)
                } else {
                    None
                };
                self.expect(&TokenKind::CloseParen)?;

                let body = Box::new(self.parse_statement()?);
                Ok(Statement::For {
                    init,
                    condition,
                    post,
                    body,
                })
            }
            // Chapter 8: break
            TokenKind::KwBreak => {
                self.advance()?;
                self.expect(&TokenKind::Semicolon)?;
                Ok(Statement::Break)
            }
            // Chapter 8: continue
            TokenKind::KwContinue => {
                self.advance()?;
                self.expect(&TokenKind::Semicolon)?;
                Ok(Statement::Continue)
            }
            // switch (expr) stmt
            TokenKind::KwSwitch => {
                self.advance()?;
                self.expect(&TokenKind::OpenParen)?;
                let expr = self.parse_expr()?;
                self.expect(&TokenKind::CloseParen)?;
                let body = Box::new(self.parse_statement()?);
                Ok(Statement::Switch { expr, body })
            }
            // case <const>: stmt
            TokenKind::KwCase => {
                self.advance()?;
                // 負値対応: '-' リテラル
                let negative = if self.peek()?.kind == TokenKind::Minus {
                    self.advance()?;
                    true
                } else {
                    false
                };
                let val_token = self.peek()?;
                let value = match &val_token.kind {
                    TokenKind::IntLiteral(v) => {
                        let v = *v;
                        self.advance()?;
                        v
                    }
                    TokenKind::LongLiteral(v) => {
                        let v = *v;
                        self.advance()?;
                        v
                    }
                    TokenKind::CharLiteral(v) => {
                        let v = *v as i64;
                        self.advance()?;
                        v
                    }
                    TokenKind::Identifier(name) => {
                        let name = name.clone();
                        self.advance()?;
                        *self.enum_constants.get(&name).ok_or_else(|| {
                            CompileError::ParseError(format!(
                                "expected constant expression in case label, got '{}'",
                                name
                            ))
                        })?
                    }
                    other => {
                        return Err(CompileError::ParseError(format!(
                            "expected constant expression in case label, got {:?}",
                            other
                        )));
                    }
                };
                let value = if negative { -value } else { value };
                self.expect(&TokenKind::Colon)?;
                let body = Box::new(self.parse_statement()?);
                Ok(Statement::Case { value, body })
            }
            // default: stmt
            TokenKind::KwDefault => {
                self.advance()?;
                self.expect(&TokenKind::Colon)?;
                let body = Box::new(self.parse_statement()?);
                Ok(Statement::Default(body))
            }
            TokenKind::KwGoto => {
                self.advance()?; // consume 'goto'
                let label_token = self.advance()?;
                let label = match &label_token.kind {
                    TokenKind::Identifier(name) => name.clone(),
                    other => {
                        return Err(CompileError::ParseError(format!(
                            "expected label name after 'goto', got {:?}",
                            other
                        )));
                    }
                };
                self.expect(&TokenKind::Semicolon)?;
                Ok(Statement::Goto(label))
            }
            // ラベル: `identifier:` — 先読みで `:` を確認
            TokenKind::Identifier(name)
                if self.pos + 1 < self.tokens.len()
                    && self.tokens[self.pos + 1].kind == TokenKind::Colon =>
            {
                let name = name.clone();
                self.advance()?; // consume identifier
                self.advance()?; // consume ':'
                let body = Box::new(self.parse_statement()?);
                Ok(Statement::Label { name, body })
            }
            _ => {
                let expr = self.parse_expr()?;
                self.expect(&TokenKind::Semicolon)?;
                Ok(Statement::Expression(expr))
            }
        }
    }

    /// 式のパース（Chapter 7 で拡張）。
    ///
    /// ```text
    /// <exp> ::= <assignment> ("," <assignment>)*
    /// ```
    ///
    /// カンマ演算子は最も低い優先順位。左辺を評価して捨て、右辺の値を返す。
    fn parse_expr(&mut self) -> Result<Expr> {
        let mut left = self.parse_assignment()?;
        while self.pos < self.tokens.len() && self.peek()?.kind == TokenKind::Comma {
            self.advance()?; // consume ','
            let right = self.parse_assignment()?;
            left = Expr::Binary(BinaryOp::Comma, Box::new(left), Box::new(right));
        }
        Ok(left)
    }

    /// 代入式のパース（Chapter 5-7, 14 で拡張）。
    ///
    /// ```text
    /// <assignment> ::= <lvalue_expr> <assign_op> <assignment> | <conditional>
    /// <assign_op>  ::= "=" | "+=" | "-=" | "*=" | "/=" | "%="
    /// ```
    ///
    /// Chapter 14: 左辺値が `*ptr` などの任意式に一般化された。
    /// 左辺を `parse_conditional()` でパースした後、代入演算子が続けば代入式とする。
    /// 左辺値の検証は型チェッカーに委譲する。
    fn parse_assignment(&mut self) -> Result<Expr> {
        let lhs = self.parse_conditional()?;

        if self.pos < self.tokens.len() {
            let op = match &self.peek()?.kind {
                TokenKind::Assign => Some(None), // 単純代入
                TokenKind::PlusAssign => Some(Some(BinaryOp::Add)),
                TokenKind::MinusAssign => Some(Some(BinaryOp::Subtract)),
                TokenKind::StarAssign => Some(Some(BinaryOp::Multiply)),
                TokenKind::SlashAssign => Some(Some(BinaryOp::Divide)),
                TokenKind::PercentAssign => Some(Some(BinaryOp::Remainder)),
                TokenKind::AmpersandAssign => Some(Some(BinaryOp::BitwiseAnd)),
                TokenKind::PipeAssign => Some(Some(BinaryOp::BitwiseOr)),
                TokenKind::CaretAssign => Some(Some(BinaryOp::BitwiseXor)),
                TokenKind::ShiftLeftAssign => Some(Some(BinaryOp::ShiftLeft)),
                TokenKind::ShiftRightAssign => Some(Some(BinaryOp::ShiftRight)),
                _ => None,
            };

            if let Some(compound_op) = op {
                self.advance()?; // consume assignment operator
                let rhs = self.parse_assignment()?; // 右結合
                return match compound_op {
                    None => Ok(Expr::Assign(Box::new(lhs), Box::new(rhs))),
                    Some(bin_op) => Ok(Expr::CompoundAssign(bin_op, Box::new(lhs), Box::new(rhs))),
                };
            }
        }

        Ok(lhs)
    }

    /// 三項演算子のパース（Chapter 6 で追加）。
    ///
    /// ```text
    /// <conditional> ::= <logical_or> ("?" <exp> ":" <conditional>)?
    /// ```
    ///
    /// `?` と `:` の間は完全な `<exp>`（代入含む）。
    /// `:` の右は `<conditional>` を再帰（右結合）。
    fn parse_conditional(&mut self) -> Result<Expr> {
        let condition = self.parse_logical_or()?;
        if self.pos < self.tokens.len() && self.peek()?.kind == TokenKind::Question {
            self.advance()?; // consume '?'
            let then_expr = self.parse_expr()?;
            self.expect(&TokenKind::Colon)?;
            let else_expr = self.parse_conditional()?; // 右結合
            Ok(Expr::Conditional {
                condition: Box::new(condition),
                then_expr: Box::new(then_expr),
                else_expr: Box::new(else_expr),
            })
        } else {
            Ok(condition)
        }
    }

    /// 論理ORの左結合パース。
    ///
    /// ```text
    /// <logical_or> ::= <logical_and> ( "||" <logical_and> )*
    /// ```
    fn parse_logical_or(&mut self) -> Result<Expr> {
        let mut left = self.parse_logical_and()?;
        loop {
            if self.pos >= self.tokens.len() {
                break;
            }
            match &self.peek()?.kind {
                TokenKind::OrOr => {
                    self.advance()?;
                    let right = self.parse_logical_and()?;
                    left = Expr::Binary(BinaryOp::LogicalOr, Box::new(left), Box::new(right));
                }
                _ => break,
            }
        }
        Ok(left)
    }

    /// 論理ANDの左結合パース。
    ///
    /// ```text
    /// <logical_and> ::= <bitwise_or> ( "&&" <bitwise_or> )*
    /// ```
    fn parse_logical_and(&mut self) -> Result<Expr> {
        let mut left = self.parse_bitwise_or()?;
        loop {
            if self.pos >= self.tokens.len() {
                break;
            }
            match &self.peek()?.kind {
                TokenKind::AndAnd => {
                    self.advance()?;
                    let right = self.parse_bitwise_or()?;
                    left = Expr::Binary(BinaryOp::LogicalAnd, Box::new(left), Box::new(right));
                }
                _ => break,
            }
        }
        Ok(left)
    }

    /// ビットORの左結合パース。
    ///
    /// ```text
    /// <bitwise_or> ::= <bitwise_xor> ( "|" <bitwise_xor> )*
    /// ```
    fn parse_bitwise_or(&mut self) -> Result<Expr> {
        let mut left = self.parse_bitwise_xor()?;
        loop {
            if self.pos >= self.tokens.len() {
                break;
            }
            match &self.peek()?.kind {
                TokenKind::Pipe => {
                    self.advance()?;
                    let right = self.parse_bitwise_xor()?;
                    left = Expr::Binary(BinaryOp::BitwiseOr, Box::new(left), Box::new(right));
                }
                _ => break,
            }
        }
        Ok(left)
    }

    /// ビットXORの左結合パース。
    ///
    /// ```text
    /// <bitwise_xor> ::= <bitwise_and> ( "^" <bitwise_and> )*
    /// ```
    fn parse_bitwise_xor(&mut self) -> Result<Expr> {
        let mut left = self.parse_bitwise_and()?;
        loop {
            if self.pos >= self.tokens.len() {
                break;
            }
            match &self.peek()?.kind {
                TokenKind::Caret => {
                    self.advance()?;
                    let right = self.parse_bitwise_and()?;
                    left = Expr::Binary(BinaryOp::BitwiseXor, Box::new(left), Box::new(right));
                }
                _ => break,
            }
        }
        Ok(left)
    }

    /// ビットANDの左結合パース。
    ///
    /// ```text
    /// <bitwise_and> ::= <equality> ( "&" <equality> )*
    /// ```
    fn parse_bitwise_and(&mut self) -> Result<Expr> {
        let mut left = self.parse_equality()?;
        loop {
            if self.pos >= self.tokens.len() {
                break;
            }
            match &self.peek()?.kind {
                TokenKind::Ampersand => {
                    self.advance()?;
                    let right = self.parse_equality()?;
                    left = Expr::Binary(BinaryOp::BitwiseAnd, Box::new(left), Box::new(right));
                }
                _ => break,
            }
        }
        Ok(left)
    }

    /// 等価演算の左結合パース。
    ///
    /// ```text
    /// <equality> ::= <relational> ( ("==" | "!=") <relational> )*
    /// ```
    fn parse_equality(&mut self) -> Result<Expr> {
        let mut left = self.parse_relational()?;
        loop {
            if self.pos >= self.tokens.len() {
                break;
            }
            let op = match &self.peek()?.kind {
                TokenKind::EqualEqual => BinaryOp::Equal,
                TokenKind::NotEqual => BinaryOp::NotEqual,
                _ => break,
            };
            self.advance()?;
            let right = self.parse_relational()?;
            left = Expr::Binary(op, Box::new(left), Box::new(right));
        }
        Ok(left)
    }

    /// 関係演算の左結合パース。
    ///
    /// ```text
    /// <relational> ::= <shift> ( ("<" | "<=" | ">" | ">=") <shift> )*
    /// ```
    fn parse_relational(&mut self) -> Result<Expr> {
        let mut left = self.parse_shift()?;
        loop {
            if self.pos >= self.tokens.len() {
                break;
            }
            let op = match &self.peek()?.kind {
                TokenKind::Less => BinaryOp::LessThan,
                TokenKind::LessEqual => BinaryOp::LessEqual,
                TokenKind::Greater => BinaryOp::GreaterThan,
                TokenKind::GreaterEqual => BinaryOp::GreaterEqual,
                _ => break,
            };
            self.advance()?;
            let right = self.parse_shift()?;
            left = Expr::Binary(op, Box::new(left), Box::new(right));
        }
        Ok(left)
    }

    /// シフト演算の左結合パース。
    ///
    /// ```text
    /// <shift> ::= <additive> ( ("<<" | ">>") <additive> )*
    /// ```
    fn parse_shift(&mut self) -> Result<Expr> {
        let mut left = self.parse_additive()?;
        loop {
            if self.pos >= self.tokens.len() {
                break;
            }
            let op = match &self.peek()?.kind {
                TokenKind::ShiftLeft => BinaryOp::ShiftLeft,
                TokenKind::ShiftRight => BinaryOp::ShiftRight,
                _ => break,
            };
            self.advance()?;
            let right = self.parse_additive()?;
            left = Expr::Binary(op, Box::new(left), Box::new(right));
        }
        Ok(left)
    }

    /// 加減算の左結合パース。
    ///
    /// ```text
    /// <additive> ::= <multiplicative> ( ("+" | "-") <multiplicative> )*
    /// ```
    fn parse_additive(&mut self) -> Result<Expr> {
        let mut left = self.parse_multiplicative()?;
        loop {
            if self.pos >= self.tokens.len() {
                break;
            }
            let op = match &self.peek()?.kind {
                TokenKind::Plus => BinaryOp::Add,
                TokenKind::Minus => BinaryOp::Subtract,
                _ => break,
            };
            self.advance()?; // 演算子を消費
            let right = self.parse_multiplicative()?;
            left = Expr::Binary(op, Box::new(left), Box::new(right));
        }
        Ok(left)
    }

    /// 乗除剰余の左結合パース。
    ///
    /// ```text
    /// <multiplicative> ::= <cast> ( ("*" | "/" | "%") <cast> )*
    /// ```
    fn parse_multiplicative(&mut self) -> Result<Expr> {
        let mut left = self.parse_cast()?;
        loop {
            if self.pos >= self.tokens.len() {
                break;
            }
            let op = match &self.peek()?.kind {
                TokenKind::Star => BinaryOp::Multiply,
                TokenKind::Slash => BinaryOp::Divide,
                TokenKind::Percent => BinaryOp::Remainder,
                _ => break,
            };
            self.advance()?; // 演算子を消費
            let right = self.parse_cast()?;
            left = Expr::Binary(op, Box::new(left), Box::new(right));
        }
        Ok(left)
    }

    /// キャスト式のパース（Chapter 14）。
    ///
    /// ```text
    /// <cast> ::= "(" <type> <abstract_declarator> ")" <cast> | <unary>
    /// ```
    ///
    /// `(` の次が型キーワードならキャスト式、そうでなければ `parse_unary()` に委譲。
    fn parse_cast(&mut self) -> Result<Expr> {
        // `(` の次が型キーワード/typedef 名ならキャスト式
        if self.pos < self.tokens.len()
            && self.peek()?.kind == TokenKind::OpenParen
            && self.pos + 1 < self.tokens.len()
            && self.is_type_token_at(self.pos + 1)
        {
            self.advance()?; // consume '('
            let base_type = self.parse_type_specifier()?;
            let target_type = self.parse_abstract_declarator(base_type)?;
            self.expect(&TokenKind::CloseParen)?;
            let inner = self.parse_cast()?; // 右結合
            return Ok(Expr::Cast {
                target_type,
                source_type: Type::Int, // プレースホルダー。型チェッカーが設定する。
                expr: Box::new(inner),
            });
        }
        self.parse_unary()
    }

    /// 単項演算のパース（右結合）。
    ///
    /// ```text
    /// <unary> ::= <unary_op> <unary> | <postfix>
    /// <unary_op> ::= "-" | "~" | "!" | "++" | "--" | "*" | "&"
    /// ```
    fn parse_unary(&mut self) -> Result<Expr> {
        let token = self.peek()?;
        match &token.kind {
            // Chapter 15: sizeof
            TokenKind::KwSizeof => {
                self.advance()?; // consume 'sizeof'
                // sizeof(type) or sizeof expr
                if self.pos < self.tokens.len()
                    && self.peek()?.kind == TokenKind::OpenParen
                    && self.pos + 1 < self.tokens.len()
                    && self.is_type_token_at(self.pos + 1)
                {
                    self.advance()?; // consume '('
                    let base_type = self.parse_type_specifier()?;
                    let ty = self.parse_abstract_declarator(base_type)?;
                    self.expect(&TokenKind::CloseParen)?;
                    return Ok(Expr::SizeOfType(ty));
                }
                // sizeof expr (unary precedence)
                let inner = self.parse_unary()?;
                Ok(Expr::SizeOfExpr(Box::new(inner)))
            }
            TokenKind::Minus | TokenKind::Tilde | TokenKind::Bang => {
                let op_token = self.advance()?;
                let op = match &op_token.kind {
                    TokenKind::Minus => UnaryOp::Negate,
                    TokenKind::Tilde => UnaryOp::Complement,
                    TokenKind::Bang => UnaryOp::Not,
                    _ => unreachable!(),
                };
                let inner = self.parse_cast()?;
                Ok(Expr::Unary(op, Box::new(inner)))
            }
            TokenKind::PlusPlus | TokenKind::MinusMinus => {
                let op_token = self.advance()?;
                let op = match &op_token.kind {
                    TokenKind::PlusPlus => UnaryOp::PreIncrement,
                    TokenKind::MinusMinus => UnaryOp::PreDecrement,
                    _ => unreachable!(),
                };
                let inner = self.parse_unary()?;
                Ok(Expr::Unary(op, Box::new(inner)))
            }
            // Chapter 14: `*expr` — 間接参照（dereference）
            TokenKind::Star => {
                self.advance()?;
                let inner = self.parse_cast()?;
                Ok(Expr::Dereref(Box::new(inner)))
            }
            // Chapter 14: `&expr` — アドレス取得
            TokenKind::Ampersand => {
                self.advance()?;
                let inner = self.parse_cast()?;
                Ok(Expr::AddrOf(Box::new(inner)))
            }
            _ => self.parse_postfix(),
        }
    }

    /// 後置演算のパース（Chapter 7）。
    ///
    /// ```text
    /// <postfix> ::= <primary> ("++" | "--")*
    /// ```
    fn parse_postfix(&mut self) -> Result<Expr> {
        let mut expr = self.parse_primary()?;
        while self.pos < self.tokens.len() {
            match &self.peek()?.kind {
                TokenKind::PlusPlus => {
                    self.advance()?;
                    expr = Expr::PostfixIncrement(Box::new(expr));
                }
                TokenKind::MinusMinus => {
                    self.advance()?;
                    expr = Expr::PostfixDecrement(Box::new(expr));
                }
                // Chapter 15: 配列添字 `arr[i]` → `*(arr + i)` に脱糖
                TokenKind::OpenBracket => {
                    self.advance()?; // consume '['
                    let index = self.parse_expr()?;
                    self.expect(&TokenKind::CloseBracket)?;
                    expr = Expr::Dereref(Box::new(Expr::Binary(
                        BinaryOp::Add,
                        Box::new(expr),
                        Box::new(index),
                    )));
                }
                // Chapter 18: `.member` — 直接メンバアクセス
                TokenKind::Dot => {
                    self.advance()?; // consume '.'
                    let member_token = self.advance()?;
                    let member_name = match &member_token.kind {
                        TokenKind::Identifier(name) => name.clone(),
                        other => {
                            return Err(CompileError::ParseError(format!(
                                "expected member name after '.', got {:?}",
                                other
                            )));
                        }
                    };
                    expr = Expr::Dot(Box::new(expr), member_name);
                }
                // 間接呼び出し: expr(args) — 関数ポインタ配列・メンバ経由の呼び出し
                TokenKind::OpenParen => {
                    self.advance()?; // consume '('
                    let mut args = Vec::new();
                    if self.peek()?.kind != TokenKind::CloseParen {
                        args.push(self.parse_assignment()?);
                        while self.peek()?.kind == TokenKind::Comma {
                            self.advance()?;
                            args.push(self.parse_assignment()?);
                        }
                    }
                    self.expect(&TokenKind::CloseParen)?;
                    expr = Expr::CallExpr(Box::new(expr), args);
                }
                // Chapter 18: `->member` — ポインタメンバアクセス → `(*ptr).member` に脱糖
                TokenKind::Arrow => {
                    self.advance()?; // consume '->'
                    let member_token = self.advance()?;
                    let member_name = match &member_token.kind {
                        TokenKind::Identifier(name) => name.clone(),
                        other => {
                            return Err(CompileError::ParseError(format!(
                                "expected member name after '->', got {:?}",
                                other
                            )));
                        }
                    };
                    expr = Expr::Dot(Box::new(Expr::Dereref(Box::new(expr))), member_name);
                }
                _ => break,
            }
        }
        Ok(expr)
    }

    /// 一次式のパース。
    ///
    /// ```text
    /// <primary> ::= <int> | <identifier> | "(" <exp> ")"
    /// ```
    fn parse_primary(&mut self) -> Result<Expr> {
        let token = self.peek()?;
        match &token.kind {
            TokenKind::IntLiteral(_) => {
                let token = self.advance()?;
                if let TokenKind::IntLiteral(value) = &token.kind {
                    Ok(Expr::Constant(*value))
                } else {
                    unreachable!()
                }
            }
            TokenKind::LongLiteral(_) => {
                let token = self.advance()?;
                if let TokenKind::LongLiteral(value) = &token.kind {
                    Ok(Expr::ConstantLong(*value))
                } else {
                    unreachable!()
                }
            }
            TokenKind::UIntLiteral(_) => {
                let token = self.advance()?;
                if let TokenKind::UIntLiteral(value) = &token.kind {
                    Ok(Expr::ConstantUInt(*value))
                } else {
                    unreachable!()
                }
            }
            TokenKind::ULongLiteral(_) => {
                let token = self.advance()?;
                if let TokenKind::ULongLiteral(value) = &token.kind {
                    Ok(Expr::ConstantULong(*value))
                } else {
                    unreachable!()
                }
            }
            TokenKind::DoubleLiteral(_) => {
                let token = self.advance()?;
                if let TokenKind::DoubleLiteral(value) = &token.kind {
                    Ok(Expr::ConstantDouble(*value))
                } else {
                    unreachable!()
                }
            }
            TokenKind::CharLiteral(_) => {
                let token = self.advance()?;
                if let TokenKind::CharLiteral(value) = &token.kind {
                    // C仕様: 文字定数は int 型
                    Ok(Expr::Constant(*value as i64))
                } else {
                    unreachable!()
                }
            }
            TokenKind::StringLiteral(_) => {
                let token = self.advance()?;
                if let TokenKind::StringLiteral(content) = &token.kind {
                    let mut combined = content.clone();
                    // C adjacent string literal concatenation
                    while self.pos < self.tokens.len() {
                        if let TokenKind::StringLiteral(next) = &self.tokens[self.pos].kind {
                            combined.push_str(next);
                            self.pos += 1;
                        } else {
                            break;
                        }
                    }
                    Ok(Expr::StringLiteral(combined))
                } else {
                    unreachable!()
                }
            }
            TokenKind::Identifier(_) => {
                let token = self.advance()?;
                if let TokenKind::Identifier(name) = &token.kind {
                    let name = name.clone();

                    // va_start(ap, last_named_param) — also __builtin_va_start
                    if name == "va_start" || name == "__builtin_va_start" {
                        self.expect(&TokenKind::OpenParen)?;
                        let ap = self.parse_assignment()?;
                        // 第2引数（最後の名前付きパラメータ）は無視（ABI で不要）
                        if self.peek()?.kind == TokenKind::Comma {
                            self.advance()?;
                            self.parse_assignment()?; // 読み捨て
                        }
                        self.expect(&TokenKind::CloseParen)?;
                        return Ok(Expr::VaStart(Box::new(ap)));
                    }

                    // va_arg(ap, type) — also __builtin_va_arg after preprocessing
                    if name == "va_arg" || name == "__builtin_va_arg" {
                        self.expect(&TokenKind::OpenParen)?;
                        let ap = self.parse_assignment()?;
                        self.expect(&TokenKind::Comma)?;
                        let arg_type = self.parse_type_specifier()?;
                        let arg_type = self.parse_abstract_declarator(arg_type)?;
                        self.expect(&TokenKind::CloseParen)?;
                        return Ok(Expr::VaArg {
                            ap: Box::new(ap),
                            arg_type,
                        });
                    }

                    // va_end(ap) — also __builtin_va_end
                    if name == "va_end" || name == "__builtin_va_end" {
                        self.expect(&TokenKind::OpenParen)?;
                        let ap = self.parse_assignment()?;
                        self.expect(&TokenKind::CloseParen)?;
                        return Ok(Expr::VaEnd(Box::new(ap)));
                    }

                    // __builtin_va_copy(dst, src) → copy va_list struct
                    if name == "__builtin_va_copy" || name == "va_copy" {
                        self.expect(&TokenKind::OpenParen)?;
                        let dst = self.parse_assignment()?;
                        self.expect(&TokenKind::Comma)?;
                        let src = self.parse_assignment()?;
                        self.expect(&TokenKind::CloseParen)?;
                        return Ok(Expr::VaCopy(Box::new(dst), Box::new(src)));
                    }

                    // enum 定数解決: 定数名 → Expr::Constant(value)
                    if let Some(&value) = self.enum_constants.get(&name) {
                        return Ok(Expr::Constant(value));
                    }

                    // 関数呼び出し: <identifier> "(" <args>? ")"
                    if self.pos < self.tokens.len() && self.peek()?.kind == TokenKind::OpenParen {
                        self.advance()?; // consume '('
                        let mut args = Vec::new();
                        if self.peek()?.kind != TokenKind::CloseParen {
                            // 各引数は parse_assignment() でパース（カンマ演算子と区別）
                            args.push(self.parse_assignment()?);
                            while self.peek()?.kind == TokenKind::Comma {
                                self.advance()?; // consume ','
                                args.push(self.parse_assignment()?);
                            }
                        }
                        self.expect(&TokenKind::CloseParen)?;
                        Ok(Expr::FunctionCall(name, args))
                    } else {
                        Ok(Expr::Var(name))
                    }
                } else {
                    unreachable!()
                }
            }
            TokenKind::OpenParen => {
                self.advance()?; // "(" を消費
                let inner = self.parse_expr()?;
                self.expect(&TokenKind::CloseParen)?;
                Ok(inner)
            }
            _ => {
                let token = self.advance()?;
                Err(CompileError::ParseError(format!(
                    "expected expression, got {:?} at line {}, column {}",
                    token.kind, token.span.line, token.span.column
                )))
            }
        }
    }

    /// 定数式を評価する（配列サイズ等のコンパイル時定数）。
    /// sizeof(type)、リテラル、算術演算、三項演算子を対応。
    fn eval_const_expr(expr: &Expr) -> Result<i64> {
        match expr {
            Expr::Constant(v) => Ok(*v),
            Expr::ConstantLong(v) => Ok(*v),
            Expr::ConstantUInt(v) => Ok(*v as i64),
            Expr::ConstantULong(v) => Ok(*v as i64),
            Expr::SizeOfType(ty) => Ok(ty.size() as i64),
            Expr::SizeOfExpr(inner) => {
                // sizeof(expr) — try to evaluate the inner expression
                Self::eval_const_expr(inner)
            }
            Expr::Unary(op, inner) => {
                let v = Self::eval_const_expr(inner)?;
                match op {
                    UnaryOp::Negate => Ok(-v),
                    UnaryOp::Complement => Ok(!v),
                    UnaryOp::Not => Ok(if v == 0 { 1 } else { 0 }),
                    _ => Err(CompileError::ParseError(
                        "unsupported unary operator in constant expression".to_string(),
                    )),
                }
            }
            Expr::Binary(op, left, right) => {
                let l = Self::eval_const_expr(left)?;
                let r = Self::eval_const_expr(right)?;
                Ok(match op {
                    BinaryOp::Add => l + r,
                    BinaryOp::Subtract => l - r,
                    BinaryOp::Multiply => l * r,
                    BinaryOp::Divide => {
                        if r == 0 {
                            0
                        } else {
                            l / r
                        }
                    }
                    BinaryOp::Remainder => {
                        if r == 0 {
                            0
                        } else {
                            l % r
                        }
                    }
                    BinaryOp::Equal => (l == r) as i64,
                    BinaryOp::NotEqual => (l != r) as i64,
                    BinaryOp::LessThan => (l < r) as i64,
                    BinaryOp::LessEqual => (l <= r) as i64,
                    BinaryOp::GreaterThan => (l > r) as i64,
                    BinaryOp::GreaterEqual => (l >= r) as i64,
                    BinaryOp::LogicalAnd => ((l != 0) && (r != 0)) as i64,
                    BinaryOp::LogicalOr => ((l != 0) || (r != 0)) as i64,
                    BinaryOp::BitwiseAnd => l & r,
                    BinaryOp::BitwiseOr => l | r,
                    BinaryOp::BitwiseXor => l ^ r,
                    BinaryOp::ShiftLeft => l << r,
                    BinaryOp::ShiftRight => l >> r,
                    BinaryOp::Comma => r, // comma operator: evaluate both, return right
                })
            }
            Expr::Conditional {
                condition,
                then_expr,
                else_expr,
            } => {
                let c = Self::eval_const_expr(condition)?;
                if c != 0 {
                    Self::eval_const_expr(then_expr)
                } else {
                    Self::eval_const_expr(else_expr)
                }
            }
            Expr::Cast { expr, .. } => Self::eval_const_expr(expr),
            _ => Err(CompileError::ParseError(format!(
                "unsupported expression in array size: {:?}",
                expr
            ))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lex;

    #[test]
    fn parse_return_2() {
        let tokens = lex::lex("int main(void) { return 2; }").unwrap();
        let program = parse(&tokens).unwrap();
        let func = match &program.declarations[0] {
            TopLevelDecl::Function(f) => f,
            _ => panic!("expected function"),
        };
        assert_eq!(func.name, "main");
        assert_eq!(
            *func.body.as_ref().unwrap(),
            vec![BlockItem::Statement(Statement::Return(Some(
                Expr::Constant(2)
            )))]
        );
    }

    #[test]
    fn parse_return_0() {
        let tokens = lex::lex("int main(void) { return 0; }").unwrap();
        let program = parse(&tokens).unwrap();
        let func = match &program.declarations[0] {
            TopLevelDecl::Function(f) => f,
            _ => panic!("expected function"),
        };
        assert_eq!(
            *func.body.as_ref().unwrap(),
            vec![BlockItem::Statement(Statement::Return(Some(
                Expr::Constant(0)
            )))]
        );
    }

    #[test]
    fn parse_missing_semicolon() {
        let tokens = lex::lex("int main(void) { return 2 }").unwrap();
        let result = parse(&tokens);
        assert!(result.is_err());
    }

    #[test]
    fn parse_extra_tokens() {
        let tokens = lex::lex("int main(void) { return 2; } int").unwrap();
        let result = parse(&tokens);
        assert!(result.is_err());
    }

    // ── Chapter 2 テスト ──

    /// `-5` → `Unary(Negate, Constant(5))`
    #[test]
    fn parse_negation() {
        let tokens = lex::lex("int main(void) { return -5; }").unwrap();
        let program = parse(&tokens).unwrap();
        let func = match &program.declarations[0] {
            TopLevelDecl::Function(f) => f,
            _ => panic!(),
        };
        assert_eq!(
            *func.body.as_ref().unwrap(),
            vec![BlockItem::Statement(Statement::Return(Some(Expr::Unary(
                UnaryOp::Negate,
                Box::new(Expr::Constant(5))
            ))))]
        );
    }

    /// `~0` → `Unary(Complement, Constant(0))`
    #[test]
    fn parse_complement() {
        let tokens = lex::lex("int main(void) { return ~0; }").unwrap();
        let program = parse(&tokens).unwrap();
        let func = match &program.declarations[0] {
            TopLevelDecl::Function(f) => f,
            _ => panic!(),
        };
        assert_eq!(
            *func.body.as_ref().unwrap(),
            vec![BlockItem::Statement(Statement::Return(Some(Expr::Unary(
                UnaryOp::Complement,
                Box::new(Expr::Constant(0))
            ))))]
        );
    }

    /// `!1` → `Unary(Not, Constant(1))`
    #[test]
    fn parse_logical_not() {
        let tokens = lex::lex("int main(void) { return !1; }").unwrap();
        let program = parse(&tokens).unwrap();
        let func = match &program.declarations[0] {
            TopLevelDecl::Function(f) => f,
            _ => panic!(),
        };
        assert_eq!(
            *func.body.as_ref().unwrap(),
            vec![BlockItem::Statement(Statement::Return(Some(Expr::Unary(
                UnaryOp::Not,
                Box::new(Expr::Constant(1))
            ))))]
        );
    }

    /// `--5` は Chapter 7 以降 `Unary(PreDecrement, Constant(5))` とパースされる
    #[test]
    fn parse_pre_decrement_literal() {
        let tokens = lex::lex("int main(void) { return --5; }").unwrap();
        let program = parse(&tokens).unwrap();
        let func = match &program.declarations[0] {
            TopLevelDecl::Function(f) => f,
            _ => panic!(),
        };
        assert_eq!(
            *func.body.as_ref().unwrap(),
            vec![BlockItem::Statement(Statement::Return(Some(Expr::Unary(
                UnaryOp::PreDecrement,
                Box::new(Expr::Constant(5))
            ))))]
        );
    }

    /// `- -5` は `Unary(Negate, Unary(Negate, Constant(5)))` とパースされる（スペース必要）
    #[test]
    fn parse_nested_negation() {
        let tokens = lex::lex("int main(void) { return - -5; }").unwrap();
        let program = parse(&tokens).unwrap();
        let func = match &program.declarations[0] {
            TopLevelDecl::Function(f) => f,
            _ => panic!(),
        };
        assert_eq!(
            *func.body.as_ref().unwrap(),
            vec![BlockItem::Statement(Statement::Return(Some(Expr::Unary(
                UnaryOp::Negate,
                Box::new(Expr::Unary(UnaryOp::Negate, Box::new(Expr::Constant(5))))
            ))))]
        );
    }

    /// `~(-3)` → 括弧で明示的にグループ化
    #[test]
    fn parse_complement_of_negation() {
        let tokens = lex::lex("int main(void) { return ~(-3); }").unwrap();
        let program = parse(&tokens).unwrap();
        let func = match &program.declarations[0] {
            TopLevelDecl::Function(f) => f,
            _ => panic!(),
        };
        assert_eq!(
            *func.body.as_ref().unwrap(),
            vec![BlockItem::Statement(Statement::Return(Some(Expr::Unary(
                UnaryOp::Complement,
                Box::new(Expr::Unary(UnaryOp::Negate, Box::new(Expr::Constant(3))))
            ))))]
        );
    }

    // ── Chapter 3 テスト ──

    /// `1 + 2` → `Binary(Add, Constant(1), Constant(2))`
    #[test]
    fn parse_addition() {
        let tokens = lex::lex("int main(void) { return 1 + 2; }").unwrap();
        let program = parse(&tokens).unwrap();
        let func = match &program.declarations[0] {
            TopLevelDecl::Function(f) => f,
            _ => panic!(),
        };
        assert_eq!(
            *func.body.as_ref().unwrap(),
            vec![BlockItem::Statement(Statement::Return(Some(Expr::Binary(
                BinaryOp::Add,
                Box::new(Expr::Constant(1)),
                Box::new(Expr::Constant(2)),
            ))))]
        );
    }

    /// `1 + 2 * 3` → 乗算が加算より優先度が高い
    #[test]
    fn parse_precedence() {
        let tokens = lex::lex("int main(void) { return 1 + 2 * 3; }").unwrap();
        let program = parse(&tokens).unwrap();
        let func = match &program.declarations[0] {
            TopLevelDecl::Function(f) => f,
            _ => panic!(),
        };
        assert_eq!(
            *func.body.as_ref().unwrap(),
            vec![BlockItem::Statement(Statement::Return(Some(Expr::Binary(
                BinaryOp::Add,
                Box::new(Expr::Constant(1)),
                Box::new(Expr::Binary(
                    BinaryOp::Multiply,
                    Box::new(Expr::Constant(2)),
                    Box::new(Expr::Constant(3)),
                )),
            ))))]
        );
    }

    /// `1 - 2 - 3` → 左結合
    #[test]
    fn parse_left_associativity() {
        let tokens = lex::lex("int main(void) { return 1 - 2 - 3; }").unwrap();
        let program = parse(&tokens).unwrap();
        let func = match &program.declarations[0] {
            TopLevelDecl::Function(f) => f,
            _ => panic!(),
        };
        assert_eq!(
            *func.body.as_ref().unwrap(),
            vec![BlockItem::Statement(Statement::Return(Some(Expr::Binary(
                BinaryOp::Subtract,
                Box::new(Expr::Binary(
                    BinaryOp::Subtract,
                    Box::new(Expr::Constant(1)),
                    Box::new(Expr::Constant(2)),
                )),
                Box::new(Expr::Constant(3)),
            ))))]
        );
    }

    /// `(1 + 2) * 3` → 括弧で優先度を変更
    #[test]
    fn parse_parenthesized_binary() {
        let tokens = lex::lex("int main(void) { return (1 + 2) * 3; }").unwrap();
        let program = parse(&tokens).unwrap();
        let func = match &program.declarations[0] {
            TopLevelDecl::Function(f) => f,
            _ => panic!(),
        };
        assert_eq!(
            *func.body.as_ref().unwrap(),
            vec![BlockItem::Statement(Statement::Return(Some(Expr::Binary(
                BinaryOp::Multiply,
                Box::new(Expr::Binary(
                    BinaryOp::Add,
                    Box::new(Expr::Constant(1)),
                    Box::new(Expr::Constant(2)),
                )),
                Box::new(Expr::Constant(3)),
            ))))]
        );
    }

    /// `7 / 2` → `Binary(Divide, Constant(7), Constant(2))`
    #[test]
    fn parse_division() {
        let tokens = lex::lex("int main(void) { return 7 / 2; }").unwrap();
        let program = parse(&tokens).unwrap();
        let func = match &program.declarations[0] {
            TopLevelDecl::Function(f) => f,
            _ => panic!(),
        };
        assert_eq!(
            *func.body.as_ref().unwrap(),
            vec![BlockItem::Statement(Statement::Return(Some(Expr::Binary(
                BinaryOp::Divide,
                Box::new(Expr::Constant(7)),
                Box::new(Expr::Constant(2)),
            ))))]
        );
    }

    /// `7 % 2` → `Binary(Remainder, Constant(7), Constant(2))`
    #[test]
    fn parse_remainder() {
        let tokens = lex::lex("int main(void) { return 7 % 2; }").unwrap();
        let program = parse(&tokens).unwrap();
        let func = match &program.declarations[0] {
            TopLevelDecl::Function(f) => f,
            _ => panic!(),
        };
        assert_eq!(
            *func.body.as_ref().unwrap(),
            vec![BlockItem::Statement(Statement::Return(Some(Expr::Binary(
                BinaryOp::Remainder,
                Box::new(Expr::Constant(7)),
                Box::new(Expr::Constant(2)),
            ))))]
        );
    }

    // ── Chapter 4 テスト ──

    /// `1 < 2`
    #[test]
    fn parse_less_than() {
        let tokens = lex::lex("int main(void) { return 1 < 2; }").unwrap();
        let program = parse(&tokens).unwrap();
        let func = match &program.declarations[0] {
            TopLevelDecl::Function(f) => f,
            _ => panic!(),
        };
        assert_eq!(
            *func.body.as_ref().unwrap(),
            vec![BlockItem::Statement(Statement::Return(Some(Expr::Binary(
                BinaryOp::LessThan,
                Box::new(Expr::Constant(1)),
                Box::new(Expr::Constant(2)),
            ))))]
        );
    }

    /// `1 == 2`
    #[test]
    fn parse_equal() {
        let tokens = lex::lex("int main(void) { return 1 == 2; }").unwrap();
        let program = parse(&tokens).unwrap();
        let func = match &program.declarations[0] {
            TopLevelDecl::Function(f) => f,
            _ => panic!(),
        };
        assert_eq!(
            *func.body.as_ref().unwrap(),
            vec![BlockItem::Statement(Statement::Return(Some(Expr::Binary(
                BinaryOp::Equal,
                Box::new(Expr::Constant(1)),
                Box::new(Expr::Constant(2)),
            ))))]
        );
    }

    /// `1 && 2`
    #[test]
    fn parse_logical_and() {
        let tokens = lex::lex("int main(void) { return 1 && 2; }").unwrap();
        let program = parse(&tokens).unwrap();
        let func = match &program.declarations[0] {
            TopLevelDecl::Function(f) => f,
            _ => panic!(),
        };
        assert_eq!(
            *func.body.as_ref().unwrap(),
            vec![BlockItem::Statement(Statement::Return(Some(Expr::Binary(
                BinaryOp::LogicalAnd,
                Box::new(Expr::Constant(1)),
                Box::new(Expr::Constant(2)),
            ))))]
        );
    }

    /// `1 || 2`
    #[test]
    fn parse_logical_or() {
        let tokens = lex::lex("int main(void) { return 1 || 2; }").unwrap();
        let program = parse(&tokens).unwrap();
        let func = match &program.declarations[0] {
            TopLevelDecl::Function(f) => f,
            _ => panic!(),
        };
        assert_eq!(
            *func.body.as_ref().unwrap(),
            vec![BlockItem::Statement(Statement::Return(Some(Expr::Binary(
                BinaryOp::LogicalOr,
                Box::new(Expr::Constant(1)),
                Box::new(Expr::Constant(2)),
            ))))]
        );
    }

    /// `1 < 2 && 3 > 1` — 関係演算子が論理ANDより優先度が高い
    #[test]
    fn parse_relational_and_logical() {
        let tokens = lex::lex("int main(void) { return 1 < 2 && 3 > 1; }").unwrap();
        let program = parse(&tokens).unwrap();
        let func = match &program.declarations[0] {
            TopLevelDecl::Function(f) => f,
            _ => panic!(),
        };
        assert_eq!(
            *func.body.as_ref().unwrap(),
            vec![BlockItem::Statement(Statement::Return(Some(Expr::Binary(
                BinaryOp::LogicalAnd,
                Box::new(Expr::Binary(
                    BinaryOp::LessThan,
                    Box::new(Expr::Constant(1)),
                    Box::new(Expr::Constant(2)),
                )),
                Box::new(Expr::Binary(
                    BinaryOp::GreaterThan,
                    Box::new(Expr::Constant(3)),
                    Box::new(Expr::Constant(1)),
                )),
            ))))]
        );
    }

    /// `2 + 3 > 4` — 加算が関係演算より優先度が高い
    #[test]
    fn parse_additive_in_relational() {
        let tokens = lex::lex("int main(void) { return 2 + 3 > 4; }").unwrap();
        let program = parse(&tokens).unwrap();
        let func = match &program.declarations[0] {
            TopLevelDecl::Function(f) => f,
            _ => panic!(),
        };
        assert_eq!(
            *func.body.as_ref().unwrap(),
            vec![BlockItem::Statement(Statement::Return(Some(Expr::Binary(
                BinaryOp::GreaterThan,
                Box::new(Expr::Binary(
                    BinaryOp::Add,
                    Box::new(Expr::Constant(2)),
                    Box::new(Expr::Constant(3)),
                )),
                Box::new(Expr::Constant(4)),
            ))))]
        );
    }

    /// `1 || 2 && 3` — `&&` が `||` より優先度が高い
    #[test]
    fn parse_or_and_precedence() {
        let tokens = lex::lex("int main(void) { return 1 || 2 && 3; }").unwrap();
        let program = parse(&tokens).unwrap();
        let func = match &program.declarations[0] {
            TopLevelDecl::Function(f) => f,
            _ => panic!(),
        };
        assert_eq!(
            *func.body.as_ref().unwrap(),
            vec![BlockItem::Statement(Statement::Return(Some(Expr::Binary(
                BinaryOp::LogicalOr,
                Box::new(Expr::Constant(1)),
                Box::new(Expr::Binary(
                    BinaryOp::LogicalAnd,
                    Box::new(Expr::Constant(2)),
                    Box::new(Expr::Constant(3)),
                )),
            ))))]
        );
    }

    /// `-1 + 2` → 単項マイナスが二項加算より優先度が高い
    #[test]
    fn parse_unary_in_binary() {
        let tokens = lex::lex("int main(void) { return -1 + 2; }").unwrap();
        let program = parse(&tokens).unwrap();
        let func = match &program.declarations[0] {
            TopLevelDecl::Function(f) => f,
            _ => panic!(),
        };
        assert_eq!(
            *func.body.as_ref().unwrap(),
            vec![BlockItem::Statement(Statement::Return(Some(Expr::Binary(
                BinaryOp::Add,
                Box::new(Expr::Unary(UnaryOp::Negate, Box::new(Expr::Constant(1)))),
                Box::new(Expr::Constant(2)),
            ))))]
        );
    }

    // ── Chapter 5 テスト ──

    /// 変数宣言と初期化: `int a = 5; return a;`
    #[test]
    fn parse_declaration_with_init() {
        let tokens = lex::lex("int main(void) { int a = 5; return a; }").unwrap();
        let program = parse(&tokens).unwrap();
        let func = match &program.declarations[0] {
            TopLevelDecl::Function(f) => f,
            _ => panic!(),
        };
        assert_eq!(
            *func.body.as_ref().unwrap(),
            vec![
                BlockItem::Declaration(Declaration {
                    name: "a".to_string(),
                    var_type: Type::Int,
                    init: Some(Expr::Constant(5)),
                    storage_class: None,
                }),
                BlockItem::Statement(Statement::Return(Some(Expr::Var("a".to_string())))),
            ]
        );
    }

    /// 初期化なし宣言: `int a;`
    #[test]
    fn parse_declaration_without_init() {
        let tokens = lex::lex("int main(void) { int a; return 0; }").unwrap();
        let program = parse(&tokens).unwrap();
        let func = match &program.declarations[0] {
            TopLevelDecl::Function(f) => f,
            _ => panic!(),
        };
        assert_eq!(
            *func.body.as_ref().unwrap(),
            vec![
                BlockItem::Declaration(Declaration {
                    name: "a".to_string(),
                    var_type: Type::Int,
                    init: None,
                    storage_class: None,
                }),
                BlockItem::Statement(Statement::Return(Some(Expr::Constant(0)))),
            ]
        );
    }

    /// 代入式: `a = 10;`
    #[test]
    fn parse_assignment() {
        let tokens = lex::lex("int main(void) { int a; a = 10; return a; }").unwrap();
        let program = parse(&tokens).unwrap();
        let func = match &program.declarations[0] {
            TopLevelDecl::Function(f) => f,
            _ => panic!(),
        };
        assert_eq!(
            *func.body.as_ref().unwrap(),
            vec![
                BlockItem::Declaration(Declaration {
                    name: "a".to_string(),
                    var_type: Type::Int,
                    init: None,
                    storage_class: None,
                }),
                BlockItem::Statement(Statement::Expression(Expr::Assign(
                    Box::new(Expr::Var("a".to_string())),
                    Box::new(Expr::Constant(10))
                ))),
                BlockItem::Statement(Statement::Return(Some(Expr::Var("a".to_string())))),
            ]
        );
    }

    /// 複数変数: `int a = 2; int b = 3; return a + b;`
    #[test]
    fn parse_multiple_declarations() {
        let tokens = lex::lex("int main(void) { int a = 2; int b = 3; return a + b; }").unwrap();
        let program = parse(&tokens).unwrap();
        let func = match &program.declarations[0] {
            TopLevelDecl::Function(f) => f,
            _ => panic!(),
        };
        assert_eq!(
            *func.body.as_ref().unwrap(),
            vec![
                BlockItem::Declaration(Declaration {
                    name: "a".to_string(),
                    var_type: Type::Int,
                    init: Some(Expr::Constant(2)),
                    storage_class: None,
                }),
                BlockItem::Declaration(Declaration {
                    name: "b".to_string(),
                    var_type: Type::Int,
                    init: Some(Expr::Constant(3)),
                    storage_class: None,
                }),
                BlockItem::Statement(Statement::Return(Some(Expr::Binary(
                    BinaryOp::Add,
                    Box::new(Expr::Var("a".to_string())),
                    Box::new(Expr::Var("b".to_string())),
                )))),
            ]
        );
    }

    /// 空文: `;`
    #[test]
    fn parse_null_statement() {
        let tokens = lex::lex("int main(void) { ; return 0; }").unwrap();
        let program = parse(&tokens).unwrap();
        let func = match &program.declarations[0] {
            TopLevelDecl::Function(f) => f,
            _ => panic!(),
        };
        assert_eq!(
            *func.body.as_ref().unwrap(),
            vec![
                BlockItem::Statement(Statement::Null),
                BlockItem::Statement(Statement::Return(Some(Expr::Constant(0)))),
            ]
        );
    }

    // ── Chapter 6 テスト ──

    /// if文: `if (1) return 2;`
    #[test]
    fn parse_if_statement() {
        let tokens = lex::lex("int main(void) { if (1) return 2; }").unwrap();
        let program = parse(&tokens).unwrap();
        let func = match &program.declarations[0] {
            TopLevelDecl::Function(f) => f,
            _ => panic!(),
        };
        assert_eq!(
            *func.body.as_ref().unwrap(),
            vec![BlockItem::Statement(Statement::If {
                condition: Expr::Constant(1),
                then_branch: Box::new(Statement::Return(Some(Expr::Constant(2)))),
                else_branch: None,
            })]
        );
    }

    /// if-else文: `if (0) return 2; else return 3;`
    #[test]
    fn parse_if_else_statement() {
        let tokens = lex::lex("int main(void) { if (0) return 2; else return 3; }").unwrap();
        let program = parse(&tokens).unwrap();
        let func = match &program.declarations[0] {
            TopLevelDecl::Function(f) => f,
            _ => panic!(),
        };
        assert_eq!(
            *func.body.as_ref().unwrap(),
            vec![BlockItem::Statement(Statement::If {
                condition: Expr::Constant(0),
                then_branch: Box::new(Statement::Return(Some(Expr::Constant(2)))),
                else_branch: Some(Box::new(Statement::Return(Some(Expr::Constant(3))))),
            })]
        );
    }

    /// 三項演算子: `return 1 ? 5 : 10;`
    #[test]
    fn parse_ternary() {
        let tokens = lex::lex("int main(void) { return 1 ? 5 : 10; }").unwrap();
        let program = parse(&tokens).unwrap();
        let func = match &program.declarations[0] {
            TopLevelDecl::Function(f) => f,
            _ => panic!(),
        };
        assert_eq!(
            *func.body.as_ref().unwrap(),
            vec![BlockItem::Statement(Statement::Return(Some(
                Expr::Conditional {
                    condition: Box::new(Expr::Constant(1)),
                    then_expr: Box::new(Expr::Constant(5)),
                    else_expr: Box::new(Expr::Constant(10)),
                }
            )))]
        );
    }

    /// 複合文: `{ int a = 2; }`
    #[test]
    fn parse_compound_statement() {
        let tokens = lex::lex("int main(void) { { int a = 2; } return 0; }").unwrap();
        let program = parse(&tokens).unwrap();
        let func = match &program.declarations[0] {
            TopLevelDecl::Function(f) => f,
            _ => panic!(),
        };
        assert_eq!(
            *func.body.as_ref().unwrap(),
            vec![
                BlockItem::Statement(Statement::Compound(vec![BlockItem::Declaration(
                    Declaration {
                        name: "a".to_string(),
                        var_type: Type::Int,
                        init: Some(Expr::Constant(2)),
                        storage_class: None,
                    }
                ),])),
                BlockItem::Statement(Statement::Return(Some(Expr::Constant(0)))),
            ]
        );
    }

    /// ダングリング else: `if (0) if (0) return 1; else return 2;`
    /// else は内側の if に結びつく
    #[test]
    fn parse_dangling_else() {
        let tokens = lex::lex("int main(void) { if (0) if (0) return 1; else return 2; }").unwrap();
        let program = parse(&tokens).unwrap();
        let func = match &program.declarations[0] {
            TopLevelDecl::Function(f) => f,
            _ => panic!(),
        };
        assert_eq!(
            *func.body.as_ref().unwrap(),
            vec![BlockItem::Statement(Statement::If {
                condition: Expr::Constant(0),
                then_branch: Box::new(Statement::If {
                    condition: Expr::Constant(0),
                    then_branch: Box::new(Statement::Return(Some(Expr::Constant(1)))),
                    else_branch: Some(Box::new(Statement::Return(Some(Expr::Constant(2))))),
                }),
                else_branch: None,
            })]
        );
    }

    // ── Chapter 7 テスト ──

    /// 複合代入: `a += 3`
    #[test]
    fn parse_compound_assign() {
        let tokens = lex::lex("int main(void) { int a = 5; a += 3; return a; }").unwrap();
        let program = parse(&tokens).unwrap();
        let func = match &program.declarations[0] {
            TopLevelDecl::Function(f) => f,
            _ => panic!(),
        };
        assert_eq!(
            func.body.as_ref().unwrap()[1],
            BlockItem::Statement(Statement::Expression(Expr::CompoundAssign(
                BinaryOp::Add,
                Box::new(Expr::Var("a".to_string())),
                Box::new(Expr::Constant(3))
            )))
        );
    }

    /// 前置インクリメント: `++a`
    #[test]
    fn parse_prefix_increment() {
        let tokens = lex::lex("int main(void) { int a = 5; return ++a; }").unwrap();
        let program = parse(&tokens).unwrap();
        let func = match &program.declarations[0] {
            TopLevelDecl::Function(f) => f,
            _ => panic!(),
        };
        assert_eq!(
            func.body.as_ref().unwrap()[1],
            BlockItem::Statement(Statement::Return(Some(Expr::Unary(
                UnaryOp::PreIncrement,
                Box::new(Expr::Var("a".to_string()))
            ))))
        );
    }

    /// 後置インクリメント: `a++`
    #[test]
    fn parse_postfix_increment() {
        let tokens = lex::lex("int main(void) { int a = 5; return a++; }").unwrap();
        let program = parse(&tokens).unwrap();
        let func = match &program.declarations[0] {
            TopLevelDecl::Function(f) => f,
            _ => panic!(),
        };
        assert_eq!(
            func.body.as_ref().unwrap()[1],
            BlockItem::Statement(Statement::Return(Some(Expr::PostfixIncrement(Box::new(
                Expr::Var("a".to_string())
            )))))
        );
    }

    /// 後置デクリメント: `a--`
    #[test]
    fn parse_postfix_decrement() {
        let tokens = lex::lex("int main(void) { int a = 5; return a--; }").unwrap();
        let program = parse(&tokens).unwrap();
        let func = match &program.declarations[0] {
            TopLevelDecl::Function(f) => f,
            _ => panic!(),
        };
        assert_eq!(
            func.body.as_ref().unwrap()[1],
            BlockItem::Statement(Statement::Return(Some(Expr::PostfixDecrement(Box::new(
                Expr::Var("a".to_string())
            )))))
        );
    }

    /// カンマ演算子: `(1, 2, 3)` → Binary(Comma, Binary(Comma, 1, 2), 3)
    #[test]
    fn parse_comma_operator() {
        let tokens = lex::lex("int main(void) { return (1, 2, 3); }").unwrap();
        let program = parse(&tokens).unwrap();
        let func = match &program.declarations[0] {
            TopLevelDecl::Function(f) => f,
            _ => panic!(),
        };
        assert_eq!(
            func.body.as_ref().unwrap()[0],
            BlockItem::Statement(Statement::Return(Some(Expr::Binary(
                BinaryOp::Comma,
                Box::new(Expr::Binary(
                    BinaryOp::Comma,
                    Box::new(Expr::Constant(1)),
                    Box::new(Expr::Constant(2)),
                )),
                Box::new(Expr::Constant(3)),
            ))))
        );
    }

    /// 宣言の初期化子ではカンマ演算子は使えない（カンマなしでパース）
    #[test]
    fn parse_declaration_no_comma_in_init() {
        // `int a = (1, 2);` — 括弧の中ではカンマが使える
        let tokens = lex::lex("int main(void) { int a = (1, 2); return a; }").unwrap();
        let program = parse(&tokens).unwrap();
        let func = match &program.declarations[0] {
            TopLevelDecl::Function(f) => f,
            _ => panic!(),
        };
        assert_eq!(
            func.body.as_ref().unwrap()[0],
            BlockItem::Declaration(Declaration {
                name: "a".to_string(),
                var_type: Type::Int,
                init: Some(Expr::Binary(
                    BinaryOp::Comma,
                    Box::new(Expr::Constant(1)),
                    Box::new(Expr::Constant(2)),
                )),
                storage_class: None,
            })
        );
    }

    /// 代入の右結合: `a = b = 5`
    #[test]
    fn parse_chained_assignment() {
        let tokens = lex::lex("int main(void) { int a; int b; a = b = 5; return a; }").unwrap();
        let program = parse(&tokens).unwrap();
        let func = match &program.declarations[0] {
            TopLevelDecl::Function(f) => f,
            _ => panic!(),
        };
        assert_eq!(
            func.body.as_ref().unwrap()[2],
            BlockItem::Statement(Statement::Expression(Expr::Assign(
                Box::new(Expr::Var("a".to_string())),
                Box::new(Expr::Assign(
                    Box::new(Expr::Var("b".to_string())),
                    Box::new(Expr::Constant(5))
                ))
            )))
        );
    }

    // ── Chapter 8 テスト ──

    /// while文: `while (1) return 0;`
    #[test]
    fn parse_while_statement() {
        let tokens = lex::lex("int main(void) { while (1) return 0; }").unwrap();
        let program = parse(&tokens).unwrap();
        let func = match &program.declarations[0] {
            TopLevelDecl::Function(f) => f,
            _ => panic!(),
        };
        assert_eq!(
            func.body.as_ref().unwrap()[0],
            BlockItem::Statement(Statement::While {
                condition: Expr::Constant(1),
                body: Box::new(Statement::Return(Some(Expr::Constant(0)))),
            })
        );
    }

    /// do-while文: `do { a = 1; } while (0);`
    #[test]
    fn parse_do_while_statement() {
        let tokens = lex::lex("int main(void) { int a; do { a = 1; } while (0); }").unwrap();
        let program = parse(&tokens).unwrap();
        let func = match &program.declarations[0] {
            TopLevelDecl::Function(f) => f,
            _ => panic!(),
        };
        assert_eq!(
            func.body.as_ref().unwrap()[1],
            BlockItem::Statement(Statement::DoWhile {
                body: Box::new(Statement::Compound(vec![BlockItem::Statement(
                    Statement::Expression(Expr::Assign(
                        Box::new(Expr::Var("a".to_string())),
                        Box::new(Expr::Constant(1))
                    ))
                ),])),
                condition: Expr::Constant(0),
            })
        );
    }

    /// for文（宣言付き）: `for (int i = 0; i < 5; i++) a++;`
    #[test]
    fn parse_for_with_declaration() {
        let tokens =
            lex::lex("int main(void) { int a; for (int i = 0; i < 5; i++) a++; }").unwrap();
        let program = parse(&tokens).unwrap();
        let func = match &program.declarations[0] {
            TopLevelDecl::Function(f) => f,
            _ => panic!(),
        };
        if let BlockItem::Statement(Statement::For {
            init,
            condition,
            post,
            body: _,
        }) = &func.body.as_ref().unwrap()[1]
        {
            assert_eq!(
                *init,
                ForInit::Declaration(vec![Declaration {
                    name: "i".to_string(),
                    var_type: Type::Int,
                    init: Some(Expr::Constant(0)),
                    storage_class: None,
                }])
            );
            assert!(condition.is_some());
            assert!(post.is_some());
        } else {
            panic!("expected For statement");
        }
    }

    /// for文（全省略）: `for (;;) break;`
    #[test]
    fn parse_for_empty() {
        let tokens = lex::lex("int main(void) { for (;;) break; }").unwrap();
        let program = parse(&tokens).unwrap();
        let func = match &program.declarations[0] {
            TopLevelDecl::Function(f) => f,
            _ => panic!(),
        };
        assert_eq!(
            func.body.as_ref().unwrap()[0],
            BlockItem::Statement(Statement::For {
                init: ForInit::Expression(None),
                condition: None,
                post: None,
                body: Box::new(Statement::Break),
            })
        );
    }

    /// break文
    #[test]
    fn parse_break_statement() {
        let tokens = lex::lex("int main(void) { while (1) break; }").unwrap();
        let program = parse(&tokens).unwrap();
        let func = match &program.declarations[0] {
            TopLevelDecl::Function(f) => f,
            _ => panic!(),
        };
        assert_eq!(
            func.body.as_ref().unwrap()[0],
            BlockItem::Statement(Statement::While {
                condition: Expr::Constant(1),
                body: Box::new(Statement::Break),
            })
        );
    }

    /// continue文
    #[test]
    fn parse_continue_statement() {
        let tokens = lex::lex("int main(void) { while (1) continue; }").unwrap();
        let program = parse(&tokens).unwrap();
        let func = match &program.declarations[0] {
            TopLevelDecl::Function(f) => f,
            _ => panic!(),
        };
        assert_eq!(
            func.body.as_ref().unwrap()[0],
            BlockItem::Statement(Statement::While {
                condition: Expr::Constant(1),
                body: Box::new(Statement::Continue),
            })
        );
    }

    // ── Chapter 9 テスト ──

    /// 関数呼び出し: `foo()`
    #[test]
    fn parse_function_call_no_args() {
        let tokens =
            lex::lex("int five(void) { return 5; } int main(void) { return five(); }").unwrap();
        let program = parse(&tokens).unwrap();
        assert_eq!(program.declarations.len(), 2);
        let f0 = match &program.declarations[0] {
            TopLevelDecl::Function(f) => f,
            _ => panic!(),
        };
        let f1 = match &program.declarations[1] {
            TopLevelDecl::Function(f) => f,
            _ => panic!(),
        };
        assert_eq!(f0.name, "five");
        assert_eq!(f0.params, Vec::<(Type, String)>::new());
        assert_eq!(f1.name, "main");
        assert_eq!(
            f1.body.as_ref().unwrap()[0],
            BlockItem::Statement(Statement::Return(Some(Expr::FunctionCall(
                "five".to_string(),
                vec![]
            ))))
        );
    }

    /// 関数呼び出し: `add(2, 3)`
    #[test]
    fn parse_function_call_with_args() {
        let tokens = lex::lex(
            "int add(int a, int b) { return a + b; } int main(void) { return add(2, 3); }",
        )
        .unwrap();
        let program = parse(&tokens).unwrap();
        let f0 = match &program.declarations[0] {
            TopLevelDecl::Function(f) => f,
            _ => panic!(),
        };
        let f1 = match &program.declarations[1] {
            TopLevelDecl::Function(f) => f,
            _ => panic!(),
        };
        assert_eq!(f0.name, "add");
        assert_eq!(
            f0.params,
            vec![(Type::Int, "a".to_string()), (Type::Int, "b".to_string())]
        );
        assert_eq!(
            f1.body.as_ref().unwrap()[0],
            BlockItem::Statement(Statement::Return(Some(Expr::FunctionCall(
                "add".to_string(),
                vec![Expr::Constant(2), Expr::Constant(3),]
            ))))
        );
    }

    /// 関数宣言（プロトタイプ）
    #[test]
    fn parse_function_declaration() {
        let tokens =
            lex::lex("int add(int a, int b); int main(void) { return add(1, 2); }").unwrap();
        let program = parse(&tokens).unwrap();
        assert_eq!(program.declarations.len(), 2);
        let f0 = match &program.declarations[0] {
            TopLevelDecl::Function(f) => f,
            _ => panic!(),
        };
        assert_eq!(f0.name, "add");
        assert!(f0.body.is_none());
        assert_eq!(
            f0.params,
            vec![(Type::Int, "a".to_string()), (Type::Int, "b".to_string())]
        );
    }

    /// 複数関数: 宣言+定義
    #[test]
    fn parse_declaration_then_definition() {
        let tokens = lex::lex("int add(int a, int b); int main(void) { return add(10, 20); } int add(int a, int b) { return a + b; }").unwrap();
        let program = parse(&tokens).unwrap();
        assert_eq!(program.declarations.len(), 3);
        let f0 = match &program.declarations[0] {
            TopLevelDecl::Function(f) => f,
            _ => panic!(),
        };
        let f1 = match &program.declarations[1] {
            TopLevelDecl::Function(f) => f,
            _ => panic!(),
        };
        let f2 = match &program.declarations[2] {
            TopLevelDecl::Function(f) => f,
            _ => panic!(),
        };
        assert!(f0.body.is_none());
        assert!(f1.body.is_some());
        assert!(f2.body.is_some());
    }

    /// 引数中のカンマはカンマ演算子ではなく引数区切りとしてパースされる
    #[test]
    fn parse_function_call_comma_not_operator() {
        let tokens = lex::lex(
            "int foo(int a, int b, int c) { return a; } int main(void) { return foo(1, 2, 3); }",
        )
        .unwrap();
        let program = parse(&tokens).unwrap();
        let f1 = match &program.declarations[1] {
            TopLevelDecl::Function(f) => f,
            _ => panic!(),
        };
        assert_eq!(
            f1.body.as_ref().unwrap()[0],
            BlockItem::Statement(Statement::Return(Some(Expr::FunctionCall(
                "foo".to_string(),
                vec![Expr::Constant(1), Expr::Constant(2), Expr::Constant(3),]
            ))))
        );
    }

    // ── Chapter 10 テスト ──

    /// グローバル変数宣言
    #[test]
    fn parse_global_variable() {
        let tokens = lex::lex("int x = 5; int main(void) { return x; }").unwrap();
        let program = parse(&tokens).unwrap();
        assert_eq!(program.declarations.len(), 2);
        match &program.declarations[0] {
            TopLevelDecl::Variable(decl) => {
                assert_eq!(decl.name, "x");
                assert_eq!(decl.init, Some(Expr::Constant(5)));
                assert_eq!(decl.storage_class, None);
            }
            _ => panic!("expected variable declaration"),
        }
    }

    /// static 関数
    #[test]
    fn parse_static_function() {
        let tokens =
            lex::lex("static int helper(void) { return 42; } int main(void) { return helper(); }")
                .unwrap();
        let program = parse(&tokens).unwrap();
        let f0 = match &program.declarations[0] {
            TopLevelDecl::Function(f) => f,
            _ => panic!(),
        };
        assert_eq!(f0.name, "helper");
        assert_eq!(f0.storage_class, Some(StorageClass::Static));
    }

    /// extern 変数宣言
    #[test]
    fn parse_extern_variable() {
        let tokens = lex::lex("extern int x; int main(void) { return x; }").unwrap();
        let program = parse(&tokens).unwrap();
        match &program.declarations[0] {
            TopLevelDecl::Variable(decl) => {
                assert_eq!(decl.name, "x");
                assert_eq!(decl.init, None);
                assert_eq!(decl.storage_class, Some(StorageClass::Extern));
            }
            _ => panic!("expected variable declaration"),
        }
    }

    /// ブロック内 static 変数
    #[test]
    fn parse_block_static_variable() {
        let tokens = lex::lex("int main(void) { static int c = 0; return c; }").unwrap();
        let program = parse(&tokens).unwrap();
        let func = match &program.declarations[0] {
            TopLevelDecl::Function(f) => f,
            _ => panic!(),
        };
        assert_eq!(
            func.body.as_ref().unwrap()[0],
            BlockItem::Declaration(Declaration {
                name: "c".to_string(),
                var_type: Type::Int,
                init: Some(Expr::Constant(0)),
                storage_class: Some(StorageClass::Static),
            })
        );
    }

    /// ブロック内 extern 変数
    #[test]
    fn parse_block_extern_variable() {
        let tokens = lex::lex("int main(void) { extern int x; return x; }").unwrap();
        let program = parse(&tokens).unwrap();
        let func = match &program.declarations[0] {
            TopLevelDecl::Function(f) => f,
            _ => panic!(),
        };
        assert_eq!(
            func.body.as_ref().unwrap()[0],
            BlockItem::Declaration(Declaration {
                name: "x".to_string(),
                var_type: Type::Int,
                init: None,
                storage_class: Some(StorageClass::Extern),
            })
        );
    }

    // ── Chapter 14 テスト ──

    /// ポインタ変数宣言: `int *ptr;`
    #[test]
    fn parse_pointer_declaration() {
        let tokens = lex::lex("int main(void) { int *ptr; return 0; }").unwrap();
        let program = parse(&tokens).unwrap();
        let func = match &program.declarations[0] {
            TopLevelDecl::Function(f) => f,
            _ => panic!(),
        };
        assert_eq!(
            func.body.as_ref().unwrap()[0],
            BlockItem::Declaration(Declaration {
                name: "ptr".to_string(),
                var_type: Type::Pointer(Box::new(Type::Int)),
                init: None,
                storage_class: None,
            })
        );
    }

    /// 多重ポインタ宣言: `int **pp;`
    #[test]
    fn parse_double_pointer_declaration() {
        let tokens = lex::lex("int main(void) { int **pp; return 0; }").unwrap();
        let program = parse(&tokens).unwrap();
        let func = match &program.declarations[0] {
            TopLevelDecl::Function(f) => f,
            _ => panic!(),
        };
        assert_eq!(
            func.body.as_ref().unwrap()[0],
            BlockItem::Declaration(Declaration {
                name: "pp".to_string(),
                var_type: Type::Pointer(Box::new(Type::Pointer(Box::new(Type::Int)))),
                init: None,
                storage_class: None,
            })
        );
    }

    /// アドレス取得: `&x`
    #[test]
    fn parse_address_of() {
        let tokens = lex::lex("int main(void) { int x; int *p = &x; return 0; }").unwrap();
        let program = parse(&tokens).unwrap();
        let func = match &program.declarations[0] {
            TopLevelDecl::Function(f) => f,
            _ => panic!(),
        };
        assert_eq!(
            func.body.as_ref().unwrap()[1],
            BlockItem::Declaration(Declaration {
                name: "p".to_string(),
                var_type: Type::Pointer(Box::new(Type::Int)),
                init: Some(Expr::AddrOf(Box::new(Expr::Var("x".to_string())))),
                storage_class: None,
            })
        );
    }

    /// 間接参照: `*p`
    #[test]
    fn parse_dereference() {
        let tokens = lex::lex("int main(void) { int x = 5; int *p = &x; return *p; }").unwrap();
        let program = parse(&tokens).unwrap();
        let func = match &program.declarations[0] {
            TopLevelDecl::Function(f) => f,
            _ => panic!(),
        };
        assert_eq!(
            func.body.as_ref().unwrap()[2],
            BlockItem::Statement(Statement::Return(Some(Expr::Dereref(Box::new(Expr::Var(
                "p".to_string()
            ))))))
        );
    }

    /// ポインタ経由の書き込み: `*p = 42;`
    #[test]
    fn parse_dereference_assign() {
        let tokens = lex::lex("int main(void) { int x; int *p = &x; *p = 42; return 0; }").unwrap();
        let program = parse(&tokens).unwrap();
        let func = match &program.declarations[0] {
            TopLevelDecl::Function(f) => f,
            _ => panic!(),
        };
        assert_eq!(
            func.body.as_ref().unwrap()[2],
            BlockItem::Statement(Statement::Expression(Expr::Assign(
                Box::new(Expr::Dereref(Box::new(Expr::Var("p".to_string())))),
                Box::new(Expr::Constant(42))
            )))
        );
    }

    /// キャスト式: `(int *)0`
    #[test]
    fn parse_cast_to_pointer() {
        let tokens = lex::lex("int main(void) { int *p = (int *)0; return 0; }").unwrap();
        let program = parse(&tokens).unwrap();
        let func = match &program.declarations[0] {
            TopLevelDecl::Function(f) => f,
            _ => panic!(),
        };
        if let BlockItem::Declaration(decl) = &func.body.as_ref().unwrap()[0] {
            assert_eq!(decl.name, "p");
            assert_eq!(decl.var_type, Type::Pointer(Box::new(Type::Int)));
            match &decl.init {
                Some(Expr::Cast {
                    target_type, expr, ..
                }) => {
                    assert_eq!(*target_type, Type::Pointer(Box::new(Type::Int)));
                    assert_eq!(**expr, Expr::Constant(0));
                }
                other => panic!("expected Cast, got {:?}", other),
            }
        } else {
            panic!("expected declaration");
        }
    }

    /// ポインタ型パラメータ: `int *p`
    #[test]
    fn parse_pointer_parameter() {
        let tokens = lex::lex("int deref(int *p) { return *p; }").unwrap();
        let program = parse(&tokens).unwrap();
        let func = match &program.declarations[0] {
            TopLevelDecl::Function(f) => f,
            _ => panic!(),
        };
        assert_eq!(func.name, "deref");
        assert_eq!(
            func.params,
            vec![(Type::Pointer(Box::new(Type::Int)), "p".to_string())]
        );
        assert_eq!(
            func.body.as_ref().unwrap()[0],
            BlockItem::Statement(Statement::Return(Some(Expr::Dereref(Box::new(Expr::Var(
                "p".to_string()
            ))))))
        );
    }

    /// ポインタ戻り値型: `int *return_ptr(int *p)`
    #[test]
    fn parse_pointer_return_type() {
        let tokens = lex::lex("int *identity(int *p) { return p; }").unwrap();
        let program = parse(&tokens).unwrap();
        let func = match &program.declarations[0] {
            TopLevelDecl::Function(f) => f,
            _ => panic!(),
        };
        assert_eq!(func.name, "identity");
        assert_eq!(func.return_type, Type::Pointer(Box::new(Type::Int)));
        assert_eq!(
            func.params,
            vec![(Type::Pointer(Box::new(Type::Int)), "p".to_string())]
        );
    }

    /// グローバルポインタ変数: `int *g;`
    #[test]
    fn parse_global_pointer_variable() {
        let tokens = lex::lex("int *g; int main(void) { return 0; }").unwrap();
        let program = parse(&tokens).unwrap();
        match &program.declarations[0] {
            TopLevelDecl::Variable(decl) => {
                assert_eq!(decl.name, "g");
                assert_eq!(decl.var_type, Type::Pointer(Box::new(Type::Int)));
            }
            _ => panic!("expected variable declaration"),
        }
    }

    /// 可変長引数関数の宣言: `int printf(char *fmt, ...);`
    #[test]
    fn parse_variadic_function_declaration() {
        let tokens = lex::lex("int printf(char *fmt, ...); int main(void) { return 0; }").unwrap();
        let program = parse(&tokens).unwrap();
        match &program.declarations[0] {
            TopLevelDecl::Function(func) => {
                assert_eq!(func.name, "printf");
                assert!(func.is_variadic);
                assert_eq!(func.params.len(), 1);
                assert_eq!(func.params[0].0, Type::Pointer(Box::new(Type::Char)));
            }
            _ => panic!("expected function declaration"),
        }
    }

    /// 非可変長関数は is_variadic == false
    #[test]
    fn parse_non_variadic_function() {
        let tokens = lex::lex("int foo(int a, int b); int main(void) { return 0; }").unwrap();
        let program = parse(&tokens).unwrap();
        match &program.declarations[0] {
            TopLevelDecl::Function(func) => {
                assert_eq!(func.name, "foo");
                assert!(!func.is_variadic);
                assert_eq!(func.params.len(), 2);
            }
            _ => panic!("expected function declaration"),
        }
    }

    /// int f(); — 引数未指定の前方宣言
    #[test]
    fn parse_unspecified_params_declaration() {
        let tokens = lex::lex("int f();").unwrap();
        let program = parse(&tokens).unwrap();
        assert_eq!(program.declarations.len(), 1);
        match &program.declarations[0] {
            TopLevelDecl::Function(func) => {
                assert_eq!(func.name, "f");
                assert_eq!(func.params.len(), 0);
                assert!(!func.has_prototype);
                assert!(func.body.is_none());
            }
            _ => panic!("expected function declaration"),
        }
    }
}
