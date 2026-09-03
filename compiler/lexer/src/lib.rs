//! Lexical analysis for the Noto programming language.
//!
//! The lexer turns source text into a flat [`Token`] stream. It is the first
//! phase of the pipeline described in `docs/architecture.md` and is
//! deliberately free of grammar knowledge: it decides what the characters
//! *are*, never what they *mean*. The one concession to the layers above is
//! [`Token::newline_before`], which records where line breaks occurred so the
//! parser can apply Noto's statement-termination rule.
//!
//! Lexing never fails outright. Malformed input produces a diagnostic and a
//! best-effort token so that later phases still see a usable stream and the
//! user gets more than one error per run.
//!
//! ```
//! # use noto_lexer::{tokenize, TokenKind};
//! # use noto_span::SourceMap;
//! # use noto_diagnostics::DiagnosticSink;
//! let mut map = SourceMap::new();
//! let file = map.add("demo.noto", "val answer = 42");
//! let mut sink = DiagnosticSink::new();
//! let tokens = tokenize(map.file(file).unwrap(), &mut sink);
//! assert!(!sink.has_errors());
//! assert_eq!(tokens.len(), 5); // val, answer, =, 42, EOF
//! ```

#![deny(missing_docs)]

mod cursor;
mod keyword;
mod number;
mod string;
mod token;

pub use keyword::{Keyword, RESERVED_FOR_FUTURE};
pub use token::{
    Base, FloatLiteral, IntLiteral, NumericSuffix, StringLiteral, StringPart, Token, TokenKind,
};

use cursor::Cursor;
use noto_diagnostics::{codes, Diagnostic, DiagnosticSink};
use noto_span::{FileId, SourceFile, Span};

/// Tokenizes a whole source file.
///
/// The returned stream always ends with a [`TokenKind::Eof`] token. Errors are
/// reported through `sink`; the stream stays usable either way.
pub fn tokenize(file: &SourceFile, sink: &mut DiagnosticSink) -> Vec<Token> {
    Lexer::new(file.id(), file.text(), sink).run()
}

/// Tokenizes a standalone snippet of source text.
///
/// Used by the interpolation lexer and by tests that do not want to build a
/// full [`SourceFile`].
pub fn tokenize_str(file: FileId, text: &str, sink: &mut DiagnosticSink) -> Vec<Token> {
    Lexer::new(file, text, sink).run()
}

/// The lexer state machine.
pub(crate) struct Lexer<'src, 'sink> {
    cursor: Cursor<'src>,
    file: FileId,
    sink: &'sink mut DiagnosticSink,
    /// Byte offset added to every span, so that tokens lexed from inside a
    /// string interpolation still point at the right place in the file.
    offset: u32,
    /// Set when trivia containing a line break has been skipped.
    pending_newline: bool,
}

impl<'src, 'sink> Lexer<'src, 'sink> {
    pub(crate) fn new(file: FileId, text: &'src str, sink: &'sink mut DiagnosticSink) -> Self {
        Lexer { cursor: Cursor::new(text), file, sink, offset: 0, pending_newline: false }
    }

    /// Shifts every span this lexer produces by `offset` bytes.
    pub(crate) fn with_offset(mut self, offset: u32) -> Self {
        self.offset = offset;
        self
    }

    /// Lexes until end of input.
    pub(crate) fn run(mut self) -> Vec<Token> {
        let mut tokens = Vec::new();
        loop {
            let token = self.next_token();
            let is_eof = token.is_eof();
            tokens.push(token);
            if is_eof {
                return tokens;
            }
        }
    }

    fn span(&self, start: u32, end: u32) -> Span {
        Span::new(self.file, start + self.offset, end + self.offset)
    }

    fn emit(&mut self, diagnostic: Diagnostic) {
        self.sink.emit(diagnostic);
    }

    fn make(&mut self, kind: TokenKind, start: u32) -> Token {
        let span = self.span(start, self.cursor.pos());
        let newline_before = std::mem::take(&mut self.pending_newline);
        Token::new(kind, span, newline_before)
    }

