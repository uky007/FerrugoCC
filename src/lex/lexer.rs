//! 字句解析器（Lexer）
//!
//! Cソースコード文字列を受け取り、トークン列 `Vec<Token>` を返す。
//!
//! # アルゴリズム
//! ソースを先頭から1バイトずつ走査し、以下のルールでトークンを切り出す:
//! 1. 空白文字 → スキップ（改行なら行番号を更新）
//! 2. 単一文字の記号 (`(`, `)`, `{`, `}`, `;`, `~`, `?`, `:`, `,`) → 即座にトークン化
//! 2b. 先読みが必要な記号 → 次の文字を見て判定
//!    - `!` → `!=` or `!`
//!    - `<` → `<=` or `<`  /  `>` → `>=` or `>`
//!    - `=` → `==` or `=`  /  `&` → `&&`  /  `|` → `||`
//!    - `+` → `++`, `+=`, or `+`  /  `-` → `--`, `-=`, or `-`  (Chapter 7)
//!    - `*` → `*=` or `*`  /  `/` → `/=` or `/`  /  `%` → `%=` or `%`  (Chapter 7)
//! 3. 数字で始まる → 連続する数字を読み取り `IntLiteral` に変換
//!    - 数字の直後に英字/`_` があればエラー（例: `123abc`）
//! 4. 英字/`_` で始まる → 連続する英数字/`_` を読み取り、
//!    キーワードテーブルと照合して `KwInt` 等かを判定。一致しなければ `Identifier`
//! 5. それ以外 → エラー
//!
//! # 例
//! ```
//! # use ferrugocc::lex::lexer::lex;
//! # use ferrugocc::lex::token::TokenKind;
//! let tokens = lex("return 42;").unwrap();
//! assert_eq!(tokens[0].kind, TokenKind::KwReturn);
//! assert_eq!(tokens[1].kind, TokenKind::IntLiteral(42));
//! assert_eq!(tokens[2].kind, TokenKind::Semicolon);
//! ```

use crate::error::{CompileError, Result};
use super::token::{Token, TokenKind, Span};

