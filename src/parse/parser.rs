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
//! # 対応する文法（Chapter 15）
//! ```text
//! <program>        ::= <top_level_decl>*
//! <top_level_decl> ::= <function_decl> | <variable_decl>
//! <function_decl>  ::= <storage_class>? <type> <declarator> "(" <params> ")" ( "{" <block>* "}" | ";" )
//! <variable_decl>  ::= <storage_class>? <type> <declarator> ("=" <expr>)? ";"
//! <declarator>     ::= "*"* <identifier> ("[" <int> "]")?      ← Ch15: 配列宣言子
//! <type>           ::= <type_specifier>+
//! <type_specifier> ::= "int" | "long" | "signed" | "unsigned" | "double"
//! <storage_class>  ::= "static" | "extern"
//! <params>         ::= "void" | <type> <declarator> ("," <type> <declarator>)*
//! <block_item>     ::= <statement> | <declaration>
//! <declaration>    ::= <storage_class>? <type> <declarator> ("=" <assignment>)? ";"
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
//! <assign_op>      ::= "=" | "+=" | "-=" | "*=" | "/=" | "%="
//! <conditional>    ::= <logical_or> ("?" <exp> ":" <conditional>)?
//! <logical_or>     ::= <logical_and> ( "||" <logical_and> )*
//! <logical_and>    ::= <equality> ( "&&" <equality> )*
//! <equality>       ::= <relational> ( ("==" | "!=") <relational> )*
//! <relational>     ::= <additive> ( ("<" | "<=" | ">" | ">=") <additive> )*
//! <additive>       ::= <multiplicative> ( ("+" | "-") <multiplicative> )*
//! <multiplicative> ::= <cast> ( ("*" | "/" | "%") <cast> )*
//! <cast>           ::= "(" <type> <abstract_declarator> ")" <cast>
//!                    | <unary>
//! <abstract_declarator> ::= "*"* ("[" <int> "]")?              ← Ch15: 配列サフィックス
//! <unary>          ::= <unary_op> <cast> | "sizeof" <unary>    ← Ch15: sizeof
//!                    | "sizeof" "(" <type> <abstract_declarator> ")"
//!                    | <postfix>
//! <unary_op>       ::= "-" | "~" | "!" | "++" | "--" | "*" | "&"
//! <postfix>        ::= <primary> ("++" | "--" | "[" <exp> "]")* ← Ch15: 配列添字
//! <primary>        ::= <int> | <long> | <uint> | <ulong> | <double>
//!                    | <identifier> ("(" <args>? ")")?
//!                    | "(" <exp> ")"
//! <args>           ::= <assignment> ("," <assignment>)*
//! ```

use crate::error::{CompileError, Result};
use crate::lex::{Token, TokenKind};
use super::ast::{Program, FunctionDecl, BlockItem, Declaration, Statement, Expr, UnaryOp, BinaryOp, ForInit, StorageClass, TopLevelDecl, Type};

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

/// パーサーの内部状態。トークン列への参照と現在の読み取り位置を保持する。
struct Parser<'a> {
    tokens: &'a [Token],
    pos: usize,
}

impl<'a> Parser<'a> {
    fn new(tokens: &'a [Token]) -> Self {
        Self { tokens, pos: 0 }
    }