    /// Produces the next token, skipping whitespace and comments.
    fn next_token(&mut self) -> Token {
        loop {
            self.skip_trivia();
            let start = self.cursor.pos();

            // A doc comment is trivia to the grammar but data to the tooling,
            // so it survives skip_trivia and becomes a real token.
            if self.cursor.starts_with("///") {
                let text = self.lex_doc_comment();
                return self.make(TokenKind::DocComment(text), start);
            }

            let Some(ch) = self.cursor.bump() else {
                return self.make(TokenKind::Eof, start);
            };

            let kind = match ch {
                '(' => TokenKind::LParen,
                ')' => TokenKind::RParen,
                '{' => TokenKind::LBrace,
                '}' => TokenKind::RBrace,
                '[' => TokenKind::LBracket,
                ']' => TokenKind::RBracket,
                ',' => TokenKind::Comma,
                ';' => TokenKind::Semicolon,
                '@' => TokenKind::At,
                '~' => TokenKind::Tilde,
                '+' => self.pick(&[('=', TokenKind::PlusEq)], TokenKind::Plus),
                '-' => self.pick(
                    &[('=', TokenKind::MinusEq), ('>', TokenKind::Arrow)],
                    TokenKind::Minus,
                ),
                '*' => self.pick(&[('=', TokenKind::StarEq)], TokenKind::Star),
                '/' => self.pick(&[('=', TokenKind::SlashEq)], TokenKind::Slash),
                '%' => self.pick(&[('=', TokenKind::PercentEq)], TokenKind::Percent),
                '^' => self.pick(&[('=', TokenKind::CaretEq)], TokenKind::Caret),
                '=' => self.pick(
                    &[('=', TokenKind::EqEq), ('>', TokenKind::FatArrow)],
                    TokenKind::Eq,
                ),
                '!' => self.pick(&[('=', TokenKind::BangEq)], TokenKind::Bang),
                '&' => self.pick(
                    &[('&', TokenKind::AmpAmp), ('=', TokenKind::AmpEq)],
                    TokenKind::Amp,
                ),
                '|' => self.pick(
                    &[('|', TokenKind::PipePipe), ('=', TokenKind::PipeEq)],
                    TokenKind::Pipe,
                ),
                ':' => self.pick(&[(':', TokenKind::ColonColon)], TokenKind::Colon),
                '?' => self.lex_question(),
                '.' => self.lex_dot(),
                '<' => self.lex_lt(),
                '>' => self.lex_gt(),
                '"' => self.lex_string(start),
                '\'' => self.lex_char(start),
                '0'..='9' => self.lex_number(start),
                ch if is_ident_start(ch) => self.lex_ident_or_keyword(start),
                ch => {
                    let span = self.span(start, self.cursor.pos());
                    self.emit(
                        Diagnostic::error(
                            codes::UNEXPECTED_CHARACTER,
                            format!("unexpected character `{}` in source", ch.escape_debug()),
                        )
                        .with_primary(span, "not valid anywhere in Noto source")
                        .with_help("remove the character, or place it inside a string literal"),
                    );
                    continue;
                }
            };

            return self.make(kind, start);
        }
    }

    /// Picks a multi-character operator based on the following character.
    fn pick(&mut self, options: &[(char, TokenKind)], fallback: TokenKind) -> TokenKind {
        for (next, kind) in options {
            if self.cursor.eat(*next) {
                return kind.clone();
            }
        }
        fallback
    }

    fn lex_question(&mut self) -> TokenKind {
        // `?.` and `?:` are single operators; a bare `?` is either the nullable
        // type marker or the propagation operator, which the parser tells apart
        // by position.
        if self.cursor.eat('.') {
            TokenKind::QuestionDot
        } else if self.cursor.eat(':') {
            TokenKind::Elvis
        } else {
            TokenKind::Question
        }
    }

    fn lex_dot(&mut self) -> TokenKind {
        if self.cursor.eat('.') {
            if self.cursor.eat('=') {
                TokenKind::DotDotEq
            } else {
                TokenKind::DotDot
            }
        } else {
            TokenKind::Dot
        }
    }

