//! The Noto formatter.
//!
//! `noto fmt` is deterministic and opinionated and has no options: one input
//! has one formatting. The rules are written down in
//! `docs/design/formatter.md`; this crate implements them.
//!
//! It works on the token stream rather than the AST, for two reasons. The
//! lexer discards `//` and `/* */` as trivia, so an AST printer would delete
//! every ordinary comment in the file; the formatter instead recovers them
//! from the source text between consecutive token spans. And every token's
//! text is copied from the source slice it came from, so a literal, an
//! identifier or a comment cannot be altered by being reprinted.
//!
//! The formatter changes whitespace and nothing else. It never moves code
//! between lines: Noto has no statement terminator, so a line break is part
//! of the grammar, and re-flowing lines would mean deciding where statements
//! end. That gives the crate its central promise, which
//! [`tokens_match`](tokens_match) checks and the tests assert over every
//! sample:
//!
//! > lexing the formatted text produces exactly the token stream that lexing
//! > the original produced.

#![deny(missing_docs)]

mod lines;
mod spacing;

use noto_diagnostics::DiagnosticSink;
use noto_lexer::Token;
use noto_span::SourceFile;

/// One indentation level.
pub const INDENT: &str = "    ";

/// Formats a source file.
///
/// Returns `None` when lexing reported an error, leaving the file alone: the
/// formatter's promise is about the token stream, and a file that does not
/// lex cleanly has none to promise anything about. Diagnostics go to `sink`
/// like every other phase.
pub fn format(file: &SourceFile, sink: &mut DiagnosticSink) -> Option<String> {
    let before = sink.error_count();
    let tokens = noto_lexer::tokenize(file, sink);
    if sink.error_count() > before {
        return None;
    }
    Some(lines::render(file.text(), &tokens))
}

/// Whether a file is already formatted.
///
/// This is what `noto fmt --check` asks. It is a plain equality on the text,
/// which is what makes the check meaningful: formatting is idempotent, so a
/// file that differs from its formatting differs for a reason.
pub fn is_formatted(file: &SourceFile, sink: &mut DiagnosticSink) -> Option<bool> {
    format(file, sink).map(|formatted| formatted == file.text())
}

/// Whether two token streams are the same but for their spans.
///
/// Spans move when whitespace changes, so they are not compared; everything
/// else is, `newline_before` included, because the line break is part of
/// Noto's grammar.
pub fn tokens_match(left: &[Token], right: &[Token]) -> bool {
    left.len() == right.len()
        && left.iter().zip(right).all(|(left, right)| {
            left.kind == right.kind && left.newline_before == right.newline_before
        })
}

#[cfg(test)]
mod tests;
