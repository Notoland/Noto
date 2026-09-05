//! Splitting a token stream into lines, and printing them back.
//!
//! The formatter never moves code between lines, so the line structure comes
//! straight from the source: a line ends where the source had a line break.
//! What the source also holds, and the token stream does not, is comments —
//! the lexer drops `//` and `/* */` as trivia. They are recovered here by
//! reading the text between one token's span and the next one's, which is by
//! definition everything the lexer skipped.

use crate::spacing;
use crate::INDENT;
use noto_lexer::{Token, TokenKind};

/// A piece of output: either a token, printed from its source text, or a
/// comment recovered from the gap between two tokens.
enum Piece<'a> {
    /// An index into the token stream, and the source text it covers.
    Token(usize, &'a str),
    /// A comment, exactly as written.
    Comment(&'a str),
}

/// One output line.
struct Line<'a> {
    pieces: Vec<Piece<'a>>,
    /// Whether a blank line separated this one from the line before it.
    blank_before: bool,
}

/// Formats a whole file.
pub(crate) fn render(source: &str, tokens: &[Token]) -> String {
    let lines = split(source, tokens);
    print(&lines, tokens)
}

/// Walks the token stream and the gaps between tokens, producing lines.
fn split<'a>(source: &'a str, tokens: &[Token]) -> Vec<Line<'a>> {
    let mut lines: Vec<Line<'a>> = Vec::new();
    let mut current: Vec<Piece<'a>> = Vec::new();
    let mut blank_before = false;
    let mut cursor = 0usize;

    let newline = |current: &mut Vec<Piece<'a>>,
                       lines: &mut Vec<Line<'a>>,
                       blank_before: &mut bool| {
        if current.is_empty() {
            // A second line break in a row, with nothing between them.
            *blank_before = true;
        } else {
            lines.push(Line {
                pieces: std::mem::take(current),
                blank_before: std::mem::take(blank_before),
            });
        }
    };

    for (index, token) in tokens.iter().enumerate() {
        let start = token.span.start as usize;
        for event in scan_gap(&source[cursor..start]) {
            match event {
                Gap::Newline => newline(&mut current, &mut lines, &mut blank_before),
                Gap::Comment(range) => {
                    let text = &source[cursor + range.0..cursor + range.1];
                    current.push(Piece::Comment(text));
                }
            }
        }
        cursor = token.span.end as usize;

        // The end-of-file token has no text; the gap before it may still hold
        // the file's last comment, which the loop above has already taken.
        if token.kind != TokenKind::Eof {
            current.push(Piece::Token(index, &source[start..cursor]));
        }
    }

    newline(&mut current, &mut lines, &mut blank_before);
    lines
}

/// What the formatter cares about in the space between two tokens.
enum Gap {
    Newline,
    /// Byte range of the comment within the gap.
    Comment((usize, usize)),
}

/// Finds the comments and line breaks in one gap, in order.
fn scan_gap(gap: &str) -> Vec<Gap> {
    let bytes = gap.as_bytes();
    let mut events = Vec::new();
    let mut index = 0;

    while index < bytes.len() {
        match bytes[index] {
            b'\n' => {
                events.push(Gap::Newline);
                index += 1;
            }
            b'/' if bytes.get(index + 1) == Some(&b'/') => {
                let start = index;
                while index < bytes.len() && bytes[index] != b'\n' {
                    index += 1;
                }
                events.push(Gap::Comment((start, trim_end(gap, start, index))));
            }
            b'/' if bytes.get(index + 1) == Some(&b'*') => {
                let start = index;
                index += 2;
                // Block comments nest, exactly as the lexer treats them.
                let mut depth = 1;
                while index < bytes.len() && depth > 0 {
                    match (bytes[index], bytes.get(index + 1)) {
                        (b'/', Some(b'*')) => {
                            depth += 1;
                            index += 2;
                        }
                        (b'*', Some(b'/')) => {
                            depth -= 1;
                            index += 2;
                        }
                        _ => index += 1,
                    }
                }
                events.push(Gap::Comment((start, index)));
            }
            _ => index += 1,
        }
    }

    events
}

/// Trims trailing spaces from a line comment so none reach the output.
fn trim_end(gap: &str, start: usize, end: usize) -> usize {
    let mut end = end;
    while end > start && gap.as_bytes()[end - 1].is_ascii_whitespace() {
        end -= 1;
    }
    end
}

