//! Expression parsing.
//!
//! Binary operators are handled by precedence climbing over the table in
//! [`Precedence`]. The full table is documented in
//! `docs/design/operator-precedence.md` and the two must be kept in step; the
//! tests at the bottom of this crate check the shape the table produces.

use crate::{Parser, Restrictions};
use noto_ast::{
    Argument, BinaryOp, Block, CallExpr, Expr, ExprKind, Ident, LambdaExpr, Literal, Param, Path,
    StringSegment, UnaryOp, WhenArm, WhenGuard,
};
use noto_diagnostics::{codes, Diagnostic};
use noto_lexer::{Keyword, StringPart, TokenKind};
use noto_span::Span;

/// Binding power of an operator, from loosest to tightest.
///
/// Bitwise operators deliberately bind *tighter* than comparison: `flags & MASK
/// == 0` reads as `(flags & MASK) == 0`, which is what the code means. C's
/// choice here is a long-standing source of bugs and Noto does not repeat it.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub enum Precedence {
    /// Anything at all.
    Lowest,
    /// `=` `+=` `-=` …, right associative.
    Assignment,
    /// `?:`
    Elvis,
    /// `||`
    Or,
    /// `&&`
    And,
    /// `==` `!=`
    Equality,
    /// `<` `<=` `>` `>=`
    Comparison,
    /// `is` `!is` `in` `!in`
    TypeTest,
    /// `|`
    BitOr,
    /// `^`
    BitXor,
    /// `&`
    BitAnd,
    /// `..` `..=`
    Range,
    /// `<<` `>>`
    Shift,
    /// `+` `-`
    Additive,
    /// `*` `/` `%`
    Multiplicative,
    /// `as` `as?`
    Cast,
    /// `-x` `!x` `~x` `await x`
    Prefix,
    /// `f()` `a.b` `a[i]` `x?`
    Postfix,
}

impl Precedence {
    /// The next level up, used to make an operator left associative.
    pub(crate) fn next(self) -> Precedence {
        use Precedence::*;
        match self {
            Lowest => Assignment,
            Assignment => Elvis,
            Elvis => Or,
            Or => And,
            And => Equality,
            Equality => Comparison,
            Comparison => TypeTest,
            TypeTest => BitOr,
            BitOr => BitXor,
            BitXor => BitAnd,
            BitAnd => Range,
            Range => Shift,
            Shift => Additive,
            Additive => Multiplicative,
            Multiplicative => Cast,
            Cast => Prefix,
            Prefix | Postfix => Postfix,
        }
    }
}

/// The binary operator a token introduces, with its precedence.
fn infix_op(kind: &TokenKind) -> Option<(BinaryOp, Precedence)> {
    use Precedence as P;
    let pair = match kind {
        TokenKind::PipePipe => (BinaryOp::Or, P::Or),
        TokenKind::AmpAmp => (BinaryOp::And, P::And),
        TokenKind::EqEq => (BinaryOp::Eq, P::Equality),
        TokenKind::BangEq => (BinaryOp::Ne, P::Equality),
        TokenKind::Lt => (BinaryOp::Lt, P::Comparison),
        TokenKind::LtEq => (BinaryOp::Le, P::Comparison),
        TokenKind::Gt => (BinaryOp::Gt, P::Comparison),
        TokenKind::GtEq => (BinaryOp::Ge, P::Comparison),
        TokenKind::Pipe => (BinaryOp::BitOr, P::BitOr),
        TokenKind::Caret => (BinaryOp::BitXor, P::BitXor),
        TokenKind::Amp => (BinaryOp::BitAnd, P::BitAnd),
        TokenKind::Shl => (BinaryOp::Shl, P::Shift),
        TokenKind::Shr => (BinaryOp::Shr, P::Shift),
        TokenKind::Plus => (BinaryOp::Add, P::Additive),
        TokenKind::Minus => (BinaryOp::Sub, P::Additive),
        TokenKind::Star => (BinaryOp::Mul, P::Multiplicative),
        TokenKind::Slash => (BinaryOp::Div, P::Multiplicative),
        TokenKind::Percent => (BinaryOp::Rem, P::Multiplicative),
        TokenKind::Elvis => (BinaryOp::Elvis, P::Elvis),
        TokenKind::Keyword(Keyword::In) => (BinaryOp::In, P::TypeTest),
        _ => return None,
    };
    Some(pair)
}

