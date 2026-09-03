//! String and character literal lexing, including interpolation.

use crate::token::{StringLiteral, StringPart, TokenKind};
use crate::{is_ident_continue, is_ident_start, Lexer};
use noto_diagnostics::{codes, Diagnostic};

impl<'src, 'sink> Lexer<'src, 'sink> {
    /// Lexes a string literal. The opening `"` has already been consumed.
    pub(crate) fn lex_string(&mut self, start: u32) -> TokenKind {
        let multiline = self.cursor.eat_str("\"\"");
        let mut parts: Vec<StringPart> = Vec::new();
        let mut text = String::new();

        loop {
            if multiline {
                if self.cursor.eat_str("\"\"\"") {
                    break;
                }
            } else if self.cursor.eat('"') {
                break;
            }

            match self.cursor.peek() {
                None => {
                    let span = self.span(start, self.cursor.pos());
                    self.emit(
                        Diagnostic::error(
                            codes::UNTERMINATED_STRING,
                            "string literal is never closed",
                        )
                        .with_primary(span, "this string reaches the end of the file")
                        .with_help(if multiline {
                            "close it with `\"\"\"`"
                        } else {
                            "close it with `\"`"
                        }),
                    );
                    break;
                }
                // A single-line string may not span a line break: the error is
                // almost always a missing quote, and reporting it here points
                // at the right line instead of swallowing the rest of the file.
                Some('\n') if !multiline => {
                    let span = self.span(start, self.cursor.pos());
                    self.emit(
                        Diagnostic::error(
                            codes::UNTERMINATED_STRING,
                            "string literal is never closed",
                        )
                        .with_primary(span, "this string is missing its closing `\"`")
                        .with_help("use a `\"\"\"` literal to write text across several lines"),
                    );
                    break;
                }
                Some('\\') => {
                    self.cursor.bump();
                    if let Some(ch) = self.lex_escape() {
                        text.push(ch);
                    }
                }
                Some('$') if self.interpolation_follows() => {
                    if !text.is_empty() {
                        parts.push(StringPart::Text(std::mem::take(&mut text)));
                    }
                    if let Some(part) = self.lex_interpolation() {
                        parts.push(part);
                    }
                }
                Some(ch) => {
                    self.cursor.bump();
                    text.push(ch);
                }
            }
        }

        if !text.is_empty() || parts.is_empty() {
            parts.push(StringPart::Text(text));
        }

        if multiline {
            trim_multiline(&mut parts);
        }

        TokenKind::Str(StringLiteral { parts, multiline })
    }

    /// Whether the `$` at the cursor introduces an interpolation. A `$`
    /// followed by anything else is an ordinary character, so prices like
    /// `"$5"` need no escaping.
    fn interpolation_follows(&self) -> bool {
        match self.cursor.peek_nth(1) {
            Some('{') => true,
            Some(ch) => is_ident_start(ch),
            None => false,
        }
    }

    /// Lexes `$name` or `${ expression }` into a token stream.
    fn lex_interpolation(&mut self) -> Option<StringPart> {
        let start = self.cursor.pos();
        self.cursor.bump(); // `$`

        let (expr_start, expr_end) = if self.cursor.eat('{') {
            let expr_start = self.cursor.pos();
            let mut depth = 1usize;
            loop {
                match self.cursor.peek() {
                    None => {
                        let span = self.span(start, self.cursor.pos());
                        self.emit(
                            Diagnostic::error(
                                codes::UNTERMINATED_INTERPOLATION,
                                "interpolated expression is never closed",
                            )
                            .with_primary(span, "this `${` has no matching `}`"),
                        );
                        return None;
                    }
                    Some('{') => {
                        depth += 1;
                        self.cursor.bump();
                    }
                    Some('}') => {
                        depth -= 1;
                        let end = self.cursor.pos();
                        self.cursor.bump();
                        if depth == 0 {
                            break (expr_start, end);
                        }
                    }
                    Some(_) => {
                        self.cursor.bump();
                    }
                }
            }
        } else {
            let expr_start = self.cursor.pos();
            self.cursor.eat_while(is_ident_continue);
            (expr_start, self.cursor.pos())
        };

        let source = self.cursor.slice(expr_start, expr_end);
        // Sub-lexing keeps spans anchored in the enclosing file, so a
        // diagnostic inside `"${a + }"` still points at the real column.
        let tokens = Lexer::new(self.file, source, self.sink)
            .with_offset(self.offset + expr_start)
            .run();

        Some(StringPart::Interpolation {
            tokens,
            span: self.span(start, self.cursor.pos()),
        })
    }

    /// Lexes the body of an escape sequence; the `\` is already consumed.
    fn lex_escape(&mut self) -> Option<char> {
        let start = self.cursor.pos() - 1;
        let ch = self.cursor.bump()?;
        let resolved = match ch {
            'n' => '\n',
            'r' => '\r',
            't' => '\t',
            '0' => '\0',
            '\\' => '\\',
            '\'' => '\'',
            '"' => '"',
            '$' => '$',
            'u' => return self.lex_unicode_escape(start),
            other => {
                let span = self.span(start, self.cursor.pos());
                self.emit(
                    Diagnostic::error(
                        codes::INVALID_ESCAPE,
                        format!("unknown escape sequence `\\{}`", other.escape_debug()),
                    )
                    .with_primary(span, "not a valid escape")
                    .with_note(
                        "valid escapes are `\\n` `\\r` `\\t` `\\0` `\\\\` `\\'` `\\\"` `\\$` and `\\u{...}`",
                    ),
                );
                return None;
            }
        };
        Some(resolved)
    }

