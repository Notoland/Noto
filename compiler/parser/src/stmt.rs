//! Statement and block parsing.

use crate::Parser;
use noto_ast::{Block, Expr, ExprKind, LetKind, Pattern, PatternKind, Stmt, StmtKind};
use noto_diagnostics::{codes, Diagnostic};
use noto_lexer::{Keyword, TokenKind};
use noto_span::Span;

impl Parser<'_> {
    /// Parses a `{ .. }` block.
    pub(crate) fn parse_block(&mut self) -> Block {
        let start = self.peek_span();
        if !self.expect(&TokenKind::LBrace) {
            let id = self.next_id();
            return Block::empty(start, id);
        }
        let statements = self.parse_statements_until_brace();
        self.expect(&TokenKind::RBrace);
        let id = self.next_id();
        Block::new(statements, start.to(self.previous_span()), id)
    }

    /// Parses statements up to, but not including, the closing brace.
    pub(crate) fn parse_statements_until_brace(&mut self) -> Vec<Stmt> {
        let mut statements = Vec::new();
        while !self.check(&TokenKind::RBrace) && !self.at_eof() {
            let before = self.position;
            statements.push(self.parse_stmt());
            // Recovery must always make progress or the loop would not end.
            if self.position == before {
                self.advance();
            }
        }
        statements
    }

    /// Parses one statement.
    pub(crate) fn parse_stmt(&mut self) -> Stmt {
        let start = self.peek_span();

        // Empty statements are allowed so that a stray `;` is not an error.
        while self.eat(&TokenKind::Semicolon) {}

        let kind = match self.peek_kind() {
            TokenKind::Keyword(Keyword::Val) => self.parse_let(LetKind::Val),
            TokenKind::Keyword(Keyword::Var) => self.parse_let(LetKind::Var),
            TokenKind::Keyword(Keyword::While) => self.parse_while(),
            TokenKind::Keyword(Keyword::Loop) => self.parse_loop(),
            TokenKind::Keyword(Keyword::For) => self.parse_for(),
            TokenKind::Keyword(Keyword::Defer) => self.parse_defer(),
            // `fn(` with no name is a function value, not a declaration.
            TokenKind::Keyword(Keyword::Fn)
                if self.peek_nth(1).kind == TokenKind::LParen =>
            {
                let expr = self.parse_expr();
                if !expr.ends_with_block() {
                    self.expect_statement_end();
                }
                StmtKind::Expr(expr)
            }
            TokenKind::Keyword(keyword) if crate::starts_item(*keyword) => {
                // A declaration inside a block: a local function or type.
                let item = self.parse_item();
                StmtKind::Item(Box::new(item))
            }
            _ => {
                let expr = self.parse_expr();
                // A block-shaped expression is a statement on its own; only a
                // plain expression needs a terminator after it.
                if !expr.ends_with_block() {
                    self.expect_statement_end();
                }
                StmtKind::Expr(expr)
            }
        };

        let span = start.to(self.previous_span());
        let id = self.next_id();
        Stmt { kind, span, id }
    }

    /// Parses `val`/`var`, including destructuring forms.
    fn parse_let(&mut self, kind: LetKind) -> StmtKind {
        self.advance();
        let pattern = self.parse_binding_pattern();
        let ty = if self.eat(&TokenKind::Colon) { Some(self.parse_type()) } else { None };

        let value = if self.eat(&TokenKind::Eq) { Some(self.parse_expr()) } else { None };

        if value.is_none() && ty.is_none() {
            let span = pattern.span;
            self.error(
                Diagnostic::error(
                    codes::CANNOT_INFER,
                    "a binding needs either a type or an initial value",
                )
                .with_primary(span, "the type of this binding is unknown")
                .with_help("write `val name: Type` or give it a value with `= ...`"),
            );
        }

        self.expect_statement_end();
        StmtKind::Let { kind, pattern, ty, value }
    }

    /// Parses the left side of a `val`/`var`: a name or a destructuring
    /// pattern.
    fn parse_binding_pattern(&mut self) -> Pattern {
        if self.check(&TokenKind::LParen) {
            return self.parse_pattern(false);
        }
        let start = self.peek_span();
        if self.eat(&TokenKind::Underscore) {
            let id = self.next_id();
            return Pattern { kind: PatternKind::Wildcard, span: start, id };
        }
        let name = self.expect_ident();
        let span = name.span;
        let id = self.next_id();
        Pattern { kind: PatternKind::Binding { name, subpattern: None }, span, id }
    }

    fn parse_while(&mut self) -> StmtKind {
        self.advance();
        let condition = self.parse_condition();
        let body = self.parse_block();
        StmtKind::While { condition, body }
    }

    fn parse_loop(&mut self) -> StmtKind {
        self.advance();
        let body = self.parse_block();
        StmtKind::Loop { body }
    }

    /// Parses `for pattern in iterable { .. }`.
    fn parse_for(&mut self) -> StmtKind {
        self.advance();
        let pattern = self.parse_binding_pattern();
        self.expect_keyword(Keyword::In);
        let iterable = self.parse_condition();
        let body = self.parse_block();
        StmtKind::For { pattern, iterable, body }
    }

    /// Parses `defer expr`.
    ///
    /// The expression is evaluated on every exit from the enclosing scope,
    /// including error paths; see `docs/rfcs/0003-defer-semantics.md`.
    fn parse_defer(&mut self) -> StmtKind {
        let start = self.peek_span();
        self.advance();
        let value = self.parse_expr();

        // `defer` is for effects. Deferring something with no effect is always
        // a mistake, and catching it here is cheaper than a runtime surprise.
        if matches!(value.kind, ExprKind::Path(_) | ExprKind::Literal(_)) {
            let span = start.to(value.span);
            self.error(
                Diagnostic::error(
                    codes::UNEXPECTED_CONSTRUCT,
                    "`defer` needs an expression that does something",
                )
                .with_primary(value.span, "this has no effect when the scope ends")
                .with_help("defer a call, such as `defer file.close()`"),
            );
            let _ = span;
        }

        self.expect_statement_end();
        StmtKind::Defer { value }
    }

    /// Parses a block whose value is the block's own trailing expression,
    /// used for single-expression function bodies written with `=`.
    pub(crate) fn parse_expr_body(&mut self) -> Block {
        let start = self.peek_span();
        let expr = self.parse_expr();
        let span = start.to(expr.span);
        let stmt_id = self.next_id();
        let block_id = self.next_id();
        let stmt = Stmt { kind: StmtKind::Expr(expr), span, id: stmt_id };
        Block::new(vec![stmt], span, block_id)
    }

    /// Wraps an expression as a block, used when lowering arm bodies.
    #[allow(dead_code)]
    pub(crate) fn expr_as_block(&mut self, expr: Expr) -> Block {
        let span: Span = expr.span;
        let stmt_id = self.next_id();
        let block_id = self.next_id();
        Block::new(vec![Stmt { kind: StmtKind::Expr(expr), span, id: stmt_id }], span, block_id)
    }
}