    /// 現在位置のトークンを消費せずに参照する（先読み）。
    /// 式のパースで「次のトークンが何か」に応じて分岐する際に使う。
    fn peek(&self) -> Result<&'a Token> {
        self.tokens.get(self.pos).ok_or_else(|| {
            CompileError::ParseError("unexpected end of input".to_string())
        })
    }

    /// 現在位置のトークンを消費して返す。位置を1つ進める。
    fn advance(&mut self) -> Result<&'a Token> {
        let token = self.tokens.get(self.pos).ok_or_else(|| {
            CompileError::ParseError("unexpected end of input".to_string())
        })?;
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
    fn is_type_keyword(&self) -> bool {
        if self.pos >= self.tokens.len() {
            return false;
        }
        matches!(
            self.tokens[self.pos].kind,
            TokenKind::KwInt | TokenKind::KwLong | TokenKind::KwUnsigned
            | TokenKind::KwSigned | TokenKind::KwDouble | TokenKind::KwChar
        )
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
        let mut count = 0;

        loop {
            if self.pos >= self.tokens.len() {
                break;
            }
            match &self.peek()?.kind {
                TokenKind::KwInt => {
                    if has_int {
                        return Err(CompileError::ParseError(
                            "duplicate 'int' type specifier".to_string()
                        ));
                    }
                    if has_double {
                        return Err(CompileError::ParseError(
                            "cannot combine 'double' with other type specifiers".to_string()
                        ));
                    }
                    if has_char {
                        return Err(CompileError::ParseError(
                            "cannot combine 'char' with 'int'".to_string()
                        ));
                    }
                    has_int = true;
                    self.advance()?;
                    count += 1;
                }
                TokenKind::KwLong => {
                    if has_long {
                        return Err(CompileError::ParseError(
                            "duplicate 'long' type specifier".to_string()
                        ));
                    }
                    if has_double {
                        return Err(CompileError::ParseError(
                            "cannot combine 'double' with other type specifiers".to_string()
                        ));
                    }
                    if has_char {
                        return Err(CompileError::ParseError(
                            "cannot combine 'char' with 'long'".to_string()
                        ));
                    }
                    has_long = true;
                    self.advance()?;
                    count += 1;
                }
                TokenKind::KwUnsigned => {
                    if has_unsigned {
                        return Err(CompileError::ParseError(
                            "duplicate 'unsigned' type specifier".to_string()
                        ));
                    }
                    if has_signed {
                        return Err(CompileError::ParseError(
                            "cannot combine 'signed' and 'unsigned'".to_string()
                        ));
                    }
                    if has_double {
                        return Err(CompileError::ParseError(
                            "cannot combine 'double' with other type specifiers".to_string()
                        ));
                    }
                    has_unsigned = true;
                    self.advance()?;
                    count += 1;
                }
                TokenKind::KwSigned => {
                    if has_signed {
                        return Err(CompileError::ParseError(
                            "duplicate 'signed' type specifier".to_string()
                        ));
                    }
                    if has_unsigned {
                        return Err(CompileError::ParseError(
                            "cannot combine 'signed' and 'unsigned'".to_string()
                        ));
                    }
                    if has_double {
                        return Err(CompileError::ParseError(
                            "cannot combine 'double' with other type specifiers".to_string()
                        ));
                    }
                    has_signed = true;
                    self.advance()?;
                    count += 1;
                }
                TokenKind::KwDouble => {
                    if has_double {
                        return Err(CompileError::ParseError(
                            "duplicate 'double' type specifier".to_string()
                        ));
                    }
                    if has_int || has_long || has_unsigned || has_signed || has_char {
                        return Err(CompileError::ParseError(
                            "cannot combine 'double' with other type specifiers".to_string()
                        ));
                    }
                    has_double = true;
                    self.advance()?;
                    count += 1;
                }
                TokenKind::KwChar => {
                    if has_char {
                        return Err(CompileError::ParseError(
                            "duplicate 'char' type specifier".to_string()
                        ));
                    }
                    if has_int || has_long || has_double {
                        return Err(CompileError::ParseError(
                            "cannot combine 'char' with other type specifiers".to_string()
                        ));
                    }
                    has_char = true;
                    self.advance()?;
                    count += 1;
                }
                _ => break,
            }
        }

        if count == 0 {
            let token = self.advance()?;
            return Err(CompileError::ParseError(format!(
                "expected type specifier, got {:?}", token.kind
            )));
        }

        // 型の解決
        if has_double {
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
            } else {
                Ok(Type::UInt)
            }
        } else if has_long {
            Ok(Type::Long)
        } else {
            // has_int or has_signed (or both)
            Ok(Type::Int)
        }
    }

    /// 宣言子のパース（Chapter 14）。
    ///
    /// `int *x`, `int **pp` などのポインタ宣言をパースする。
    /// 先頭の `*` の個数をカウントし、識別子を読み、型をラップして返す。
    fn parse_declarator(&mut self, base_type: Type) -> Result<(Type, String)> {
        // leading '*' をカウント
        let mut stars = 0;
        while self.pos < self.tokens.len() && self.peek()?.kind == TokenKind::Star {
            self.advance()?;
            stars += 1;
        }

        let name_token = self.advance()?;
        let name = match &name_token.kind {
            TokenKind::Identifier(name) => name.clone(),
            other => {
                return Err(CompileError::ParseError(format!(
                    "expected identifier in declarator, got {:?}", other
                )));
            }
        };

        let mut ty = base_type;
        for _ in 0..stars {
            ty = Type::Pointer(Box::new(ty));
        }

        // Chapter 15: 配列サフィックス `[N]`
        if self.pos < self.tokens.len() && self.peek()?.kind == TokenKind::OpenBracket {
            self.advance()?; // consume '['
            if let TokenKind::IntLiteral(n) = &self.peek()?.kind {
                let n = *n as usize;
                self.advance()?;
                ty = Type::Array(Box::new(ty), n);
            } else if self.peek()?.kind == TokenKind::CloseBracket {
                // パラメータ用: `int arr[]` → Array(ty, 0)
                ty = Type::Array(Box::new(ty), 0);
            } else {
                return Err(CompileError::ParseError(
                    "expected array size or ']' in declarator".to_string()
                ));
            }
            self.expect(&TokenKind::CloseBracket)?;
        }

        Ok((ty, name))
    }

    /// 抽象宣言子のパース（Chapter 14）。
    ///
    /// キャスト式 `(int *)`, `(double **)` で使用する。
    /// 識別子なしで型だけを返す。
    fn parse_abstract_declarator(&mut self, base_type: Type) -> Result<Type> {
        let mut stars = 0;
        while self.pos < self.tokens.len() && self.peek()?.kind == TokenKind::Star {
            self.advance()?;
            stars += 1;
        }

        let mut ty = base_type;
        for _ in 0..stars {
            ty = Type::Pointer(Box::new(ty));
        }

        // Chapter 15: 配列サフィックス `[N]` for sizeof(int[10]) etc.
        if self.pos < self.tokens.len() && self.peek()?.kind == TokenKind::OpenBracket {
            self.advance()?; // consume '['
            if let TokenKind::IntLiteral(n) = &self.peek()?.kind {
                let n = *n as usize;
                self.advance()?;
                ty = Type::Array(Box::new(ty), n);
            }
            self.expect(&TokenKind::CloseBracket)?;
        }

        Ok(ty)
    }

    /// `<program> ::= <top_level_decl>*`
    fn parse_program(&mut self) -> Result<Program> {
        let mut declarations = Vec::new();
        while self.pos < self.tokens.len() {
            declarations.push(self.parse_top_level_decl()?);
        }
        Ok(Program { declarations })
    }

    /// `<top_level_decl> ::= <function_decl> | <variable_decl>`
    ///
    /// トップレベルでの関数 vs 変数の区別:
    /// `[static|extern]? int <id>` の後に `(` → 関数、`=` or `;` → 変数。
    fn parse_top_level_decl(&mut self) -> Result<TopLevelDecl> {
        let storage_class = self.parse_storage_class()?;
        let base_type = self.parse_type_specifier()?;
        let (decl_type, name) = self.parse_declarator(base_type)?;

        // `(` → 関数、`=`/`;` → 変数
        if self.peek()?.kind == TokenKind::OpenParen {
            // 関数宣言/定義
            self.expect(&TokenKind::OpenParen)?;

            let params = if self.peek()?.kind == TokenKind::KwVoid {
                self.advance()?;
                Vec::new()
            } else {
                let mut params = Vec::new();
                let param_base = self.parse_type_specifier()?;
                let (mut param_type, param_name) = self.parse_declarator(param_base)?;
                // Chapter 15: 配列パラメータ → ポインタに変換
                if let Type::Array(elem, _) = param_type {
                    param_type = Type::Pointer(elem);
                }
                params.push((param_type, param_name));
                while self.peek()?.kind == TokenKind::Comma {
                    self.advance()?;
                    let param_base = self.parse_type_specifier()?;
                    let (mut param_type, param_name) = self.parse_declarator(param_base)?;
                    if let Type::Array(elem, _) = param_type {
                        param_type = Type::Pointer(elem);
                    }
                    params.push((param_type, param_name));
                }
                params
            };

            self.expect(&TokenKind::CloseParen)?;

            let body = if self.peek()?.kind == TokenKind::OpenBrace {
                self.expect(&TokenKind::OpenBrace)?;
                let mut items = Vec::new();
                while self.peek()?.kind != TokenKind::CloseBrace {
                    items.push(self.parse_block_item()?);
                }
                self.expect(&TokenKind::CloseBrace)?;
                Some(items)
            } else {
                self.expect(&TokenKind::Semicolon)?;
                None
            };

            Ok(TopLevelDecl::Function(FunctionDecl { name, return_type: decl_type, params, body, storage_class }))
        } else {
            // 変数宣言
            let init = if self.peek()?.kind == TokenKind::Assign {
                self.advance()?;
                Some(self.parse_assignment()?)
            } else {
                None
            };
            self.expect(&TokenKind::Semicolon)?;
            Ok(TopLevelDecl::Variable(Declaration { name, var_type: decl_type, init, storage_class }))
        }
    }

    /// `<block_item> ::= <statement> | <declaration>`
    ///
    /// 型キーワードまたはストレージクラスで始まれば宣言、それ以外は文。
    fn parse_block_item(&mut self) -> Result<BlockItem> {
        match &self.peek()?.kind {
            TokenKind::KwInt | TokenKind::KwLong | TokenKind::KwUnsigned | TokenKind::KwSigned
            | TokenKind::KwDouble | TokenKind::KwChar
            | TokenKind::KwStatic | TokenKind::KwExtern => {
                Ok(BlockItem::Declaration(self.parse_declaration()?))
            }
            _ => {
                Ok(BlockItem::Statement(self.parse_statement()?))
            }
        }
    }

    /// `<declaration> ::= <storage_class>? <type_specifier> <declarator> ("=" <assignment>)? ";"`
    fn parse_declaration(&mut self) -> Result<Declaration> {
        let storage_class = self.parse_storage_class()?;
        let base_type = self.parse_type_specifier()?;
        let (var_type, name) = self.parse_declarator(base_type)?;

        let init = if self.peek()?.kind == TokenKind::Assign {
            self.advance()?; // consume '='
            Some(self.parse_assignment()?)
        } else {
            None
        };

        self.expect(&TokenKind::Semicolon)?;
        Ok(Declaration { name, var_type, init, storage_class })
    }

    /// `<statement> ::= "return" <exp> ";" | <exp> ";" | ";"
    ///                | "if" "(" <exp> ")" <statement> ("else" <statement>)?
    ///                | "{" <block_item>* "}"`
    fn parse_statement(&mut self) -> Result<Statement> {
        match &self.peek()?.kind {
            TokenKind::KwReturn => {
                self.advance()?;
                let expr = self.parse_expr()?;
                self.expect(&TokenKind::Semicolon)?;
                Ok(Statement::Return(expr))
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
                let else_branch = if self.pos < self.tokens.len()
                    && self.peek()?.kind == TokenKind::KwElse
                {
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
                    items.push(self.parse_block_item()?);
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
                    TokenKind::KwInt | TokenKind::KwLong | TokenKind::KwUnsigned | TokenKind::KwSigned
                    | TokenKind::KwDouble | TokenKind::KwChar
                    | TokenKind::KwStatic | TokenKind::KwExtern => {
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
                Ok(Statement::For { init, condition, post, body })
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
    /// <logical_and> ::= <equality> ( "&&" <equality> )*
    /// ```
    fn parse_logical_and(&mut self) -> Result<Expr> {
        let mut left = self.parse_equality()?;
        loop {
            if self.pos >= self.tokens.len() {
                break;
            }
            match &self.peek()?.kind {
                TokenKind::AndAnd => {
                    self.advance()?;
                    let right = self.parse_equality()?;
                    left = Expr::Binary(BinaryOp::LogicalAnd, Box::new(left), Box::new(right));
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
    /// <relational> ::= <additive> ( ("<" | "<=" | ">" | ">=") <additive> )*
    /// ```
    fn parse_relational(&mut self) -> Result<Expr> {
        let mut left = self.parse_additive()?;
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
        // `(` の次が型キーワードならキャスト式
        if self.pos < self.tokens.len()
            && self.peek()?.kind == TokenKind::OpenParen
            && self.pos + 1 < self.tokens.len()
        {
            // 先読み: `(` の次のトークンが型キーワードか
            let next = &self.tokens[self.pos + 1].kind;
            if matches!(
                next,
                TokenKind::KwInt | TokenKind::KwLong | TokenKind::KwUnsigned
                | TokenKind::KwSigned | TokenKind::KwDouble | TokenKind::KwChar
            ) {
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
                {
                    let next = &self.tokens[self.pos + 1].kind;
                    if matches!(
                        next,
                        TokenKind::KwInt | TokenKind::KwLong | TokenKind::KwUnsigned
                        | TokenKind::KwSigned | TokenKind::KwDouble | TokenKind::KwChar
                    ) {
                        self.advance()?; // consume '('
                        let base_type = self.parse_type_specifier()?;
                        let ty = self.parse_abstract_declarator(base_type)?;
                        self.expect(&TokenKind::CloseParen)?;
                        return Ok(Expr::SizeOfType(ty));
                    }
                }
                // sizeof expr (unary precedence)
                let inner = self.parse_unary()?;
                return Ok(Expr::SizeOfExpr(Box::new(inner)));
            }
            TokenKind::Minus | TokenKind::Tilde | TokenKind::Bang => {
                let op_token = self.advance()?;
                let op = match &op_token.kind {
                    TokenKind::Minus => UnaryOp::Negate,
                    TokenKind::Tilde => UnaryOp::Complement,
                    TokenKind::Bang => UnaryOp::Not,
                    _ => unreachable!(),
                };
                let inner = self.parse_unary()?;
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
                    Ok(Expr::StringLiteral(content.clone()))
                } else {
                    unreachable!()
                }
            }
            TokenKind::Identifier(_) => {
                let token = self.advance()?;
                if let TokenKind::Identifier(name) = &token.kind {
                    let name = name.clone();
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
            vec![BlockItem::Statement(Statement::Return(Expr::Constant(2)))]
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
            vec![BlockItem::Statement(Statement::Return(Expr::Constant(0)))]
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
        let func = match &program.declarations[0] { TopLevelDecl::Function(f) => f, _ => panic!() };
        assert_eq!(
            *func.body.as_ref().unwrap(),
            vec![BlockItem::Statement(Statement::Return(Expr::Unary(UnaryOp::Negate, Box::new(Expr::Constant(5)))))]
        );
    }

    /// `~0` → `Unary(Complement, Constant(0))`
    #[test]
    fn parse_complement() {
        let tokens = lex::lex("int main(void) { return ~0; }").unwrap();
        let program = parse(&tokens).unwrap();
        let func = match &program.declarations[0] { TopLevelDecl::Function(f) => f, _ => panic!() };
        assert_eq!(
            *func.body.as_ref().unwrap(),
            vec![BlockItem::Statement(Statement::Return(Expr::Unary(UnaryOp::Complement, Box::new(Expr::Constant(0)))))]
        );
    }

    /// `!1` → `Unary(Not, Constant(1))`
    #[test]
    fn parse_logical_not() {
        let tokens = lex::lex("int main(void) { return !1; }").unwrap();
        let program = parse(&tokens).unwrap();
        let func = match &program.declarations[0] { TopLevelDecl::Function(f) => f, _ => panic!() };
        assert_eq!(
            *func.body.as_ref().unwrap(),
            vec![BlockItem::Statement(Statement::Return(Expr::Unary(UnaryOp::Not, Box::new(Expr::Constant(1)))))]
        );
    }

    /// `--5` は Chapter 7 以降 `Unary(PreDecrement, Constant(5))` とパースされる
    #[test]
    fn parse_pre_decrement_literal() {
        let tokens = lex::lex("int main(void) { return --5; }").unwrap();
        let program = parse(&tokens).unwrap();
        let func = match &program.declarations[0] { TopLevelDecl::Function(f) => f, _ => panic!() };
        assert_eq!(
            *func.body.as_ref().unwrap(),
            vec![BlockItem::Statement(Statement::Return(Expr::Unary(
                UnaryOp::PreDecrement,
                Box::new(Expr::Constant(5))
            )))]
        );
    }

    /// `- -5` は `Unary(Negate, Unary(Negate, Constant(5)))` とパースされる（スペース必要）
    #[test]
    fn parse_nested_negation() {
        let tokens = lex::lex("int main(void) { return - -5; }").unwrap();
        let program = parse(&tokens).unwrap();
        let func = match &program.declarations[0] { TopLevelDecl::Function(f) => f, _ => panic!() };
        assert_eq!(
            *func.body.as_ref().unwrap(),
            vec![BlockItem::Statement(Statement::Return(Expr::Unary(
                UnaryOp::Negate,
                Box::new(Expr::Unary(UnaryOp::Negate, Box::new(Expr::Constant(5))))
            )))]
        );
    }

    /// `~(-3)` → 括弧で明示的にグループ化
    #[test]
    fn parse_complement_of_negation() {
        let tokens = lex::lex("int main(void) { return ~(-3); }").unwrap();
        let program = parse(&tokens).unwrap();
        let func = match &program.declarations[0] { TopLevelDecl::Function(f) => f, _ => panic!() };
        assert_eq!(
            *func.body.as_ref().unwrap(),
            vec![BlockItem::Statement(Statement::Return(Expr::Unary(
                UnaryOp::Complement,
                Box::new(Expr::Unary(UnaryOp::Negate, Box::new(Expr::Constant(3))))
            )))]
        );
    }

    // ── Chapter 3 テスト ──

    /// `1 + 2` → `Binary(Add, Constant(1), Constant(2))`
    #[test]
    fn parse_addition() {
        let tokens = lex::lex("int main(void) { return 1 + 2; }").unwrap();
        let program = parse(&tokens).unwrap();
        let func = match &program.declarations[0] { TopLevelDecl::Function(f) => f, _ => panic!() };
        assert_eq!(
            *func.body.as_ref().unwrap(),
            vec![BlockItem::Statement(Statement::Return(Expr::Binary(
                BinaryOp::Add,
                Box::new(Expr::Constant(1)),
                Box::new(Expr::Constant(2)),
            )))]
        );
    }

    /// `1 + 2 * 3` → 乗算が加算より優先度が高い
    #[test]
    fn parse_precedence() {
        let tokens = lex::lex("int main(void) { return 1 + 2 * 3; }").unwrap();
        let program = parse(&tokens).unwrap();
        let func = match &program.declarations[0] { TopLevelDecl::Function(f) => f, _ => panic!() };
        assert_eq!(
            *func.body.as_ref().unwrap(),
            vec![BlockItem::Statement(Statement::Return(Expr::Binary(
                BinaryOp::Add,
                Box::new(Expr::Constant(1)),
                Box::new(Expr::Binary(
                    BinaryOp::Multiply,
                    Box::new(Expr::Constant(2)),
                    Box::new(Expr::Constant(3)),
                )),
            )))]
        );
    }

    /// `1 - 2 - 3` → 左結合
    #[test]
    fn parse_left_associativity() {
        let tokens = lex::lex("int main(void) { return 1 - 2 - 3; }").unwrap();
        let program = parse(&tokens).unwrap();
        let func = match &program.declarations[0] { TopLevelDecl::Function(f) => f, _ => panic!() };
        assert_eq!(
            *func.body.as_ref().unwrap(),
            vec![BlockItem::Statement(Statement::Return(Expr::Binary(
                BinaryOp::Subtract,
                Box::new(Expr::Binary(
                    BinaryOp::Subtract,
                    Box::new(Expr::Constant(1)),
                    Box::new(Expr::Constant(2)),
                )),
                Box::new(Expr::Constant(3)),
            )))]
        );
    }

    /// `(1 + 2) * 3` → 括弧で優先度を変更
    #[test]
    fn parse_parenthesized_binary() {
        let tokens = lex::lex("int main(void) { return (1 + 2) * 3; }").unwrap();
        let program = parse(&tokens).unwrap();
        let func = match &program.declarations[0] { TopLevelDecl::Function(f) => f, _ => panic!() };
        assert_eq!(
            *func.body.as_ref().unwrap(),
            vec![BlockItem::Statement(Statement::Return(Expr::Binary(
                BinaryOp::Multiply,
                Box::new(Expr::Binary(
                    BinaryOp::Add,
                    Box::new(Expr::Constant(1)),
                    Box::new(Expr::Constant(2)),
                )),
                Box::new(Expr::Constant(3)),
            )))]
        );
    }

    /// `7 / 2` → `Binary(Divide, Constant(7), Constant(2))`
    #[test]
    fn parse_division() {
        let tokens = lex::lex("int main(void) { return 7 / 2; }").unwrap();
        let program = parse(&tokens).unwrap();
        let func = match &program.declarations[0] { TopLevelDecl::Function(f) => f, _ => panic!() };
        assert_eq!(
            *func.body.as_ref().unwrap(),
            vec![BlockItem::Statement(Statement::Return(Expr::Binary(
                BinaryOp::Divide,
                Box::new(Expr::Constant(7)),
                Box::new(Expr::Constant(2)),
            )))]
        );
    }

    /// `7 % 2` → `Binary(Remainder, Constant(7), Constant(2))`
    #[test]
    fn parse_remainder() {
        let tokens = lex::lex("int main(void) { return 7 % 2; }").unwrap();
        let program = parse(&tokens).unwrap();
        let func = match &program.declarations[0] { TopLevelDecl::Function(f) => f, _ => panic!() };
        assert_eq!(
            *func.body.as_ref().unwrap(),
            vec![BlockItem::Statement(Statement::Return(Expr::Binary(
                BinaryOp::Remainder,
                Box::new(Expr::Constant(7)),
                Box::new(Expr::Constant(2)),
            )))]
        );
    }

    // ── Chapter 4 テスト ──

    /// `1 < 2`
    #[test]
    fn parse_less_than() {
        let tokens = lex::lex("int main(void) { return 1 < 2; }").unwrap();
        let program = parse(&tokens).unwrap();
        let func = match &program.declarations[0] { TopLevelDecl::Function(f) => f, _ => panic!() };
        assert_eq!(
            *func.body.as_ref().unwrap(),
            vec![BlockItem::Statement(Statement::Return(Expr::Binary(
                BinaryOp::LessThan,
                Box::new(Expr::Constant(1)),
                Box::new(Expr::Constant(2)),
            )))]
        );
    }

    /// `1 == 2`
    #[test]
    fn parse_equal() {
        let tokens = lex::lex("int main(void) { return 1 == 2; }").unwrap();
        let program = parse(&tokens).unwrap();
        let func = match &program.declarations[0] { TopLevelDecl::Function(f) => f, _ => panic!() };
        assert_eq!(
            *func.body.as_ref().unwrap(),
            vec![BlockItem::Statement(Statement::Return(Expr::Binary(
                BinaryOp::Equal,
                Box::new(Expr::Constant(1)),
                Box::new(Expr::Constant(2)),
            )))]
        );
    }

    /// `1 && 2`
    #[test]
    fn parse_logical_and() {
        let tokens = lex::lex("int main(void) { return 1 && 2; }").unwrap();
        let program = parse(&tokens).unwrap();
        let func = match &program.declarations[0] { TopLevelDecl::Function(f) => f, _ => panic!() };
        assert_eq!(
            *func.body.as_ref().unwrap(),
            vec![BlockItem::Statement(Statement::Return(Expr::Binary(
                BinaryOp::LogicalAnd,
                Box::new(Expr::Constant(1)),
                Box::new(Expr::Constant(2)),
            )))]
        );
    }

    /// `1 || 2`
    #[test]
    fn parse_logical_or() {
        let tokens = lex::lex("int main(void) { return 1 || 2; }").unwrap();
        let program = parse(&tokens).unwrap();
        let func = match &program.declarations[0] { TopLevelDecl::Function(f) => f, _ => panic!() };
        assert_eq!(
            *func.body.as_ref().unwrap(),
            vec![BlockItem::Statement(Statement::Return(Expr::Binary(
                BinaryOp::LogicalOr,
                Box::new(Expr::Constant(1)),
                Box::new(Expr::Constant(2)),
            )))]
        );
    }

    /// `1 < 2 && 3 > 1` — 関係演算子が論理ANDより優先度が高い
    #[test]
    fn parse_relational_and_logical() {
        let tokens = lex::lex("int main(void) { return 1 < 2 && 3 > 1; }").unwrap();
        let program = parse(&tokens).unwrap();
        let func = match &program.declarations[0] { TopLevelDecl::Function(f) => f, _ => panic!() };
        assert_eq!(
            *func.body.as_ref().unwrap(),
            vec![BlockItem::Statement(Statement::Return(Expr::Binary(
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
            )))]
        );
    }

    /// `2 + 3 > 4` — 加算が関係演算より優先度が高い
    #[test]
    fn parse_additive_in_relational() {
        let tokens = lex::lex("int main(void) { return 2 + 3 > 4; }").unwrap();
        let program = parse(&tokens).unwrap();
        let func = match &program.declarations[0] { TopLevelDecl::Function(f) => f, _ => panic!() };
        assert_eq!(
            *func.body.as_ref().unwrap(),
            vec![BlockItem::Statement(Statement::Return(Expr::Binary(
                BinaryOp::GreaterThan,
                Box::new(Expr::Binary(
                    BinaryOp::Add,
                    Box::new(Expr::Constant(2)),
                    Box::new(Expr::Constant(3)),
                )),
                Box::new(Expr::Constant(4)),
            )))]
        );
    }

    /// `1 || 2 && 3` — `&&` が `||` より優先度が高い
    #[test]
    fn parse_or_and_precedence() {
        let tokens = lex::lex("int main(void) { return 1 || 2 && 3; }").unwrap();
        let program = parse(&tokens).unwrap();
        let func = match &program.declarations[0] { TopLevelDecl::Function(f) => f, _ => panic!() };
        assert_eq!(
            *func.body.as_ref().unwrap(),
            vec![BlockItem::Statement(Statement::Return(Expr::Binary(
                BinaryOp::LogicalOr,
                Box::new(Expr::Constant(1)),
                Box::new(Expr::Binary(
                    BinaryOp::LogicalAnd,
                    Box::new(Expr::Constant(2)),
                    Box::new(Expr::Constant(3)),
                )),
            )))]
        );
    }

    /// `-1 + 2` → 単項マイナスが二項加算より優先度が高い
    #[test]
    fn parse_unary_in_binary() {
        let tokens = lex::lex("int main(void) { return -1 + 2; }").unwrap();
        let program = parse(&tokens).unwrap();
        let func = match &program.declarations[0] { TopLevelDecl::Function(f) => f, _ => panic!() };
        assert_eq!(
            *func.body.as_ref().unwrap(),
            vec![BlockItem::Statement(Statement::Return(Expr::Binary(
                BinaryOp::Add,
                Box::new(Expr::Unary(UnaryOp::Negate, Box::new(Expr::Constant(1)))),
                Box::new(Expr::Constant(2)),
            )))]
        );
    }

    // ── Chapter 5 テスト ──

    /// 変数宣言と初期化: `int a = 5; return a;`
    #[test]
    fn parse_declaration_with_init() {
        let tokens = lex::lex("int main(void) { int a = 5; return a; }").unwrap();
        let program = parse(&tokens).unwrap();
        let func = match &program.declarations[0] { TopLevelDecl::Function(f) => f, _ => panic!() };
        assert_eq!(
            *func.body.as_ref().unwrap(),
            vec![
                BlockItem::Declaration(Declaration {
                    name: "a".to_string(),
                    var_type: Type::Int,
                    init: Some(Expr::Constant(5)),
                    storage_class: None,
                }),
                BlockItem::Statement(Statement::Return(Expr::Var("a".to_string()))),
            ]
        );
    }

    /// 初期化なし宣言: `int a;`
    #[test]
    fn parse_declaration_without_init() {
        let tokens = lex::lex("int main(void) { int a; return 0; }").unwrap();
        let program = parse(&tokens).unwrap();
        let func = match &program.declarations[0] { TopLevelDecl::Function(f) => f, _ => panic!() };
        assert_eq!(
            *func.body.as_ref().unwrap(),
            vec![
                BlockItem::Declaration(Declaration {
                    name: "a".to_string(),
                    var_type: Type::Int,
                    init: None,
                    storage_class: None,
                }),
                BlockItem::Statement(Statement::Return(Expr::Constant(0))),
            ]
        );
    }

    /// 代入式: `a = 10;`
    #[test]
    fn parse_assignment() {
        let tokens = lex::lex("int main(void) { int a; a = 10; return a; }").unwrap();
        let program = parse(&tokens).unwrap();
        let func = match &program.declarations[0] { TopLevelDecl::Function(f) => f, _ => panic!() };
        assert_eq!(
            *func.body.as_ref().unwrap(),
            vec![
                BlockItem::Declaration(Declaration {
                    name: "a".to_string(),
                    var_type: Type::Int,
                    init: None,
                    storage_class: None,
                }),
                BlockItem::Statement(Statement::Expression(
                    Expr::Assign(Box::new(Expr::Var("a".to_string())), Box::new(Expr::Constant(10)))
                )),
                BlockItem::Statement(Statement::Return(Expr::Var("a".to_string()))),
            ]
        );
    }

    /// 複数変数: `int a = 2; int b = 3; return a + b;`
    #[test]
    fn parse_multiple_declarations() {
        let tokens = lex::lex("int main(void) { int a = 2; int b = 3; return a + b; }").unwrap();
        let program = parse(&tokens).unwrap();
        let func = match &program.declarations[0] { TopLevelDecl::Function(f) => f, _ => panic!() };
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
                BlockItem::Statement(Statement::Return(Expr::Binary(
                    BinaryOp::Add,
                    Box::new(Expr::Var("a".to_string())),
                    Box::new(Expr::Var("b".to_string())),
                ))),
            ]
        );
    }

    /// 空文: `;`
    #[test]
    fn parse_null_statement() {
        let tokens = lex::lex("int main(void) { ; return 0; }").unwrap();
        let program = parse(&tokens).unwrap();
        let func = match &program.declarations[0] { TopLevelDecl::Function(f) => f, _ => panic!() };
        assert_eq!(
            *func.body.as_ref().unwrap(),
            vec![
                BlockItem::Statement(Statement::Null),
                BlockItem::Statement(Statement::Return(Expr::Constant(0))),
            ]
        );
    }

    // ── Chapter 6 テスト ──

    /// if文: `if (1) return 2;`
    #[test]
    fn parse_if_statement() {
        let tokens = lex::lex("int main(void) { if (1) return 2; }").unwrap();
        let program = parse(&tokens).unwrap();
        let func = match &program.declarations[0] { TopLevelDecl::Function(f) => f, _ => panic!() };
        assert_eq!(
            *func.body.as_ref().unwrap(),
            vec![BlockItem::Statement(Statement::If {
                condition: Expr::Constant(1),
                then_branch: Box::new(Statement::Return(Expr::Constant(2))),
                else_branch: None,
            })]
        );
    }

    /// if-else文: `if (0) return 2; else return 3;`
    #[test]
    fn parse_if_else_statement() {
        let tokens = lex::lex("int main(void) { if (0) return 2; else return 3; }").unwrap();
        let program = parse(&tokens).unwrap();
        let func = match &program.declarations[0] { TopLevelDecl::Function(f) => f, _ => panic!() };
        assert_eq!(
            *func.body.as_ref().unwrap(),
            vec![BlockItem::Statement(Statement::If {
                condition: Expr::Constant(0),
                then_branch: Box::new(Statement::Return(Expr::Constant(2))),
                else_branch: Some(Box::new(Statement::Return(Expr::Constant(3)))),
            })]
        );
    }

    /// 三項演算子: `return 1 ? 5 : 10;`
    #[test]
    fn parse_ternary() {
        let tokens = lex::lex("int main(void) { return 1 ? 5 : 10; }").unwrap();
        let program = parse(&tokens).unwrap();
        let func = match &program.declarations[0] { TopLevelDecl::Function(f) => f, _ => panic!() };
        assert_eq!(
            *func.body.as_ref().unwrap(),
            vec![BlockItem::Statement(Statement::Return(Expr::Conditional {
                condition: Box::new(Expr::Constant(1)),
                then_expr: Box::new(Expr::Constant(5)),
                else_expr: Box::new(Expr::Constant(10)),
            }))]
        );
    }

    /// 複合文: `{ int a = 2; }`
    #[test]
    fn parse_compound_statement() {
        let tokens = lex::lex("int main(void) { { int a = 2; } return 0; }").unwrap();
        let program = parse(&tokens).unwrap();
        let func = match &program.declarations[0] { TopLevelDecl::Function(f) => f, _ => panic!() };
        assert_eq!(
            *func.body.as_ref().unwrap(),
            vec![
                BlockItem::Statement(Statement::Compound(vec![
                    BlockItem::Declaration(Declaration {
                        name: "a".to_string(),
                        var_type: Type::Int,
                        init: Some(Expr::Constant(2)),
                        storage_class: None,
                    }),
                ])),
                BlockItem::Statement(Statement::Return(Expr::Constant(0))),
            ]
        );
    }

    /// ダングリング else: `if (0) if (0) return 1; else return 2;`
    /// else は内側の if に結びつく
    #[test]
    fn parse_dangling_else() {
        let tokens = lex::lex("int main(void) { if (0) if (0) return 1; else return 2; }").unwrap();
        let program = parse(&tokens).unwrap();
        let func = match &program.declarations[0] { TopLevelDecl::Function(f) => f, _ => panic!() };
        assert_eq!(
            *func.body.as_ref().unwrap(),
            vec![BlockItem::Statement(Statement::If {
                condition: Expr::Constant(0),
                then_branch: Box::new(Statement::If {
                    condition: Expr::Constant(0),
                    then_branch: Box::new(Statement::Return(Expr::Constant(1))),
                    else_branch: Some(Box::new(Statement::Return(Expr::Constant(2)))),
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
        let func = match &program.declarations[0] { TopLevelDecl::Function(f) => f, _ => panic!() };
        assert_eq!(
            func.body.as_ref().unwrap()[1],
            BlockItem::Statement(Statement::Expression(
                Expr::CompoundAssign(BinaryOp::Add, Box::new(Expr::Var("a".to_string())), Box::new(Expr::Constant(3)))
            ))
        );
    }

    /// 前置インクリメント: `++a`
    #[test]
    fn parse_prefix_increment() {
        let tokens = lex::lex("int main(void) { int a = 5; return ++a; }").unwrap();
        let program = parse(&tokens).unwrap();
        let func = match &program.declarations[0] { TopLevelDecl::Function(f) => f, _ => panic!() };
        assert_eq!(
            func.body.as_ref().unwrap()[1],
            BlockItem::Statement(Statement::Return(
                Expr::Unary(UnaryOp::PreIncrement, Box::new(Expr::Var("a".to_string())))
            ))
        );
    }

    /// 後置インクリメント: `a++`
    #[test]
    fn parse_postfix_increment() {
        let tokens = lex::lex("int main(void) { int a = 5; return a++; }").unwrap();
        let program = parse(&tokens).unwrap();
        let func = match &program.declarations[0] { TopLevelDecl::Function(f) => f, _ => panic!() };
        assert_eq!(
            func.body.as_ref().unwrap()[1],
            BlockItem::Statement(Statement::Return(
                Expr::PostfixIncrement(Box::new(Expr::Var("a".to_string())))
            ))
        );
    }

    /// 後置デクリメント: `a--`
    #[test]
    fn parse_postfix_decrement() {
        let tokens = lex::lex("int main(void) { int a = 5; return a--; }").unwrap();
        let program = parse(&tokens).unwrap();
        let func = match &program.declarations[0] { TopLevelDecl::Function(f) => f, _ => panic!() };
        assert_eq!(
            func.body.as_ref().unwrap()[1],
            BlockItem::Statement(Statement::Return(
                Expr::PostfixDecrement(Box::new(Expr::Var("a".to_string())))
            ))
        );
    }

    /// カンマ演算子: `(1, 2, 3)` → Binary(Comma, Binary(Comma, 1, 2), 3)
    #[test]
    fn parse_comma_operator() {
        let tokens = lex::lex("int main(void) { return (1, 2, 3); }").unwrap();
        let program = parse(&tokens).unwrap();
        let func = match &program.declarations[0] { TopLevelDecl::Function(f) => f, _ => panic!() };
        assert_eq!(
            func.body.as_ref().unwrap()[0],
            BlockItem::Statement(Statement::Return(
                Expr::Binary(
                    BinaryOp::Comma,
                    Box::new(Expr::Binary(
                        BinaryOp::Comma,
                        Box::new(Expr::Constant(1)),
                        Box::new(Expr::Constant(2)),
                    )),
                    Box::new(Expr::Constant(3)),
                )
            ))
        );
    }

    /// 宣言の初期化子ではカンマ演算子は使えない（カンマなしでパース）
    #[test]
    fn parse_declaration_no_comma_in_init() {
        // `int a = (1, 2);` — 括弧の中ではカンマが使える
        let tokens = lex::lex("int main(void) { int a = (1, 2); return a; }").unwrap();
        let program = parse(&tokens).unwrap();
        let func = match &program.declarations[0] { TopLevelDecl::Function(f) => f, _ => panic!() };
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
        let func = match &program.declarations[0] { TopLevelDecl::Function(f) => f, _ => panic!() };
        assert_eq!(
            func.body.as_ref().unwrap()[2],
            BlockItem::Statement(Statement::Expression(
                Expr::Assign(
                    Box::new(Expr::Var("a".to_string())),
                    Box::new(Expr::Assign(Box::new(Expr::Var("b".to_string())), Box::new(Expr::Constant(5))))
                )
            ))
        );
    }

    // ── Chapter 8 テスト ──

    /// while文: `while (1) return 0;`
    #[test]
    fn parse_while_statement() {
        let tokens = lex::lex("int main(void) { while (1) return 0; }").unwrap();
        let program = parse(&tokens).unwrap();
        let func = match &program.declarations[0] { TopLevelDecl::Function(f) => f, _ => panic!() };
        assert_eq!(
            func.body.as_ref().unwrap()[0],
            BlockItem::Statement(Statement::While {
                condition: Expr::Constant(1),
                body: Box::new(Statement::Return(Expr::Constant(0))),
            })
        );
    }

    /// do-while文: `do { a = 1; } while (0);`
    #[test]
    fn parse_do_while_statement() {
        let tokens = lex::lex("int main(void) { int a; do { a = 1; } while (0); }").unwrap();
        let program = parse(&tokens).unwrap();
        let func = match &program.declarations[0] { TopLevelDecl::Function(f) => f, _ => panic!() };
        assert_eq!(
            func.body.as_ref().unwrap()[1],
            BlockItem::Statement(Statement::DoWhile {
                body: Box::new(Statement::Compound(vec![
                    BlockItem::Statement(Statement::Expression(
                        Expr::Assign(Box::new(Expr::Var("a".to_string())), Box::new(Expr::Constant(1)))
                    )),
                ])),
                condition: Expr::Constant(0),
            })
        );
    }

    /// for文（宣言付き）: `for (int i = 0; i < 5; i++) a++;`
    #[test]
    fn parse_for_with_declaration() {
        let tokens = lex::lex("int main(void) { int a; for (int i = 0; i < 5; i++) a++; }").unwrap();
        let program = parse(&tokens).unwrap();
        let func = match &program.declarations[0] { TopLevelDecl::Function(f) => f, _ => panic!() };
        if let BlockItem::Statement(Statement::For { init, condition, post, body: _ }) = &func.body.as_ref().unwrap()[1] {
            assert_eq!(*init, ForInit::Declaration(Declaration {
                name: "i".to_string(),
                var_type: Type::Int,
                init: Some(Expr::Constant(0)),
                storage_class: None,
            }));
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
        let func = match &program.declarations[0] { TopLevelDecl::Function(f) => f, _ => panic!() };
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
        let func = match &program.declarations[0] { TopLevelDecl::Function(f) => f, _ => panic!() };
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
        let func = match &program.declarations[0] { TopLevelDecl::Function(f) => f, _ => panic!() };
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
        let tokens = lex::lex("int five(void) { return 5; } int main(void) { return five(); }").unwrap();
        let program = parse(&tokens).unwrap();
        assert_eq!(program.declarations.len(), 2);
        let f0 = match &program.declarations[0] { TopLevelDecl::Function(f) => f, _ => panic!() };
        let f1 = match &program.declarations[1] { TopLevelDecl::Function(f) => f, _ => panic!() };
        assert_eq!(f0.name, "five");
        assert_eq!(f0.params, Vec::<(Type, String)>::new());
        assert_eq!(f1.name, "main");
        assert_eq!(
            f1.body.as_ref().unwrap()[0],
            BlockItem::Statement(Statement::Return(
                Expr::FunctionCall("five".to_string(), vec![])
            ))
        );
    }

    /// 関数呼び出し: `add(2, 3)`
    #[test]
    fn parse_function_call_with_args() {
        let tokens = lex::lex("int add(int a, int b) { return a + b; } int main(void) { return add(2, 3); }").unwrap();
        let program = parse(&tokens).unwrap();
        let f0 = match &program.declarations[0] { TopLevelDecl::Function(f) => f, _ => panic!() };
        let f1 = match &program.declarations[1] { TopLevelDecl::Function(f) => f, _ => panic!() };
        assert_eq!(f0.name, "add");
        assert_eq!(f0.params, vec![(Type::Int, "a".to_string()), (Type::Int, "b".to_string())]);
        assert_eq!(
            f1.body.as_ref().unwrap()[0],
            BlockItem::Statement(Statement::Return(
                Expr::FunctionCall("add".to_string(), vec![
                    Expr::Constant(2),
                    Expr::Constant(3),
                ])
            ))
        );
    }

    /// 関数宣言（プロトタイプ）
    #[test]
    fn parse_function_declaration() {
        let tokens = lex::lex("int add(int a, int b); int main(void) { return add(1, 2); }").unwrap();
        let program = parse(&tokens).unwrap();
        assert_eq!(program.declarations.len(), 2);
        let f0 = match &program.declarations[0] { TopLevelDecl::Function(f) => f, _ => panic!() };
        assert_eq!(f0.name, "add");
        assert!(f0.body.is_none());
        assert_eq!(f0.params, vec![(Type::Int, "a".to_string()), (Type::Int, "b".to_string())]);
    }

    /// 複数関数: 宣言+定義
    #[test]
    fn parse_declaration_then_definition() {
        let tokens = lex::lex("int add(int a, int b); int main(void) { return add(10, 20); } int add(int a, int b) { return a + b; }").unwrap();
        let program = parse(&tokens).unwrap();
        assert_eq!(program.declarations.len(), 3);
        let f0 = match &program.declarations[0] { TopLevelDecl::Function(f) => f, _ => panic!() };
        let f1 = match &program.declarations[1] { TopLevelDecl::Function(f) => f, _ => panic!() };
        let f2 = match &program.declarations[2] { TopLevelDecl::Function(f) => f, _ => panic!() };
        assert!(f0.body.is_none());
        assert!(f1.body.is_some());
        assert!(f2.body.is_some());
    }

    /// 引数中のカンマはカンマ演算子ではなく引数区切りとしてパースされる
    #[test]
    fn parse_function_call_comma_not_operator() {
        let tokens = lex::lex("int foo(int a, int b, int c) { return a; } int main(void) { return foo(1, 2, 3); }").unwrap();
        let program = parse(&tokens).unwrap();
        let f1 = match &program.declarations[1] { TopLevelDecl::Function(f) => f, _ => panic!() };
        assert_eq!(
            f1.body.as_ref().unwrap()[0],
            BlockItem::Statement(Statement::Return(
                Expr::FunctionCall("foo".to_string(), vec![
                    Expr::Constant(1),
                    Expr::Constant(2),
                    Expr::Constant(3),
                ])
            ))
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
        let tokens = lex::lex("static int helper(void) { return 42; } int main(void) { return helper(); }").unwrap();
        let program = parse(&tokens).unwrap();
        let f0 = match &program.declarations[0] { TopLevelDecl::Function(f) => f, _ => panic!() };
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
        let func = match &program.declarations[0] { TopLevelDecl::Function(f) => f, _ => panic!() };
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
        let func = match &program.declarations[0] { TopLevelDecl::Function(f) => f, _ => panic!() };
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
        let func = match &program.declarations[0] { TopLevelDecl::Function(f) => f, _ => panic!() };
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
        let func = match &program.declarations[0] { TopLevelDecl::Function(f) => f, _ => panic!() };
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
        let func = match &program.declarations[0] { TopLevelDecl::Function(f) => f, _ => panic!() };
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
        let func = match &program.declarations[0] { TopLevelDecl::Function(f) => f, _ => panic!() };
        assert_eq!(
            func.body.as_ref().unwrap()[2],
            BlockItem::Statement(Statement::Return(
                Expr::Dereref(Box::new(Expr::Var("p".to_string())))
            ))
        );
    }

    /// ポインタ経由の書き込み: `*p = 42;`
    #[test]
    fn parse_dereference_assign() {
        let tokens = lex::lex("int main(void) { int x; int *p = &x; *p = 42; return 0; }").unwrap();
        let program = parse(&tokens).unwrap();
        let func = match &program.declarations[0] { TopLevelDecl::Function(f) => f, _ => panic!() };
        assert_eq!(
            func.body.as_ref().unwrap()[2],
            BlockItem::Statement(Statement::Expression(
                Expr::Assign(
                    Box::new(Expr::Dereref(Box::new(Expr::Var("p".to_string())))),
                    Box::new(Expr::Constant(42))
                )
            ))
        );
    }

    /// キャスト式: `(int *)0`
    #[test]
    fn parse_cast_to_pointer() {
        let tokens = lex::lex("int main(void) { int *p = (int *)0; return 0; }").unwrap();
        let program = parse(&tokens).unwrap();
        let func = match &program.declarations[0] { TopLevelDecl::Function(f) => f, _ => panic!() };
        if let BlockItem::Declaration(decl) = &func.body.as_ref().unwrap()[0] {
            assert_eq!(decl.name, "p");
            assert_eq!(decl.var_type, Type::Pointer(Box::new(Type::Int)));
            match &decl.init {
                Some(Expr::Cast { target_type, expr, .. }) => {
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
        let func = match &program.declarations[0] { TopLevelDecl::Function(f) => f, _ => panic!() };
        assert_eq!(func.name, "deref");
        assert_eq!(func.params, vec![(Type::Pointer(Box::new(Type::Int)), "p".to_string())]);
        assert_eq!(
            func.body.as_ref().unwrap()[0],
            BlockItem::Statement(Statement::Return(
                Expr::Dereref(Box::new(Expr::Var("p".to_string())))
            ))
        );
    }

    /// ポインタ戻り値型: `int *return_ptr(int *p)`
    #[test]
    fn parse_pointer_return_type() {
        let tokens = lex::lex("int *identity(int *p) { return p; }").unwrap();
        let program = parse(&tokens).unwrap();
        let func = match &program.declarations[0] { TopLevelDecl::Function(f) => f, _ => panic!() };
        assert_eq!(func.name, "identity");
        assert_eq!(func.return_type, Type::Pointer(Box::new(Type::Int)));
        assert_eq!(func.params, vec![(Type::Pointer(Box::new(Type::Int)), "p".to_string())]);
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
}
