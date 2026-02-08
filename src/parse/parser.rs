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
//! # 対応する文法（Chapter 7）
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
//! <exp>            ::= <assignment> ("," <assignment>)*
//! <assignment>     ::= <identifier> <assign_op> <assignment> | <conditional>
//! <assign_op>      ::= "=" | "+=" | "-=" | "*=" | "/=" | "%="
//! <conditional>    ::= <logical_or> ("?" <exp> ":" <conditional>)?
//! <logical_or>     ::= <logical_and> ( "||" <logical_and> )*
//! <logical_and>    ::= <equality> ( "&&" <equality> )*
//! <equality>       ::= <relational> ( ("==" | "!=") <relational> )*
//! <relational>     ::= <additive> ( ("<" | "<=" | ">" | ">=") <additive> )*
//! <additive>       ::= <multiplicative> ( ("+" | "-") <multiplicative> )*
//! <multiplicative> ::= <unary> ( ("*" | "/" | "%") <unary> )*
//! <unary>          ::= <unary_op> <unary> | <postfix>
//! <unary_op>       ::= "-" | "~" | "!" | "++" | "--"
//! <postfix>        ::= <primary> ("++" | "--")*
//! <primary>        ::= <int> | <identifier> | "(" <exp> ")"
//! ```

use crate::error::{CompileError, Result};
use crate::lex::{Token, TokenKind};
use super::ast::{Program, Function, BlockItem, Declaration, Statement, Expr, UnaryOp, BinaryOp};

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

    /// `<program> ::= <function>`
    fn parse_program(&mut self) -> Result<Program> {
        let function = self.parse_function()?;
        Ok(Program { function })
    }

    /// `<function> ::= "int" <identifier> "(" "void" ")" "{" <block_item>* "}"`
    fn parse_function(&mut self) -> Result<Function> {
        self.expect(&TokenKind::KwInt)?;

        let name_token = self.advance()?;
        let name = match &name_token.kind {
            TokenKind::Identifier(name) => name.clone(),
            other => {
                return Err(CompileError::ParseError(format!(
                    "expected function name, got {:?}", other
                )));
            }
        };

        self.expect(&TokenKind::OpenParen)?;
        self.expect(&TokenKind::KwVoid)?;
        self.expect(&TokenKind::CloseParen)?;
        self.expect(&TokenKind::OpenBrace)?;

        let mut body = Vec::new();
        while self.peek()?.kind != TokenKind::CloseBrace {
            body.push(self.parse_block_item()?);
        }

        self.expect(&TokenKind::CloseBrace)?;

        Ok(Function { name, body })
    }

    /// `<block_item> ::= <statement> | <declaration>`
    ///
    /// `KwInt` で始まれば宣言、それ以外は文。
    fn parse_block_item(&mut self) -> Result<BlockItem> {
        if self.peek()?.kind == TokenKind::KwInt {
            Ok(BlockItem::Declaration(self.parse_declaration()?))
        } else {
            Ok(BlockItem::Statement(self.parse_statement()?))
        }
    }

    /// `<declaration> ::= "int" <identifier> ("=" <exp>)? ";"`
    fn parse_declaration(&mut self) -> Result<Declaration> {
        self.expect(&TokenKind::KwInt)?;

        let name_token = self.advance()?;
        let name = match &name_token.kind {
            TokenKind::Identifier(name) => name.clone(),
            other => {
                return Err(CompileError::ParseError(format!(
                    "expected variable name, got {:?}", other
                )));
            }
        };

        let init = if self.peek()?.kind == TokenKind::Assign {
            self.advance()?; // consume '='
            Some(self.parse_assignment()?)
        } else {
            None
        };

        self.expect(&TokenKind::Semicolon)?;
        Ok(Declaration { name, init })
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

    /// 代入式のパース（Chapter 5-7 で拡張）。
    ///
    /// ```text
    /// <assignment> ::= <identifier> <assign_op> <assignment> | <conditional>
    /// <assign_op>  ::= "=" | "+=" | "-=" | "*=" | "/=" | "%="
    /// ```
    ///
    /// 代入は右結合。先読みで `<identifier> <assign_op>` パターンを検出する。
    fn parse_assignment(&mut self) -> Result<Expr> {
        if let TokenKind::Identifier(_) = &self.peek()?.kind {
            if self.pos + 1 < self.tokens.len() {
                let next = &self.tokens[self.pos + 1].kind;
                if matches!(next,
                    TokenKind::Assign | TokenKind::PlusAssign | TokenKind::MinusAssign
                    | TokenKind::StarAssign | TokenKind::SlashAssign | TokenKind::PercentAssign
                ) {
                    let name_token = self.advance()?;
                    let name = match &name_token.kind {
                        TokenKind::Identifier(name) => name.clone(),
                        _ => unreachable!(),
                    };
                    let op_token = self.advance()?;
                    let rhs = self.parse_assignment()?; // 右結合
                    return match &op_token.kind {
                        TokenKind::Assign => Ok(Expr::Assign(name, Box::new(rhs))),
                        TokenKind::PlusAssign => Ok(Expr::CompoundAssign(BinaryOp::Add, name, Box::new(rhs))),
                        TokenKind::MinusAssign => Ok(Expr::CompoundAssign(BinaryOp::Subtract, name, Box::new(rhs))),
                        TokenKind::StarAssign => Ok(Expr::CompoundAssign(BinaryOp::Multiply, name, Box::new(rhs))),
                        TokenKind::SlashAssign => Ok(Expr::CompoundAssign(BinaryOp::Divide, name, Box::new(rhs))),
                        TokenKind::PercentAssign => Ok(Expr::CompoundAssign(BinaryOp::Remainder, name, Box::new(rhs))),
                        _ => unreachable!(),
                    };
                }
            }
        }
        self.parse_conditional()
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
    /// <multiplicative> ::= <unary> ( ("*" | "/" | "%") <unary> )*
    /// ```
    fn parse_multiplicative(&mut self) -> Result<Expr> {
        let mut left = self.parse_unary()?;
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
            let right = self.parse_unary()?;
            left = Expr::Binary(op, Box::new(left), Box::new(right));
        }
        Ok(left)
    }

    /// 単項演算のパース（右結合）。
    ///
    /// ```text
    /// <unary> ::= <unary_op> <unary> | <postfix>
    /// <unary_op> ::= "-" | "~" | "!" | "++" | "--"
    /// ```
    fn parse_unary(&mut self) -> Result<Expr> {
        let token = self.peek()?;
        match &token.kind {
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
                    if let Expr::Var(name) = expr {
                        expr = Expr::PostfixIncrement(name);
                    } else {
                        return Err(CompileError::ParseError(
                            "lvalue required for postfix '++'".to_string()
                        ));
                    }
                }
                TokenKind::MinusMinus => {
                    self.advance()?;
                    if let Expr::Var(name) = expr {
                        expr = Expr::PostfixDecrement(name);
                    } else {
                        return Err(CompileError::ParseError(
                            "lvalue required for postfix '--'".to_string()
                        ));
                    }
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
            TokenKind::Identifier(_) => {
                let token = self.advance()?;
                if let TokenKind::Identifier(name) = &token.kind {
                    Ok(Expr::Var(name.clone()))
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
        assert_eq!(program.function.name, "main");
        assert_eq!(
            program.function.body,
            vec![BlockItem::Statement(Statement::Return(Expr::Constant(2)))]
        );
    }

    #[test]
    fn parse_return_0() {
        let tokens = lex::lex("int main(void) { return 0; }").unwrap();
        let program = parse(&tokens).unwrap();
        assert_eq!(
            program.function.body,
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
        assert_eq!(
            program.function.body,
            vec![BlockItem::Statement(Statement::Return(Expr::Unary(UnaryOp::Negate, Box::new(Expr::Constant(5)))))]
        );
    }

    /// `~0` → `Unary(Complement, Constant(0))`
    #[test]
    fn parse_complement() {
        let tokens = lex::lex("int main(void) { return ~0; }").unwrap();
        let program = parse(&tokens).unwrap();
        assert_eq!(
            program.function.body,
            vec![BlockItem::Statement(Statement::Return(Expr::Unary(UnaryOp::Complement, Box::new(Expr::Constant(0)))))]
        );
    }

    /// `!1` → `Unary(Not, Constant(1))`
    #[test]
    fn parse_logical_not() {
        let tokens = lex::lex("int main(void) { return !1; }").unwrap();
        let program = parse(&tokens).unwrap();
        assert_eq!(
            program.function.body,
            vec![BlockItem::Statement(Statement::Return(Expr::Unary(UnaryOp::Not, Box::new(Expr::Constant(1)))))]
        );
    }

    /// `--5` は Chapter 7 以降 `Unary(PreDecrement, Constant(5))` とパースされる
    #[test]
    fn parse_pre_decrement_literal() {
        let tokens = lex::lex("int main(void) { return --5; }").unwrap();
        let program = parse(&tokens).unwrap();
        assert_eq!(
            program.function.body,
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
        assert_eq!(
            program.function.body,
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
        assert_eq!(
            program.function.body,
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
        assert_eq!(
            program.function.body,
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
        assert_eq!(
            program.function.body,
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
        assert_eq!(
            program.function.body,
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
        assert_eq!(
            program.function.body,
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
        assert_eq!(
            program.function.body,
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
        assert_eq!(
            program.function.body,
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
        assert_eq!(
            program.function.body,
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
        assert_eq!(
            program.function.body,
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
        assert_eq!(
            program.function.body,
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
        assert_eq!(
            program.function.body,
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
        assert_eq!(
            program.function.body,
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
        assert_eq!(
            program.function.body,
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
        assert_eq!(
            program.function.body,
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
        assert_eq!(
            program.function.body,
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
        assert_eq!(
            program.function.body,
            vec![
                BlockItem::Declaration(Declaration {
                    name: "a".to_string(),
                    init: Some(Expr::Constant(5)),
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
        assert_eq!(
            program.function.body,
            vec![
                BlockItem::Declaration(Declaration {
                    name: "a".to_string(),
                    init: None,
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
        assert_eq!(
            program.function.body,
            vec![
                BlockItem::Declaration(Declaration {
                    name: "a".to_string(),
                    init: None,
                }),
                BlockItem::Statement(Statement::Expression(
                    Expr::Assign("a".to_string(), Box::new(Expr::Constant(10)))
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
        assert_eq!(
            program.function.body,
            vec![
                BlockItem::Declaration(Declaration {
                    name: "a".to_string(),
                    init: Some(Expr::Constant(2)),
                }),
                BlockItem::Declaration(Declaration {
                    name: "b".to_string(),
                    init: Some(Expr::Constant(3)),
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
        assert_eq!(
            program.function.body,
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
        assert_eq!(
            program.function.body,
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
        assert_eq!(
            program.function.body,
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
        assert_eq!(
            program.function.body,
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
        assert_eq!(
            program.function.body,
            vec![
                BlockItem::Statement(Statement::Compound(vec![
                    BlockItem::Declaration(Declaration {
                        name: "a".to_string(),
                        init: Some(Expr::Constant(2)),
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
        assert_eq!(
            program.function.body,
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
        assert_eq!(
            program.function.body[1],
            BlockItem::Statement(Statement::Expression(
                Expr::CompoundAssign(BinaryOp::Add, "a".to_string(), Box::new(Expr::Constant(3)))
            ))
        );
    }

    /// 前置インクリメント: `++a`
    #[test]
    fn parse_prefix_increment() {
        let tokens = lex::lex("int main(void) { int a = 5; return ++a; }").unwrap();
        let program = parse(&tokens).unwrap();
        assert_eq!(
            program.function.body[1],
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
        assert_eq!(
            program.function.body[1],
            BlockItem::Statement(Statement::Return(
                Expr::PostfixIncrement("a".to_string())
            ))
        );
    }

    /// 後置デクリメント: `a--`
    #[test]
    fn parse_postfix_decrement() {
        let tokens = lex::lex("int main(void) { int a = 5; return a--; }").unwrap();
        let program = parse(&tokens).unwrap();
        assert_eq!(
            program.function.body[1],
            BlockItem::Statement(Statement::Return(
                Expr::PostfixDecrement("a".to_string())
            ))
        );
    }

    /// カンマ演算子: `(1, 2, 3)` → Binary(Comma, Binary(Comma, 1, 2), 3)
    #[test]
    fn parse_comma_operator() {
        let tokens = lex::lex("int main(void) { return (1, 2, 3); }").unwrap();
        let program = parse(&tokens).unwrap();
        assert_eq!(
            program.function.body[0],
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
        assert_eq!(
            program.function.body[0],
            BlockItem::Declaration(Declaration {
                name: "a".to_string(),
                init: Some(Expr::Binary(
                    BinaryOp::Comma,
                    Box::new(Expr::Constant(1)),
                    Box::new(Expr::Constant(2)),
                )),
            })
        );
    }

    /// 代入の右結合: `a = b = 5`
    #[test]
    fn parse_chained_assignment() {
        let tokens = lex::lex("int main(void) { int a; int b; a = b = 5; return a; }").unwrap();
        let program = parse(&tokens).unwrap();
        assert_eq!(
            program.function.body[2],
            BlockItem::Statement(Statement::Expression(
                Expr::Assign(
                    "a".to_string(),
                    Box::new(Expr::Assign("b".to_string(), Box::new(Expr::Constant(5))))
                )
            ))
        );
    }
}
