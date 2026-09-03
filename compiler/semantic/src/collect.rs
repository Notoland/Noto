//! The first pass: collecting declaration signatures.
//!
//! Every top-level signature is recorded before any body is checked, so
//! declaration order inside a file never matters.

use crate::analysis::{
    ClassId, ClassInfo, ConstId, ConstInfo, ConstValue, FieldInfo, FunctionId, FunctionInfo,
    MethodInfo, Resolution,
};
use crate::Checker;
use noto_ast::{ClassKind, FnItem, Item, ItemKind, Module, TypeDeclItem, TypeExpr, TypeExprKind};
use noto_diagnostics::{codes, Diagnostic};
use noto_span::Span;
use noto_types::{Primitive, Type, TypeId};

impl Checker<'_> {
    /// Records the signature of every top-level declaration.
    /// Registers every class name in one module.
    ///
    /// This runs for every module before any signature is collected, so a
    /// field, a parameter or a result type can name a class declared further
    /// down the file or in another module entirely.
    pub(crate) fn declare_classes(&mut self, module: &Module) {
        for item in &module.items {
            if let ItemKind::TypeDecl(decl) = &item.kind {
                self.declare_class(item, decl);
            }
        }
    }

    pub(crate) fn collect_items(&mut self, module: &Module) {
        for item in &module.items {
            match &item.kind {
                ItemKind::Fn(function) => self.collect_fn(item, function),
                ItemKind::Const(constant) => self.collect_const(item, constant),
                ItemKind::Test(test) => self.collect_test(test),
                ItemKind::TypeDecl(decl) => self.collect_class_fields(decl),
                // Interfaces, enums and imports parse today but are not yet
                // given semantics; `noto check` reports them rather than
                // silently accepting a program it cannot compile.
                ItemKind::Interface(_) | ItemKind::Enum(_) => self.report_unsupported(item),
                // Imports are resolved by the driver and checked separately;
                // there is no signature to collect from one.
                ItemKind::Import(_) => {}
                ItemKind::Error => {}
            }
        }
    }

    /// Registers a class name, or reports why the declaration cannot be one.
    ///
    /// Only `class` is implemented. `struct` and the `data` flavours promise
    /// value semantics — copied on assignment, compared by contents — and an
    /// object here is a pointer into a heap that never frees. Accepting them
    /// would mean giving them reference semantics under a keyword that says
    /// otherwise, so they wait for the memory model in RFC 0001.
    fn declare_class(&mut self, item: &Item, decl: &TypeDeclItem) {
        let unsupported = |what: &str, span| {
            Diagnostic::error(
                codes::UNSUPPORTED_CONSTRUCT,
                format!("{what} are not supported by this compiler yet"),
            )
            .with_primary(span, "not implemented in Noto 0.4")
        };

        if decl.class_kind != ClassKind::Class {
            self.sink.emit(
                Diagnostic::error(
                    codes::UNSUPPORTED_CONSTRUCT,
                    format!("`{}` declarations are not supported by this compiler yet", decl.class_kind.as_str()),
                )
                .with_primary(item.span, "not implemented in Noto 0.4")
                .with_note("a value type is copied on assignment, which needs the memory model")
                .with_help("declare it as a `class` for now: an object is a reference"),
            );
            return;
        }
        if let Some(param) = decl.type_params.first() {
            self.sink.emit(unsupported("generic types", param.span));
            return;
        }
        if let Some(base) = decl.base.as_ref().or(decl.interfaces.first()) {
            self.sink.emit(unsupported("base classes and interfaces", base.span));
            return;
        }
        if let Some(property) = decl.properties.first() {
            self.sink.emit(unsupported("properties", property.span));
            return;
        }
        if let Some(field) = decl.fields.first() {
            self.sink.emit(
                unsupported("fields declared in the class body", field.span).with_help(
                    "declare them as constructor parameters: `class Point(val x: Int)`",
                ),
            );
            return;
        }

        let name = decl.name.name.clone();
        if let Some(existing) = self.own_type(&name) {
            let previous = self.classes[existing.0 as usize].span;
            self.sink.emit(
                Diagnostic::error(
                    codes::DUPLICATE_NAME,
                    format!("`{name}` is declared more than once"),
                )
                .with_primary(decl.name.span, "redeclared here")
                .with_secondary(previous, "first declared here"),
            );
            return;
        }

        let id = ClassId(self.classes.len() as u32);
        let qualified = self.qualify(&name);
        let def = self.store.declare(qualified);
        let ty = self.store.intern(Type::Named { def, arguments: Vec::new() });
        self.classes.push(ClassInfo {
            name: name.clone(),
            module: self.current_module,
            is_exported: item.modifiers.is_exported,
            fields: Vec::new(),
            methods: Vec::new(),
            ty,
            def,
            span: item.span,
        });
        let module = self.current_module.0 as usize;
        self.module_types[module].insert(name.clone(), id);
        if item.modifiers.is_exported {
            self.exported[module].insert(name.clone());
        }
        // The name is also a value: it is the constructor. Classes are
        // declared before any scope is open, so this is recorded directly.
        self.module_names[module].insert(name, Resolution::Class(id));
    }

    /// Resolves the field types of a class whose name is already registered.
    fn collect_class_fields(&mut self, decl: &TypeDeclItem) {
        let Some(id) = self.own_type(&decl.name.name) else {
            // `declare_class` reported why this declaration is not a class.
            return;
        };

        let mut fields: Vec<FieldInfo> = Vec::new();
        for param in &decl.primary_params {
            if let Some(default) = &param.default {
                self.sink.emit(
                    Diagnostic::error(
                        codes::UNSUPPORTED_CONSTRUCT,
                        "default values for fields are not supported by this compiler yet",
                    )
                    .with_primary(default.span, "not implemented in Noto 0.4"),
                );
            }

            let ty = match &param.ty {
                Some(ty) => self.resolve_type(ty),
                None => {
                    // A field has no initialiser to infer from, so the type is
                    // not optional the way a `val` in a body is.
                    self.sink.emit(
                        Diagnostic::error(
                            codes::CANNOT_INFER,
                            format!("field `{}` needs a declared type", param.name.name),
                        )
                        .with_primary(param.span, "no type to infer from here")
                        .with_help(format!("write `{}: Int`, or whatever it holds", param.name.name)),
                    );
                    self.store.error()
                }
            };

            let name = param.name.name.clone();
            if let Some(previous) = fields.iter().find(|field| field.name == name) {
                let previous = previous.span;
                self.sink.emit(
                    Diagnostic::error(
                        codes::DUPLICATE_NAME,
                        format!("`{name}` is declared more than once in `{}`", decl.name.name),
                    )
                    .with_primary(param.name.span, "redeclared here")
                    .with_secondary(previous, "first declared here"),
                );
                continue;
            }

            fields.push(FieldInfo {
                name,
                ty,
                is_mutable: param.kind == noto_ast::LetKind::Var,
                span: param.span,
            });
        }

        self.classes[id.0 as usize].fields = fields;

        for item in &decl.methods {
            let ItemKind::Fn(function) = &item.kind else { continue };
            self.collect_method(id, item, function);
        }
    }

    /// Records a method's signature as a function taking the receiver first.
    ///
    /// A method is an ordinary function with one extra parameter, so calls,
    /// bodies, lowering and the calling convention all work on it unchanged.
    /// The name is mangled with the class — an identifier cannot contain a
    /// `.`, so `Point.distance` cannot collide with a free function, and it
    /// is what a diagnostic wants to print anyway.
    fn collect_method(&mut self, class: ClassId, item: &Item, function: &FnItem) {
        if function.receiver.is_some() {
            self.sink.emit(
                Diagnostic::error(
                    codes::UNSUPPORTED_CONSTRUCT,
                    "a method already has a receiver",
                )
                .with_primary(item.span, "remove the explicit receiver"),
            );
            return;
        }
        if !function.type_params.is_empty() {
            self.sink.emit(
                Diagnostic::error(
                    codes::UNSUPPORTED_CONSTRUCT,
                    "generic methods are not supported by this compiler yet",
                )
                .with_primary(function.type_params[0].span, "not implemented in Noto 0.4"),
            );
            return;
        }

        let class_name = self.classes[class.0 as usize].name.clone();
        let receiver_ty = self.classes[class.0 as usize].ty;
        let short = function.name.name.clone();

        if self.classes[class.0 as usize].method(&short).is_some() {
            self.sink.emit(
                Diagnostic::error(
                    codes::DUPLICATE_NAME,
                    format!("`{class_name}.{short}` is declared more than once"),
                )
                .with_primary(function.name.span, "redeclared here"),
            );
            return;
        }
        if let Some((_, field)) = self.classes[class.0 as usize].field(&short) {
            let declared_at = field.span;
            self.sink.emit(
                Diagnostic::error(
                    codes::DUPLICATE_NAME,
                    format!("`{class_name}` already has a field named `{short}`"),
                )
                .with_primary(function.name.span, "a method cannot share its name")
                .with_secondary(declared_at, "the field is declared here"),
            );
            return;
        }

        let id = FunctionId(self.functions.len() as u32);
        let result = match &function.result {
            Some(ty) => self.resolve_type(ty),
            None => self.store.unit(),
        };

        self.functions.push(FunctionInfo {
            name: format!("{class_name}.{short}"),
            module: self.current_module,
            // A method follows its class: exporting the class exports them.
            is_exported: self.classes[class.0 as usize].is_exported,
            parameters: Vec::new(),
            result,
            locals: Vec::new(),
            body: function.body.as_ref().map(|body| body.id),
            is_async: function.is_async,
            span: item.span,
        });

        let previous_function = self.current_function.replace(id);
        self.scopes.push();
        let receiver = self.declare_local(
            crate::RECEIVER_NAME,
            receiver_ty,
            false,
            true,
            function.name.span,
        );
        self.functions[id.0 as usize].parameters.push(receiver);
        for param in &function.params {
            let ty = match &param.ty {
                Some(ty) => self.resolve_type(ty),
                None => self.store.error(),
            };
            let local = self.declare_local(&param.name.name, ty, false, true, param.span);
            self.functions[id.0 as usize].parameters.push(local);
        }
        self.scopes.pop();
        self.current_function = previous_function;

        self.classes[class.0 as usize].methods.push(MethodInfo { name: short, function: id });
    }

    fn report_unsupported(&mut self, item: &Item) {
        let name = item.describe();
        self.sink.emit(
            Diagnostic::error(
                codes::UNSUPPORTED_CONSTRUCT,
                format!("`{name}` declarations are not supported by this compiler yet"),
            )
            .with_primary(item.span, "not implemented in Noto 0.4")
            .with_note(
                "the syntax is accepted so that tooling can read the whole language; \
                 code generation for it lands in a later release",
            ),
        );
    }

    fn collect_fn(&mut self, item: &Item, function: &FnItem) {
        if let Some(receiver) = &function.receiver {
            self.sink.emit(
                Diagnostic::error(
                    codes::UNSUPPORTED_CONSTRUCT,
                    "extension functions are not supported by this compiler yet",
                )
                .with_primary(receiver.span, "not implemented in Noto 0.4"),
            );
            return;
        }
        if !function.type_params.is_empty() {
            self.sink.emit(
                Diagnostic::error(
                    codes::UNSUPPORTED_CONSTRUCT,
                    "generic functions are not supported by this compiler yet",
                )
                .with_primary(function.type_params[0].span, "not implemented in Noto 0.4"),
            );
            return;
        }

        let name = function.name.name.clone();
        if let Some(existing) = self.lookup_function(&name) {
            let previous = self.functions[existing.0 as usize].span;
            self.sink.emit(
                Diagnostic::error(
                    codes::DUPLICATE_NAME,
                    format!("`{name}` is declared more than once"),
                )
                .with_primary(function.name.span, "redeclared here")
                .with_secondary(previous, "first declared here")
                .with_note("function overloading is not supported by this compiler yet"),
            );
            return;
        }

        let id = FunctionId(self.functions.len() as u32);
        let result = match &function.result {
            Some(ty) => self.resolve_type(ty),
            None => self.store.unit(),
        };

        self.functions.push(FunctionInfo {
            name: name.clone(),
            module: self.current_module,
            is_exported: item.modifiers.is_exported,
            parameters: Vec::new(),
            result,
            locals: Vec::new(),
            body: function.body.as_ref().map(|body| body.id),
            is_async: function.is_async,
            span: item.span,
        });

        // Parameters are declared into the function's own scope during the
        // second pass; their types are resolved now so calls can be checked
        // before the body is.
        let previous_function = self.current_function.replace(id);
        self.scopes.push();
        for param in &function.params {
            let ty = match &param.ty {
                Some(ty) => self.resolve_type(ty),
                None => self.store.error(),
            };
            let local = self.declare_local(&param.name.name, ty, false, true, param.span);
            self.functions[id.0 as usize].parameters.push(local);
        }
        self.scopes.pop();
        self.current_function = previous_function;

        self.scopes.declare(name.clone(), Resolution::Function(id));
        if item.modifiers.is_exported {
            self.exported[self.current_module.0 as usize].insert(name.clone());
        }
        // Only the root module's `main` is the program's entry point.
        if name == "main" && self.current_module == crate::ModuleId::ROOT {
            self.entry = Some(id);
        }
    }

    /// Checks that `main` has a signature the runtime can call.
    pub(crate) fn check_entry_signature(&mut self, entry: FunctionId) {
        let function = &self.functions[entry.0 as usize];
        let span = function.span;
        let takes_arguments = !function.parameters.is_empty();
        let result = function.result;
        let is_async = function.is_async;

        if takes_arguments {
            self.sink.emit(
                Diagnostic::error(
                    codes::TYPE_MISMATCH,
                    "`main` must not take any parameters",
                )
                .with_primary(span, "declared with parameters")
                .with_help("read command line arguments with `std.env.arguments()`"),
            );
        }

        let unit = self.store.unit();
        let int = self.store.int();
        if result != unit && result != int {
            let rendered = self.store.render(result);
            self.sink.emit(
                Diagnostic::error(
                    codes::TYPE_MISMATCH,
                    format!("`main` must return `Unit` or `Int`, not `{rendered}`"),
                )
                .with_primary(span, "unsupported result type")
                .with_note("the value `main` returns becomes the process exit status"),
            );
        }

        if is_async {
            self.sink.emit(
                Diagnostic::error(
                    codes::UNSUPPORTED_CONSTRUCT,
                    "`main` cannot be `async` in this compiler yet",
                )
                .with_primary(span, "not implemented in Noto 0.4"),
            );
        }
    }

    fn collect_const(&mut self, item: &Item, constant: &noto_ast::ConstItem) {
        let value = self.fold_const(&constant.value);
        let inferred = match &value {
            ConstValue::Int(_) => self.store.int(),
            ConstValue::Bool(_) => self.store.bool(),
            ConstValue::Str(_) => self.store.string(),
            ConstValue::Char(_) => self.store.char(),
            ConstValue::Error => self.store.error(),
        };

        let ty = match &constant.ty {
            Some(declared) => {
                let declared_ty = self.resolve_type(declared);
                if !self.store.is_assignable(inferred, declared_ty) {
                    let (found, expected) =
                        (self.store.render(inferred), self.store.render(declared_ty));
                    self.sink.emit(
                        Diagnostic::error(
                            codes::TYPE_MISMATCH,
                            format!("expected `{expected}`, found `{found}`"),
                        )
                        .with_primary(constant.value.span, format!("this is a `{found}`"))
                        .with_secondary(declared.span, format!("declared as `{expected}` here")),
                    );
                }
                declared_ty
            }
            None => inferred,
        };

        let id = ConstId(self.constants.len() as u32);
        self.constants.push(ConstInfo {
            name: constant.name.name.clone(),
            module: self.current_module,
            is_exported: item.modifiers.is_exported,
            ty,
            value,
            span: item.span,
        });
        if item.modifiers.is_exported {
            self.exported[self.current_module.0 as usize]
                .insert(constant.name.name.clone());
        }
        self.scopes.declare(constant.name.name.clone(), Resolution::Const(id));
    }

    /// Registers a test body as a function so it can be checked and lowered
    /// like any other.
    fn collect_test(&mut self, test: &noto_ast::TestItem) {
        let id = FunctionId(self.functions.len() as u32);
        let unit = self.store.unit();
        self.functions.push(FunctionInfo {
            // The mangled name keeps tests out of the ordinary namespace while
            // staying readable in a stack trace.
            name: format!("test${}", test.name),
            module: self.current_module,
            is_exported: false,
            parameters: Vec::new(),
            result: unit,
            locals: Vec::new(),
            body: Some(test.body.id),
            is_async: false,
            span: test.name_span,
        });
        self.tests.push(crate::analysis::TestInfo {
            name: test.name.clone(),
            function: id,
            span: test.name_span,
        });
    }

    /// Finds a function this module already declares under `name`.
    ///
    /// Two modules may each declare a `helper`; only a repeat inside one
    /// module is a redeclaration.
    fn lookup_function(&self, name: &str) -> Option<FunctionId> {
        self.functions
            .iter()
            .position(|function| {
                function.module == self.current_module && function.name == name
            })
            .map(|index| FunctionId(index as u32))
    }

    /// Evaluates a constant expression at compile time.
    ///
    /// Only the forms a `const` may use are folded; anything else is reported
    /// rather than deferred to runtime, because a `const` must be a value the
    /// compiler can put in the executable.
    pub(crate) fn fold_const(&mut self, expr: &noto_ast::Expr) -> ConstValue {
        use noto_ast::{ExprKind, Literal, StringSegment, UnaryOp};

        match &expr.kind {
            ExprKind::Literal(Literal::Int { value, .. }) => ConstValue::Int(*value as i128),
            ExprKind::Literal(Literal::Bool(value)) => ConstValue::Bool(*value),
            ExprKind::Literal(Literal::Char(value)) => ConstValue::Char(*value),
            ExprKind::Literal(Literal::Str(segments)) => match segments.as_slice() {
                [StringSegment::Text(text)] => ConstValue::Str(text.clone()),
                [] => ConstValue::Str(String::new()),
                _ => {
                    self.sink.emit(
                        Diagnostic::error(
                            codes::UNSUPPORTED_CONSTRUCT,
                            "a constant cannot use string interpolation",
                        )
                        .with_primary(expr.span, "this depends on a runtime value")
                        .with_help("build the text where it is used, or use a plain literal"),
                    );
                    ConstValue::Error
                }
            },
            ExprKind::Unary { op: UnaryOp::Neg, operand } => match self.fold_const(operand) {
                ConstValue::Int(value) => ConstValue::Int(-value),
                other => other,
            },
            ExprKind::Unary { op: UnaryOp::Not, operand } => match self.fold_const(operand) {
                ConstValue::Bool(value) => ConstValue::Bool(!value),
                other => other,
            },
            ExprKind::Binary { op, left, right, op_span } => {
                let (left, right) = (self.fold_const(left), self.fold_const(right));
                self.fold_binary(*op, left, right, *op_span)
            }
            // One constant may be defined in terms of another declared above
            // it; the value is already folded, so it is simply copied.
            ExprKind::Path(path) => {
                let name = path.to_dotted();
                match self.scopes.lookup(&name) {
                    Some(Resolution::Const(id)) => self.constants[id.0 as usize].value.clone(),
                    _ => {
                        let mut diagnostic = Diagnostic::error(
                            codes::UNKNOWN_NAME,
                            format!("cannot find constant `{name}` in this scope"),
                        )
                        .with_primary(expr.span, "not a constant declared above this point");
                        if self.scopes.lookup(&name).is_some() {
                            diagnostic = diagnostic
                                .with_help("a `const` can only refer to other constants");
                        }
                        self.sink.emit(diagnostic);
                        ConstValue::Error
                    }
                }
            }
            _ => {
                self.sink.emit(
                    Diagnostic::error(
                        codes::UNSUPPORTED_CONSTRUCT,
                        "a constant must be computable at compile time",
                    )
                    .with_primary(expr.span, "this cannot be evaluated while compiling")
                    .with_help("use `val` for a value computed while the program runs"),
                );
                ConstValue::Error
            }
        }
    }

    fn fold_binary(
        &mut self,
        op: noto_ast::BinaryOp,
        left: ConstValue,
        right: ConstValue,
        span: Span,
    ) -> ConstValue {
        use noto_ast::BinaryOp::*;
        match (left, right) {
            (ConstValue::Int(a), ConstValue::Int(b)) => match op {
                Add => ConstValue::Int(a + b),
                Sub => ConstValue::Int(a - b),
                Mul => ConstValue::Int(a * b),
                Div | Rem if b == 0 => {
                    self.sink.emit(
                        Diagnostic::error(
                            codes::INVALID_OPERANDS,
                            "division by zero in a constant",
                        )
                        .with_primary(span, "the right side is zero"),
                    );
                    ConstValue::Error
                }
                Div => ConstValue::Int(a / b),
                Rem => ConstValue::Int(a % b),
                Eq => ConstValue::Bool(a == b),
                Ne => ConstValue::Bool(a != b),
                Lt => ConstValue::Bool(a < b),
                Le => ConstValue::Bool(a <= b),
                Gt => ConstValue::Bool(a > b),
                Ge => ConstValue::Bool(a >= b),
                _ => self.unsupported_const_op(op, span),
            },
            (ConstValue::Bool(a), ConstValue::Bool(b)) => match op {
                And => ConstValue::Bool(a && b),
                Or => ConstValue::Bool(a || b),
                Eq => ConstValue::Bool(a == b),
                Ne => ConstValue::Bool(a != b),
                _ => self.unsupported_const_op(op, span),
            },
            (ConstValue::Error, _) | (_, ConstValue::Error) => ConstValue::Error,
            _ => self.unsupported_const_op(op, span),
        }
    }

    fn unsupported_const_op(&mut self, op: noto_ast::BinaryOp, span: Span) -> ConstValue {
        self.sink.emit(
            Diagnostic::error(
                codes::INVALID_OPERANDS,
                format!("`{}` cannot be evaluated at compile time for these operands", op.as_str()),
            )
            .with_primary(span, "unsupported in a constant"),
        );
        ConstValue::Error
    }

    /// Turns a source type expression into an interned type.
    pub(crate) fn resolve_type(&mut self, ty: &TypeExpr) -> TypeId {
        match &ty.kind {
            TypeExprKind::Named { path, arguments } => {
                if !arguments.is_empty() {
                    self.sink.emit(
                        Diagnostic::error(
                            codes::UNSUPPORTED_CONSTRUCT,
                            "generic types are not supported by this compiler yet",
                        )
                        .with_primary(ty.span, "not implemented in Noto 0.4"),
                    );
                    return self.store.error();
                }
                self.resolve_type_name(&path.to_dotted(), ty.span)
            }
            TypeExprKind::Nullable(inner) => {
                let inner = self.resolve_type(inner);
                self.store.nullable(inner)
            }
            TypeExprKind::Tuple(items) => {
                let items: Vec<TypeId> = items.iter().map(|item| self.resolve_type(item)).collect();
                self.store.intern(Type::Tuple(items))
            }
            TypeExprKind::Function { parameters, result, is_async } => {
                let parameters: Vec<TypeId> =
                    parameters.iter().map(|param| self.resolve_type(param)).collect();
                let result = self.resolve_type(result);
                self.store.intern(Type::Function { parameters, result, is_async: *is_async })
            }
            TypeExprKind::List(_) => {
                self.sink.emit(
                    Diagnostic::error(
                        codes::UNSUPPORTED_CONSTRUCT,
                        "list types are not supported by this compiler yet",
                    )
                    .with_primary(ty.span, "not implemented in Noto 0.4"),
                );
                self.store.error()
            }
            TypeExprKind::Error => self.store.error(),
        }
    }

    /// Looks a type name up among the built-in types and declared classes.
    fn resolve_type_name(&mut self, name: &str, span: Span) -> TypeId {
        if let Some(primitive) = Primitive::from_name(name) {
            return self.store.primitive(primitive);
        }
        if let Some(id) = self.lookup_type(name) {
            return self.classes[id.0 as usize].ty;
        }
        match name {
            "String" => self.store.string(),
            "Unit" => self.store.unit(),
            "Nothing" => self.store.nothing(),
            "Any" => self.store.any(),
            _ => {
                let mut diagnostic = Diagnostic::error(
                    codes::UNKNOWN_TYPE,
                    format!("cannot find type `{name}`"),
                )
                .with_primary(span, "not a type in scope");
                if let Some(suggestion) = suggest_type_name(name) {
                    diagnostic = diagnostic.with_help(format!("did you mean `{suggestion}`?"));
                }
                self.sink.emit(diagnostic);
                self.store.error()
            }
        }
    }
}

/// Suggests a built-in type for a name that looks like a near miss.
fn suggest_type_name(name: &str) -> Option<&'static str> {
    const KNOWN: &[&str] = &[
        "Int", "Int8", "Int16", "Int32", "Int64", "UInt", "UInt8", "UInt16", "UInt32", "UInt64",
        "Float32", "Float64", "Bool", "Char", "Byte", "String", "Unit", "Nothing", "Any",
    ];
    let lowered = name.to_lowercase();
    KNOWN.iter().copied().find(|known| known.to_lowercase() == lowered)
}
