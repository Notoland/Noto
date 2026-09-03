//! Declaration parsing.

use crate::Parser;
use noto_ast::{
    Attribute, ClassKind, ConstItem, EnumCase, EnumItem, Field, FnItem, ImportItem, InterfaceItem,
    Item, ItemKind, LetKind, Modifiers, Module, Param, Property, PropertyAccessor, TestItem,
    TypeDeclItem, TypeParam, Visibility,
};
use noto_diagnostics::{codes, Diagnostic};
use noto_lexer::{Keyword, TokenKind};
use noto_span::Span;

impl Parser<'_> {
    /// Parses a whole module: every top-level declaration in a file.
    pub fn parse_module(&mut self) -> Module {
        let mut items = Vec::new();

        while !self.at_eof() {
            let before = self.position;
            let item = self.parse_item();
            if !matches!(item.kind, ItemKind::Error) {
                items.push(item);
            }
            if self.position == before {
                self.advance();
            }
        }

        let id = self.next_id();
        Module { items, span: self.file_span, id }
    }

    /// Parses one declaration, including its doc comment, attributes and
    /// modifiers.
    pub(crate) fn parse_item(&mut self) -> Item {
        let start = self.peek_span();
        let doc = self.parse_doc_comment();
        let attributes = self.parse_attributes();
        let modifiers = self.parse_modifiers();

        let kind = match self.peek_kind() {
            TokenKind::Keyword(Keyword::Fn) => self.parse_fn(&modifiers),
            TokenKind::Keyword(Keyword::Class) => self.parse_type_decl(ClassKind::Class),
            TokenKind::Keyword(Keyword::Struct) => self.parse_type_decl(ClassKind::Struct),
            TokenKind::Keyword(Keyword::Interface) => self.parse_interface(),
            TokenKind::Keyword(Keyword::Enum) => self.parse_enum(),
            TokenKind::Keyword(Keyword::Const) => self.parse_const(),
            TokenKind::Keyword(Keyword::Import) => self.parse_import(),
            TokenKind::Keyword(Keyword::Test) => self.parse_test(),
            _ => {
                let found = self.peek_kind().describe();
                let span = self.peek_span();
                self.error(
                    Diagnostic::error(
                        codes::UNEXPECTED_TOKEN,
                        format!("expected a declaration, found {found}"),
                    )
                    .with_primary(span, "not a declaration")
                    .with_help(
                        "a file may contain `fn`, `class`, `struct`, `interface`, `enum`, \
                         `const`, `import` and `test` declarations",
                    ),
                );
                self.recover_to_item();
                ItemKind::Error
            }
        };

        let span = start.to(self.previous_span());
        Item { kind, modifiers, attributes, doc, span }
    }

    /// Collects the `///` lines written above a declaration.
    fn parse_doc_comment(&mut self) -> Option<String> {
        let mut lines = Vec::new();
        while let TokenKind::DocComment(text) = self.peek_kind() {
            lines.push(text.clone());
            self.advance();
        }
        (!lines.is_empty()).then(|| lines.join("\n"))
    }

    /// Parses `@name` and `@name(args)` annotations.
    fn parse_attributes(&mut self) -> Vec<Attribute> {
        let mut attributes = Vec::new();
        while self.check(&TokenKind::At) {
            let start = self.peek_span();
            self.advance();
            let name = self.expect_ident();
            let arguments = if self.check(&TokenKind::LParen) && !self.at_line_start() {
                self.advance();
                let mut arguments = Vec::new();
                while !self.check(&TokenKind::RParen) && !self.at_eof() {
                    arguments.push(self.parse_expr());
                    if !self.eat(&TokenKind::Comma) {
                        break;
                    }
                }
                self.expect(&TokenKind::RParen);
                arguments
            } else {
                Vec::new()
            };
            let span = start.to(self.previous_span());
            attributes.push(Attribute { name, arguments, span });
        }
        attributes
    }

    /// Parses the modifier list in front of a declaration.
    ///
    /// `data` is read here rather than as part of the declaration keyword so
    /// that `public data class` and `data class` both work.
    fn parse_modifiers(&mut self) -> Modifiers {
        let start = self.peek_span();
        let mut modifiers = Modifiers::default();
        let mut saw_any = false;
        let mut seen_data = false;

        loop {
            let Some(keyword) = self.peek().keyword() else { break };
            let span = self.peek_span();

            match keyword {
                Keyword::Public | Keyword::Private | Keyword::Protected | Keyword::Internal => {
                    if modifiers.visibility_explicit {
                        self.error(
                            Diagnostic::error(
                                codes::INVALID_MODIFIER,
                                "a declaration may have only one visibility modifier",
                            )
                            .with_primary(span, "second visibility modifier"),
                        );
                    }
                    modifiers.visibility = match keyword {
                        Keyword::Public => Visibility::Public,
                        Keyword::Private => Visibility::Private,
                        Keyword::Protected => Visibility::Protected,
                        _ => Visibility::Internal,
                    };
                    modifiers.visibility_explicit = true;
                }
                Keyword::Abstract => modifiers.is_abstract = true,
                Keyword::Sealed => modifiers.is_sealed = true,
                Keyword::Override => modifiers.is_override = true,
                Keyword::Async => {
                    // `async fn` is a modifier; `async fn(): T` in type position
                    // is not, but types are never parsed here.
                    modifiers.is_async = true;
                }
                Keyword::Export => modifiers.is_exported = true,
                Keyword::Data => {
                    // `data` only modifies `class` and `struct`.
                    if !matches!(
                        self.peek_nth(1).keyword(),
                        Some(Keyword::Class) | Some(Keyword::Struct)
                    ) {
                        break;
                    }
                    seen_data = true;
                }
                _ => break,
            }

            saw_any = true;
            self.advance();
        }

        if seen_data {
            // Recorded on the parser so that parse_type_decl can pick the data
            // flavour without re-reading the token.
            self.pending_data = true;
        }
        if saw_any {
            modifiers.span = Some(start.to(self.previous_span()));
        }
        modifiers
    }

    /// Parses a function declaration.
    fn parse_fn(&mut self, modifiers: &Modifiers) -> ItemKind {
        self.expect_keyword(Keyword::Fn);

        let first = self.expect_ident();
        let type_params = self.parse_type_params();

        // `fn String.isValidEmail()` declares an extension on `String`.
        let (receiver, name) = if self.check(&TokenKind::Dot) && !self.at_line_start() {
            self.advance();
            let id = self.next_id();
            let path = noto_ast::Path::single(first);
            (Some(noto_ast::TypeExpr::named(path, id)), self.expect_ident())
        } else {
            (None, first)
        };

        let params = self.parse_params();
        let result = if self.eat(&TokenKind::Colon) { Some(self.parse_type()) } else { None };

        // A body is either a block or `= expression` for a one-liner; an
        // abstract or interface method has neither.
        let body = if self.eat(&TokenKind::Eq) {
            Some(self.parse_expr_body())
        } else if self.check(&TokenKind::LBrace) {
            Some(self.parse_block())
        } else {
            None
        };

        let id = self.next_id();
        ItemKind::Fn(FnItem {
            name,
            receiver,
            type_params,
            params,
            result,
            body,
            is_async: modifiers.is_async,
            id,
        })
    }

    /// Parses a parenthesised parameter list.
    pub(crate) fn parse_params(&mut self) -> Vec<Param> {
        self.expect(&TokenKind::LParen);
        let mut params = Vec::new();
        let mut seen_default: Option<Span> = None;

        while !self.check(&TokenKind::RParen) && !self.at_eof() {
            let start = self.peek_span();
            let name = self.expect_ident();
            let ty = if self.eat(&TokenKind::Colon) { Some(self.parse_type()) } else { None };
            let default = if self.eat(&TokenKind::Eq) { Some(self.parse_expr()) } else { None };
            let span = start.to(self.previous_span());

            if ty.is_none() && default.is_none() {
                self.error(
                    Diagnostic::error(
                        codes::CANNOT_INFER,
                        format!("parameter `{name}` needs a type"),
                    )
                    .with_primary(span, "no type given")
                    .with_help("write the type after the name, as in `count: Int`"),
                );
            }

            // Once a parameter has a default, the ones after it must too, or a
            // positional call could not tell which argument it was passing.
            match (&default, seen_default) {
                (Some(_), _) => seen_default = Some(span),
                (None, Some(previous)) => self.error(
                    Diagnostic::error(
                        codes::UNEXPECTED_CONSTRUCT,
                        format!("parameter `{name}` must have a default value"),
                    )
                    .with_primary(span, "this parameter has no default")
                    .with_secondary(previous, "an earlier parameter has one")
                    .with_help("give it a default, or move it before the defaulted parameters"),
                ),
                (None, None) => {}
            }

            let id = self.next_id();
            params.push(Param { name, ty, default, span, id });

            if !self.eat(&TokenKind::Comma) {
                break;
            }
        }

        self.expect(&TokenKind::RParen);
        params
    }

    /// Parses `<T, U: Bound>` after a declaration name.
    fn parse_type_params(&mut self) -> Vec<TypeParam> {
        if !self.check(&TokenKind::Lt) || self.at_line_start() {
            return Vec::new();
        }
        self.advance();

        let mut params = Vec::new();
        while !self.check(&TokenKind::Gt) && !self.at_eof() {
            let start = self.peek_span();
            let name = self.expect_ident();
            let mut bounds = Vec::new();
            if self.eat(&TokenKind::Colon) {
                bounds.push(self.parse_type());
                while self.eat(&TokenKind::Plus) {
                    bounds.push(self.parse_type());
                }
            }
            let span = start.to(self.previous_span());
            let id = self.next_id();
            params.push(TypeParam { name, bounds, span, id });

            if !self.eat(&TokenKind::Comma) {
                break;
            }
        }

        self.expect(&TokenKind::Gt);
        params
    }

    /// Parses a `class`, `struct`, `data class` or `data struct`.
    fn parse_type_decl(&mut self, base_kind: ClassKind) -> ItemKind {
        let is_data = std::mem::take(&mut self.pending_data);
        let class_kind = match (base_kind, is_data) {
            (ClassKind::Class, true) => ClassKind::DataClass,
            (ClassKind::Struct, true) => ClassKind::DataStruct,
            (kind, _) => kind,
        };

        self.advance();
        let name = self.expect_ident();
        let type_params = self.parse_type_params();

        // `class User(val name: String)` declares fields and a constructor at
        // once.
        let primary_params = if self.check(&TokenKind::LParen) {
            self.parse_primary_constructor()
        } else {
            Vec::new()
        };

        let (base, interfaces) = self.parse_supertypes();

        let mut fields = Vec::new();
        let mut properties = Vec::new();
        let mut methods = Vec::new();
        if self.check(&TokenKind::LBrace) {
            self.parse_type_body(&mut fields, &mut properties, &mut methods);
        }

        let id = self.next_id();
        ItemKind::TypeDecl(TypeDeclItem {
            class_kind,
            name,
            type_params,
            primary_params,
            base,
            interfaces,
            fields,
            properties,
            methods,
            id,
        })
    }

    /// Parses the parameter list that declares a type's primary constructor.
    fn parse_primary_constructor(&mut self) -> Vec<Field> {
        self.expect(&TokenKind::LParen);
        let mut fields = Vec::new();

        while !self.check(&TokenKind::RParen) && !self.at_eof() {
            let start = self.peek_span();
            let modifiers = self.parse_modifiers();
            let kind = if self.eat_keyword(Keyword::Var) {
                LetKind::Var
            } else {
                self.eat_keyword(Keyword::Val);
                LetKind::Val
            };
            let name = self.expect_ident();
            let ty = if self.eat(&TokenKind::Colon) { Some(self.parse_type()) } else { None };
            let default = if self.eat(&TokenKind::Eq) { Some(self.parse_expr()) } else { None };
            let span = start.to(self.previous_span());

            if ty.is_none() {
                self.error(
                    Diagnostic::error(codes::CANNOT_INFER, format!("field `{name}` needs a type"))
                        .with_primary(span, "no type given"),
                );
            }

            let id = self.next_id();
            fields.push(Field { modifiers, kind, name, ty, default, span, id });

            if !self.eat(&TokenKind::Comma) {
                break;
            }
        }

        self.expect(&TokenKind::RParen);
        fields
    }

    /// Parses `: Base, Interface1, Interface2`.
    ///
    /// Noto has single inheritance: at most one of the listed types may be a
    /// class, and it must come first. Which of them are classes is not known
    /// until resolution, so the parser keeps the first entry as the candidate
    /// base and `noto-semantic` reports the rest if they turn out to be
    /// classes.
    fn parse_supertypes(&mut self) -> (Option<noto_ast::TypeExpr>, Vec<noto_ast::TypeExpr>) {
        if !self.eat(&TokenKind::Colon) {
            return (None, Vec::new());
        }
        let mut types = vec![self.parse_type()];
        while self.eat(&TokenKind::Comma) {
            types.push(self.parse_type());
        }
        let mut iter = types.into_iter();
        let base = iter.next();
        (base, iter.collect())
    }

    /// Parses the `{ .. }` body of a class, struct or enum.
    fn parse_type_body(
        &mut self,
        fields: &mut Vec<Field>,
        properties: &mut Vec<Property>,
        methods: &mut Vec<Item>,
    ) {
        self.expect(&TokenKind::LBrace);

        while !self.check(&TokenKind::RBrace) && !self.at_eof() {
            let before = self.position;

            if matches!(self.peek_kind(), TokenKind::Keyword(Keyword::Val | Keyword::Var))
                || self.at_modified_field()
            {
                match self.parse_member_binding() {
                    Member::Field(field) => fields.push(field),
                    Member::Property(property) => properties.push(property),
                }
            } else {
                methods.push(self.parse_item());
            }

            if self.position == before {
                self.advance();
            }
        }

        self.expect(&TokenKind::RBrace);
    }

    /// Whether the cursor is at a member binding written with modifiers, such
    /// as `private val name: String`.
    fn at_modified_field(&self) -> bool {
        let mut offset = 0;
        while let Some(keyword) = self.peek_nth(offset).keyword() {
            if keyword.is_visibility() || keyword == Keyword::Override {
                offset += 1;
                continue;
            }
            return matches!(keyword, Keyword::Val | Keyword::Var) && offset > 0;
        }
        false
    }

    /// Parses a `val`/`var` member, which becomes a property when it has
    /// accessors and a field otherwise.
    fn parse_member_binding(&mut self) -> Member {
        let start = self.peek_span();
        let modifiers = self.parse_modifiers();
        let kind = if self.eat_keyword(Keyword::Var) { LetKind::Var } else {
            self.expect_keyword(Keyword::Val);
            LetKind::Val
        };
        let name = self.expect_ident();
        let ty = if self.eat(&TokenKind::Colon) { Some(self.parse_type()) } else { None };
        let default = if self.eat(&TokenKind::Eq) { Some(self.parse_expr()) } else { None };

        // `{ get = ... }` turns the member into a property.
        if self.check(&TokenKind::LBrace) && !self.at_line_start() {
            let (getter, setter) = self.parse_accessors();
            let span = start.to(self.previous_span());
            let id = self.next_id();
            return Member::Property(Property {
                modifiers,
                kind,
                name,
                ty,
                default,
                getter,
                setter,
                span,
                id,
            });
        }

        self.expect_statement_end();
        let span = start.to(self.previous_span());
        let id = self.next_id();
        Member::Field(Field { modifiers, kind, name, ty, default, span, id })
    }

    /// Parses the `{ get ... set ... }` block of a property.
    fn parse_accessors(&mut self) -> (Option<PropertyAccessor>, Option<PropertyAccessor>) {
        self.expect(&TokenKind::LBrace);
        let mut getter = None;
        let mut setter = None;

        while !self.check(&TokenKind::RBrace) && !self.at_eof() {
            let start = self.peek_span();
            let modifiers = self.parse_modifiers();
            let is_getter = self.check_keyword(Keyword::Get);

            if !is_getter && !self.check_keyword(Keyword::Set) {
                self.expected("`get` or `set`");
                self.recover_to_statement();
                continue;
            }
            self.advance();

            // `get = expression`, `get { .. }`, or a bare `get` asking for the
            // default implementation.
            let body = if self.eat(&TokenKind::Eq) {
                Some(self.parse_expr_body())
            } else if self.check(&TokenKind::LBrace) {
                Some(self.parse_block())
            } else {
                self.expect_statement_end();
                None
            };

            let span = start.to(self.previous_span());
            let id = self.next_id();
            let accessor = PropertyAccessor { modifiers, body, span, id };
            if is_getter {
                getter = Some(accessor);
            } else {
                setter = Some(accessor);
            }
        }

        self.expect(&TokenKind::RBrace);
        (getter, setter)
    }

    fn parse_interface(&mut self) -> ItemKind {
        self.expect_keyword(Keyword::Interface);
        let name = self.expect_ident();
        let type_params = self.parse_type_params();
        let (base, mut interfaces) = self.parse_supertypes();
        if let Some(base) = base {
            interfaces.insert(0, base);
        }

        let mut fields = Vec::new();
        let mut properties = Vec::new();
        let mut methods = Vec::new();
        if self.check(&TokenKind::LBrace) {
            self.parse_type_body(&mut fields, &mut properties, &mut methods);
        }

        // An interface has no storage of its own; a `val` inside one declares a
        // property the implementer must provide.
        for field in fields {
            let id = self.next_id();
            properties.push(Property {
                modifiers: field.modifiers,
                kind: field.kind,
                name: field.name,
                ty: field.ty,
                default: field.default,
                getter: None,
                setter: None,
                span: field.span,
                id,
            });
        }

        let id = self.next_id();
        ItemKind::Interface(InterfaceItem { name, type_params, interfaces, properties, methods, id })
    }

    fn parse_enum(&mut self) -> ItemKind {
        self.expect_keyword(Keyword::Enum);
        let name = self.expect_ident();
        let type_params = self.parse_type_params();
        let (base, mut interfaces) = self.parse_supertypes();
        if let Some(base) = base {
            interfaces.insert(0, base);
        }

        self.expect(&TokenKind::LBrace);
        let mut cases = Vec::new();
        let mut methods = Vec::new();

        while !self.check(&TokenKind::RBrace) && !self.at_eof() {
            let before = self.position;

            // Cases come first; once a declaration keyword appears, the rest of
            // the body is members.
            if self.peek().keyword().is_some_and(crate::starts_item) || !matches!(self.peek_kind(), TokenKind::Ident(_)) {
                methods.push(self.parse_item());
            } else {
                cases.push(self.parse_enum_case());
            }

            if self.position == before {
                self.advance();
            }
        }

        self.expect(&TokenKind::RBrace);
        let id = self.next_id();
        ItemKind::Enum(EnumItem { name, type_params, interfaces, cases, methods, id })
    }

    fn parse_enum_case(&mut self) -> EnumCase {
        let start = self.peek_span();
        let name = self.expect_ident();

        let fields = if self.check(&TokenKind::LParen) && !self.at_line_start() {
            self.parse_case_fields()
        } else {
            Vec::new()
        };

        let value = if self.eat(&TokenKind::Eq) { Some(self.parse_expr()) } else { None };

        self.eat(&TokenKind::Comma);
        let span = start.to(self.previous_span());
        let id = self.next_id();
        EnumCase { name, fields, value, span, id }
    }

    /// Parses the associated data of an enum case: `Success(value: Int)`.
    fn parse_case_fields(&mut self) -> Vec<Field> {
        self.expect(&TokenKind::LParen);
        let mut fields = Vec::new();

        while !self.check(&TokenKind::RParen) && !self.at_eof() {
            let start = self.peek_span();
            let name = self.expect_ident();
            let ty = if self.eat(&TokenKind::Colon) { Some(self.parse_type()) } else { None };
            let span = start.to(self.previous_span());
            if ty.is_none() {
                self.error(
                    Diagnostic::error(
                        codes::CANNOT_INFER,
                        format!("associated value `{name}` needs a type"),
                    )
                    .with_primary(span, "no type given")
                    .with_help("write it as `name: Type`"),
                );
            }
            let id = self.next_id();
            fields.push(Field {
                modifiers: Modifiers::default(),
                kind: LetKind::Val,
                name,
                ty,
                default: None,
                span,
                id,
            });

            if !self.eat(&TokenKind::Comma) {
                break;
            }
        }

        self.expect(&TokenKind::RParen);
        fields
    }

    fn parse_const(&mut self) -> ItemKind {
        self.expect_keyword(Keyword::Const);
        let name = self.expect_ident();
        let ty = if self.eat(&TokenKind::Colon) { Some(self.parse_type()) } else { None };
        self.expect(&TokenKind::Eq);
        let value = self.parse_expr();
        self.expect_statement_end();
        let id = self.next_id();
        ItemKind::Const(ConstItem { name, ty, value, id })
    }

    /// Parses `import std.io`, `import std.io { File }` and
    /// `import std.io as io`.
    fn parse_import(&mut self) -> ItemKind {
        self.expect_keyword(Keyword::Import);
        let path = self.parse_path();

        let mut names = Vec::new();
        if self.check(&TokenKind::LBrace) && !self.at_line_start() {
            self.advance();
            while !self.check(&TokenKind::RBrace) && !self.at_eof() {
                names.push(self.expect_ident());
                if !self.eat(&TokenKind::Comma) {
                    break;
                }
            }
            self.expect(&TokenKind::RBrace);
        }

        let alias = if self.eat_keyword(Keyword::As) { Some(self.expect_ident()) } else { None };

        if !names.is_empty() && alias.is_some() {
            self.error(
                Diagnostic::error(
                    codes::UNEXPECTED_CONSTRUCT,
                    "an import cannot both select names and take an alias",
                )
                .with_primary(path.span, "in this import")
                .with_help("write two imports, or drop the alias"),
            );
        }

        self.expect_statement_end();
        let id = self.next_id();
        ItemKind::Import(ImportItem { path, names, alias, id })
    }

    /// Parses `test "description" { .. }`.
    fn parse_test(&mut self) -> ItemKind {
        self.expect_keyword(Keyword::Test);
        let name_span = self.peek_span();

        let name = match self.peek_kind().clone() {
            TokenKind::Str(literal) => {
                self.advance();
                match literal.as_plain_text() {
                    Some(text) => text.to_string(),
                    None => {
                        self.error(
                            Diagnostic::error(
                                codes::UNEXPECTED_CONSTRUCT,
                                "a test name must be a plain string",
                            )
                            .with_primary(name_span, "this name is interpolated")
                            .with_help("test names are collected at compile time, so they cannot depend on runtime values"),
                        );
                        "<invalid>".to_string()
                    }
                }
            }
            _ => {
                self.expected("a test name in quotes");
                "<missing>".to_string()
            }
        };

        let body = self.parse_block();
        let id = self.next_id();
        ItemKind::Test(TestItem { name, name_span, body, id })
    }
}

/// What a `val`/`var` member turned out to be.
enum Member {
    /// Storage.
    Field(Field),
    /// Storage or computation behind accessors.
    Property(Property),
}