/// ソースコード文字列を字句解析してトークン列に変換する。
///
/// 正常時は `Ok(Vec<Token>)` を返す。不正な文字やトークンがあれば
/// `Err(CompileError::LexError)` を返す。
pub fn lex(source: &str) -> Result<Vec<Token>> {
    let mut tokens = Vec::new();
    let bytes = source.as_bytes();
    let mut pos = 0;        // 現在のバイト位置
    let mut line = 1;       // 現在の行番号（1始まり）
    let mut column = 1;     // 現在の列番号（1始まり）

    while pos < bytes.len() {
        let b = bytes[pos];

        // ── 空白のスキップ ──
        // 改行文字なら行番号を進め、列番号をリセットする
        if b.is_ascii_whitespace() {
            if b == b'\n' {
                line += 1;
                column = 1;
            } else {
                column += 1;
            }
            pos += 1;
            continue;
        }

        // ── 単一文字トークン ──
        // 1文字で完結するトークンをまとめて処理する
        let single = match b {
            b'(' => Some(TokenKind::OpenParen),
            b')' => Some(TokenKind::CloseParen),
            b'{' => Some(TokenKind::OpenBrace),
            b'}' => Some(TokenKind::CloseBrace),
            b';' => Some(TokenKind::Semicolon),
            // Chapter 2: 単項演算子
            b'~' => Some(TokenKind::Tilde),
            // Chapter 6: 三項演算子
            b'?' => Some(TokenKind::Question),
            b':' => Some(TokenKind::Colon),
            // Chapter 7: カンマ演算子
            b',' => Some(TokenKind::Comma),
            _ => None,
        };

        if let Some(kind) = single {
            tokens.push(Token {
                kind,
                span: Span { offset: pos, len: 1, line, column },
            });
            pos += 1;
            column += 1;
            continue;
        }

        // ── 複数文字トークン（先読みが必要）──（Chapter 4 で追加）
        // `!`, `<`, `>`, `=`, `&`, `|` は次の文字を見てトークンの種類を決定する
        if let Some((kind, len)) = match b {
            b'!' => {
                if pos + 1 < bytes.len() && bytes[pos + 1] == b'=' {
                    Some((TokenKind::NotEqual, 2))
                } else {
                    Some((TokenKind::Bang, 1))
                }
            }
            b'<' => {
                if pos + 1 < bytes.len() && bytes[pos + 1] == b'=' {
                    Some((TokenKind::LessEqual, 2))
                } else {
                    Some((TokenKind::Less, 1))
                }
            }
            b'>' => {
                if pos + 1 < bytes.len() && bytes[pos + 1] == b'=' {
                    Some((TokenKind::GreaterEqual, 2))
                } else {
                    Some((TokenKind::Greater, 1))
                }
            }
            b'=' => {
                if pos + 1 < bytes.len() && bytes[pos + 1] == b'=' {
                    Some((TokenKind::EqualEqual, 2))
                } else {
                    Some((TokenKind::Assign, 1))
                }
            }
            b'&' => {
                if pos + 1 < bytes.len() && bytes[pos + 1] == b'&' {
                    Some((TokenKind::AndAnd, 2))
                } else {
                    return Err(CompileError::LexError(format!(
                        "unexpected character '&' at line {line}, column {column} \
                         (bitwise AND is not supported)"
                    )));
                }
            }
            b'|' => {
                if pos + 1 < bytes.len() && bytes[pos + 1] == b'|' {
                    Some((TokenKind::OrOr, 2))
                } else {
                    return Err(CompileError::LexError(format!(
                        "unexpected character '|' at line {line}, column {column} \
                         (bitwise OR is not supported)"
                    )));
                }
            }
            // Chapter 7: +, -, *, /, % は先読みが必要
            b'+' => {
                if pos + 1 < bytes.len() && bytes[pos + 1] == b'+' {
                    Some((TokenKind::PlusPlus, 2))
                } else if pos + 1 < bytes.len() && bytes[pos + 1] == b'=' {
                    Some((TokenKind::PlusAssign, 2))
                } else {
                    Some((TokenKind::Plus, 1))
                }
            }
            b'-' => {
                if pos + 1 < bytes.len() && bytes[pos + 1] == b'-' {
                    Some((TokenKind::MinusMinus, 2))
                } else if pos + 1 < bytes.len() && bytes[pos + 1] == b'=' {
                    Some((TokenKind::MinusAssign, 2))
                } else {
                    Some((TokenKind::Minus, 1))
                }
            }
            b'*' => {
                if pos + 1 < bytes.len() && bytes[pos + 1] == b'=' {
                    Some((TokenKind::StarAssign, 2))
                } else {
                    Some((TokenKind::Star, 1))
                }
            }
            b'/' => {
                if pos + 1 < bytes.len() && bytes[pos + 1] == b'=' {
                    Some((TokenKind::SlashAssign, 2))
                } else {
                    Some((TokenKind::Slash, 1))
                }
            }
            b'%' => {
                if pos + 1 < bytes.len() && bytes[pos + 1] == b'=' {
                    Some((TokenKind::PercentAssign, 2))
                } else {
                    Some((TokenKind::Percent, 1))
                }
            }
            _ => None,
        } {
            tokens.push(Token {
                kind,
                span: Span { offset: pos, len, line, column },
            });
            pos += len;
            column += len;
            continue;
        }

        // ── 整数リテラル ──
        // 数字の連続を読み取り、i64 に変換する。
        // 直後に英字や `_` が続く場合（例: `123abc`）はエラーとする。
        if b.is_ascii_digit() {
            let start = pos;
            let start_col = column;
            while pos < bytes.len() && bytes[pos].is_ascii_digit() {
                pos += 1;
                column += 1;
            }
            // 整数リテラルの直後に識別子文字が来るのは不正
            if pos < bytes.len() && (bytes[pos].is_ascii_alphabetic() || bytes[pos] == b'_') {
                return Err(CompileError::LexError(format!(
                    "invalid token at line {line}, column {start_col}: \
                     integer literal followed by identifier character"
                )));
            }
            let text = &source[start..pos];
            let value: i64 = text.parse().map_err(|e| {
                CompileError::LexError(format!(
                    "invalid integer literal '{text}' at line {line}, column {start_col}: {e}"
                ))
            })?;
            tokens.push(Token {
                kind: TokenKind::IntLiteral(value),
                span: Span { offset: start, len: pos - start, line, column: start_col },
            });
            continue;
        }

        // ── 識別子・キーワード ──
        // 英字または `_` で始まり、英数字・`_` が続く限り読み取る。
        // 読み取った文字列がキーワードに一致すれば対応する TokenKind、
        // 一致しなければ Identifier とする。
        if b.is_ascii_alphabetic() || b == b'_' {
            let start = pos;
            let start_col = column;
            while pos < bytes.len() && (bytes[pos].is_ascii_alphanumeric() || bytes[pos] == b'_') {
                pos += 1;
                column += 1;
            }
            let text = &source[start..pos];
            let kind = match text {
                "int"    => TokenKind::KwInt,
                "void"   => TokenKind::KwVoid,
                "return" => TokenKind::KwReturn,
                "if"       => TokenKind::KwIf,
                "else"     => TokenKind::KwElse,
                "while"    => TokenKind::KwWhile,
                "do"       => TokenKind::KwDo,
                "for"      => TokenKind::KwFor,
                "break"    => TokenKind::KwBreak,
                "continue" => TokenKind::KwContinue,
                "static"   => TokenKind::KwStatic,
                "extern"   => TokenKind::KwExtern,
                _        => TokenKind::Identifier(text.to_string()),
            };
            tokens.push(Token {
                kind,
                span: Span { offset: start, len: pos - start, line, column: start_col },
            });
            continue;
        }

        // ── 未知の文字 ──
        return Err(CompileError::LexError(format!(
            "unexpected character '{}' at line {line}, column {column}",
            b as char
        )));
    }

    Ok(tokens)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lex_return_2() {
        let source = "int main(void) { return 2; }";
        let tokens = lex(source).unwrap();
        let kinds: Vec<_> = tokens.iter().map(|t| &t.kind).collect();
        assert_eq!(
            kinds,
            vec![
                &TokenKind::KwInt,
                &TokenKind::Identifier("main".to_string()),
                &TokenKind::OpenParen,
                &TokenKind::KwVoid,
                &TokenKind::CloseParen,
                &TokenKind::OpenBrace,
                &TokenKind::KwReturn,
                &TokenKind::IntLiteral(2),
                &TokenKind::Semicolon,
                &TokenKind::CloseBrace,
            ]
        );
    }

    #[test]
    fn lex_multiline() {
        let source = "int main(void) {\n    return 0;\n}";
        let tokens = lex(source).unwrap();
        assert_eq!(tokens.len(), 10);
        // 'return' は2行目にあるはず
        let ret_token = tokens.iter().find(|t| t.kind == TokenKind::KwReturn).unwrap();
        assert_eq!(ret_token.span.line, 2);
    }

    #[test]
    fn lex_unexpected_char() {
        let result = lex("int @main");
        assert!(result.is_err());
    }

    #[test]
    fn lex_invalid_integer_suffix() {
        let result = lex("123abc");
        assert!(result.is_err());
    }

    /// Chapter 2: 単項演算子のトークン化
    #[test]
    fn lex_unary_operators() {
        let tokens = lex("-~!42").unwrap();
        let kinds: Vec<_> = tokens.iter().map(|t| &t.kind).collect();
        assert_eq!(
            kinds,
            vec![
                &TokenKind::Minus,
                &TokenKind::Tilde,
                &TokenKind::Bang,
                &TokenKind::IntLiteral(42),
            ]
        );
    }

    /// Chapter 3: 二項演算子のトークン化
    #[test]
    fn lex_binary_operators() {
        let tokens = lex("1 + 2 * 3 / 4 % 5 - 6").unwrap();
        let kinds: Vec<_> = tokens.iter().map(|t| &t.kind).collect();
        assert_eq!(
            kinds,
            vec![
                &TokenKind::IntLiteral(1),
                &TokenKind::Plus,
                &TokenKind::IntLiteral(2),
                &TokenKind::Star,
                &TokenKind::IntLiteral(3),
                &TokenKind::Slash,
                &TokenKind::IntLiteral(4),
                &TokenKind::Percent,
                &TokenKind::IntLiteral(5),
                &TokenKind::Minus,
                &TokenKind::IntLiteral(6),
            ]
        );
    }

    /// Chapter 4: 関係・等価・論理演算子のトークン化
    #[test]
    fn lex_relational_operators() {
        let tokens = lex("1 < 2 <= 3 > 4 >= 5").unwrap();
        let kinds: Vec<_> = tokens.iter().map(|t| &t.kind).collect();
        assert_eq!(
            kinds,
            vec![
                &TokenKind::IntLiteral(1),
                &TokenKind::Less,
                &TokenKind::IntLiteral(2),
                &TokenKind::LessEqual,
                &TokenKind::IntLiteral(3),
                &TokenKind::Greater,
                &TokenKind::IntLiteral(4),
                &TokenKind::GreaterEqual,
                &TokenKind::IntLiteral(5),
            ]
        );
    }

    #[test]
    fn lex_equality_operators() {
        let tokens = lex("1 == 2 != 3").unwrap();
        let kinds: Vec<_> = tokens.iter().map(|t| &t.kind).collect();
        assert_eq!(
            kinds,
            vec![
                &TokenKind::IntLiteral(1),
                &TokenKind::EqualEqual,
                &TokenKind::IntLiteral(2),
                &TokenKind::NotEqual,
                &TokenKind::IntLiteral(3),
            ]
        );
    }

    #[test]
    fn lex_logical_operators() {
        let tokens = lex("1 && 2 || 3").unwrap();
        let kinds: Vec<_> = tokens.iter().map(|t| &t.kind).collect();
        assert_eq!(
            kinds,
            vec![
                &TokenKind::IntLiteral(1),
                &TokenKind::AndAnd,
                &TokenKind::IntLiteral(2),
                &TokenKind::OrOr,
                &TokenKind::IntLiteral(3),
            ]
        );
    }

    /// `!` 単体は論理否定（Bang）として扱う
    #[test]
    fn lex_bang_vs_not_equal() {
        let tokens = lex("!1 != 2").unwrap();
        let kinds: Vec<_> = tokens.iter().map(|t| &t.kind).collect();
        assert_eq!(
            kinds,
            vec![
                &TokenKind::Bang,
                &TokenKind::IntLiteral(1),
                &TokenKind::NotEqual,
                &TokenKind::IntLiteral(2),
            ]
        );
    }

    /// Chapter 5: `=` 単体は代入トークン
    #[test]
    fn lex_assign() {
        let tokens = lex("x = 1").unwrap();
        let kinds: Vec<_> = tokens.iter().map(|t| &t.kind).collect();
        assert_eq!(
            kinds,
            vec![
                &TokenKind::Identifier("x".to_string()),
                &TokenKind::Assign,
                &TokenKind::IntLiteral(1),
            ]
        );
    }

    /// `&` 単体はエラー
    #[test]
    fn lex_single_ampersand_error() {
        let result = lex("1 & 2");
        assert!(result.is_err());
    }

    /// `|` 単体はエラー
    #[test]
    fn lex_single_pipe_error() {
        let result = lex("1 | 2");
        assert!(result.is_err());
    }

    /// Chapter 6: if/else キーワードと三項演算子トークン
    #[test]
    fn lex_if_else_ternary() {
        let tokens = lex("if (1) return 2; else return 3;").unwrap();
        let kinds: Vec<_> = tokens.iter().map(|t| &t.kind).collect();
        assert_eq!(
            kinds,
            vec![
                &TokenKind::KwIf,
                &TokenKind::OpenParen,
                &TokenKind::IntLiteral(1),
                &TokenKind::CloseParen,
                &TokenKind::KwReturn,
                &TokenKind::IntLiteral(2),
                &TokenKind::Semicolon,
                &TokenKind::KwElse,
                &TokenKind::KwReturn,
                &TokenKind::IntLiteral(3),
                &TokenKind::Semicolon,
            ]
        );
    }

    #[test]
    fn lex_ternary_operator() {
        let tokens = lex("1 ? 5 : 10").unwrap();
        let kinds: Vec<_> = tokens.iter().map(|t| &t.kind).collect();
        assert_eq!(
            kinds,
            vec![
                &TokenKind::IntLiteral(1),
                &TokenKind::Question,
                &TokenKind::IntLiteral(5),
                &TokenKind::Colon,
                &TokenKind::IntLiteral(10),
            ]
        );
    }

    /// Chapter 7: 複合代入演算子のトークン化
    #[test]
    fn lex_compound_assign_operators() {
        let tokens = lex("a += 1 -= 2 *= 3 /= 4 %= 5").unwrap();
        let kinds: Vec<_> = tokens.iter().map(|t| &t.kind).collect();
        assert_eq!(
            kinds,
            vec![
                &TokenKind::Identifier("a".to_string()),
                &TokenKind::PlusAssign,
                &TokenKind::IntLiteral(1),
                &TokenKind::MinusAssign,
                &TokenKind::IntLiteral(2),
                &TokenKind::StarAssign,
                &TokenKind::IntLiteral(3),
                &TokenKind::SlashAssign,
                &TokenKind::IntLiteral(4),
                &TokenKind::PercentAssign,
                &TokenKind::IntLiteral(5),
            ]
        );
    }

    /// Chapter 7: インクリメント・デクリメントのトークン化
    #[test]
    fn lex_increment_decrement() {
        let tokens = lex("a++ b--").unwrap();
        let kinds: Vec<_> = tokens.iter().map(|t| &t.kind).collect();
        assert_eq!(
            kinds,
            vec![
                &TokenKind::Identifier("a".to_string()),
                &TokenKind::PlusPlus,
                &TokenKind::Identifier("b".to_string()),
                &TokenKind::MinusMinus,
            ]
        );
    }

    /// Chapter 7: カンマのトークン化
    #[test]
    fn lex_comma() {
        let tokens = lex("1, 2, 3").unwrap();
        let kinds: Vec<_> = tokens.iter().map(|t| &t.kind).collect();
        assert_eq!(
            kinds,
            vec![
                &TokenKind::IntLiteral(1),
                &TokenKind::Comma,
                &TokenKind::IntLiteral(2),
                &TokenKind::Comma,
                &TokenKind::IntLiteral(3),
            ]
        );
    }

    /// Chapter 8: ループ・制御フローキーワードのトークン化
    #[test]
    fn lex_loop_keywords() {
        let tokens = lex("while do for break continue").unwrap();
        let kinds: Vec<_> = tokens.iter().map(|t| &t.kind).collect();
        assert_eq!(
            kinds,
            vec![
                &TokenKind::KwWhile,
                &TokenKind::KwDo,
                &TokenKind::KwFor,
                &TokenKind::KwBreak,
                &TokenKind::KwContinue,
            ]
        );
    }

    /// Chapter 10: static/extern キーワードのトークン化
    #[test]
    fn lex_storage_class_keywords() {
        let tokens = lex("static extern").unwrap();
        let kinds: Vec<_> = tokens.iter().map(|t| &t.kind).collect();
        assert_eq!(
            kinds,
            vec![
                &TokenKind::KwStatic,
                &TokenKind::KwExtern,
            ]
        );
    }

    /// Chapter 2: 括弧付きの式
    #[test]
    fn lex_parenthesized_expression() {
        let tokens = lex("return (-42);").unwrap();
        let kinds: Vec<_> = tokens.iter().map(|t| &t.kind).collect();
        assert_eq!(
            kinds,
            vec![
                &TokenKind::KwReturn,
                &TokenKind::OpenParen,
                &TokenKind::Minus,
                &TokenKind::IntLiteral(42),
                &TokenKind::CloseParen,
                &TokenKind::Semicolon,
            ]
        );
    }
}