/// The compound assignment a token introduces.
fn assign_op(kind: &TokenKind) -> Option<Option<BinaryOp>> {
    Some(match kind {
        TokenKind::Eq => None,
        TokenKind::PlusEq => Some(BinaryOp::Add),
        TokenKind::MinusEq => Some(BinaryOp::Sub),
        TokenKind::StarEq => Some(BinaryOp::Mul),
        TokenKind::SlashEq => Some(BinaryOp::Div),
        TokenKind::PercentEq => Some(BinaryOp::Rem),
        TokenKind::AmpEq => Some(BinaryOp::BitAnd),
        TokenKind::PipeEq => Some(BinaryOp::BitOr),
        TokenKind::CaretEq => Some(BinaryOp::BitXor),
        TokenKind::ShlEq => Some(BinaryOp::Shl),
        TokenKind::ShrEq => Some(BinaryOp::Shr),
        _ => return None,
    })
}

impl Parser<'_> {
    /// Parses an expression with no restrictions.
    pub(crate) fn parse_expr(&mut self) -> Expr {
        self.parse_expr_with(Precedence::Lowest, Restrictions::NONE)
    }

    /// Parses an expression that may not take a trailing lambda, used for the
    /// condition of `if`/`while` and the subject of `for`.
    pub(crate) fn parse_condition(&mut self) -> Expr {
        self.parse_expr_with(Precedence::Lowest, Restrictions::NO_TRAILING_LAMBDA)
    }

    /// Precedence climbing over binary and postfix operators.
    pub(crate) fn parse_expr_with(&mut self, min: Precedence, r: Restrictions) -> Expr {
        let mut left = self.parse_prefix(r);

        loop {
            // A binary operator that starts a line does not continue the
            // previous expression; the statement ended at the line break.
            if self.at_line_start() && !self.continues_across_newline() {
                break;
            }

            if let Some(op) = assign_op(self.peek_kind()) {
                if min > Precedence::Assignment {
                    break;
                }
                let op_span = self.peek_span();
                self.advance();
                // Assignment is right associative, so the right side is parsed
                // at the same level rather than the next one up.
                let value = self.parse_expr_with(Precedence::Assignment, r);
                let span = left.span.to(value.span);
                if !left.is_assignable() {
                    self.error(
                        Diagnostic::error(
                            codes::NOT_ASSIGNABLE,
                            "left side of an assignment must be a name, a field or an index",
                        )
                        .with_primary(left.span, "cannot be assigned to"),
                    );
                }
                left = self.expr(
                    ExprKind::Assign { target: Box::new(left), value: Box::new(value), op, op_span },
                    span,
                );
                continue;
            }

            if self.check_keyword(Keyword::As) {
                if min > Precedence::Cast {
                    break;
                }
                self.advance();
                let safe = self.eat(&TokenKind::Question);
                let ty = self.parse_type();
                let span = left.span.to(ty.span);
                left = self.expr(ExprKind::As { value: Box::new(left), ty, safe }, span);
                continue;
            }

            if self.at_is_test() {
                if min > Precedence::TypeTest {
                    break;
                }
                let negated = self.eat(&TokenKind::Bang);
                self.expect_keyword(Keyword::Is);
                let ty = self.parse_type();
                let span = left.span.to(ty.span);
                left = self.expr(ExprKind::Is { value: Box::new(left), ty, negated }, span);
                continue;
            }

            if self.at_negated_in() {
                if min > Precedence::TypeTest {
                    break;
                }
                let op_span = self.peek_span();
                self.advance();
                self.advance();
                let right = self.parse_expr_with(Precedence::TypeTest.next(), r);
                let span = left.span.to(right.span);
                let contains = self.expr(
                    ExprKind::Binary {
                        op: BinaryOp::In,
                        left: Box::new(left),
                        right: Box::new(right),
                        op_span,
                    },
                    span,
                );
                left = self.expr(ExprKind::Unary { op: UnaryOp::Not, operand: Box::new(contains) }, span);
                continue;
            }

            if matches!(self.peek_kind(), TokenKind::DotDot | TokenKind::DotDotEq) {
                if min > Precedence::Range {
                    break;
                }
                let inclusive = self.check(&TokenKind::DotDotEq);
                self.advance();
                let end = if self.starts_expr() {
                    Some(Box::new(self.parse_expr_with(Precedence::Range.next(), r)))
                } else {
                    None
                };
                let span = left.span.to(self.previous_span());
                left = self.expr(
                    ExprKind::Range { start: Some(Box::new(left)), end, inclusive },
                    span,
                );
                continue;
            }

            let Some((op, precedence)) = infix_op(self.peek_kind()) else { break };
            if precedence < min {
                break;
            }
            let op_span = self.peek_span();
            self.advance();
            let right = self.parse_expr_with(precedence.next(), r);
            let span = left.span.to(right.span);
            left = self.expr(
                ExprKind::Binary { op, left: Box::new(left), right: Box::new(right), op_span },
                span,
            );
        }

        left
    }

    /// Whether the token stream is at `is Type` or `!is Type`.
    fn at_is_test(&self) -> bool {
        self.check_keyword(Keyword::Is)
            || (self.check(&TokenKind::Bang) && self.peek_nth(1).keyword() == Some(Keyword::Is))
    }

    /// Whether the token stream is at `!in`.
    fn at_negated_in(&self) -> bool {
        self.check(&TokenKind::Bang) && self.peek_nth(1).keyword() == Some(Keyword::In)
    }

    /// Whether an expression starting on a new line continues the previous one.
    ///
    /// Only member access does, which is what makes a chain of calls readable
    /// when each link is on its own line.
    fn continues_across_newline(&self) -> bool {
        matches!(self.peek_kind(), TokenKind::Dot | TokenKind::QuestionDot)
    }

    /// Parses a prefix operator or falls through to a postfix expression.
    fn parse_prefix(&mut self, r: Restrictions) -> Expr {
        let start = self.peek_span();
        let op = match self.peek_kind() {
            TokenKind::Minus => UnaryOp::Neg,
            TokenKind::Bang => UnaryOp::Not,
            TokenKind::Tilde => UnaryOp::BitNot,
            TokenKind::Keyword(Keyword::Await) => {
                self.advance();
                let inner = self.parse_prefix(r);
                let span = start.to(inner.span);
                return self.expr(ExprKind::Await(Box::new(inner)), span);
            }
            _ => return self.parse_postfix(r),
        };
        self.advance();
        let operand = self.parse_prefix(r);
        let span = start.to(operand.span);
        self.expr(ExprKind::Unary { op, operand: Box::new(operand) }, span)
    }

    /// Parses calls, member access, indexing and `?` propagation.
    fn parse_postfix(&mut self, r: Restrictions) -> Expr {
        let mut expr = self.parse_primary(r);

        loop {
            // Only `.` and `?.` reach across a line break.
            if self.at_line_start() && !self.continues_across_newline() {
                break;
            }
            match self.peek_kind() {
                TokenKind::Dot | TokenKind::QuestionDot => {
                    let safe = self.check(&TokenKind::QuestionDot);
                    self.advance();
                    let name = self.expect_ident();
                    let span = expr.span.to(name.span);
                    expr = self.expr(
                        ExprKind::Member { receiver: Box::new(expr), name, safe },
                        span,
                    );
                }
                TokenKind::LParen => {
                    let arguments = self.parse_arguments();
                    let mut span = expr.span.to(self.previous_span());
                    let mut call =
                        CallExpr { callee: Box::new(expr), arguments, type_arguments: Vec::new() };
                    if let Some(lambda) = self.parse_trailing_lambda(r) {
                        span = span.to(lambda.span);
                        call.arguments.push(Argument {
                            name: None,
                            span: lambda.span,
                            value: lambda,
                        });
                    }
                    expr = self.expr(ExprKind::Call(call), span);
                }
                TokenKind::LBracket => {
                    self.advance();
                    let index = self.parse_expr();
                    self.expect(&TokenKind::RBracket);
                    let span = expr.span.to(self.previous_span());
                    expr = self.expr(
                        ExprKind::Index { target: Box::new(expr), index: Box::new(index) },
                        span,
                    );
                }
                TokenKind::Question => {
                    self.advance();
                    let span = expr.span.to(self.previous_span());
                    expr = self.expr(ExprKind::Try(Box::new(expr)), span);
                }
                // `users.filter { .. }` — a lambda passed without parentheses.
                TokenKind::LBrace if !r.contains(Restrictions::NO_TRAILING_LAMBDA) => {
                    let Some(lambda) = self.parse_trailing_lambda(r) else { break };
                    let span = expr.span.to(lambda.span);
                    expr = self.expr(
                        ExprKind::Call(CallExpr {
                            callee: Box::new(expr),
                            arguments: vec![Argument { name: None, span: lambda.span, value: lambda }],
                            type_arguments: Vec::new(),
                        }),
                        span,
                    );
                }
                _ => break,
            }
        }

        expr
    }

    /// Parses a `{ .. }` lambda written directly after a call.
    fn parse_trailing_lambda(&mut self, r: Restrictions) -> Option<Expr> {
        if r.contains(Restrictions::NO_TRAILING_LAMBDA)
            || !self.check(&TokenKind::LBrace)
            || self.at_line_start()
        {
            return None;
        }
        Some(self.parse_lambda(true))
    }

    /// Parses the argument list of a call, including named arguments.
    fn parse_arguments(&mut self) -> Vec<Argument> {
        self.expect(&TokenKind::LParen);
        let mut arguments = Vec::new();

        while !self.check(&TokenKind::RParen) && !self.at_eof() {
            let start = self.peek_span();
            // `name: value` is a named argument; a bare `name` is an
            // expression that happens to be a path.
            let name = if matches!(self.peek_kind(), TokenKind::Ident(_))
                && self.peek_nth(1).kind == TokenKind::Colon
            {
                let ident = self.expect_ident();
                self.advance();
                Some(ident)
            } else {
                None
            };
            let value = self.parse_expr();
            let span = start.to(value.span);
            arguments.push(Argument { name, value, span });

            if !self.eat(&TokenKind::Comma) {
                break;
            }
        }

        self.expect(&TokenKind::RParen);
        arguments
    }

    /// Parses the innermost expression forms.
    fn parse_primary(&mut self, r: Restrictions) -> Expr {
        let start = self.peek_span();

        match self.peek_kind().clone() {
            TokenKind::Int(literal) => {
                self.advance();
                let value =
                    Literal::Int { value: literal.value, suffix: literal.suffix };
                self.expr(ExprKind::Literal(value), start)
            }
            TokenKind::Float(literal) => {
                self.advance();
                let value = Literal::Float { value: literal.value, suffix: literal.suffix };
                self.expr(ExprKind::Literal(value), start)
            }
            TokenKind::Char(ch) => {
                self.advance();
                self.expr(ExprKind::Literal(Literal::Char(ch)), start)
            }
            TokenKind::Str(literal) => {
                self.advance();
                let segments = self.lower_string(literal);
                self.expr(ExprKind::Literal(Literal::Str(segments)), start)
            }
            TokenKind::Keyword(Keyword::True) => {
                self.advance();
                self.expr(ExprKind::Literal(Literal::Bool(true)), start)
            }
            TokenKind::Keyword(Keyword::False) => {
                self.advance();
                self.expr(ExprKind::Literal(Literal::Bool(false)), start)
            }
            TokenKind::Keyword(Keyword::Null) => {
                self.advance();
                self.expr(ExprKind::Literal(Literal::Null), start)
            }
            TokenKind::Keyword(Keyword::This) => {
                self.advance();
                self.expr(ExprKind::This, start)
            }
            TokenKind::Keyword(Keyword::Super) => {
                self.advance();
                self.expr(ExprKind::Super, start)
            }
            TokenKind::Ident(_) => {
                // A dotted name is member access, not a longer path: `a.b` is
                // "field `b` of `a`" until resolution proves `a` is a module.
                let name = self.expect_ident();
                let span = name.span;
                self.expr(ExprKind::Path(Path::single(name)), span)
            }
            TokenKind::LParen => self.parse_paren_or_tuple(),
            TokenKind::LBracket => self.parse_list_literal(),
            TokenKind::LBrace => self.parse_lambda(false),
            TokenKind::Keyword(Keyword::Fn) => self.parse_anonymous_fn(),
            TokenKind::Keyword(Keyword::If) => self.parse_if(),
            TokenKind::Keyword(Keyword::When) => self.parse_when(),
            TokenKind::Keyword(Keyword::Unsafe) => {
                self.advance();
                let block = self.parse_block();
                let span = start.to(block.span);
                self.expr(ExprKind::Unsafe(block), span)
            }
            TokenKind::Keyword(Keyword::Return) => {
                self.advance();
                // `return` on its own line returns Unit; a value must follow on
                // the same line.
                let value = if self.starts_expr() && !self.at_line_start() {
                    Some(Box::new(self.parse_expr_with(Precedence::Lowest, r)))
                } else {
                    None
                };
                let span = start.to(self.previous_span());
                self.expr(ExprKind::Return(value), span)
            }
            TokenKind::Keyword(Keyword::Break) => {
                self.advance();
                self.expr(ExprKind::Break, start)
            }
            TokenKind::Keyword(Keyword::Continue) => {
                self.advance();
                self.expr(ExprKind::Continue, start)
            }
            TokenKind::DotDot | TokenKind::DotDotEq => {
                let inclusive = self.check(&TokenKind::DotDotEq);
                self.advance();
                let end = Box::new(self.parse_expr_with(Precedence::Range.next(), r));
                let span = start.to(end.span);
                self.expr(ExprKind::Range { start: None, end: Some(end), inclusive }, span)
            }
            _ => {
                self.expected("an expression");
                self.advance();
                self.expr(ExprKind::Error, start)
            }
        }
    }

    /// `(a)` is a parenthesised expression; `(a, b)` is a tuple.
    fn parse_paren_or_tuple(&mut self) -> Expr {
        let start = self.peek_span();
        self.expect(&TokenKind::LParen);

        if self.eat(&TokenKind::RParen) {
            // `()` is the unit value, modelled as the empty tuple.
            let span = start.to(self.previous_span());
            return self.expr(ExprKind::Tuple(Vec::new()), span);
        }

        let first = self.parse_expr();
        if !self.check(&TokenKind::Comma) {
            self.expect(&TokenKind::RParen);
            // Parentheses only group; the span grows to cover them so that
            // diagnostics underline what the user wrote.
            let span = start.to(self.previous_span());
            return Expr { span, ..first };
        }

        let mut items = vec![first];
        while self.eat(&TokenKind::Comma) {
            if self.check(&TokenKind::RParen) {
                break;
            }
            items.push(self.parse_expr());
        }
        self.expect(&TokenKind::RParen);
        let span = start.to(self.previous_span());
        self.expr(ExprKind::Tuple(items), span)
    }

    fn parse_list_literal(&mut self) -> Expr {
        let start = self.peek_span();
        self.expect(&TokenKind::LBracket);
        let mut items = Vec::new();
        while !self.check(&TokenKind::RBracket) && !self.at_eof() {
            items.push(self.parse_expr());
            if !self.eat(&TokenKind::Comma) {
                break;
            }
        }
        self.expect(&TokenKind::RBracket);
        let span = start.to(self.previous_span());
        self.expr(ExprKind::ListLiteral(items), span)
    }

    /// Parses `{ x -> body }`, `{ body }` or a trailing lambda.
    fn parse_lambda(&mut self, is_trailing: bool) -> Expr {
        let start = self.peek_span();
        self.expect(&TokenKind::LBrace);

        let parameters = self.parse_lambda_params();
        let body = self.parse_block_body(start);
        let span = start.to(self.previous_span());
        let lambda =
            LambdaExpr { parameters, result: None, body, is_async: false, is_trailing };
        self.expr(ExprKind::Lambda(Box::new(lambda)), span)
    }

    /// Reads `x ->` or `x, y ->` at the head of a lambda body, if present.
    ///
    /// A lambda with no parameter list still takes one argument, bound to the
    /// implicit name `it`.
    fn parse_lambda_params(&mut self) -> Vec<Param> {
        let Some(arrow) = self.find_lambda_arrow(0) else { return Vec::new() };
        let mut parameters = Vec::new();

        for _ in 0..arrow {
            if self.check(&TokenKind::Comma) {
                self.advance();
                continue;
            }
            let start = self.peek_span();
            let name = self.expect_ident();
            let ty = if self.eat(&TokenKind::Colon) { Some(self.parse_type()) } else { None };
            let id = self.next_id();
            let span = start.to(self.previous_span());
            parameters.push(Param { name, ty, default: None, span, id });
        }

        self.expect(&TokenKind::Arrow);
        parameters
    }

    /// Parses the body of a `when` arm.
    ///
    /// After `->`, a brace opens a block of statements — `Ignored -> { .. }`
    /// is what an arm that does several things looks like. Everywhere else in
    /// expression position a brace opens a lambda, and it still does here when
    /// the braces hold a parameter list: `x -> { n -> n + 1 }` is an arm
    /// producing a function.
    fn parse_arm_body(&mut self) -> Expr {
        // The lookahead starts past the brace, which is where a parameter
        // list would be.
        if self.check(&TokenKind::LBrace) && self.find_lambda_arrow(1).is_none() {
            let start = self.peek_span();
            let block = self.parse_block();
            let span = start.to(block.span);
            return self.expr(ExprKind::Block(block), span);
        }
        self.parse_expr()
    }

    /// Looks ahead for the `->` that separates a lambda's parameters from its
    /// body, returning how many tokens precede it.
    ///
    /// The search stops at the first token that cannot appear in a parameter
    /// list, so `{ a + b }` is a body and `{ a, b -> a + b }` is not.
    fn find_lambda_arrow(&self, from: usize) -> Option<usize> {
        let mut offset = from;
        loop {
            match &self.peek_nth(offset).kind {
                TokenKind::Arrow => return Some(offset),
                TokenKind::Ident(_) | TokenKind::Comma | TokenKind::Colon => offset += 1,
                // A type annotation may appear: `{ x: Int -> x }`.
                TokenKind::Question | TokenKind::Lt | TokenKind::Gt | TokenKind::Dot => offset += 1,
                _ => return None,
            }
            if offset > 64 {
                return None;
            }
        }
    }

    /// Parses `fn(a: Int, b: Int) { .. }` used as a value.
    fn parse_anonymous_fn(&mut self) -> Expr {
        let start = self.peek_span();
        self.expect_keyword(Keyword::Fn);
        let parameters = self.parse_params();
        let result = if self.eat(&TokenKind::Colon) { Some(self.parse_type()) } else { None };
        let body = self.parse_block();
        let span = start.to(body.span);
        let lambda =
            LambdaExpr { parameters, result, body, is_async: false, is_trailing: false };
        self.expr(ExprKind::Lambda(Box::new(lambda)), span)
    }

    fn parse_if(&mut self) -> Expr {
        let start = self.peek_span();
        self.expect_keyword(Keyword::If);
        let condition = self.parse_condition();
        let then_branch = self.parse_block();

        let else_branch = if self.check_keyword(Keyword::Else) {
            self.advance();
            if self.check_keyword(Keyword::If) {
                Some(Box::new(self.parse_if()))
            } else {
                let block = self.parse_block();
                let span = block.span;
                Some(Box::new(self.expr(ExprKind::Block(block), span)))
            }
        } else {
            None
        };

        let span = start.to(self.previous_span());
        self.expr(
            ExprKind::If { condition: Box::new(condition), then_branch, else_branch },
            span,
        )
    }

    /// Parses `when (value) { .. }` and the subjectless `when { .. }`.
    fn parse_when(&mut self) -> Expr {
        let start = self.peek_span();
        self.expect_keyword(Keyword::When);

        let scrutinee = if self.check(&TokenKind::LParen) {
            self.advance();
            let value = self.parse_expr();
            self.expect(&TokenKind::RParen);
            Some(Box::new(value))
        } else {
            None
        };

        self.expect(&TokenKind::LBrace);
        let mut arms = Vec::new();
        let mut seen_else: Option<Span> = None;

        while !self.check(&TokenKind::RBrace) && !self.at_eof() {
            let arm_start = self.peek_span();

            let (patterns, is_else) = if self.check_keyword(Keyword::Else) {
                self.advance();
                (Vec::new(), true)
            } else if scrutinee.is_none() {
                // `when { a > b -> .. }` tests conditions. An arm with no
                // patterns and a guard means exactly that, so the two forms
                // share one representation.
                let condition = self.parse_condition();
                let guard = Some(WhenGuard { condition });
                self.expect(&TokenKind::Arrow);
                let body = self.parse_arm_body();
                let span = arm_start.to(body.span);
                arms.push(WhenArm { patterns: Vec::new(), guard, body, is_else: false, span });
                self.eat(&TokenKind::Comma);
                continue;
            } else {
                let mut patterns = vec![self.parse_pattern(scrutinee.is_some())];
                while self.eat(&TokenKind::Comma) {
                    if self.check(&TokenKind::Arrow) {
                        break;
                    }
                    patterns.push(self.parse_pattern(scrutinee.is_some()));
                }
                (patterns, false)
            };

            let guard = if self.check_keyword(Keyword::If) {
                self.advance();
                Some(WhenGuard { condition: self.parse_condition() })
            } else {
                None
            };

            self.expect(&TokenKind::Arrow);
            let body = self.parse_arm_body();
            let span = arm_start.to(body.span);

            if let Some(previous) = seen_else {
                self.error(
                    Diagnostic::error(
                        codes::MALFORMED_WHEN,
                        "`else` must be the last arm of a `when`",
                    )
                    .with_primary(span, "this arm can never be reached")
                    .with_secondary(previous, "`else` already matches everything here"),
                );
            }
            if is_else {
                seen_else = Some(span);
            }

            arms.push(WhenArm { patterns, guard, body, is_else, span });
            self.eat(&TokenKind::Comma);
        }

        self.expect(&TokenKind::RBrace);
        let span = start.to(self.previous_span());
        self.expr(ExprKind::When { scrutinee, arms }, span)
    }

    /// Turns a lexed string literal into AST segments, parsing any embedded
    /// expressions.
    fn lower_string(&mut self, literal: noto_lexer::StringLiteral) -> Vec<StringSegment> {
        let mut segments = Vec::new();
        for part in literal.parts {
            match part {
                StringPart::Text(text) => {
                    if !text.is_empty() {
                        segments.push(StringSegment::Text(text));
                    }
                }
                StringPart::Interpolation { tokens, span } => {
                    let expr = self.parse_interpolation(tokens, span);
                    segments.push(StringSegment::Interpolation(Box::new(expr)));
                }
            }
        }
        if segments.is_empty() {
            segments.push(StringSegment::Text(String::new()));
        }
        segments
    }

    /// Parses the expression inside `${ .. }` from its own token stream.
    fn parse_interpolation(&mut self, tokens: Vec<noto_lexer::Token>, span: Span) -> Expr {
        let mut inner = Parser::new(tokens, span, self.sink);
        inner.ids = std::mem::take(&mut self.ids);
        let expr = inner.parse_expr();
        if !inner.at_eof() {
            let found = inner.peek_kind().describe();
            inner.error(
                Diagnostic::error(
                    codes::UNEXPECTED_TOKEN,
                    format!("unexpected {found} after the interpolated expression"),
                )
                .with_primary(inner.peek_span(), "not part of the expression"),
            );
        }
        self.ids = std::mem::take(&mut inner.ids);
        expr
    }

    /// Whether the next token can begin an expression.
    pub(crate) fn starts_expr(&self) -> bool {
        match self.peek_kind() {
            TokenKind::Ident(_)
            | TokenKind::Int(_)
            | TokenKind::Float(_)
            | TokenKind::Str(_)
            | TokenKind::Char(_)
            | TokenKind::LParen
            | TokenKind::LBracket
            | TokenKind::LBrace
            | TokenKind::Minus
            | TokenKind::Bang
            | TokenKind::Tilde
            | TokenKind::DotDot
            | TokenKind::DotDotEq => true,
            TokenKind::Keyword(keyword) => matches!(
                keyword,
                Keyword::True
                    | Keyword::False
                    | Keyword::Null
                    | Keyword::This
                    | Keyword::Super
                    | Keyword::If
                    | Keyword::When
                    | Keyword::Fn
                    | Keyword::Await
                    | Keyword::Unsafe
                    | Keyword::Return
                    | Keyword::Break
                    | Keyword::Continue
            ),
            _ => false,
        }
    }

    /// Builds an expression node with a fresh id.
    fn expr(&mut self, kind: ExprKind, span: Span) -> Expr {
        let id = self.next_id();
        Expr { kind, span, id }
    }

    /// A `{ .. }` body whose opening brace has already been consumed.
    fn parse_block_body(&mut self, start: Span) -> Block {
        let statements = self.parse_statements_until_brace();
        self.expect(&TokenKind::RBrace);
        let id = self.next_id();
        Block::new(statements, start.to(self.previous_span()), id)
    }

    /// A path expression, used when lowering shorthand interpolation.
    #[allow(dead_code)]
    fn path_expr(&mut self, ident: Ident) -> Expr {
        let span = ident.span;
        self.expr(ExprKind::Path(Path::single(ident)), span)
    }
}
