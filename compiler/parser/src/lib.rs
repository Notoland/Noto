//! The Noto parser.
//!
//! A hand-written recursive-descent parser with a precedence climbing loop for
//! binary operators. It is written by hand rather than generated so that error
//! messages can be specific and so that the parser can recover: a syntax error
//! produces a diagnostic and an `Error` node, and parsing continues at the next
//! statement or declaration boundary. One run therefore reports many problems.
//!
//! # Statement termination
//!
//! Noto has no statement terminator. A statement ends at a line break once it
//! is syntactically complete, so an operator left at the end of a line
//! continues the expression:
//!
//! ```text
//! val total = price +      // continues: `+` cannot end a statement
//!             tax
//!
//! val a = first            // ends here
//! -second                  // a new statement, not `first - second`
//! ```
//!
//! Member access is the one exception: a line starting with `.` or `?.`
//! continues the previous expression, which is what makes chained calls
//! readable. A `;` may still be written to put several statements on one line.

#![deny(missing_docs)]

mod expr;
mod item;
mod pattern;
mod stmt;
mod ty;

use noto_ast::{Ident, Module, NodeId, NodeIdGenerator, Path};
use noto_diagnostics::{codes, Diagnostic, DiagnosticSink};
use noto_lexer::{tokenize, Keyword, Token, TokenKind};
use noto_span::{SourceFile, Span};

pub use expr::Precedence;

/// Parses a whole source file into a [`Module`].
///
/// Lexing happens first; both phases report into `sink`. A module is always
/// returned, even when it contains `Error` nodes, so that tooling can keep
/// working on a file the user is still typing.
pub fn parse_file(file: &SourceFile, sink: &mut DiagnosticSink) -> Module {
    parse_file_from(file, noto_ast::NodeId(0), sink).0
}

/// Parses a file whose node ids continue from `first`, and says where the
/// next file should start.
///
/// One program is many files, and everything analysis learns is keyed by
/// [`NodeId`](noto_ast::NodeId) in one table, so two modules must never hand
/// out the same id.
pub fn parse_file_from(
    file: &SourceFile,
    first: noto_ast::NodeId,
    sink: &mut DiagnosticSink,
) -> (Module, noto_ast::NodeId) {
    let tokens = tokenize(file, sink);
    let span = Span::new(file.id(), 0, file.len());
    let mut parser = Parser::starting_at(tokens, span, first, sink);
    let module = parser.parse_module();
    let next = parser.ids.next_free();
    (module, next)
}

/// Parses a token stream that was produced elsewhere, such as the body of a
/// string interpolation.
pub fn parse_tokens(tokens: Vec<Token>, span: Span, sink: &mut DiagnosticSink) -> Module {
    Parser::new(tokens, span, sink).parse_module()
}

/// Restrictions that change how the next expression is parsed.
///
/// `if user.isActive { .. }` is ambiguous without them: the `{` could open the
/// body or a trailing lambda passed to `isActive`. Inside a condition the
/// parser sets [`Restrictions::NO_TRAILING_LAMBDA`] and the brace always opens
/// the body.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct Restrictions(u8);

impl Restrictions {
    /// No restrictions.
    pub const NONE: Restrictions = Restrictions(0);
    /// A `{` does not start a trailing lambda.
    pub const NO_TRAILING_LAMBDA: Restrictions = Restrictions(1 << 0);

    /// Whether every flag in `other` is set.
    pub fn contains(self, other: Restrictions) -> bool {
        self.0 & other.0 == other.0
    }

    /// The union of two restriction sets.
    pub fn with(self, other: Restrictions) -> Restrictions {
        Restrictions(self.0 | other.0)
    }
}

/// The recursive-descent parser.
pub struct Parser<'sink> {
    tokens: Vec<Token>,
    /// Index of the next token to read.
    position: usize,
    sink: &'sink mut DiagnosticSink,
    ids: NodeIdGenerator,
    /// The span of the whole input, used for end-of-file diagnostics.
    file_span: Span,
    /// Guards against a recovery loop that never consumes a token.
    last_error_position: Option<usize>,
    /// Set when a `data` modifier has been read and the declaration keyword it
    /// applies to has not been reached yet.
    pending_data: bool,
}