/// Prints the lines with the indentation and spacing rules applied.
fn print(lines: &[Line], tokens: &[Token]) -> String {
    let unary = unary_flags(tokens);
    let in_types = spacing::type_parameter_spans(tokens);
    let mut out = String::new();
    let mut depth: i32 = 0;

    for (index, line) in lines.iter().enumerate() {
        let opens_block = index > 0 && ends_with_open_brace(&lines[index - 1], tokens);
        let closes_block = starts_with_closer(line, tokens);

        // A blank line is the author's paragraph break, but not against a
        // brace: an empty line just inside a block reads as an accident.
        if line.blank_before && index > 0 && !opens_block && !closes_block {
            out.push('\n');
        }

        let level = depth - i32::from(closes_block)
            + i32::from(is_continuation(lines, index, tokens));
        for _ in 0..level.max(0) {
            out.push_str(INDENT);
        }
        out.push_str(&print_line(line, tokens, &unary, &in_types));
        out.push('\n');

        depth += line
            .pieces
            .iter()
            .filter_map(|piece| match piece {
                Piece::Token(index, _) => Some(spacing::depth_change(&tokens[*index].kind)),
                Piece::Comment(_) => None,
            })
            .sum::<i32>();
    }

    out
}

/// Joins one line's pieces, deciding each gap between them.
fn print_line(line: &Line, tokens: &[Token], unary: &[bool], in_types: &[bool]) -> String {
    let mut out = String::new();
    let mut previous: Option<&TokenKind> = None;

    for (position, piece) in line.pieces.iter().enumerate() {
        match piece {
            Piece::Token(index, text) => {
                let kind = &tokens[*index].kind;
                let space = position > 0
                    && match previous {
                        // Code after a comment on the same line is rare but
                        // legal after a `/* */`; one space, like everything.
                        None => true,
                        Some(previous) => {
                            spacing::allows_space_after(
                                previous,
                                unary[*index - 1],
                                in_types[*index - 1],
                            ) && spacing::allows_space_before(
                                kind,
                                Some(previous),
                                in_types[*index],
                                in_types[*index - 1],
                            )
                        }
                    };
                if space {
                    out.push(' ');
                }
                out.push_str(text);
                previous = Some(kind);
            }
            Piece::Comment(text) => {
                if position > 0 {
                    out.push(' ');
                }
                out.push_str(text);
                previous = None;
            }
        }
    }

    out
}

/// Decides, for every token, whether a `-` or `+` there is a sign.
///
/// This is a property of the token stream rather than of a line: an operator
/// can be the last token on one line and its operand the first on the next.
fn unary_flags(tokens: &[Token]) -> Vec<bool> {
    let mut flags = Vec::with_capacity(tokens.len());
    let mut previous: Option<&TokenKind> = None;
    for token in tokens {
        flags.push(spacing::is_unary_position(previous));
        previous = Some(&token.kind);
    }
    flags
}

/// Whether a line continues the one before it.
fn is_continuation(lines: &[Line], index: usize, tokens: &[Token]) -> bool {
    if starts_with_closer(&lines[index], tokens) {
        return false;
    }
    if first_token(&lines[index], tokens)
        .is_some_and(spacing::continues_previous_line)
    {
        return true;
    }
    index > 0
        && last_token(&lines[index - 1], tokens).is_some_and(spacing::continues_line)
}

fn starts_with_closer(line: &Line, tokens: &[Token]) -> bool {
    first_token(line, tokens).is_some_and(spacing::is_closer)
}

fn ends_with_open_brace(line: &Line, tokens: &[Token]) -> bool {
    last_token(line, tokens) == Some(&TokenKind::LBrace)
}

fn first_token<'a>(line: &Line, tokens: &'a [Token]) -> Option<&'a TokenKind> {
    line.pieces.iter().find_map(|piece| match piece {
        Piece::Token(index, _) => Some(&tokens[*index].kind),
        Piece::Comment(_) => None,
    })
}

fn last_token<'a>(line: &Line, tokens: &'a [Token]) -> Option<&'a TokenKind> {
    line.pieces.iter().rev().find_map(|piece| match piece {
        Piece::Token(index, _) => Some(&tokens[*index].kind),
        Piece::Comment(_) => None,
    })
}
