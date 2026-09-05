//! Which pairs of tokens are separated by a space.
//!
//! The rule is one space between tokens unless one of the two says otherwise,
//! and the two questions are asked separately: does the left token allow a
//! space after it, and does the right token allow one before it. Keeping them
//! apart is what makes `String? = null` come out right — `?` refuses a space
//! *before* it and has nothing to say about what follows.

use noto_lexer::{Keyword, Token, TokenKind};

/// Marks the tokens that belong to a `fn name<T, U>` parameter list.
///
/// `<` is a comparison everywhere else, and a formatter that works on tokens
/// cannot tell the two apart from the pair alone. This is the one place the
/// grammar leaves no choice: after `fn` and a name, a `<` opens type
/// parameters. Everything from it to the matching `>` is marked, and the
/// spacing rules below tighten what is marked.
pub(crate) fn type_parameter_spans(tokens: &[Token]) -> Vec<bool> {
    let mut marked = vec![false; tokens.len()];
    let mut index = 0;

    while index + 2 < tokens.len() {
        let opens = matches!(tokens[index].kind, TokenKind::Keyword(Keyword::Fn))
            && matches!(tokens[index + 1].kind, TokenKind::Ident(_))
            && tokens[index + 2].kind == TokenKind::Lt;
        if !opens {
            index += 1;
            continue;
        }

        let mut depth = 0usize;
        let mut at = index + 2;
        while at < tokens.len() {
            match tokens[at].kind {
                TokenKind::Lt => depth += 1,
                TokenKind::Gt => depth -= 1,
                // A type parameter list holds names, commas and nothing that
                // could close it another way; anything else means this was a
                // comparison after all.
                TokenKind::Ident(_) | TokenKind::Comma | TokenKind::Colon => {}
                _ => break,
            }
            marked[at] = true;
            if depth == 0 {
                break;
            }
            at += 1;
        }
        index = at.max(index + 1);
    }

    marked
}

/// Whether a space may follow this token.
///
/// `is_unary` distinguishes `-x` from `a - b`; it is decided by
/// [`is_unary_position`] from the token before it.
pub(crate) fn allows_space_after(kind: &TokenKind, is_unary: bool, in_types: bool) -> bool {
    use TokenKind::*;
    if in_types && matches!(kind, Lt) {
        return false;
    }
    match kind {
        LParen | LBracket | Dot | QuestionDot | ColonColon | DotDot | DotDotEq | At | Bang
        | Tilde => false,
        Minus | Plus => !is_unary,
        _ => true,
    }
}

/// Whether a space may precede this token.
///
/// `previous` decides the two tokens that mean different things depending on
/// what they follow: `(` and `[` open a call or an index after a name, and a
/// grouped expression or a list literal after anything else.
pub(crate) fn allows_space_before(
    kind: &TokenKind,
    previous: Option<&TokenKind>,
    in_types: bool,
    previous_in_types: bool,
) -> bool {
    use TokenKind::*;
    if in_types && matches!(kind, Lt | Gt) {
        return false;
    }
    match kind {
        RParen | RBracket | Comma | Semicolon | Colon | Dot | QuestionDot | ColonColon
        | DotDot | DotDotEq | Question => false,
        // An empty block is written `{}`, not `{ }`.
        RBrace => previous != Some(&LBrace),
        // `fn(Int): Int` is a function type, and the parenthesis belongs to
        // the `fn`; `f<T>(x)` closes its type parameters onto its own
        // parameter list.
        LParen | LBracket => {
            !previous.is_some_and(is_callee)
                && previous != Some(&Keyword(self::Keyword::Fn))
                && !(previous_in_types && previous == Some(&Gt))
        }
        _ => true,
    }
}

/// Whether a token can be called or indexed, making a following `(` or `[`
/// part of the same expression rather than a new one.
fn is_callee(kind: &TokenKind) -> bool {
    use TokenKind::*;
    matches!(kind, Ident(_) | RParen | RBracket | Str(_) | Question)
}

/// Whether a `-` or `+` after this token is a sign rather than an operation.
///
/// Anything that can end an expression — a name, a literal, a closing
/// bracket — makes the operator binary. Everything else, including the start
/// of a line, makes it unary.
pub(crate) fn is_unary_position(previous: Option<&TokenKind>) -> bool {
    use TokenKind::*;
    let Some(previous) = previous else { return true };
    match previous {
        Ident(_) | Int(_) | Float(_) | Str(_) | Char(_) | RParen | RBracket | RBrace
        | Underscore | Question => false,
        Keyword(keyword) => !is_value_keyword(*keyword),
        _ => true,
    }
}

/// Whether a keyword is a value rather than something that expects one.
fn is_value_keyword(keyword: Keyword) -> bool {
    use Keyword::*;
    matches!(keyword, True | False | Null | This | Super | Break | Continue)
}

/// Whether a line ending in this token continues on the next one.
///
/// A trailing operator is Noto's line continuation. A trailing comma is not:
/// it appears inside brackets, which already indent what follows, and adding
/// a continuation level on top would indent it twice.
pub(crate) fn continues_line(kind: &TokenKind) -> bool {
    use TokenKind::*;
    matches!(
        kind,
        Plus | Minus
            | Star
            | Slash
            | Percent
            | Eq
            | EqEq
            | BangEq
            | Lt
            | LtEq
            | Gt
            | GtEq
            | AmpAmp
            | PipePipe
            | Amp
            | Pipe
            | Caret
            | Shl
            | Shr
            | PlusEq
            | MinusEq
            | StarEq
            | SlashEq
            | PercentEq
            | AmpEq
            | PipeEq
            | CaretEq
            | ShlEq
            | ShrEq
            | Elvis
            | Arrow
            | FatArrow
            | Colon
    )
}

/// Whether a line beginning with this token continues the line before it.
pub(crate) fn continues_previous_line(kind: &TokenKind) -> bool {
    matches!(kind, TokenKind::Dot | TokenKind::QuestionDot | TokenKind::Elvis)
}

/// How much this token changes the bracket depth.
pub(crate) fn depth_change(kind: &TokenKind) -> i32 {
    use TokenKind::*;
    match kind {
        LParen | LBracket | LBrace => 1,
        RParen | RBracket | RBrace => -1,
        _ => 0,
    }
}

/// Whether this token closes a bracket, so a line starting with it belongs at
/// the level of the line that opened it.
pub(crate) fn is_closer(kind: &TokenKind) -> bool {
    matches!(kind, TokenKind::RParen | TokenKind::RBracket | TokenKind::RBrace)
}