    fn lex_lt(&mut self) -> TokenKind {
        if self.cursor.eat('<') {
            if self.cursor.eat('=') {
                TokenKind::ShlEq
            } else {
                TokenKind::Shl
            }
        } else if self.cursor.eat('=') {
            TokenKind::LtEq
        } else {
            TokenKind::Lt
        }
    }

    /// Lexes `>`, `>=`, `>>` and `>>=`.
    ///
    /// `>>` is produced as one token; the parser splits it when closing nested
    /// generic arguments such as `List<List<Int>>`.
    fn lex_gt(&mut self) -> TokenKind {
        if self.cursor.eat('>') {
            if self.cursor.eat('=') {
                TokenKind::ShrEq
            } else {
                TokenKind::Shr
            }
        } else if self.cursor.eat('=') {
            TokenKind::GtEq
        } else {
            TokenKind::Gt
        }
    }

    fn lex_ident_or_keyword(&mut self, start: u32) -> TokenKind {
        self.cursor.eat_while(is_ident_continue);
        let text = self.cursor.slice(start, self.cursor.pos());

        if text == "_" {
            return TokenKind::Underscore;
        }
        match Keyword::from_str(text) {
            Some(keyword) => TokenKind::Keyword(keyword),
            None => TokenKind::Ident(text.to_string()),
        }
    }

    /// Skips whitespace and non-doc comments, remembering line breaks.
    fn skip_trivia(&mut self) {
        loop {
            match self.cursor.peek() {
                Some('\n') => {
                    self.pending_newline = true;
                    self.cursor.bump();
                }
                Some(ch) if ch.is_whitespace() => {
                    self.cursor.bump();
                }
                Some('/') if self.cursor.peek_nth(1) == Some('/') => {
                    // `///` is a doc comment and is not trivia.
                    if self.cursor.peek_nth(2) == Some('/') {
                        return;
                    }
                    self.cursor.eat_while(|c| c != '\n');
                }
                Some('/') if self.cursor.peek_nth(1) == Some('*') => self.skip_block_comment(),
                _ => return,
            }
        }
    }

    /// Skips a `/* ... */` comment. Block comments nest.
    fn skip_block_comment(&mut self) {
        let start = self.cursor.pos();
        self.cursor.bump();
        self.cursor.bump();
        let mut depth = 1usize;

        while depth > 0 {
            match self.cursor.bump() {
                None => {
                    let span = self.span(start, self.cursor.pos());
                    self.emit(
                        Diagnostic::error(
                            codes::UNTERMINATED_COMMENT,
                            "block comment is never closed",
                        )
                        .with_primary(span, "this comment reaches the end of the file")
                        .with_help("close it with `*/`"),
                    );
                    return;
                }
                Some('\n') => self.pending_newline = true,
                Some('/') if self.cursor.eat('*') => depth += 1,
                Some('*') if self.cursor.eat('/') => depth -= 1,
                Some(_) => {}
            }
        }
    }

    /// Reads a `///` comment, returning its text without the marker.
    fn lex_doc_comment(&mut self) -> String {
        self.cursor.eat_str("///");
        let start = self.cursor.pos();
        self.cursor.eat_while(|c| c != '\n');
        self.cursor.slice(start, self.cursor.pos()).trim().to_string()
    }
}

/// Whether `ch` may start an identifier.
///
/// Noto identifiers are Unicode: `usuário` and `名前` are valid names. The
/// classification is intentionally simple — alphabetic or `_` — rather than
/// the full UAX #31 property set, which the language will adopt once the
/// standard library owns a Unicode table.
pub fn is_ident_start(ch: char) -> bool {
    ch == '_' || ch.is_alphabetic()
}

/// Whether `ch` may continue an identifier.
pub fn is_ident_continue(ch: char) -> bool {
    ch == '_' || ch.is_alphanumeric()
}

#[cfg(test)]
mod tests;
