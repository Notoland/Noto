//! Numeric literal lexing.

use crate::token::{Base, FloatLiteral, IntLiteral, NumericSuffix, TokenKind};
use crate::{is_ident_continue, Lexer};
use noto_diagnostics::{codes, Diagnostic};

impl Lexer<'_, '_> {
    /// Lexes an integer or float literal starting at `start`.
    ///
    /// The leading digit has already been consumed. Underscores are permitted
    /// as digit separators anywhere except at the start of the literal.
    pub(crate) fn lex_number(&mut self, start: u32) -> TokenKind {
        let first = self.cursor.slice(start, self.cursor.pos()).chars().next().unwrap_or('0');

        if first == '0' {
            if self.cursor.eat('x') || self.cursor.eat('X') {
                return self.lex_radix(start, Base::Hex, 16, |c| c.is_ascii_hexdigit());
            }
            if self.cursor.eat('b') || self.cursor.eat('B') {
                return self.lex_radix(start, Base::Binary, 2, |c| matches!(c, '0' | '1'));
            }
            if self.cursor.eat('o') || self.cursor.eat('O') {
                return self.lex_radix(start, Base::Octal, 8, |c| matches!(c, '0'..='7'));
            }
        }

        self.cursor.eat_while(|c| c.is_ascii_digit() || c == '_');

        // `1..10` is a range over integers, not the float `1.` followed by
        // `.10`, so a `.` only continues the literal when a digit follows.
        let mut is_float = false;
        if self.cursor.peek() == Some('.') && self.cursor.peek_nth(1).is_some_and(|c| c.is_ascii_digit())
        {
            is_float = true;
            self.cursor.bump();
            self.cursor.eat_while(|c| c.is_ascii_digit() || c == '_');
        }

        if matches!(self.cursor.peek(), Some('e' | 'E'))
            && self.exponent_follows()
        {
            is_float = true;
            self.cursor.bump();
            if matches!(self.cursor.peek(), Some('+' | '-')) {
                self.cursor.bump();
            }
            self.cursor.eat_while(|c| c.is_ascii_digit() || c == '_');
        }

        let digits_end = self.cursor.pos();
        let suffix = self.lex_suffix(digits_end);
        let digits: String =
            self.cursor.slice(start, digits_end).chars().filter(|c| *c != '_').collect();

        if is_float || suffix.is_some_and(NumericSuffix::is_float) {
            return match digits.parse::<f64>() {
                Ok(value) => TokenKind::Float(FloatLiteral { value, suffix }),
                Err(_) => {
                    self.invalid_number(start, "not a valid floating point literal");
                    TokenKind::Float(FloatLiteral { value: 0.0, suffix })
                }
            };
        }

        match digits.parse::<u128>() {
            Ok(value) => TokenKind::Int(IntLiteral { value, base: Base::Decimal, suffix }),
            Err(_) => {
                let span = self.span(start, self.cursor.pos());
                self.emit(
                    Diagnostic::error(
                        codes::NUMBER_OUT_OF_RANGE,
                        "integer literal is too large to represent",
                    )
                    .with_primary(span, "does not fit in any Noto integer type")
                    .with_help("the widest Noto integer type is `UInt64`"),
                );
                TokenKind::Int(IntLiteral { value: 0, base: Base::Decimal, suffix })
            }
        }
    }

    /// Whether an `e` at the cursor really starts an exponent, as opposed to
    /// the suffix-like tail of something else.
    fn exponent_follows(&self) -> bool {
        match self.cursor.peek_nth(1) {
            Some(c) if c.is_ascii_digit() => true,
            Some('+' | '-') => self.cursor.peek_nth(2).is_some_and(|c| c.is_ascii_digit()),
            _ => false,
        }
    }

    /// Lexes the body of a `0x` / `0b` / `0o` literal.
    fn lex_radix(
        &mut self,
        start: u32,
        base: Base,
        radix: u32,
        is_digit: fn(char) -> bool,
    ) -> TokenKind {
        let digits_start = self.cursor.pos();
        self.cursor.eat_while(|c| is_digit(c) || c == '_');
        let digits_end = self.cursor.pos();
        let suffix = self.lex_suffix(digits_end);

        let digits: String =
            self.cursor.slice(digits_start, digits_end).chars().filter(|c| *c != '_').collect();

        if digits.is_empty() {
            self.invalid_number(start, "this literal has no digits after its base prefix");
            return TokenKind::Int(IntLiteral { value: 0, base, suffix });
        }

        match u128::from_str_radix(&digits, radix) {
            Ok(value) => TokenKind::Int(IntLiteral { value, base, suffix }),
            Err(_) => {
                let span = self.span(start, self.cursor.pos());
                self.emit(
                    Diagnostic::error(
                        codes::NUMBER_OUT_OF_RANGE,
                        "integer literal is too large to represent",
                    )
                    .with_primary(span, "does not fit in any Noto integer type"),
                );
                TokenKind::Int(IntLiteral { value: 0, base, suffix })
            }
        }
    }

    /// Reads a trailing type suffix such as `u8` if one is present.
    fn lex_suffix(&mut self, digits_end: u32) -> Option<NumericSuffix> {
        if !self.cursor.peek().is_some_and(is_ident_continue) {
            return None;
        }
        let suffix_start = self.cursor.pos();
        self.cursor.eat_while(is_ident_continue);
        let text = self.cursor.slice(suffix_start, self.cursor.pos());

        match NumericSuffix::from_str(text) {
            Some(suffix) => Some(suffix),
            None => {
                let span = self.span(suffix_start, self.cursor.pos());
                self.emit(
                    Diagnostic::error(
                        codes::INVALID_NUMBER,
                        format!("`{text}` is not a numeric type suffix"),
                    )
                    .with_primary(span, "unknown suffix")
                    .with_note(
                        "valid suffixes are `i8` `i16` `i32` `i64` `u8` `u16` `u32` `u64` `f32` `f64`",
                    ),
                );
                let _ = digits_end;
                None
            }
        }
    }

    fn invalid_number(&mut self, start: u32, label: &str) {
        let span = self.span(start, self.cursor.pos());
        self.emit(
            Diagnostic::error(codes::INVALID_NUMBER, "malformed numeric literal")
                .with_primary(span, label),
        );
    }
}