    /// Lexes `\u{1F600}`; the `\u` is already consumed.
    fn lex_unicode_escape(&mut self, start: u32) -> Option<char> {
        if !self.cursor.eat('{') {
            let span = self.span(start, self.cursor.pos());
            self.emit(
                Diagnostic::error(codes::INVALID_ESCAPE, "malformed unicode escape")
                    .with_primary(span, "expected `{` after `\\u`")
                    .with_help("write the code point as `\\u{1F600}`"),
            );
            return None;
        }

        let digits_start = self.cursor.pos();
        self.cursor.eat_while(|c| c.is_ascii_hexdigit());
        let digits = self.cursor.slice(digits_start, self.cursor.pos()).to_string();
        let closed = self.cursor.eat('}');
        let span = self.span(start, self.cursor.pos());

        if !closed || digits.is_empty() {
            self.emit(
                Diagnostic::error(codes::INVALID_ESCAPE, "malformed unicode escape")
                    .with_primary(span, "expected hexadecimal digits followed by `}`")
                    .with_help("write the code point as `\\u{1F600}`"),
            );
            return None;
        }

        match u32::from_str_radix(&digits, 16).ok().and_then(char::from_u32) {
            Some(ch) => Some(ch),
            None => {
                self.emit(
                    Diagnostic::error(
                        codes::INVALID_ESCAPE,
                        format!("`{digits}` is not a Unicode scalar value"),
                    )
                    .with_primary(span, "no character has this code point")
                    .with_note("valid code points are U+0000..=U+D7FF and U+E000..=U+10FFFF"),
                );
                None
            }
        }
    }

    /// Lexes a character literal. The opening `'` has already been consumed.
    pub(crate) fn lex_char(&mut self, start: u32) -> TokenKind {
        let mut value = None;
        let mut count = 0usize;

        loop {
            match self.cursor.peek() {
                None | Some('\n') => {
                    let span = self.span(start, self.cursor.pos());
                    self.emit(
                        Diagnostic::error(
                            codes::INVALID_CHAR_LITERAL,
                            "character literal is never closed",
                        )
                        .with_primary(span, "expected a closing `'`"),
                    );
                    return TokenKind::Char(value.unwrap_or('\u{FFFD}'));
                }
                Some('\'') => {
                    self.cursor.bump();
                    break;
                }
                Some('\\') => {
                    self.cursor.bump();
                    let ch = self.lex_escape();
                    if count == 0 {
                        value = ch;
                    }
                    count += 1;
                }
                Some(ch) => {
                    self.cursor.bump();
                    if count == 0 {
                        value = Some(ch);
                    }
                    count += 1;
                }
            }
        }

        if count != 1 {
            let span = self.span(start, self.cursor.pos());
            let message = if count == 0 {
                "character literal is empty"
            } else {
                "character literal holds more than one character"
            };
            self.emit(
                Diagnostic::error(codes::INVALID_CHAR_LITERAL, message)
                    .with_primary(span, "a `Char` is exactly one Unicode scalar value")
                    .with_help("use a `\"...\"` string literal for text"),
            );
        }

        TokenKind::Char(value.unwrap_or('\u{FFFD}'))
    }
}

/// Applies the multiline-literal layout rule.
///
/// A `"""` literal drops the line break that follows the opening delimiter and
/// removes the common leading indentation from every line, so the text reads
/// the way it is laid out in the source rather than carrying the surrounding
/// indentation into the value.
fn trim_multiline(parts: &mut [StringPart]) {
    let Some(StringPart::Text(first)) = parts.first_mut() else { return };
    if let Some(rest) = first.strip_prefix('\n') {
        *first = rest.to_string();
    } else if let Some(rest) = first.strip_prefix("\r\n") {
        *first = rest.to_string();
    }

    // Indentation is measured over literal text only; an interpolation can
    // hold anything, so lines it produces are left untouched.
    let mut common: Option<usize> = None;
    for (index, part) in parts.iter().enumerate() {
        let StringPart::Text(text) = part else { continue };
        for (line_index, line) in text.split('\n').enumerate() {
            let is_continuation = line_index > 0 || index == 0;
            if !is_continuation || line.trim().is_empty() {
                continue;
            }
            let indent = line.len() - line.trim_start_matches(' ').len();
            common = Some(common.map_or(indent, |c: usize| c.min(indent)));
        }
    }

    let Some(indent) = common.filter(|i| *i > 0) else { return };
    for (index, part) in parts.iter_mut().enumerate() {
        let StringPart::Text(text) = part else { continue };
        let mut out = String::with_capacity(text.len());
        for (line_index, line) in text.split('\n').enumerate() {
            if line_index > 0 {
                out.push('\n');
            }
            let strip = line_index > 0 || index == 0;
            if strip && line.len() >= indent && line[..indent].bytes().all(|b| b == b' ') {
                out.push_str(&line[indent..]);
            } else {
                out.push_str(line);
            }
        }
        *text = out;
    }

    // The last line of a multiline literal is the indentation before the
    // closing delimiter and is not part of the value.
    if let Some(StringPart::Text(last)) = parts.last_mut() {
        if let Some(cut) = last.rfind('\n') {
            if last[cut + 1..].chars().all(|c| c == ' ') {
                last.truncate(cut);
            }
        }
    }
}
