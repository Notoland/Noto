//! The second pass: checking bodies against collected signatures.

use crate::analysis::{FunctionId, Resolution};
use crate::{builtins, Checker};
use noto_ast::{
    BinaryOp, Block, Expr, ExprKind, ItemKind, LetKind, Literal, Module, Pattern, PatternKind,
    Stmt, StmtKind, StringSegment, UnaryOp,
};
use noto_diagnostics::{codes, Diagnostic};
use noto_span::Span;
use noto_types::{Primitive, Type, TypeId};

impl Checker<'_> {
    /// Checks the body of every declaration.
    pub(crate) fn check_items(&mut self, module: &Module) {
        for item in &module.items {
            match &item.kind {
                ItemKind::Fn(function) => {
                    let Some(body) = &function.body else { continue };
                    let Some(id) = self.function_with_body(body.id) else { continue };
                    self.check_function_body(id, function, body);
                }
                ItemKind::Test(test) => {
                    let Some(id) = self.function_with_body(test.body.id) else { continue };
                    self.check_test_body(id, &test.body);
                }
                ItemKind::TypeDecl(decl) => {
                    for method in &decl.methods {
                        let ItemKind::Fn(function) = &method.kind else { continue };
                        let Some(body) = &function.body else { continue };
                        let Some(id) = self.function_with_body(body.id) else { continue };
                        self.check_function_body(id, function, body);
                    }
                }
                _ => {}
            }
        }

        if self.entry.is_none() && !self.functions.is_empty() {
            // Reported by the driver rather than here: a library has no `main`
            // and analysing it is still meaningful.
        }
    }

    /// Finds the function whose body is this block.
    ///
    /// Looking it up by name would be ambiguous once a program is many
    /// modules: two modules may each declare a `helper`. A body's node id
    /// belongs to one declaration in one file and to nothing else.
    fn function_with_body(&self, body: noto_ast::NodeId) -> Option<FunctionId> {
        self.functions
            .iter()
            .position(|function| function.body == Some(body))
            .map(|index| FunctionId(index as u32))
    }

    fn check_function_body(&mut self, id: FunctionId, function: &noto_ast::FnItem, body: &Block) {
        let info = &self.functions[id.0 as usize];
        let result = info.result;
        let parameters = info.parameters.clone();

        self.current_function = Some(id);
        self.expected_result = result;
        self.scopes.push();

        for local in &parameters {
            let name = self.locals[local.0 as usize].name.clone();
            self.scopes.declare(name, Resolution::Local(*local));
        }

        let body_type = self.check_block(body);
        self.scopes.pop();
        self.current_function = None;

        // A body whose last expression has the right type is a valid result,
        // which is what makes `fn double(n: Int): Int = n * 2` work.
        let unit = self.store.unit();
        if result != unit && !self.store.is_assignable(body_type, result) {
            let tail = body.tail_expr().map(|expr| expr.span).unwrap_or(body.span);
            let (expected, found) = (self.store.render(result), self.store.render(body_type));
            let mut diagnostic = Diagnostic::error(
                codes::MISSING_RETURN,
                format!("this function must produce a `{expected}`"),
            )
            .with_primary(tail, format!("found `{found}`"));
            if let Some(declared) = &function.result {
                diagnostic = diagnostic
                    .with_secondary(declared.span, format!("declared to return `{expected}` here"));
            }
            if body_type == unit {
                diagnostic = diagnostic
                    .with_help("end the function with the value to return, or use `return`");
            }
            self.sink.emit(diagnostic);
        }
    }

    fn check_test_body(&mut self, id: FunctionId, body: &Block) {
        self.current_function = Some(id);
        self.expected_result = self.store.unit();
        self.scopes.push();
        self.check_block(body);
        self.scopes.pop();
        self.current_function = None;
    }

    // --- statements -------------------------------------------------------

    /// Checks a block and returns the type it evaluates to.
    pub(crate) fn check_block(&mut self, block: &Block) -> TypeId {
        self.scopes.push();
        let ty = self.check_statements(&block.statements, block.span);
        self.scopes.pop();
        self.record_type(block.id, ty)
    }

    /// Checks a block that shares the enclosing scope, used for loop bodies
    /// where the pattern is already bound.
    fn check_block_in_scope(&mut self, block: &Block) -> TypeId {
        let ty = self.check_statements(&block.statements, block.span);
        self.record_type(block.id, ty)
    }

    fn check_statements(&mut self, statements: &[Stmt], span: Span) -> TypeId {
        let mut result = self.store.unit();
        let mut unreachable_from: Option<Span> = None;

        for (index, stmt) in statements.iter().enumerate() {
            if let Some(cause) = unreachable_from {
                self.sink.emit(
                    Diagnostic::warning(codes::UNREACHABLE_CODE, "this code can never run")
                        .with_primary(stmt.span, "unreachable")
                        .with_secondary(cause, "control never continues past here"),
                );
                unreachable_from = None;
            }

            let ty = self.check_stmt(stmt);
            let is_last = index + 1 == statements.len();

            if self.store.get(ty).is_never() && !is_last {
                unreachable_from = Some(stmt.span);
            }
            if is_last {
                result = ty;
            }
        }

        let _ = span;
        result
    }

    /// Checks a statement and returns the type it contributes when it is the
    /// last one in its block.
    fn check_stmt(&mut self, stmt: &Stmt) -> TypeId {
        match &stmt.kind {
            StmtKind::Let { kind, pattern, ty, value } => {
                self.check_let(*kind, pattern, ty.as_ref(), value.as_ref());
                self.store.unit()
            }
            StmtKind::Expr(expr) => self.check_expr(expr),
            StmtKind::While { condition, body } => {
                self.check_condition(condition);
                self.scopes.push_loop();
                self.check_block_in_scope(body);
                self.scopes.pop();
                self.store.unit()
            }
            StmtKind::Loop { body } => {
                self.scopes.push_loop();
                self.check_block_in_scope(body);
                self.scopes.pop();
                self.store.unit()
            }
            StmtKind::For { pattern, iterable, body } => {
                self.check_for(pattern, iterable, body);
                self.store.unit()
            }
            StmtKind::Defer { value } => {
                self.check_expr(value);
                self.store.unit()
            }
            StmtKind::Item(_) => {
                self.sink.emit(
                    Diagnostic::error(
                        codes::UNSUPPORTED_CONSTRUCT,
                        "declarations inside a function body are not supported yet",
                    )
                    .with_primary(stmt.span, "not implemented in Noto 0.5")
                    .with_help("move the declaration to the top level of the file"),
                );
                self.store.unit()
            }
            StmtKind::Error => self.store.error(),
        }
    }

    fn check_let(
        &mut self,
        kind: LetKind,
        pattern: &Pattern,
        declared: Option<&noto_ast::TypeExpr>,
        value: Option<&Expr>,
    ) {
        let declared_ty = declared.map(|ty| self.resolve_type(ty));

        let value_ty = match value {
            Some(expr) => match declared_ty {
                Some(expected) => self.check_expr_expecting(expr, expected),
                None => self.check_expr(expr),
            },
            None => declared_ty.unwrap_or_else(|| self.store.error()),
        };

        if let (Some(expected), Some(expr)) = (declared_ty, value) {
            self.expect_assignable(value_ty, expected, expr.span, declared.map(|d| d.span));
        }

        // A binding may not hold `Nothing`: there would be no value to bind.
        let mut ty = declared_ty.unwrap_or(value_ty);
        if self.store.get(ty).is_never() {
            ty = self.store.error();
        }

        self.bind_pattern(pattern, ty, kind == LetKind::Var);
    }

    /// Introduces the names a binding pattern declares.
    fn bind_pattern(&mut self, pattern: &Pattern, ty: TypeId, is_mutable: bool) {
        match &pattern.kind {
            PatternKind::Wildcard => {}
            PatternKind::Binding { name, .. } => {
                if self.scopes.is_declared_here(&name.name) {
                    self.sink.emit(
                        Diagnostic::error(
                            codes::DUPLICATE_NAME,
                            format!("`{name}` is already declared in this scope"),
                        )
                        .with_primary(name.span, "redeclared here")
                        .with_help("give the new binding a different name"),
                    );
                }
                let local = self.declare_local(&name.name, ty, is_mutable, false, name.span);
                self.record_resolution(pattern.id, Resolution::Local(local));
            }
            PatternKind::Tuple(items) => {
                let element_types = match self.store.get(ty).clone() {
                    Type::Tuple(types) if types.len() == items.len() => types,
                    Type::Error => vec![self.store.error(); items.len()],
                    other => {
                        let rendered = self.store.render(ty);
                        let _ = other;
                        self.sink.emit(
                            Diagnostic::error(
                                codes::TYPE_MISMATCH,
                                format!("`{rendered}` cannot be destructured into {} parts", items.len()),
                            )
                            .with_primary(pattern.span, "this pattern does not match the value"),
                        );
                        vec![self.store.error(); items.len()]
                    }
                };
                for (item, item_ty) in items.iter().zip(element_types) {
                    self.bind_pattern(item, item_ty, is_mutable);
                }
            }
            _ => {
                self.sink.emit(
                    Diagnostic::error(
                        codes::UNSUPPORTED_CONSTRUCT,
                        "this pattern is not supported in a binding yet",
                    )
                    .with_primary(pattern.span, "not implemented in Noto 0.5")
                    .with_help("bind a name, a tuple of names, or `_`"),
                );
            }
        }
    }

    fn check_for(&mut self, pattern: &Pattern, iterable: &Expr, body: &Block) {
        let iterable_ty = self.check_expr(iterable);

        // Ranges are the only iterable this compiler lowers so far.
        let element = match &iterable.kind {
            ExprKind::Range { .. } => self.store.int(),
            _ if self.store.get(iterable_ty).is_error() => self.store.error(),
            _ => {
                let rendered = self.store.render(iterable_ty);
                self.sink.emit(
                    Diagnostic::error(
                        codes::UNSUPPORTED_CONSTRUCT,
                        format!("cannot iterate over a `{rendered}` yet"),
                    )
                    .with_primary(iterable.span, "not iterable in Noto 0.5")
                    .with_help("iterate over a range, as in `for i in 0..10`"),
                );
                self.store.error()
            }
        };

        self.scopes.push_loop();
        self.bind_pattern(pattern, element, false);
        self.check_block_in_scope(body);
        self.scopes.pop();
    }

    // --- expressions ------------------------------------------------------

    /// Checks an expression and records its type.
    pub(crate) fn check_expr(&mut self, expr: &Expr) -> TypeId {
        let ty = self.check_expr_inner(expr, None);
        self.record_type(expr.id, ty)
    }

    /// Checks an expression that is known to be used where `expected` is
    /// required, which lets literals adopt the expected width.
    fn check_expr_expecting(&mut self, expr: &Expr, expected: TypeId) -> TypeId {
        let ty = self.check_expr_inner(expr, Some(expected));
        self.record_type(expr.id, ty)
    }

    fn check_expr_inner(&mut self, expr: &Expr, expected: Option<TypeId>) -> TypeId {
        match &expr.kind {
            ExprKind::Literal(literal) => self.check_literal(literal, expr.span, expected),
            ExprKind::Path(path) => self.check_path(expr, path),
            ExprKind::This => self.check_this(expr),
            ExprKind::Unary { op, operand } => self.check_unary(*op, operand, expr.span),
            ExprKind::Binary { op, left, right, op_span } => {
                self.check_binary(*op, left, right, *op_span)
            }
            ExprKind::Assign { target, value, op, op_span } => {
                self.check_assign(target, value, *op, *op_span)
            }
            ExprKind::If { condition, then_branch, else_branch } => {
                self.check_if(condition, then_branch, else_branch.as_deref(), expr.span)
            }
            ExprKind::When { scrutinee, arms } => {
                self.check_when(scrutinee.as_deref(), arms, expr.span)
            }
            ExprKind::Block(block) => self.check_block(block),
            ExprKind::Call(call) => self.check_call(expr, call),
            ExprKind::Member { receiver, name, safe } => {
                self.check_member(expr, receiver, name, *safe)
            }
            ExprKind::Return(value) => self.check_return(value.as_deref(), expr.span),
            ExprKind::Break | ExprKind::Continue => {
                if !self.scopes.in_loop() {
                    let word = if matches!(expr.kind, ExprKind::Break) { "break" } else { "continue" };
                    self.sink.emit(
                        Diagnostic::error(
                            codes::OUTSIDE_LOOP,
                            format!("`{word}` can only be used inside a loop"),
                        )
                        .with_primary(expr.span, format!("no loop encloses this `{word}`")),
                    );
                }
                self.store.nothing()
            }
            ExprKind::Range { start, end, .. } => {
                let int = self.store.int();
                for bound in [start, end].into_iter().flatten() {
                    let ty = self.check_expr_expecting(bound, int);
                    self.expect_assignable(ty, int, bound.span, None);
                }
                // Ranges exist only inside `for` and `when` in Noto 0.5; there
                // is no first-class `Range` type to give them yet.
                self.store.unit()
            }
            ExprKind::Tuple(items) => {
                let types: Vec<TypeId> = items.iter().map(|item| self.check_expr(item)).collect();
                if types.is_empty() {
                    self.store.unit()
                } else {
                    self.store.intern(Type::Tuple(types))
                }
            }
            _ => {
                self.sink.emit(
                    Diagnostic::error(
                        codes::UNSUPPORTED_CONSTRUCT,
                        "this expression is not supported by this compiler yet",
                    )
                    .with_primary(expr.span, "not implemented in Noto 0.5"),
                );
                self.store.error()
            }
        }
    }

    /// Checks `this`, which names the receiver a method was called on.
    ///
    /// The receiver is an ordinary parameter bound under a keyword, so this
    /// is a scope lookup and nothing more.
    fn check_this(&mut self, expr: &Expr) -> TypeId {
        match self.scopes.lookup(crate::RECEIVER_NAME) {
            Some(Resolution::Local(local)) => {
                self.record_resolution(expr.id, Resolution::Local(local));
                self.locals[local.0 as usize].ty
            }
            _ => {
                self.sink.emit(
                    Diagnostic::error(
                        codes::OUTSIDE_FUNCTION,
                        "`this` can only be used inside a method",
                    )
                    .with_primary(expr.span, "no receiver here"),
                );
                self.store.error()
            }
        }
    }

    fn check_literal(
        &mut self,
        literal: &Literal,
        span: Span,
        expected: Option<TypeId>,
    ) -> TypeId {
        match literal {
            Literal::Int { value, suffix } => {
                // A literal takes the width it is used at when that width can
                // hold it: `val n: Int8 = 5` needs no suffix.
                let ty = match suffix {
                    Some(suffix) => self.store.primitive(suffix_primitive(*suffix)),
                    None => match expected.map(|ty| self.store.get(ty).clone()) {
                        Some(Type::Primitive(primitive)) if primitive.is_integer() => {
                            self.store.primitive(primitive)
                        }
                        _ => self.store.int(),
                    },
                };
                if let Some(primitive) = self.store.get(ty).as_primitive() {
                    if let Some((low, high)) = primitive.int_range() {
                        let value = *value as i128;
                        if value < low || value > high {
                            self.sink.emit(
                                Diagnostic::error(
                                    codes::NUMBER_OUT_OF_RANGE,
                                    format!("`{value}` does not fit in `{}`", primitive.name()),
                                )
                                .with_primary(span, "out of range")
                                .with_note(format!(
                                    "`{}` holds values from {low} to {high}",
                                    primitive.name()
                                )),
                            );
                        }
                    }
                }
                ty
            }
            Literal::Float { suffix, .. } => match suffix {
                Some(suffix) => self.store.primitive(suffix_primitive(*suffix)),
                None => self.store.float64(),
            },
            Literal::Bool(_) => self.store.bool(),
            Literal::Char(_) => self.store.char(),
            Literal::Null => {
                // `null` on its own is `Nothing?`, which is assignable to every
                // nullable type and to nothing else.
                let nothing = self.store.nothing();
                self.store.nullable(nothing)
            }
            Literal::Str(segments) => {
                for segment in segments {
                    let StringSegment::Interpolation(inner) = segment else { continue };
                    let ty = self.check_expr(inner);
                    if self.store.get(ty).is_error() {
                        continue;
                    }
                    if builtins::to_string_for(&self.store, ty).is_none() {
                        let rendered = self.store.render(ty);
                        self.sink.emit(
                            Diagnostic::error(
                                codes::UNKNOWN_MEMBER,
                                format!("`{rendered}` cannot be interpolated into a string"),
                            )
                            .with_primary(inner.span, "no text representation for this type")
                            .with_help("interpolation needs a `toString()`; `Int`, `Bool` and `String` have one"),
                        );
                    }
                }
                self.store.string()
            }
        }
    }

    fn check_path(&mut self, expr: &Expr, path: &noto_ast::Path) -> TypeId {
        let name = path.to_dotted();
        match self.lookup_value(&name) {
            Some(Resolution::Local(local)) => {
                self.record_resolution(expr.id, Resolution::Local(local));
                self.locals[local.0 as usize].ty
            }
            Some(Resolution::Const(id)) => {
                self.record_resolution(expr.id, Resolution::Const(id));
                self.constants[id.0 as usize].ty
            }
            Some(Resolution::Function(id)) => {
                self.record_resolution(expr.id, Resolution::Function(id));
                let info = &self.functions[id.0 as usize];
                let parameters: Vec<TypeId> =
                    info.parameters.iter().map(|local| self.locals[local.0 as usize].ty).collect();
                let result = info.result;
                let is_async = info.is_async;
                self.store.intern(Type::Function { parameters, result, is_async })
            }
            Some(other) => {
                self.record_resolution(expr.id, other);
                self.store.error()
            }
            None => {
                // A builtin used as a value rather than called: report it as a
                // name that exists but cannot be referenced on its own.
                if !builtins::free_overloads(&name).is_empty() {
                    self.sink.emit(
                        Diagnostic::error(
                            codes::UNSUPPORTED_CONSTRUCT,
                            format!("`{name}` can only be called, not used as a value"),
                        )
                        .with_primary(expr.span, "built-in functions are not first-class yet"),
                    );
                } else {
                    self.report_unknown_name(&name, expr.span);
                }
                self.record_resolution(expr.id, Resolution::Error);
                self.store.error()
            }
        }
    }

    fn report_unknown_name(&mut self, name: &str, span: Span) {
        let mut diagnostic =
            Diagnostic::error(codes::UNKNOWN_NAME, format!("cannot find `{name}` in this scope"))
                .with_primary(span, "not found");
        if let Some(suggestion) = self.suggest_name(name) {
            diagnostic = diagnostic.with_help(format!("did you mean `{suggestion}`?"));
        }
        self.sink.emit(diagnostic);
    }

    /// Finds a declared name that differs from `name` only in case or by one
    /// character, which catches the common typo.
    fn suggest_name(&self, name: &str) -> Option<String> {
        let candidates = self
            .locals
            .iter()
            .map(|local| local.name.clone())
            .chain(self.functions.iter().map(|function| function.name.clone()))
            .chain(self.constants.iter().map(|constant| constant.name.clone()))
            .chain(builtins::FREE_FUNCTIONS.iter().map(|b| b.name().to_string()));

        candidates
            .filter(|candidate| candidate != name)
            .min_by_key(|candidate| edit_distance(name, candidate))
            .filter(|candidate| edit_distance(name, candidate) <= max_distance(name))
    }

    fn check_unary(&mut self, op: UnaryOp, operand: &Expr, span: Span) -> TypeId {
        let ty = self.check_expr(operand);
        if self.store.get(ty).is_error() {
            return ty;
        }

        let ok = match op {
            UnaryOp::Neg => self.store.get(ty).is_numeric(),
            UnaryOp::Not => ty == self.store.bool(),
            UnaryOp::BitNot => self.store.get(ty).is_integer(),
        };

        if !ok {
            let rendered = self.store.render(ty);
            self.sink.emit(
                Diagnostic::error(
                    codes::INVALID_OPERANDS,
                    format!("`{}` cannot be applied to a `{rendered}`", op.as_str()),
                )
                .with_primary(span, format!("`{}` is not defined for this type", op.as_str()))
                .with_primary(operand.span, format!("this is a `{rendered}`")),
            );
            return self.store.error();
        }

        // Negating an unsigned value cannot produce a representable result.
        if op == UnaryOp::Neg {
            if let Some(primitive) = self.store.get(ty).as_primitive() {
                if primitive.is_integer() && !primitive.is_signed() {
                    self.sink.emit(
                        Diagnostic::error(
                            codes::INVALID_OPERANDS,
                            format!("`{}` is unsigned and cannot be negated", primitive.name()),
                        )
                        .with_primary(span, "no negative values exist for this type")
                        .with_help("convert to a signed type first, as in `value.toInt64()`"),
                    );
                    return self.store.error();
                }
            }
        }

        ty
    }

    fn check_binary(&mut self, op: BinaryOp, left: &Expr, right: &Expr, span: Span) -> TypeId {
        // `?:` takes a nullable left side and a default of the same base type.
        if op == BinaryOp::Elvis {
            return self.check_elvis(left, right, span);
        }

        let left_ty = self.check_expr(left);
        let right_ty = self.check_expr_expecting(right, left_ty);

        if self.store.get(left_ty).is_error() || self.store.get(right_ty).is_error() {
            return self.store.error();
        }

        // Using a nullable value in arithmetic or comparison is the mistake
        // null safety exists to catch.
        if !matches!(op, BinaryOp::Eq | BinaryOp::Ne) {
            for (ty, expr) in [(left_ty, left), (right_ty, right)] {
                if self.store.is_nullable(ty) {
                    let rendered = self.store.render(ty);
                    self.sink.emit(
                        Diagnostic::error(
                            codes::NULLABLE_NOT_ALLOWED,
                            format!("`{}` cannot be applied to a `{rendered}`", op.as_str()),
                        )
                        .with_primary(expr.span, "this may be null")
                        .with_help("supply a default with `?:`, or check for null first"),
                    );
                    return self.store.error();
                }
            }
        }

        use BinaryOp::*;
        let bool_ty = self.store.bool();

        match op {
            And | Or => {
                for (ty, expr) in [(left_ty, left), (right_ty, right)] {
                    self.expect_assignable(ty, bool_ty, expr.span, None);
                }
                bool_ty
            }
            Eq | Ne => {
                if self.store.join(left_ty, right_ty).is_none() {
                    let (a, b) = (self.store.render(left_ty), self.store.render(right_ty));
                    self.sink.emit(
                        Diagnostic::error(
                            codes::INVALID_OPERANDS,
                            format!("`{a}` and `{b}` can never be equal"),
                        )
                        .with_primary(span, "these types have no values in common")
                        .with_help("compare values of the same type"),
                    );
                }
                bool_ty
            }
            Lt | Le | Gt | Ge => {
                self.require_same_numeric(op, left_ty, right_ty, left, right, span);
                bool_ty
            }
            Add | Sub | Mul | Div | Rem => {
                // `+` also concatenates strings; every other arithmetic
                // operator is numbers only.
                if op == Add && left_ty == self.store.string() && right_ty == self.store.string() {
                    return self.store.string();
                }
                self.require_same_numeric(op, left_ty, right_ty, left, right, span);
                left_ty
            }
            BitAnd | BitOr | BitXor | Shl | Shr => {
                for (ty, expr) in [(left_ty, left), (right_ty, right)] {
                    if !self.store.get(ty).is_integer() {
                        let rendered = self.store.render(ty);
                        self.sink.emit(
                            Diagnostic::error(
                                codes::INVALID_OPERANDS,
                                format!("`{}` needs integer operands", op.as_str()),
                            )
                            .with_primary(expr.span, format!("this is a `{rendered}`")),
                        );
                        return self.store.error();
                    }
                }
                left_ty
            }
            In => {
                self.sink.emit(
                    Diagnostic::error(
                        codes::UNSUPPORTED_CONSTRUCT,
                        "`in` is not supported outside a `when` arm yet",
                    )
                    .with_primary(span, "not implemented in Noto 0.5"),
                );
                bool_ty
            }
            Elvis => unreachable!("handled above"),
        }
    }

    fn check_elvis(&mut self, left: &Expr, right: &Expr, span: Span) -> TypeId {
        let left_ty = self.check_expr(left);
        let base = self.store.unwrap_nullable(left_ty);
        let right_ty = self.check_expr_expecting(right, base);

        if self.store.get(left_ty).is_error() {
            return right_ty;
        }
        if !self.store.is_nullable(left_ty) {
            let rendered = self.store.render(left_ty);
            self.sink.emit(
                Diagnostic::warning(
                    codes::NULLABLE_NOT_ALLOWED,
                    format!("the left side of `?:` is never null"),
                )
                .with_primary(left.span, format!("this is a `{rendered}`, not a `{rendered}?`"))
                .with_help("the right side can never run; remove the `?:`"),
            );
            return left_ty;
        }

        match self.store.join(base, right_ty) {
            Some(ty) => ty,
            None => {
                let (a, b) = (self.store.render(base), self.store.render(right_ty));
                self.sink.emit(
                    Diagnostic::error(
                        codes::TYPE_MISMATCH,
                        format!("expected `{a}` after `?:`, found `{b}`"),
                    )
                    .with_primary(right.span, format!("this is a `{b}`"))
                    .with_secondary(span, format!("the left side is a `{a}?`")),
                );
                self.store.error()
            }
        }
    }

    fn require_same_numeric(
        &mut self,
        op: BinaryOp,
        left_ty: TypeId,
        right_ty: TypeId,
        left: &Expr,
        right: &Expr,
        span: Span,
    ) {
        for (ty, expr) in [(left_ty, left), (right_ty, right)] {
            if !self.store.get(ty).is_numeric() {
                let rendered = self.store.render(ty);
                self.sink.emit(
                    Diagnostic::error(
                        codes::INVALID_OPERANDS,
                        format!("`{}` cannot be applied to a `{rendered}`", op.as_str()),
                    )
                    .with_primary(expr.span, format!("this is a `{rendered}`"))
                    .with_secondary(span, format!("`{}` needs numbers", op.as_str())),
                );
                return;
            }
        }

        if left_ty != right_ty {
            let (a, b) = (self.store.render(left_ty), self.store.render(right_ty));
            let mut diagnostic = Diagnostic::error(
                codes::NO_IMPLICIT_CONVERSION,
                format!("`{}` cannot mix `{a}` and `{b}`", op.as_str()),
            )
            .with_primary(right.span, format!("this is a `{b}`"))
            .with_secondary(left.span, format!("this is a `{a}`"))
            .with_note("Noto never converts between number types on its own");

            if let (Some(from), Some(to)) =
                (self.store.get(right_ty).as_primitive(), self.store.get(left_ty).as_primitive())
            {
                if from.widens_to(to) {
                    diagnostic = diagnostic
                        .with_help(format!("convert it with `.to{}()`", to.name()));
                }
            }
            self.sink.emit(diagnostic);
        }
    }

    fn check_assign(
        &mut self,
        target: &Expr,
        value: &Expr,
        op: Option<BinaryOp>,
        span: Span,
    ) -> TypeId {
        let target_ty = self.check_expr(target);

        // Assigning to a `val` is the mistake `val` exists to catch, so the
        // message names the declaration.
        if let Some(Resolution::Local(local)) = self.resolutions.get(&target.id).copied() {
            let info = &self.locals[local.0 as usize];
            if !info.is_mutable {
                let (name, declared_at, is_parameter) =
                    (info.name.clone(), info.span, info.is_parameter);
                let mut diagnostic = Diagnostic::error(
                    codes::REASSIGNED_VAL,
                    format!("cannot assign to `{name}`"),
                )
                .with_primary(span, "assigned here");
                diagnostic = if is_parameter {
                    diagnostic
                        .with_secondary(declared_at, "parameters cannot be reassigned")
                        .with_help("copy it into a `var` first")
                } else {
                    diagnostic
                        .with_secondary(declared_at, "declared with `val` here")
                        .with_help(format!("declare it with `var {name}` to allow reassignment"))
                };
                self.sink.emit(diagnostic);
            }
        } else if let Some(Resolution::Field { class, index }) =
            self.resolutions.get(&target.id).copied()
        {
            let class = &self.classes[class.0 as usize];
            let field = &class.fields[index as usize];
            if !field.is_mutable {
                let (class_name, name, declared_at) =
                    (class.name.clone(), field.name.clone(), field.span);
                self.sink.emit(
                    Diagnostic::error(
                        codes::REASSIGNED_VAL,
                        format!("cannot assign to `{class_name}.{name}`"),
                    )
                    .with_primary(span, "assigned here")
                    .with_secondary(declared_at, "declared with `val` here")
                    .with_help(format!("declare it as `var {name}` to allow reassignment")),
                );
            }
        } else if !matches!(self.resolutions.get(&target.id), Some(Resolution::Error)) {
            self.sink.emit(
                Diagnostic::error(codes::NOT_ASSIGNABLE, "this cannot be assigned to")
                    .with_primary(target.span, "not a place a value can be stored"),
            );
        }

        let value_ty = self.check_expr_expecting(value, target_ty);

        match op {
            // A compound assignment must be a valid binary operation first.
            Some(op) => {
                let result =
                    self.check_binary_types(op, target_ty, value_ty, target, value, span);
                self.expect_assignable(result, target_ty, span, None);
            }
            None => {
                self.expect_assignable(value_ty, target_ty, value.span, Some(target.span));
            }
        }

        self.store.unit()
    }

    /// The type-level half of [`Self::check_binary`], used by compound
    /// assignment where the operands have already been checked.
    fn check_binary_types(
        &mut self,
        op: BinaryOp,
        left_ty: TypeId,
        right_ty: TypeId,
        left: &Expr,
        right: &Expr,
        span: Span,
    ) -> TypeId {
        use BinaryOp::*;
        if self.store.get(left_ty).is_error() || self.store.get(right_ty).is_error() {
            return self.store.error();
        }
        match op {
            Add if left_ty == self.store.string() && right_ty == self.store.string() => {
                self.store.string()
            }
            Add | Sub | Mul | Div | Rem => {
                self.require_same_numeric(op, left_ty, right_ty, left, right, span);
                left_ty
            }
            BitAnd | BitOr | BitXor | Shl | Shr => left_ty,
            _ => left_ty,
        }
    }

    fn check_condition(&mut self, condition: &Expr) {
        let ty = self.check_expr(condition);
        let bool_ty = self.store.bool();
        if self.store.get(ty).is_error() || self.store.is_assignable(ty, bool_ty) {
            return;
        }
        let rendered = self.store.render(ty);
        let mut diagnostic = Diagnostic::error(
            codes::NON_BOOL_CONDITION,
            format!("a condition must be a `Bool`, not a `{rendered}`"),
        )
        .with_primary(condition.span, format!("this is a `{rendered}`"));
        if self.store.is_nullable(ty) && self.store.unwrap_nullable(ty) == bool_ty {
            diagnostic = diagnostic.with_help("supply a default with `?: false`");
        } else if self.store.get(ty).is_numeric() {
            diagnostic = diagnostic
                .with_help("Noto has no truthiness; compare explicitly, as in `count != 0`");
        }
        self.sink.emit(diagnostic);
    }

    fn check_if(
        &mut self,
        condition: &Expr,
        then_branch: &Block,
        else_branch: Option<&Expr>,
        span: Span,
    ) -> TypeId {
        self.check_condition(condition);
        let then_ty = self.check_block(then_branch);

        let Some(else_branch) = else_branch else {
            // Without an `else` the `if` may not run at all, so it has no value
            // to offer.
            return self.store.unit();
        };
        let else_ty = self.check_expr(else_branch);

        match self.store.join(then_ty, else_ty) {
            Some(ty) => ty,
            None => {
                let (a, b) = (self.store.render(then_ty), self.store.render(else_ty));
                self.sink.emit(
                    Diagnostic::error(
                        codes::TYPE_MISMATCH,
                        format!("the branches of this `if` have different types: `{a}` and `{b}`"),
                    )
                    .with_primary(else_branch.span, format!("this branch is a `{b}`"))
                    .with_secondary(
                        then_branch.tail_expr().map(|e| e.span).unwrap_or(then_branch.span),
                        format!("this branch is a `{a}`"),
                    )
                    .with_secondary(span, "both branches must agree when the `if` produces a value"),
                );
                self.store.error()
            }
        }
    }

    fn check_when(
        &mut self,
        scrutinee: Option<&Expr>,
        arms: &[noto_ast::WhenArm],
        span: Span,
    ) -> TypeId {
        let subject_ty = scrutinee.map(|expr| self.check_expr(expr));
        let mut result: Option<TypeId> = None;
        let mut has_else = false;

        let mut covered: Vec<u32> = Vec::new();

        for arm in arms {
            self.scopes.push();

            for pattern in &arm.patterns {
                let expected = subject_ty.unwrap_or_else(|| self.store.error());
                self.check_pattern(pattern, expected);
                // An unguarded arm is what actually covers a case; one behind
                // a guard may still fall through.
                if arm.guard.is_none() {
                    if let Some(Resolution::EnumCase { index, .. }) =
                        self.resolutions.get(&pattern.id)
                    {
                        covered.push(*index);
                    }
                }
            }
            if let Some(guard) = &arm.guard {
                self.check_condition(&guard.condition);
            }
            let arm_ty = self.check_expr(&arm.body);
            self.scopes.pop();

            has_else |= arm.is_else;

            result = Some(match result {
                None => arm_ty,
                Some(previous) => match self.store.join(previous, arm_ty) {
                    Some(joined) => joined,
                    None => {
                        let (a, b) = (self.store.render(previous), self.store.render(arm_ty));
                        self.sink.emit(
                            Diagnostic::error(
                                codes::TYPE_MISMATCH,
                                format!("this `when` arm produces a `{b}`, but earlier arms produce a `{a}`"),
                            )
                            .with_primary(arm.body.span, format!("this is a `{b}`"))
                            .with_help("make every arm produce the same type, or use `when` as a statement"),
                        );
                        self.store.error()
                    }
                },
            });
        }

        let result = result.unwrap_or_else(|| self.store.unit());

        // Matching every case of an enum is as complete as an `else`, and
        // saying so is the point of an enum: adding a case then turns every
        // `when` over it into an error that names what is missing.
        let missing = subject_ty.and_then(|ty| self.uncovered_cases(ty, &covered));
        let is_exhaustive = has_else || missing.as_ref().is_some_and(Vec::is_empty);

        let unit = self.store.unit();
        if !is_exhaustive && result != unit && !self.store.get(result).is_error() {
            let rendered = self.store.render(result);
            let mut diagnostic = Diagnostic::error(
                codes::NON_EXHAUSTIVE_WHEN,
                "this `when` produces a value but does not cover every case",
            )
            .with_primary(
                span,
                format!("no arm matches some values, so there is no `{rendered}` to produce"),
            );
            diagnostic = match &missing {
                Some(missing) => {
                    let names: Vec<String> =
                        missing.iter().map(|name| format!("`{name}`")).collect();
                    diagnostic
                        .with_note(format!("not covered: {}", names.join(", ")))
                        .with_help("add an arm for each, or an `else ->` arm")
                }
                None => diagnostic.with_help("add an `else ->` arm"),
            };
            self.sink.emit(diagnostic);
            return self.store.error();
        }

        result
    }

    /// The cases of `ty` that `covered` leaves out, or `None` if `ty` is not
    /// an enum.
    fn uncovered_cases(&self, ty: TypeId, covered: &[u32]) -> Option<Vec<String>> {
        // A nullable enum has one more possibility than its cases — `null` —
        // which no case pattern covers, so it is never exhaustive this way.
        if self.store.is_nullable(ty) {
            return None;
        }
        let (_, info) = self.enum_of(ty)?;
        Some(
            info.cases
                .iter()
                .enumerate()
                .filter(|(index, _)| !covered.contains(&(*index as u32)))
                .map(|(_, case)| case.name.clone())
                .collect(),
        )
    }

    /// Checks a pattern against the type it will be matched against.
    fn check_pattern(&mut self, pattern: &Pattern, expected: TypeId) {
        match &pattern.kind {
            PatternKind::Wildcard | PatternKind::Error => {}
            PatternKind::Binding { name, subpattern } => {
                if let Some(inner) = subpattern {
                    self.check_pattern(inner, expected);
                }
                let local = self.declare_local(&name.name, expected, false, false, name.span);
                self.record_resolution(pattern.id, Resolution::Local(local));
            }
            PatternKind::Value(expr) => {
                let ty = self.check_expr_expecting(expr, expected);
                self.expect_assignable(ty, expected, expr.span, None);
            }
            PatternKind::Range { start, end, .. } => {
                for bound in [start, end].into_iter().flatten() {
                    let ty = self.check_expr_expecting(bound, expected);
                    self.expect_assignable(ty, expected, bound.span, None);
                }
            }
            PatternKind::EnumCase { path, fields } => {
                self.check_enum_pattern(pattern, path, fields.as_deref(), expected);
            }
            PatternKind::Null => {
                if !self.store.is_nullable(expected) && !self.store.get(expected).is_error() {
                    let rendered = self.store.render(expected);
                    self.sink.emit(
                        Diagnostic::warning(
                            codes::NULLABLE_NOT_ALLOWED,
                            format!("a `{rendered}` is never null"),
                        )
                        .with_primary(pattern.span, "this arm can never match"),
                    );
                }
            }
            _ => {
                self.sink.emit(
                    Diagnostic::error(
                        codes::UNSUPPORTED_CONSTRUCT,
                        "this pattern is not supported by this compiler yet",
                    )
                    .with_primary(pattern.span, "not implemented in Noto 0.5"),
                );
            }
        }
    }

    /// Checks `Red` or `Color.Red` as a `when` pattern.
    ///
    /// The scrutinee's type says which enum the case must belong to, which is
    /// what lets an arm be written as the bare case name.
    fn check_enum_pattern(
        &mut self,
        pattern: &Pattern,
        path: &noto_ast::Path,
        fields: Option<&[Pattern]>,
        expected: TypeId,
    ) {
        if self.store.get(expected).is_error() {
            return;
        }

        let written = path.to_dotted();
        let Some((id, info)) = self.enum_of(self.store.unwrap_nullable(expected)) else {
            let rendered = self.store.render(expected);
            self.sink.emit(
                Diagnostic::error(
                    codes::TYPE_MISMATCH,
                    format!("`{written}` is a case, but this `when` matches a `{rendered}`"),
                )
                .with_primary(pattern.span, "not a case of anything")
                .with_help("match on an enum, or use a value pattern"),
            );
            return;
        };

        // `Red` and `Color.Red` name the same case; the qualified form is
        // what makes an arm readable away from its declaration.
        let case_name = match written.split_once('.') {
            Some((qualifier, rest)) if qualifier == info.name => rest.to_string(),
            Some(_) | None => written.clone(),
        };

        let (enum_name, cases) = (info.name.clone(), Self::case_list(info));
        let Some((index, _)) = info.case(&case_name) else {
            self.sink.emit(
                Diagnostic::error(
                    codes::UNKNOWN_MEMBER,
                    format!("`{enum_name}` has no case `{case_name}`"),
                )
                .with_primary(pattern.span, "no such case")
                .with_note(cases),
            );
            return;
        };

        let carried: Vec<TypeId> = info.cases[index as usize]
            .fields
            .iter()
            .map(|field| field.ty)
            .collect();

        match fields {
            Some(patterns) => {
                if carried.is_empty() {
                    if let Some(first) = patterns.first() {
                        self.sink.emit(
                            Diagnostic::error(
                                codes::ARITY_MISMATCH,
                                format!("`{enum_name}.{case_name}` carries no data"),
                            )
                            .with_primary(first.span, "nothing to destructure here"),
                        );
                    }
                } else if patterns.len() != carried.len() {
                    self.sink.emit(
                        Diagnostic::error(
                            codes::ARITY_MISMATCH,
                            format!(
                                "`{enum_name}.{case_name}` carries {} value{}, but {} {} matched",
                                carried.len(),
                                if carried.len() == 1 { "" } else { "s" },
                                patterns.len(),
                                if patterns.len() == 1 { "is" } else { "are" },
                            ),
                        )
                        .with_primary(pattern.span, "the shapes do not line up"),
                    );
                }
                for (sub, expected) in patterns.iter().zip(&carried) {
                    self.check_pattern(sub, *expected);
                }
            }
            None if !carried.is_empty() => {
                // Matching a case that carries data without naming what it
                // carries is legal and common: the arm only cares which case
                // it is.
            }
            None => {}
        }

        self.record_resolution(pattern.id, Resolution::EnumCase { enum_id: id, index });
    }

    fn check_return(&mut self, value: Option<&Expr>, span: Span) -> TypeId {
        if self.current_function.is_none() {
            self.sink.emit(
                Diagnostic::error(
                    codes::OUTSIDE_FUNCTION,
                    "`return` can only be used inside a function",
                )
                .with_primary(span, "no function encloses this `return`"),
            );
            return self.store.nothing();
        }

        let expected = self.expected_result;
        let unit = self.store.unit();

        match value {
            Some(expr) => {
                let ty = self.check_expr_expecting(expr, expected);
                if expected == unit && ty != unit {
                    let rendered = self.store.render(ty);
                    self.sink.emit(
                        Diagnostic::error(
                            codes::TYPE_MISMATCH,
                            "this function does not return a value",
                        )
                        .with_primary(expr.span, format!("returning a `{rendered}`"))
                        .with_help("declare a result type, as in `fn f(): Int`"),
                    );
                } else {
                    self.expect_assignable(ty, expected, expr.span, None);
                }
            }
            None => {
                if expected != unit {
                    let rendered = self.store.render(expected);
                    self.sink.emit(
                        Diagnostic::error(
                            codes::TYPE_MISMATCH,
                            format!("this function must return a `{rendered}`"),
                        )
                        .with_primary(span, "no value returned"),
                    );
                }
            }
        }

        // Control never continues past a `return`.
        self.store.nothing()
    }

    fn check_call(&mut self, expr: &Expr, call: &noto_ast::CallExpr) -> TypeId {
        if !call.type_arguments.is_empty() {
            self.sink.emit(
                Diagnostic::error(
                    codes::UNSUPPORTED_CONSTRUCT,
                    "explicit type arguments are not supported by this compiler yet",
                )
                .with_primary(expr.span, "not implemented in Noto 0.5"),
            );
        }

        for argument in &call.arguments {
            if let Some(name) = &argument.name {
                self.sink.emit(
                    Diagnostic::error(
                        codes::UNSUPPORTED_CONSTRUCT,
                        "named arguments are not supported by this compiler yet",
                    )
                    .with_primary(name.span, "not implemented in Noto 0.5")
                    .with_help("pass the arguments positionally"),
                );
            }
        }

        // A call of a plain name may be a builtin, which is resolved by
        // argument types rather than by scope.
        if let ExprKind::Path(path) = &call.callee.kind {
            let name = path.to_dotted();
            if self.lookup_value(&name).is_none() {
                let overloads = builtins::free_overloads(&name);
                if !overloads.is_empty() {
                    return self.check_builtin_call(expr, call, &name, &overloads);
                }
            }
        }

        // A call of a class name constructs one: `Point(1, 2)`.
        if let ExprKind::Path(path) = &call.callee.kind {
            if let Some(Resolution::Class(id)) = self.lookup_value(&path.to_dotted()) {
                return self.check_construction(expr, call, id);
            }
        }

        // A case carrying data is constructed by calling it:
        // `Shape.Circle(3)`.
        if let ExprKind::Member { receiver, name, safe: false } = &call.callee.kind {
            if let Some(id) = self.receiver_enum(receiver) {
                return self.check_case_construction(expr, call, id, name);
            }
        }

        // A qualified call through an imported namespace: `point.distance(1)`.
        if let ExprKind::Member { receiver, name, safe } = &call.callee.kind {
            if let Some(module) = self.receiver_module(receiver) {
                if *safe {
                    self.sink.emit(
                        Diagnostic::error(
                            codes::UNSUPPORTED_CONSTRUCT,
                            "a module is not a value, so `?.` means nothing here",
                        )
                        .with_primary(call.callee.span, "use `.`"),
                    );
                    return self.store.error();
                }
                return self.check_qualified_call(expr, call, module, name);
            }
        }

        // A method call on a builtin member: `value.toString()`.
        if let ExprKind::Member { receiver, name, safe } = &call.callee.kind {
            let receiver_ty = self.check_expr(receiver);
            if *safe {
                self.sink.emit(
                    Diagnostic::error(
                        codes::UNSUPPORTED_CONSTRUCT,
                        "safe calls are not supported by this compiler yet",
                    )
                    .with_primary(expr.span, "not implemented in Noto 0.5"),
                );
                return self.store.error();
            }
            let base = self.store.unwrap_nullable(receiver_ty);
            if self.store.is_nullable(receiver_ty) {
                let rendered = self.store.render(receiver_ty);
                self.sink.emit(
                    Diagnostic::error(
                        codes::NULLABLE_NOT_ALLOWED,
                        format!("`{}` cannot be called on a `{rendered}`", name.name),
                    )
                    .with_primary(receiver.span, "this may be null")
                    .with_help("use `?.` to skip the call when the value is null"),
                );
                return self.store.error();
            }
            if let Some((_, class)) = self.class_of(base) {
                let Some(method) = class.method(&name.name) else {
                    let (class_name, note) = (class.name.clone(), Self::member_list(class));
                    self.sink.emit(
                        Diagnostic::error(
                            codes::UNKNOWN_MEMBER,
                            format!("`{class_name}` has no method `{}`", name.name),
                        )
                        .with_primary(name.span, "no such method")
                        .with_note(note),
                    );
                    return self.store.error();
                };
                let function = method.function;
                self.record_resolution(call.callee.id, Resolution::Method(function));
                return self.check_method_arguments(expr, call, function);
            }

            match builtins::member(&self.store, base, &name.name) {
                Some(builtin) if !builtin.is_property() => {
                    self.record_resolution(call.callee.id, Resolution::Builtin(builtin));
                    self.check_argument_count(expr.span, call.arguments.len(), 0, &name.name);
                    let result = builtin.result(&self.store);
                    self.record_type(call.callee.id, result);
                    return result;
                }
                _ if self.store.get(base).is_error() => return self.store.error(),
                _ => {
                    let rendered = self.store.render(base);
                    self.sink.emit(
                        Diagnostic::error(
                            codes::UNKNOWN_MEMBER,
                            format!("`{rendered}` has no method `{}`", name.name),
                        )
                        .with_primary(name.span, "no such method"),
                    );
                    return self.store.error();
                }
            }
        }

        let callee_ty = self.check_expr(&call.callee);
        let Type::Function { parameters, result, .. } = self.store.get(callee_ty).clone() else {
            if !self.store.get(callee_ty).is_error() {
                let rendered = self.store.render(callee_ty);
                self.sink.emit(
                    Diagnostic::error(
                        codes::NOT_CALLABLE,
                        format!("a `{rendered}` cannot be called"),
                    )
                    .with_primary(call.callee.span, "not a function"),
                );
            }
            for argument in &call.arguments {
                self.check_expr(&argument.value);
            }
            return self.store.error();
        };

        let name = match &call.callee.kind {
            ExprKind::Path(path) => path.to_dotted(),
            _ => "this function".to_string(),
        };
        self.check_argument_count(expr.span, call.arguments.len(), parameters.len(), &name);

        for (argument, expected) in call.arguments.iter().zip(&parameters) {
            let ty = self.check_expr_expecting(&argument.value, *expected);
            self.expect_assignable(ty, *expected, argument.value.span, None);
        }
        for argument in call.arguments.iter().skip(parameters.len()) {
            self.check_expr(&argument.value);
        }

        result
    }

    /// Checks the arguments of a call to a known function.
    fn check_function_arguments(
        &mut self,
        expr: &Expr,
        call: &noto_ast::CallExpr,
        function: FunctionId,
    ) -> TypeId {
        let info = &self.functions[function.0 as usize];
        let (name, result) = (info.name.clone(), info.result);
        let expected: Vec<TypeId> =
            info.parameters.iter().map(|local| self.locals[local.0 as usize].ty).collect();

        self.check_argument_count(expr.span, call.arguments.len(), expected.len(), &name);

        for (argument, expected) in call.arguments.iter().zip(&expected) {
            let found = self.check_expr_expecting(&argument.value, *expected);
            self.expect_assignable(found, *expected, argument.value.span, None);
        }
        for argument in call.arguments.iter().skip(expected.len()) {
            self.check_expr(&argument.value);
        }

        self.record_type(call.callee.id, result);
        result
    }

    /// The enum a receiver expression names, if it names one.
    fn receiver_enum(&mut self, receiver: &Expr) -> Option<crate::EnumId> {
        let id = match &receiver.kind {
            ExprKind::Path(path) => {
                let written = path.to_dotted();
                // The scope is asked first because a binding shadows the
                // enum's name: after `val Colour = 1`, `Colour.x` reads a
                // member of that value.
                match self.scopes.lookup(&written) {
                    Some(Resolution::Enum(id)) => id,
                    Some(_) => return None,
                    None => self.lookup_enum(&written)?,
                }
            }
            // `paint.Colour.Red`: the case's receiver is itself an enum
            // reached through an imported namespace.
            ExprKind::Member { receiver: inner, name, safe: false } => {
                let module = self.receiver_module(inner)?;
                self.export_enum(module, &name.name)?
            }
            _ => return None,
        };

        self.record_resolution(receiver.id, Resolution::Enum(id));
        Some(id)
    }

    /// Checks `Color.Red`.
    fn check_enum_case(
        &mut self,
        expr: &Expr,
        id: crate::EnumId,
        name: &noto_ast::Ident,
    ) -> TypeId {
        let info = &self.enums[id.0 as usize];
        let (enum_name, ty) = (info.name.clone(), info.ty);
        let Some((index, _)) = info.case(&name.name) else {
            let note = Self::case_list(info);
            self.sink.emit(
                Diagnostic::error(
                    codes::UNKNOWN_MEMBER,
                    format!("`{enum_name}` has no case `{}`", name.name),
                )
                .with_primary(name.span, "no such case")
                .with_note(note),
            );
            return self.store.error();
        };
        self.record_resolution(expr.id, Resolution::EnumCase { enum_id: id, index });
        ty
    }

    /// Checks `Shape.Circle(3)`: a case applied to one value per field.
    fn check_case_construction(
        &mut self,
        expr: &Expr,
        call: &noto_ast::CallExpr,
        id: crate::EnumId,
        name: &noto_ast::Ident,
    ) -> TypeId {
        let info = &self.enums[id.0 as usize];
        let (enum_name, ty) = (info.name.clone(), info.ty);
        let Some((index, case)) = info.case(&name.name) else {
            let note = Self::case_list(info);
            self.sink.emit(
                Diagnostic::error(
                    codes::UNKNOWN_MEMBER,
                    format!("`{enum_name}` has no case `{}`", name.name),
                )
                .with_primary(name.span, "no such case")
                .with_note(note),
            );
            for argument in &call.arguments {
                self.check_expr(&argument.value);
            }
            return self.store.error();
        };

        let case_name = case.name.clone();
        let fields: Vec<(String, TypeId, Span)> = case
            .fields
            .iter()
            .map(|field| (field.name.clone(), field.ty, field.span))
            .collect();

        if fields.is_empty() {
            self.sink.emit(
                Diagnostic::error(
                    codes::ARITY_MISMATCH,
                    format!("`{enum_name}.{case_name}` carries no data"),
                )
                .with_primary(expr.span, "nothing to pass here")
                .with_help(format!("write it as `{enum_name}.{case_name}`")),
            );
            for argument in &call.arguments {
                self.check_expr(&argument.value);
            }
            return ty;
        }

        self.check_argument_count(
            expr.span,
            call.arguments.len(),
            fields.len(),
            &format!("{enum_name}.{case_name}"),
        );

        for (argument, (field, expected, declared_at)) in call.arguments.iter().zip(&fields) {
            let found = self.check_expr_expecting(&argument.value, *expected);
            if !self.store.is_assignable(found, *expected) {
                let (found, expected) = (self.store.render(found), self.store.render(*expected));
                self.sink.emit(
                    Diagnostic::error(
                        codes::TYPE_MISMATCH,
                        format!("`{enum_name}.{case_name}.{field}` is a `{expected}`"),
                    )
                    .with_primary(argument.value.span, format!("this is a `{found}`"))
                    .with_secondary(*declared_at, "declared here"),
                );
            }
        }
        for argument in call.arguments.iter().skip(fields.len()) {
            self.check_expr(&argument.value);
        }

        self.record_resolution(call.callee.id, Resolution::EnumCase { enum_id: id, index });
        self.record_resolution(expr.id, Resolution::EnumCase { enum_id: id, index });
        self.record_type(call.callee.id, ty);
        ty
    }

    /// Lists an enum's cases for a diagnostic.
    fn case_list(info: &crate::EnumInfo) -> String {
        if info.cases.is_empty() {
            return format!("`{}` has no cases", info.name);
        }
        let names: Vec<String> =
            info.cases.iter().map(|case| format!("`{}`", case.name)).collect();
        format!("`{}` has {}", info.name, names.join(", "))
    }

    /// The module a receiver expression names, if it names one.
    ///
    /// Only a bare name can be a namespace: a module is not a value, so it
    /// cannot come out of a call or a field.
    fn receiver_module(&mut self, receiver: &Expr) -> Option<crate::ModuleId> {
        let ExprKind::Path(path) = &receiver.kind else { return None };
        match self.lookup_value(&path.to_dotted()) {
            Some(Resolution::Module(id)) => {
                // Recorded so that tooling — the unused-import lint, a
                // language server's go-to-definition — can see which import a
                // qualified name went through.
                self.record_resolution(receiver.id, Resolution::Module(id));
                Some(id)
            }
            _ => None,
        }
    }

    /// Checks `module.name` used as a value.
    fn check_qualified_name(
        &mut self,
        expr: &Expr,
        module: crate::ModuleId,
        name: &noto_ast::Ident,
    ) -> TypeId {
        match self.export_value(module, &name.name) {
            Some(Resolution::Const(id)) => {
                self.record_resolution(expr.id, Resolution::Const(id));
                self.constants[id.0 as usize].ty
            }
            Some(Resolution::Function(id)) => {
                self.record_resolution(expr.id, Resolution::Function(id));
                let info = &self.functions[id.0 as usize];
                let parameters: Vec<TypeId> =
                    info.parameters.iter().map(|local| self.locals[local.0 as usize].ty).collect();
                let (result, is_async) = (info.result, info.is_async);
                self.store.intern(Type::Function { parameters, result, is_async })
            }
            Some(Resolution::Enum(_)) => {
                let path = self.modules[module.0 as usize].clone();
                self.sink.emit(
                    Diagnostic::error(
                        codes::UNKNOWN_NAME,
                        format!("`{}` is an enum, not a value", name.name),
                    )
                    .with_primary(name.span, "an enum names its cases, it is not one")
                    .with_help(format!(
                        "name a case: `{path}.{}.SomeCase`",
                        name.name
                    )),
                );
                self.store.error()
            }
            _ => {
                self.report_missing_export(module, name);
                self.store.error()
            }
        }
    }

    /// Checks `module.name(...)`.
    fn check_qualified_call(
        &mut self,
        expr: &Expr,
        call: &noto_ast::CallExpr,
        module: crate::ModuleId,
        name: &noto_ast::Ident,
    ) -> TypeId {
        match self.export_value(module, &name.name) {
            Some(Resolution::Class(id)) => {
                self.record_resolution(call.callee.id, Resolution::Class(id));
                self.check_construction_of(expr, call, id)
            }
            Some(Resolution::Function(id)) => {
                self.record_resolution(call.callee.id, Resolution::Function(id));
                self.check_function_arguments(expr, call, id)
            }
            _ => {
                for argument in &call.arguments {
                    self.check_expr(&argument.value);
                }
                self.report_missing_export(module, name);
                self.store.error()
            }
        }
    }

    /// Reports a name a module does not export, saying which of the two
    /// reasons it is: it is not there, or it is not exported.
    fn report_missing_export(&mut self, module: crate::ModuleId, name: &noto_ast::Ident) {
        let index = module.0 as usize;
        let path = self.modules[index].clone();
        let declared = self.module_names[index].contains_key(&name.name)
            || self.module_types[index].contains_key(&name.name);

        let diagnostic = if declared {
            Diagnostic::error(
                codes::UNKNOWN_NAME,
                format!("`{}` is private to `{path}`", name.name),
            )
            .with_primary(name.span, "declared there, but not exported")
            .with_help(format!("write `export` on its declaration in `{path}`"))
        } else {
            Diagnostic::error(
                codes::UNKNOWN_NAME,
                format!("`{path}` declares no `{}`", name.name),
            )
            .with_primary(name.span, "not declared there")
        };
        self.sink.emit(diagnostic);
    }

    /// Checks the arguments of `receiver.method(...)`.
    ///
    /// The receiver is already checked and occupies the first parameter, so
    /// only the written arguments are matched against what follows it.
    fn check_method_arguments(
        &mut self,
        expr: &Expr,
        call: &noto_ast::CallExpr,
        function: FunctionId,
    ) -> TypeId {
        let info = &self.functions[function.0 as usize];
        let (name, result) = (info.name.clone(), info.result);
        let expected: Vec<TypeId> = info
            .parameters
            .iter()
            .skip(1)
            .map(|local| self.locals[local.0 as usize].ty)
            .collect();

        self.check_argument_count(expr.span, call.arguments.len(), expected.len(), &name);

        for (argument, expected) in call.arguments.iter().zip(&expected) {
            let found = self.check_expr_expecting(&argument.value, *expected);
            self.expect_assignable(found, *expected, argument.value.span, None);
        }
        for argument in call.arguments.iter().skip(expected.len()) {
            self.check_expr(&argument.value);
        }

        self.record_type(call.callee.id, result);
        result
    }

    /// Lists what a class does have, for a diagnostic about what it does not.
    fn member_list(class: &crate::analysis::ClassInfo) -> String {
        let mut names: Vec<String> =
            class.methods.iter().map(|method| format!("`{}`", method.name)).collect();
        if names.is_empty() {
            return format!("`{}` has no methods", class.name);
        }
        names.sort();
        format!("`{}` has {}", class.name, names.join(", "))
    }

    /// Lists a class's fields for a diagnostic note.
    fn field_list(class: &crate::analysis::ClassInfo) -> String {
        if class.fields.is_empty() {
            return format!("`{}` has no fields", class.name);
        }
        let names: Vec<String> =
            class.fields.iter().map(|field| format!("`{}`", field.name)).collect();
        format!("`{}` has {}", class.name, names.join(", "))
    }

    /// Checks `Point(1, 2)`: a class name applied to one value per field.
    ///
    /// A class has exactly one constructor, its field list, so the check is
    /// the same arity and assignability check a function call gets. The field
    /// each argument initialises is named in the mismatch, because `expected
    /// `Int`, found `String`` on its own does not say which of three `Int`
    /// fields was meant.
    fn check_construction(
        &mut self,
        expr: &Expr,
        call: &noto_ast::CallExpr,
        id: crate::analysis::ClassId,
    ) -> TypeId {
        self.record_resolution(call.callee.id, Resolution::Class(id));
        self.check_construction_of(expr, call, id)
    }

    /// Checks a construction whose callee is already resolved.
    fn check_construction_of(
        &mut self,
        expr: &Expr,
        call: &noto_ast::CallExpr,
        id: crate::analysis::ClassId,
    ) -> TypeId {
        self.record_resolution(expr.id, Resolution::Class(id));

        let class = &self.classes[id.0 as usize];
        let (name, ty) = (class.name.clone(), class.ty);
        let fields: Vec<(String, TypeId, Span)> = class
            .fields
            .iter()
            .map(|field| (field.name.clone(), field.ty, field.span))
            .collect();

        self.check_argument_count(expr.span, call.arguments.len(), fields.len(), &name);

        for (argument, (field, expected, declared_at)) in call.arguments.iter().zip(&fields) {
            let found = self.check_expr_expecting(&argument.value, *expected);
            if !self.store.is_assignable(found, *expected) {
                let (found, expected) = (self.store.render(found), self.store.render(*expected));
                self.sink.emit(
                    Diagnostic::error(
                        codes::TYPE_MISMATCH,
                        format!("`{name}.{field}` is a `{expected}`"),
                    )
                    .with_primary(argument.value.span, format!("this is a `{found}`"))
                    .with_secondary(*declared_at, "declared here"),
                );
            }
        }
        for argument in call.arguments.iter().skip(fields.len()) {
            self.check_expr(&argument.value);
        }

        self.record_type(call.callee.id, ty);
        ty
    }

    fn check_argument_count(&mut self, span: Span, found: usize, expected: usize, name: &str) {
        if found == expected {
            return;
        }
        let plural = if expected == 1 { "" } else { "s" };
        self.sink.emit(
            Diagnostic::error(
                codes::ARITY_MISMATCH,
                format!("`{name}` takes {expected} argument{plural}, but {found} were given"),
            )
            .with_primary(span, format!("expected {expected}, found {found}")),
        );
    }

    /// Resolves a call of a built-in function by its argument types.
    fn check_builtin_call(
        &mut self,
        expr: &Expr,
        call: &noto_ast::CallExpr,
        name: &str,
        overloads: &[builtins::Builtin],
    ) -> TypeId {
        let argument_types: Vec<TypeId> =
            call.arguments.iter().map(|argument| self.check_expr(&argument.value)).collect();

        let matching = overloads.iter().find(|builtin| {
            let parameters = builtin.parameters(&self.store);
            parameters.len() == argument_types.len()
                && parameters
                    .iter()
                    .zip(&argument_types)
                    .all(|(expected, found)| self.store.is_assignable(*found, *expected))
        });

        match matching {
            Some(builtin) => {
                self.record_resolution(call.callee.id, Resolution::Builtin(*builtin));
                let result = builtin.result(&self.store);
                self.record_type(call.callee.id, result);
                result
            }
            None => {
                if argument_types.iter().any(|ty| self.store.get(*ty).is_error()) {
                    return self.store.error();
                }
                let found: Vec<String> =
                    argument_types.iter().map(|ty| self.store.render(*ty)).collect();
                let mut accepted: Vec<String> = overloads
                    .iter()
                    .map(|builtin| {
                        let parameters: Vec<String> = builtin
                            .parameters(&self.store)
                            .iter()
                            .map(|ty| self.store.render(*ty))
                            .collect();
                        format!("{name}({})", parameters.join(", "))
                    })
                    .collect();
                accepted.sort();
                accepted.dedup();

                self.sink.emit(
                    Diagnostic::error(
                        codes::TYPE_MISMATCH,
                        format!("no version of `{name}` accepts ({})", found.join(", ")),
                    )
                    .with_primary(expr.span, "no matching overload")
                    .with_note(format!("`{name}` accepts: {}", accepted.join(", ")))
                    .with_help("convert the value first, as in `value.toString()`"),
                );
                self.store.error()
            }
        }
    }

    fn check_member(
        &mut self,
        expr: &Expr,
        receiver: &Expr,
        name: &noto_ast::Ident,
        safe: bool,
    ) -> TypeId {
        // `util.LIMIT` reads through an imported namespace rather than
        // through a value, so the receiver is never evaluated.
        if let Some(module) = self.receiver_module(receiver) {
            return self.check_qualified_name(expr, module, name);
        }

        // `Color.Red` names a case. The enum is a type, not a value, so the
        // receiver is never evaluated here either.
        if let Some(id) = self.receiver_enum(receiver) {
            return self.check_enum_case(expr, id, name);
        }

        let receiver_ty = self.check_expr(receiver);
        let base = self.store.unwrap_nullable(receiver_ty);

        if self.store.get(base).is_error() {
            return self.store.error();
        }

        if self.store.is_nullable(receiver_ty) && !safe {
            let rendered = self.store.render(receiver_ty);
            self.sink.emit(
                Diagnostic::error(
                    codes::NULLABLE_NOT_ALLOWED,
                    format!("`{}` cannot be read from a `{rendered}`", name.name),
                )
                .with_primary(receiver.span, "this may be null")
                .with_help(format!("use `?.{}` to get null instead when it is", name.name)),
            );
            return self.store.error();
        }

        if let Some((id, class)) = self.class_of(base) {
            let Some((index, field)) = class.field(&name.name) else {
                let class_name = class.name.clone();
                let mut diagnostic = if class.method(&name.name).is_some() {
                    Diagnostic::error(
                        codes::UNKNOWN_MEMBER,
                        format!("`{}` is a method of `{class_name}`", name.name),
                    )
                    .with_primary(name.span, "a method is not a value")
                    .with_help(format!("call it: `{}()`", name.name))
                } else {
                    Diagnostic::error(
                        codes::UNKNOWN_MEMBER,
                        format!("`{class_name}` has no field `{}`", name.name),
                    )
                    .with_primary(name.span, "no such field")
                };
                if class.method(&name.name).is_none() {
                    diagnostic = diagnostic.with_note(Self::field_list(class));
                }
                self.sink.emit(diagnostic);
                return self.store.error();
            };
            let ty = field.ty;

            if safe {
                self.sink.emit(
                    Diagnostic::error(
                        codes::UNSUPPORTED_CONSTRUCT,
                        "safe field access is not supported by this compiler yet",
                    )
                    .with_primary(expr.span, "not implemented in Noto 0.5"),
                );
                return self.store.error();
            }

            self.record_resolution(expr.id, Resolution::Field { class: id, index });
            return ty;
        }

        match builtins::member(&self.store, base, &name.name) {
            Some(builtin) if builtin.is_property() => {
                self.record_resolution(expr.id, Resolution::Builtin(builtin));
                let result = builtin.result(&self.store);
                if safe {
                    self.store.nullable(result)
                } else {
                    result
                }
            }
            _ => {
                let rendered = self.store.render(base);
                self.sink.emit(
                    Diagnostic::error(
                        codes::UNKNOWN_MEMBER,
                        format!("`{rendered}` has no member `{}`", name.name),
                    )
                    .with_primary(name.span, "no such member"),
                );
                self.store.error()
            }
        }
    }

    /// Reports a mismatch when `found` cannot be used where `expected` is
    /// required.
    fn expect_assignable(
        &mut self,
        found: TypeId,
        expected: TypeId,
        span: Span,
        declared_at: Option<Span>,
    ) {
        if self.store.is_assignable(found, expected) {
            return;
        }

        let (expected_name, found_name) =
            (self.store.render(expected), self.store.render(found));
        let mut diagnostic = Diagnostic::error(
            codes::TYPE_MISMATCH,
            format!("expected `{expected_name}`, found `{found_name}`"),
        )
        .with_primary(span, format!("this is a `{found_name}`"));

        if let Some(declared_at) = declared_at {
            diagnostic =
                diagnostic.with_secondary(declared_at, format!("expected `{expected_name}` here"));
        }

        // The two mistakes worth spelling out: a missing conversion, and a
        // nullable value where a plain one is needed.
        if let (Some(from), Some(to)) =
            (self.store.get(found).as_primitive(), self.store.get(expected).as_primitive())
        {
            if from.widens_to(to) {
                diagnostic =
                    diagnostic.with_help(format!("convert it with `.to{}()`", to.name()));
            }
        }
        if self.store.is_nullable(found) && self.store.unwrap_nullable(found) == expected {
            diagnostic = diagnostic
                .with_help("supply a default with `?:`, or check for null before using the value");
        }

        self.sink.emit(diagnostic);
    }
}