impl<'sink> Parser<'sink> {
    /// Creates a parser over an already-lexed token stream.
    pub fn new(tokens: Vec<Token>, file_span: Span, sink: &'sink mut DiagnosticSink) -> Self {
        Parser::starting_at(tokens, file_span, noto_ast::NodeId(0), sink)
    }

    /// Creates a parser whose node ids continue from `first`.
    pub fn starting_at(
        tokens: Vec<Token>,
        file_span: Span,
        first: noto_ast::NodeId,
        sink: &'sink mut DiagnosticSink,
    ) -> Self {
        Parser {
            tokens,
            position: 0,
            sink,
            ids: NodeIdGenerator::starting_at(first),
            file_span,
            last_error_position: None,
            pending_data: false,
        }
    }

    // --- token stream ---------------------------------------------------

    /// The token about to be read.
    fn peek(&self) -> &Token {
        &self.tokens[self.position.min(self.tokens.len() - 1)]
    }

    /// The token `n` positions ahead.
    fn peek_nth(&self, n: usize) -> &Token {
        &self.tokens[(self.position + n).min(self.tokens.len() - 1)]
    }

    /// The kind of the token about to be read.
    fn peek_kind(&self) -> &TokenKind {
        &self.peek().kind
    }

    /// Whether the next token has the given kind.
    fn check(&self, kind: &TokenKind) -> bool {
        self.peek_kind() == kind
    }

    /// Whether the next token is the given keyword.
    fn check_keyword(&self, keyword: Keyword) -> bool {
        self.peek().keyword() == Some(keyword)
    }

    /// Whether input is exhausted.
    fn at_eof(&self) -> bool {
        self.peek().is_eof()
    }

    /// Whether a line break separates the next token from the previous one.
    fn at_line_start(&self) -> bool {
        self.peek().newline_before
    }

    /// Consumes and returns the next token.
    fn advance(&mut self) -> Token {
        let token = self.tokens[self.position.min(self.tokens.len() - 1)].clone();
        if self.position < self.tokens.len() - 1 {
            self.position += 1;
        }
        token
    }

    /// Consumes the next token if it has the given kind.
    fn eat(&mut self, kind: &TokenKind) -> bool {
        if self.check(kind) {
            self.advance();
            true
        } else {
            false
        }
    }

    /// Consumes the next token if it is the given keyword.
    fn eat_keyword(&mut self, keyword: Keyword) -> bool {
        if self.check_keyword(keyword) {
            self.advance();
            true
        } else {
            false
        }
    }

    /// Replaces the token at the cursor.
    ///
    /// Used when a token has to be split, as when `>>` closes two nested type
    /// argument lists.
    fn replace_current(&mut self, token: Token) {
        let index = self.position.min(self.tokens.len() - 1);
        self.tokens[index] = token;
    }

    /// The span of the token about to be read.
    fn peek_span(&self) -> Span {
        self.peek().span
    }

    /// The span of the token just read.
    fn previous_span(&self) -> Span {
        if self.position == 0 {
            self.file_span
        } else {
            self.tokens[self.position - 1].span
        }
    }

    /// Allocates a node id.
    fn next_id(&mut self) -> NodeId {
        self.ids.next_id()
    }

    // --- errors and recovery ---------------------------------------------

    /// Reports a diagnostic, suppressing repeats at the same position so that
    /// recovery cannot flood the output.
    fn error(&mut self, diagnostic: Diagnostic) {
        if self.last_error_position == Some(self.position) {
            return;
        }
        self.last_error_position = Some(self.position);
        self.sink.emit(diagnostic);
    }

    /// Reports "expected X, found Y" at the current token.
    fn expected(&mut self, what: &str) {
        let found = self.peek_kind().describe();
        let span = self.peek_span();
        self.error(
            Diagnostic::error(codes::UNEXPECTED_TOKEN, format!("expected {what}, found {found}"))
                .with_primary(span, format!("expected {what} here")),
        );
    }

    /// Consumes the expected token, or reports and leaves the stream alone.
    fn expect(&mut self, kind: &TokenKind) -> bool {
        if self.eat(kind) {
            return true;
        }
        let what = kind.symbol().map(|s| format!("`{s}`")).unwrap_or_else(|| kind.describe());
        self.expected(&what);
        false
    }

