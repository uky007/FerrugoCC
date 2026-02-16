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
//!    - `=` → `==` or `=`  /  `&` → `&&` or `&`  /  `|` → `||`
//!    - `+` → `++`, `+=`, or `+`  /  `-` → `--`, `-=`, `->`, or `-`  (Chapter 7, 18)
//!    - `*` → `*=` or `*`  /  `/` → `/=` or `/`  /  `%` → `%=` or `%`  (Chapter 7)
//!    - `.` → `Dot`（数字が後続しなければ。浮動小数点リテラルとの区別）(Chapter 18)
//! 3. 数字で始まる → 連続する数字を読み取り、サフィックスに応じて変換
//!    - `L`/`l` サフィックス → `LongLiteral` に変換（Chapter 11）
//!    - サフィックスなし → `IntLiteral` に変換
//!    - 数字の直後に英字/`_` があればエラー（例: `123abc`）
//! 4. 英字/`_` で始まる → 連続する英数字/`_` を読み取り、
//!    キーワードテーブルと照合して `KwInt`, `KwLong` 等かを判定。一致しなければ `Identifier`
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
            // Chapter 15: 配列添字
            b'[' => Some(TokenKind::OpenBracket),
            b']' => Some(TokenKind::CloseBracket),
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
                    Some((TokenKind::Ampersand, 1))
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
                } else if pos + 1 < bytes.len() && bytes[pos + 1] == b'>' {
                    Some((TokenKind::Arrow, 2))
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

        // ── Chapter 18: `.` 演算子 / `...` 省略記号 ──
        // `...` → Ellipsis（3文字消費）、`.` の後に数字が続かない場合はメンバアクセス演算子
        if b == b'.' && !(pos + 1 < bytes.len() && bytes[pos + 1].is_ascii_digit()) {
            if pos + 2 < bytes.len() && bytes[pos + 1] == b'.' && bytes[pos + 2] == b'.' {
                tokens.push(Token {
                    kind: TokenKind::Ellipsis,
                    span: Span { offset: pos, len: 3, line, column },
                });
                pos += 3;
                column += 3;
                continue;
            }
            tokens.push(Token {
                kind: TokenKind::Dot,
                span: Span { offset: pos, len: 1, line, column },
            });
            pos += 1;
            column += 1;
            continue;
        }

        // ── 数値リテラル（整数 or 浮動小数点）──
        // 数字で始まる場合: `.` か `e`/`E` があれば浮動小数点、なければ整数。
        // `.` で始まり次が数字の場合も浮動小数点（`.5` 等）。
        if b.is_ascii_digit() || (b == b'.' && pos + 1 < bytes.len() && bytes[pos + 1].is_ascii_digit()) {
            let start = pos;
            let start_col = column;
            let mut is_float = false;

            // `.` で始まる場合（例: `.5`）
            if b == b'.' {
                is_float = true;
            }

            // 整数部を読み取り
            while pos < bytes.len() && bytes[pos].is_ascii_digit() {
                pos += 1;
                column += 1;
            }

            // 小数点
            if pos < bytes.len() && bytes[pos] == b'.' {
                is_float = true;
                pos += 1;
                column += 1;
                // 小数部
                while pos < bytes.len() && bytes[pos].is_ascii_digit() {
                    pos += 1;
                    column += 1;
                }
            }

            // 指数部（e/E）
            if pos < bytes.len() && (bytes[pos] == b'e' || bytes[pos] == b'E') {
                is_float = true;
                pos += 1;
                column += 1;
                // オプショナルの符号
                if pos < bytes.len() && (bytes[pos] == b'+' || bytes[pos] == b'-') {
                    pos += 1;
                    column += 1;
                }
                // 指数の数字（必須）
                if pos >= bytes.len() || !bytes[pos].is_ascii_digit() {
                    return Err(CompileError::LexError(format!(
                        "invalid floating-point literal at line {line}, column {start_col}: \
                         expected digit after exponent"
                    )));
                }
                while pos < bytes.len() && bytes[pos].is_ascii_digit() {
                    pos += 1;
                    column += 1;
                }
            }

            if is_float {
                // 後続文字チェック（英字, _, . は不正）
                if pos < bytes.len() && (bytes[pos].is_ascii_alphabetic() || bytes[pos] == b'_' || bytes[pos] == b'.') {
                    return Err(CompileError::LexError(format!(
                        "invalid token at line {line}, column {start_col}: \
                         invalid suffix on floating-point literal"
                    )));
                }

                let text = &source[start..pos];
                let value: f64 = text.parse().map_err(|e| {
                    CompileError::LexError(format!(
                        "invalid floating-point literal '{text}' at line {line}, column {start_col}: {e}"
                    ))
                })?;
                tokens.push(Token {
                    kind: TokenKind::DoubleLiteral(value),
                    span: Span { offset: start, len: pos - start, line, column: start_col },
                });
                continue;
            }

            // 整数リテラル: サフィックス解析（Chapter 11, 12）
            let digit_end = pos;
            let mut has_u = false;
            let mut has_l = false;
            // 最大2文字のサフィックスを消費（U/u, L/l の組み合わせ）
            for _ in 0..2 {
                if pos < bytes.len() && (bytes[pos] == b'U' || bytes[pos] == b'u') && !has_u {
                    has_u = true;
                    pos += 1;
                    column += 1;
                } else if pos < bytes.len() && (bytes[pos] == b'L' || bytes[pos] == b'l') && !has_l {
                    has_l = true;
                    pos += 1;
                    column += 1;
                } else {
                    break;
                }
            }

            // サフィックスの後にさらに英数字が続くのは不正
            if pos < bytes.len() && (bytes[pos].is_ascii_alphanumeric() || bytes[pos] == b'_') {
                return Err(CompileError::LexError(format!(
                    "invalid token at line {line}, column {start_col}: \
                     invalid suffix on integer literal"
                )));
            }

            let text = &source[start..digit_end];
            let kind = if has_u {
                let value: u64 = text.parse().map_err(|e| {
                    CompileError::LexError(format!(
                        "invalid integer literal '{text}' at line {line}, column {start_col}: {e}"
                    ))
                })?;
                if has_l {
                    TokenKind::ULongLiteral(value)
                } else {
                    TokenKind::UIntLiteral(value)
                }
            } else {
                let value: i64 = text.parse().map_err(|e| {
                    CompileError::LexError(format!(
                        "invalid integer literal '{text}' at line {line}, column {start_col}: {e}"
                    ))
                })?;
                if has_l {
                    TokenKind::LongLiteral(value)
                } else {
                    TokenKind::IntLiteral(value)
                }
            };
            tokens.push(Token {
                kind,
                span: Span { offset: start, len: pos - start, line, column: start_col },
            });
            continue;
        }

        // ── 文字リテラル ──（Chapter 16）
        // `'` で始まり、1文字（またはエスケープシーケンス）を読み取り、`'` で閉じる。
        if b == b'\'' {
            let start = pos;
            let start_col = column;
            pos += 1; // consume opening '\''
            column += 1;

            if pos >= bytes.len() {
                return Err(CompileError::LexError(format!(
                    "unterminated character literal at line {line}, column {start_col}"
                )));
            }

            let value: i8 = if bytes[pos] == b'\\' {
                // エスケープシーケンス
                pos += 1;
                column += 1;
                if pos >= bytes.len() {
                    return Err(CompileError::LexError(format!(
                        "unterminated escape sequence in character literal at line {line}, column {start_col}"
                    )));
                }
                let esc = bytes[pos];
                pos += 1;
                column += 1;
                match esc {
                    b'n'  => 10,
                    b't'  => 9,
                    b'r'  => 13,
                    b'\\' => 92,
                    b'\'' => 39,
                    b'"'  => 34,
                    b'0'  => 0,
                    b'a'  => 7,
                    b'b'  => 8,
                    b'f'  => 12,
                    b'v'  => 11,
                    b'?'  => 63,
                    _ => {
                        return Err(CompileError::LexError(format!(
                            "unknown escape sequence '\\{}' at line {line}, column {start_col}",
                            esc as char
                        )));
                    }
                }
            } else {
                let ch = bytes[pos] as i8;
                pos += 1;
                column += 1;
                ch
            };

            if pos >= bytes.len() || bytes[pos] != b'\'' {
                return Err(CompileError::LexError(format!(
                    "unterminated character literal at line {line}, column {start_col}"
                )));
            }
            pos += 1; // consume closing '\''
            column += 1;

            tokens.push(Token {
                kind: TokenKind::CharLiteral(value),
                span: Span { offset: start, len: pos - start, line, column: start_col },
            });
            continue;
        }

        // ── 文字列リテラル ──（Chapter 16）
        // `"` で始まり、エスケープシーケンスを処理しながら `"` で閉じる。
        if b == b'"' {
            let start = pos;
            let start_col = column;
            pos += 1; // consume opening '"'
            column += 1;

            let mut content = String::new();
            loop {
                if pos >= bytes.len() {
                    return Err(CompileError::LexError(format!(
                        "unterminated string literal at line {line}, column {start_col}"
                    )));
                }
                if bytes[pos] == b'"' {
                    pos += 1; // consume closing '"'
                    column += 1;
                    break;
                }
                if bytes[pos] == b'\\' {
                    pos += 1;
                    column += 1;
                    if pos >= bytes.len() {
                        return Err(CompileError::LexError(format!(
                            "unterminated escape sequence in string literal at line {line}, column {start_col}"
                        )));
                    }
                    let esc = bytes[pos];
                    pos += 1;
                    column += 1;
                    let ch = match esc {
                        b'n'  => '\n',
                        b't'  => '\t',
                        b'r'  => '\r',
                        b'\\' => '\\',
                        b'\'' => '\'',
                        b'"'  => '"',
                        b'0'  => '\0',
                        b'a'  => '\x07',
                        b'b'  => '\x08',
                        b'f'  => '\x0C',
                        b'v'  => '\x0B',
                        b'?'  => '?',
                        _ => {
                            return Err(CompileError::LexError(format!(
                                "unknown escape sequence '\\{}' in string literal at line {line}, column {start_col}",
                                esc as char
                            )));
                        }
                    };
                    content.push(ch);
                } else {
                    if bytes[pos] == b'\n' {
                        line += 1;
                        column = 1;
                    } else {
                        column += 1;
                    }
                    content.push(bytes[pos] as char);
                    pos += 1;
                }
            }

            tokens.push(Token {
                kind: TokenKind::StringLiteral(content),
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
                "long"     => TokenKind::KwLong,
                "unsigned" => TokenKind::KwUnsigned,
                "signed"   => TokenKind::KwSigned,
                "double"   => TokenKind::KwDouble,
                "sizeof"   => TokenKind::KwSizeof,
                "char"     => TokenKind::KwChar,
                "struct"   => TokenKind::KwStruct,
                "typedef"  => TokenKind::KwTypedef,
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

    /// `&` 単体はアドレス演算子（Chapter 14）
    #[test]
    fn lex_single_ampersand() {
        let tokens = lex("&x").unwrap();
        let kinds: Vec<_> = tokens.iter().map(|t| &t.kind).collect();
        assert_eq!(kinds, vec![&TokenKind::Ampersand, &TokenKind::Identifier("x".to_string())]);
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

    /// Chapter 11: long キーワードのトークン化
    #[test]
    fn lex_long_keyword() {
        let tokens = lex("long").unwrap();
        assert_eq!(tokens[0].kind, TokenKind::KwLong);
    }

    /// Chapter 11: L サフィックス付きリテラル
    #[test]
    fn lex_long_literal() {
        let tokens = lex("100L").unwrap();
        assert_eq!(tokens[0].kind, TokenKind::LongLiteral(100));
    }

    /// Chapter 11: l サフィックス付きリテラル（小文字）
    #[test]
    fn lex_long_literal_lowercase() {
        let tokens = lex("0l").unwrap();
        assert_eq!(tokens[0].kind, TokenKind::LongLiteral(0));
    }

    /// Chapter 11: L の後に英数字が続くとエラー
    #[test]
    fn lex_long_literal_invalid_suffix() {
        let result = lex("123La");
        assert!(result.is_err());
    }

    /// Chapter 12: unsigned/signed キーワード
    #[test]
    fn lex_unsigned_signed_keywords() {
        let tokens = lex("unsigned signed").unwrap();
        let kinds: Vec<_> = tokens.iter().map(|t| &t.kind).collect();
        assert_eq!(kinds, vec![&TokenKind::KwUnsigned, &TokenKind::KwSigned]);
    }

    /// Chapter 12: U サフィックス付きリテラル
    #[test]
    fn lex_uint_literal() {
        let tokens = lex("42U").unwrap();
        assert_eq!(tokens[0].kind, TokenKind::UIntLiteral(42));
    }

    /// Chapter 12: u サフィックス付きリテラル（小文字）
    #[test]
    fn lex_uint_literal_lowercase() {
        let tokens = lex("42u").unwrap();
        assert_eq!(tokens[0].kind, TokenKind::UIntLiteral(42));
    }

    /// Chapter 12: UL サフィックス付きリテラル
    #[test]
    fn lex_ulong_literal() {
        let tokens = lex("42UL").unwrap();
        assert_eq!(tokens[0].kind, TokenKind::ULongLiteral(42));
    }

    /// Chapter 12: ul サフィックス付きリテラル（小文字）
    #[test]
    fn lex_ulong_literal_lowercase() {
        let tokens = lex("42ul").unwrap();
        assert_eq!(tokens[0].kind, TokenKind::ULongLiteral(42));
    }

    /// Chapter 12: LU サフィックス（逆順）
    #[test]
    fn lex_ulong_literal_lu() {
        let tokens = lex("42LU").unwrap();
        assert_eq!(tokens[0].kind, TokenKind::ULongLiteral(42));
    }

    /// Chapter 12: lu サフィックス（逆順・小文字）
    #[test]
    fn lex_ulong_literal_lu_lowercase() {
        let tokens = lex("42lu").unwrap();
        assert_eq!(tokens[0].kind, TokenKind::ULongLiteral(42));
    }

    /// Chapter 12: 混合ケース Ul
    #[test]
    fn lex_ulong_literal_mixed_case() {
        let tokens = lex("42Ul").unwrap();
        assert_eq!(tokens[0].kind, TokenKind::ULongLiteral(42));
    }

    /// Chapter 12: U の後に英数字が続くとエラー
    #[test]
    fn lex_uint_literal_invalid_suffix() {
        let result = lex("123Ua");
        assert!(result.is_err());
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

    /// `...` トークンの字句解析
    #[test]
    fn lex_ellipsis() {
        let tokens = lex("int printf(const char *, ...);").unwrap();
        let kinds: Vec<_> = tokens.iter().map(|t| &t.kind).collect();
        // "const" は Identifier（キーワード未対応）
        assert!(kinds.contains(&&TokenKind::Ellipsis));
    }

    /// `.` と `...` の区別
    #[test]
    fn lex_dot_vs_ellipsis() {
        let tokens = lex("a.b, ...").unwrap();
        let kinds: Vec<_> = tokens.iter().map(|t| &t.kind).collect();
        assert_eq!(
            kinds,
            vec![
                &TokenKind::Identifier("a".to_string()),
                &TokenKind::Dot,
                &TokenKind::Identifier("b".to_string()),
                &TokenKind::Comma,
                &TokenKind::Ellipsis,
            ]
        );
    }
}
