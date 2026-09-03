//! Pattern parsing, used by `when` arms and destructuring bindings.

use crate::{expr::Precedence, Parser, Restrictions};
use noto_ast::{ExprKind, Pattern, PatternKind};
use noto_lexer::{Keyword, TokenKind};

impl Parser<'_> {
    /// Parses one pattern.
    ///
    /// `in_when` tells the parser whether a bare name should bind a new
    /// variable or name a constant to compare against: in a `when` arm a bare
    /// name is a binding, and a dotted path is an enum case.
    pub(crate) fn parse_pattern(&mut self, in_when: bool) -> Pattern {
        let start = self.peek_span();

        if self.eat(&TokenKind::Underscore) {
            return self.pattern(PatternKind::Wildcard, start);
        }

        if self.check_keyword(Keyword::Null) {
            self.advance();
            return self.pattern(PatternKind::Null, start);
        }

        // `is Type` narrows by runtime type.
        if self.eat_keyword(Keyword::Is) {
            let ty = self.parse_type();
            let span = start.to(ty.span);
            return self.pattern(PatternKind::Type(ty), span);
        }

        // `in 0..9` reads naturally in a `when` arm and means the same as the
        // bare range pattern.
        if self.eat_keyword(Keyword::In) {
            return self.parse_pattern_after_in(start);
        }

        if self.check(&TokenKind::LParen) {
            return self.parse_tuple_pattern();
        }

        // `..9` — a range with no lower bound.
        if matches!(self.peek_kind(), TokenKind::DotDot | TokenKind::DotDotEq) {
            let inclusive = self.check(&TokenKind::DotDotEq);
            self.advance();
            let end = Box::new(self.parse_expr_with(Precedence::Range.next(), Restrictions::NO_TRAILING_LAMBDA));
            let span = start.to(end.span);
            return self.pattern(PatternKind::Range { start: None, end: Some(end), inclusive }, span);
        }

        if matches!(self.peek_kind(), TokenKind::Ident(_)) {
            return self.parse_named_pattern(in_when);
        }

        // Anything else is a constant to compare against: a literal, or a
        // negative number.
        let value = self.parse_expr_with(Precedence::Range, Restrictions::NO_TRAILING_LAMBDA);
        let span = start.to(value.span);
        match value.kind {
            ExprKind::Range { start: low, end, inclusive } => {
                self.pattern(PatternKind::Range { start: low, end, inclusive }, span)
            }
            _ => self.pattern(PatternKind::Value(Box::new(value)), span),
        }
    }

    /// Parses the pattern after an explicit `in`, which must be a range or a
    /// collection to test membership against.
    fn parse_pattern_after_in(&mut self, start: noto_span::Span) -> Pattern {
        let value = self.parse_expr_with(Precedence::Range, Restrictions::NO_TRAILING_LAMBDA);
        let span = start.to(value.span);
        match value.kind {
            ExprKind::Range { start: low, end, inclusive } => {
                self.pattern(PatternKind::Range { start: low, end, inclusive }, span)
            }
            _ => self.pattern(PatternKind::Value(Box::new(value)), span),
        }
    }

    /// Parses a pattern that begins with a name.
    fn parse_named_pattern(&mut self, in_when: bool) -> Pattern {
        let start = self.peek_span();
        let path = self.parse_path();

        // `Result.Success(value)` or `Color.Red` — an enum case, with optional
        // bindings for the data it carries.
        if !path.is_single() || starts_upper(&path.last().name) {
            let fields = if self.check(&TokenKind::LParen) && !self.at_line_start() {
                self.advance();
                let mut fields = Vec::new();
                while !self.check(&TokenKind::RParen) && !self.at_eof() {
                    fields.push(self.parse_pattern(in_when));
                    if !self.eat(&TokenKind::Comma) {
                        break;
                    }
                }
                self.expect(&TokenKind::RParen);
                Some(fields)
            } else {
                None
            };
            let span = start.to(self.previous_span());
            return self.pattern(PatternKind::EnumCase { path, fields }, span);
        }

        let name = path.segments.into_iter().next().expect("a path has a first segment");

        // A range whose lower bound is a name: `min..max`.
        if matches!(self.peek_kind(), TokenKind::DotDot | TokenKind::DotDotEq) {
            let inclusive = self.check(&TokenKind::DotDotEq);
            let low_id = self.next_id();
            let low = noto_ast::Expr {
                kind: ExprKind::Path(noto_ast::Path::single(name)),
                span: start,
                id: low_id,
            };
            self.advance();
            let end = if self.starts_expr() {
                Some(Box::new(
                    self.parse_expr_with(Precedence::Range.next(), Restrictions::NO_TRAILING_LAMBDA),
                ))
            } else {
                None
            };
            let span = start.to(self.previous_span());
            return self
                .pattern(PatternKind::Range { start: Some(Box::new(low)), end, inclusive }, span);
        }

        // `n @ 0..9` binds the whole value while still matching a subpattern.
        let subpattern = if self.eat(&TokenKind::At) {
            Some(Box::new(self.parse_pattern(in_when)))
        } else {
            None
        };

        let span = start.to(self.previous_span());
        self.pattern(PatternKind::Binding { name, subpattern }, span)
    }

    fn parse_tuple_pattern(&mut self) -> Pattern {
        let start = self.peek_span();
        self.expect(&TokenKind::LParen);
        let mut items = Vec::new();
        while !self.check(&TokenKind::RParen) && !self.at_eof() {
            items.push(self.parse_pattern(false));
            if !self.eat(&TokenKind::Comma) {
                break;
            }
        }
        self.expect(&TokenKind::RParen);
        let span = start.to(self.previous_span());
        self.pattern(PatternKind::Tuple(items), span)
    }

    fn pattern(&mut self, kind: PatternKind, span: noto_span::Span) -> Pattern {
        let id = self.next_id();
        Pattern { kind, span, id }
    }
}

/// Whether a name looks like a type or enum case rather than a binding.
///
/// Noto asks for `UpperCamelCase` type names and `lowerCamelCase` value names,
/// which lets a `when` arm tell `Red` (an enum case to match) from `red` (a new
/// binding) without knowing the scrutinee's type.
fn starts_upper(name: &str) -> bool {
    name.chars().next().is_some_and(char::is_uppercase)
}