/// The primitive a numeric literal suffix names.
fn suffix_primitive(suffix: noto_ast::NumericSuffix) -> Primitive {
    use noto_ast::NumericSuffix::*;
    match suffix {
        I8 => Primitive::Int8,
        I16 => Primitive::Int16,
        I32 => Primitive::Int32,
        I64 => Primitive::Int64,
        U8 => Primitive::UInt8,
        U16 => Primitive::UInt16,
        U32 => Primitive::UInt32,
        U64 => Primitive::UInt64,
        F32 => Primitive::Float32,
        F64 => Primitive::Float64,
    }
}

/// The largest edit distance at which a name is still worth suggesting.
fn max_distance(name: &str) -> usize {
    match name.len() {
        0..=3 => 1,
        4..=7 => 2,
        _ => 3,
    }
}

/// Levenshtein distance between two names.
fn edit_distance(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let mut previous: Vec<usize> = (0..=b.len()).collect();
    let mut current = vec![0usize; b.len() + 1];

    for (i, a_char) in a.iter().enumerate() {
        current[0] = i + 1;
        for (j, b_char) in b.iter().enumerate() {
            let cost = usize::from(a_char != b_char);
            current[j + 1] =
                (current[j] + 1).min(previous[j + 1] + 1).min(previous[j] + cost);
        }
        std::mem::swap(&mut previous, &mut current);
    }

    previous[b.len()]
}