    /// Consumes the expected keyword, or reports.
    fn expect_keyword(&mut self, keyword: Keyword) -> bool {
        if self.eat_keyword(keyword) {
            return true;
        }
        self.expected(&format!("`{}`", keyword.as_str()));
        false
    }

    /// Consumes an identifier, or reports and returns a placeholder.
    fn expect_ident(&mut self) -> Ident {
        if let Some(name) = self.peek().ident().map(str::to_string) {
            let span = self.peek_span();
            self.advance();
            return Ident::new(name, span);
        }
        // A reserved word in name position is a common mistake and deserves a
        // better message than "expected identifier".
        if let Some(keyword) = self.peek().keyword() {
            let span = self.peek_span();
            self.error(
                Diagnostic::error(
                    codes::UNEXPECTED_TOKEN,
                    format!("`{}` is a reserved word and cannot be used as a name", keyword.as_str()),
                )
                .with_primary(span, "reserved word")
                .with_help("choose a different name"),
            );
            self.advance();
            return Ident::new(keyword.as_str(), span);
        }
        self.expected("a name");
        Ident::new("<error>", self.peek_span())
    }

    /// Parses a dotted path such as `std.io.File`.
    fn parse_path(&mut self) -> Path {
        let start = self.peek_span();
        let mut segments = vec![self.expect_ident()];
        // A `.` on the next line starts a chained call, not a longer path.
        while self.check(&TokenKind::Dot)
            && !self.at_line_start()
            && matches!(self.peek_nth(1).kind, TokenKind::Ident(_))
        {
            self.advance();
            segments.push(self.expect_ident());
        }
        let span = start.to(self.previous_span());
        Path::new(segments, span)
    }

    /// Skips tokens until something that can begin a new declaration.
    fn recover_to_item(&mut self) {
        let mut depth = 0i32;
        while !self.at_eof() {
            match self.peek_kind() {
                TokenKind::LBrace => depth += 1,
                TokenKind::RBrace => {
                    depth -= 1;
                    if depth < 0 {
                        return;
                    }
                }
                TokenKind::Keyword(keyword) if depth == 0 && starts_item(*keyword) => return,
                _ => {}
            }
            self.advance();
        }
    }

    /// Skips tokens until the end of the current statement.
    fn recover_to_statement(&mut self) {
        let mut depth = 0i32;
        while !self.at_eof() {
            match self.peek_kind() {
                TokenKind::Semicolon => {
                    self.advance();
                    return;
                }
                TokenKind::LBrace | TokenKind::LParen | TokenKind::LBracket => depth += 1,
                TokenKind::RBrace | TokenKind::RParen | TokenKind::RBracket => {
                    if depth == 0 {
                        return;
                    }
                    depth -= 1;
                }
                _ => {}
            }
            self.advance();
            if depth == 0 && self.at_line_start() {
                return;
            }
        }
    }

    /// Consumes the end of a statement, reporting when one is missing.
    ///
    /// A statement ends at a line break, at a `;`, or at the `}` that closes
    /// the enclosing block.
    fn expect_statement_end(&mut self) {
        if self.eat(&TokenKind::Semicolon) || self.at_line_start() || self.at_eof() {
            return;
        }
        if matches!(self.peek_kind(), TokenKind::RBrace) {
            return;
        }
        let span = self.peek_span();
        let found = self.peek_kind().describe();
        self.error(
            Diagnostic::error(
                codes::UNEXPECTED_TOKEN,
                format!("expected the statement to end, found {found}"),
            )
            .with_primary(span, "unexpected here")
            .with_help("put the next statement on its own line, or separate them with `;`"),
        );
        self.recover_to_statement();
    }
}

/// Whether a keyword can begin a top-level declaration.
fn starts_item(keyword: Keyword) -> bool {
    use Keyword::*;
    matches!(
        keyword,
        Fn | Class
            | Struct
            | Data
            | Interface
            | Enum
            | Const
            | Import
            | Export
            | Test
            | Public
            | Private
            | Protected
            | Internal
            | Abstract
            | Sealed
            | Async
            | Override
    )
}

#[cfg(test)]
mod tests;
