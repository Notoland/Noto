//! Type expression parsing.

use crate::Parser;
use noto_ast::{TypeExpr, TypeExprKind};
use noto_lexer::{Keyword, TokenKind};

impl Parser<'_> {
    /// Parses a type expression.
    pub(crate) fn parse_type(&mut self) -> TypeExpr {
        let start = self.peek_span();
        let mut ty = self.parse_type_atom();

        // `?` binds tighter than anything else in a type, and stacking it is
        // pointless: `T??` and `T?` describe the same set of values.
        while self.check(&TokenKind::Question) && !self.at_line_start() {
            self.advance();
            let span = start.to(self.previous_span());
            let id = self.next_id();
            ty = TypeExpr { kind: TypeExprKind::Nullable(Box::new(ty)), span, id };
        }

        ty
    }

    fn parse_type_atom(&mut self) -> TypeExpr {
        let start = self.peek_span();

        match self.peek_kind() {
            TokenKind::Ident(_) => {
                let path = self.parse_path();
                let arguments = self.parse_type_arguments();
                let span = start.to(self.previous_span());
                let id = self.next_id();
                TypeExpr { kind: TypeExprKind::Named { path, arguments }, span, id }
            }
            TokenKind::LBracket => {
                self.advance();
                let element = self.parse_type();
                self.expect(&TokenKind::RBracket);
                let span = start.to(self.previous_span());
                let id = self.next_id();
                TypeExpr { kind: TypeExprKind::List(Box::new(element)), span, id }
            }
            TokenKind::LParen => {
                self.advance();
                let mut items = Vec::new();
                while !self.check(&TokenKind::RParen) && !self.at_eof() {
                    items.push(self.parse_type());
                    if !self.eat(&TokenKind::Comma) {
                        break;
                    }
                }
                self.expect(&TokenKind::RParen);
                let span = start.to(self.previous_span());
                let id = self.next_id();
                // `(T)` groups; `(A, B)` is a tuple type.
                if items.len() == 1 {
                    let inner = items.pop().expect("checked length");
                    TypeExpr { span, ..inner }
                } else {
                    TypeExpr { kind: TypeExprKind::Tuple(items), span, id }
                }
            }
            TokenKind::Keyword(Keyword::Fn) | TokenKind::Keyword(Keyword::Async) => {
                let is_async = self.eat_keyword(Keyword::Async);
                self.expect_keyword(Keyword::Fn);
                self.expect(&TokenKind::LParen);
                let mut parameters = Vec::new();
                while !self.check(&TokenKind::RParen) && !self.at_eof() {
                    parameters.push(self.parse_type());
                    if !self.eat(&TokenKind::Comma) {
                        break;
                    }
                }
                self.expect(&TokenKind::RParen);
                let result = if self.eat(&TokenKind::Colon) {
                    Box::new(self.parse_type())
                } else {
                    Box::new(self.unit_type())
                };
                let span = start.to(self.previous_span());
                let id = self.next_id();
                TypeExpr { kind: TypeExprKind::Function { parameters, result, is_async }, span, id }
            }
            _ => {
                self.expected("a type");
                self.advance();
                let id = self.next_id();
                TypeExpr { kind: TypeExprKind::Error, span: start, id }
            }
        }
    }

    /// Parses `<A, B>` after a type name, if present.
    fn parse_type_arguments(&mut self) -> Vec<TypeExpr> {
        if !self.check(&TokenKind::Lt) || self.at_line_start() {
            return Vec::new();
        }
        self.advance();

        let mut arguments = Vec::new();
        while !self.at_type_argument_end() && !self.at_eof() {
            arguments.push(self.parse_type());
            if !self.eat(&TokenKind::Comma) {
                break;
            }
        }
        self.close_type_arguments();
        arguments
    }

    /// Whether the cursor is at the `>` (or the `>>`) closing a type argument
    /// list.
    fn at_type_argument_end(&self) -> bool {
        matches!(self.peek_kind(), TokenKind::Gt | TokenKind::Shr | TokenKind::GtEq)
    }

    /// Consumes the `>` that closes a type argument list.
    ///
    /// The lexer produces `>>` as one token because it is usually a shift, so
    /// `List<List<Int>>` needs the token split in half here. The first half
    /// closes the inner list and the rest is pushed back for the outer one.
    fn close_type_arguments(&mut self) {
        match self.peek_kind().clone() {
            TokenKind::Gt => {
                self.advance();
            }
            TokenKind::Shr => self.split_leading_gt(TokenKind::Gt),
            TokenKind::GtEq => self.split_leading_gt(TokenKind::Eq),
            _ => {
                self.expected("`>`");
            }
        }
    }

    /// Replaces the current token with `rest`, having consumed a leading `>`.
    fn split_leading_gt(&mut self, rest: TokenKind) {
        let token = self.peek().clone();
        let tail_span =
            noto_span::Span::new(token.span.file, token.span.start + 1, token.span.end);
        self.replace_current(noto_lexer::Token::new(rest, tail_span, false));
    }

    /// The implicit `Unit` result of a function type written without one.
    fn unit_type(&mut self) -> TypeExpr {
        let span = self.previous_span();
        let id = self.next_id();
        let path = noto_ast::Path::single(noto_ast::Ident::new("Unit", span));
        TypeExpr::named(path, id)
    }
}
