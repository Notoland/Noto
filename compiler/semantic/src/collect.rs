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
            match &item.kind {
                ItemKind::TypeDecl(decl) => self.declare_class(item, decl),
                ItemKind::Enum(decl) => self.declare_enum(item, decl),
                _ => {}
            }
        }
    }

    /// Registers an enum and its cases.
    ///
    /// A case's position in the declaration is its tag, and an enum with no
    /// associated data *is* that tag: a value of it is an `Int`, with no
    /// allocation and no indirection. Associated data will make an enum a
    /// pointer to its tag and fields, the way a class already is.
    fn declare_enum(&mut self, item: &Item, decl: &noto_ast::EnumItem) {
        let unsupported = |what: &str, span| {
            Diagnostic::error(
                codes::UNSUPPORTED_CONSTRUCT,
                format!("{what} are not supported by this compiler yet"),
            )
            .with_primary(span, "not implemented in Noto 0.12")
        };

        if let Some(param) = decl.type_params.first() {
            self.sink.emit(unsupported("generic enums", param.span));
            return;
        }
        if let Some(interface) = decl.interfaces.first() {
            self.sink.emit(unsupported("interfaces on an enum", interface.span));
            return;
        }
        if let Some(method) = decl.methods.first() {
            self.sink.emit(unsupported("methods on an enum", method.span));
            return;
        }
        if let Some(case) = decl.cases.iter().find(|case| case.value.is_some()) {
            let span = case.value.as_ref().expect("just matched").span;
            self.sink.emit(
                unsupported("explicit case values", span)
                    .with_note("a case's tag is its position in the declaration"),
            );
            return;
        }

        let name = decl.name.name.clone();
        if self.own_type(&name).is_some() || self.own_enum(&name).is_some() {
            self.sink.emit(
                Diagnostic::error(
                    codes::DUPLICATE_NAME,
                    format!("`{name}` is declared more than once"),
                )
                .with_primary(decl.name.span, "redeclared here"),
            );
            return;
        }

        let mut cases: Vec<crate::analysis::EnumCaseInfo> = Vec::new();
        for case in &decl.cases {
            if let Some(previous) = cases.iter().find(|seen| seen.name == case.name.name) {
                let previous = previous.span;
                self.sink.emit(
                    Diagnostic::error(
                        codes::DUPLICATE_NAME,
                        format!("`{name}.{}` is declared more than once", case.name.name),
                    )
                    .with_primary(case.name.span, "redeclared here")
                    .with_secondary(previous, "first declared here"),
                );
                continue;
            }
            cases.push(crate::analysis::EnumCaseInfo {
                name: case.name.name.clone(),
                // Field types are resolved in the next pass, once every
                // type name in every module is registered.
                fields: Vec::new(),
                span: case.span,
            });
        }

        // Whether a case carries data is visible in the syntax, so the
        // representation is settled before any type is resolved.
        let has_data = decl.cases.iter().any(|case| !case.fields.is_empty());

        let id = crate::analysis::EnumId(self.enums.len() as u32);
        let qualified = self.qualify(&name);
        let kind = if has_data {
            noto_types::DefKind::EnumWithData
        } else {
            noto_types::DefKind::Enum
        };
        let def = self.store.declare(qualified, kind);
        let ty = self.store.intern(Type::Named { def, arguments: Vec::new() });
        self.enums.push(crate::analysis::EnumInfo {
            name: name.clone(),
            module: self.current_module,
            is_exported: item.modifiers.is_exported,
            cases,
            has_data,
            ty,
            def,
            span: item.span,
        });

        let module = self.current_module.0 as usize;
        self.module_enums[module].insert(name.clone(), id);
        if item.modifiers.is_exported {
            self.exported[module].insert(name.clone());
        }
        // The name is a value too: it is the namespace of the cases.
        self.module_names[module].insert(name, Resolution::Enum(id));
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
                ItemKind::Interface(_) => self.report_unsupported(item),
                ItemKind::Enum(decl) => self.collect_enum_fields(decl),
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
            .with_primary(span, "not implemented in Noto 0.12")
        };

        if decl.class_kind != ClassKind::Class {
            self.sink.emit(
                Diagnostic::error(
                    codes::UNSUPPORTED_CONSTRUCT,
                    format!("`{}` declarations are not supported by this compiler yet", decl.class_kind.as_str()),
                )
                .with_primary(item.span, "not implemented in Noto 0.12")
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
        let def = self.store.declare(qualified, noto_types::DefKind::Class);
        let ty = self.store.intern(Type::Named { def, arguments: Vec::new() });
        self.classes.push(ClassInfo {
            name: name.clone(),
            module: self.current_module,
            is_exported: item.modifiers.is_exported,
            fields: Vec::new(),
            primary_count: 0,
            properties: Vec::new(),
            methods: Vec::new(),
            init: None,
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
                        "default values for constructor parameters are not supported by this compiler yet",
                    )
                    .with_primary(default.span, "not implemented in Noto 0.12"),
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
                // The argument a construction call passes initialises it.
                initializer: None,
                span: param.span,
            });
        }
        let primary_count = fields.len() as u32;

        // A field declared in the body has no argument to receive, so it must
        // carry its own initialiser; without one there is nothing sound to put
        // in the slot.
        for field in &decl.fields {
            if field.default.is_none() {
                self.sink.emit(
                    Diagnostic::error(
                        codes::CANNOT_INFER,
                        format!("field `{}` has nothing to initialise it", field.name.name),
                    )
                    .with_primary(field.span, "no value here")
                    .with_help(format!(
                        "give it one — `val {}: .. = ..` — or make it a constructor parameter",
                        field.name.name
                    )),
                );
            }

            let ty = match &field.ty {
                Some(ty) => self.resolve_type(ty),
                None => {
                    self.sink.emit(
                        Diagnostic::error(
                            codes::CANNOT_INFER,
                            format!("field `{}` needs a declared type", field.name.name),
                        )
                        .with_primary(field.span, "no type to infer from here"),
                    );
                    self.store.error()
                }
            };

            let name = field.name.name.clone();
            if let Some(previous) = fields.iter().find(|seen| seen.name == name) {
                let previous = previous.span;
                self.sink.emit(
                    Diagnostic::error(
                        codes::DUPLICATE_NAME,
                        format!("`{name}` is declared more than once in `{}`", decl.name.name),
                    )
                    .with_primary(field.name.span, "redeclared here")
                    .with_secondary(previous, "first declared here"),
                );
                continue;
            }

            fields.push(FieldInfo {
                name,
                ty,
                is_mutable: field.kind == noto_ast::LetKind::Var,
                initializer: field.default.as_ref().map(|default| default.id),
                span: field.span,
            });
        }

        self.classes[id.0 as usize].fields = fields;
        self.classes[id.0 as usize].primary_count = primary_count;

        self.collect_properties(id, decl);
        self.collect_init(id, decl);

        for item in &decl.methods {
            let ItemKind::Fn(function) = &item.kind else { continue };
            self.collect_method(id, item, function);
        }
    }

    /// Collects a class's properties, which come in two kinds.
    ///
    /// One written with only body-less accessors and an initialiser is stored:
    /// it becomes an ordinary field, and the default accessors are what a
    /// field read and write already are. Any accessor with a body makes the
    /// property computed: a read calls its getter, a write its setter, and
    /// nothing is stored — which is also why an initialiser on one is refused:
    /// a custom accessor has no way to reach storage the syntax never names.
    fn collect_properties(&mut self, class: ClassId, decl: &TypeDeclItem) {
        for property in &decl.properties {
            let ty = match &property.ty {
                Some(ty) => self.resolve_type(ty),
                None => {
                    self.sink.emit(
                        Diagnostic::error(
                            codes::CANNOT_INFER,
                            format!("property `{}` needs a declared type", property.name.name),
                        )
                        .with_primary(property.span, "no type to infer from here"),
                    );
                    self.store.error()
                }
            };

            let name = property.name.name.clone();
            let class_name = self.classes[class.0 as usize].name.clone();
            if let Some(previous) = self.classes[class.0 as usize]
                .fields
                .iter()
                .find(|field| field.name == name)
            {
                let previous = previous.span;
                self.sink.emit(
                    Diagnostic::error(
                        codes::DUPLICATE_NAME,
                        format!("`{class_name}` already has a field named `{name}`"),
                    )
                    .with_primary(property.name.span, "declared again here")
                    .with_secondary(previous, "the field is declared here"),
                );
                continue;
            }
            if let Some(previous) = self.classes[class.0 as usize]
                .properties
                .iter()
                .find(|seen| seen.name == name)
            {
                let previous = previous.span;
                self.sink.emit(
                    Diagnostic::error(
                        codes::DUPLICATE_NAME,
                        format!("`{class_name}.{name}` is declared more than once"),
                    )
                    .with_primary(property.name.span, "redeclared here")
                    .with_secondary(previous, "first declared here"),
                );
                continue;
            }

            let custom = |accessor: &Option<noto_ast::PropertyAccessor>| {
                accessor.as_ref().is_some_and(|accessor| accessor.body.is_some())
            };
            let is_computed =
                custom(&property.getter) || custom(&property.setter);

            if !is_computed {
                // Stored: the accessors, if written at all, ask for the
                // default implementations, which is what a field already is.
                let Some(default) = &property.default else {
                    self.sink.emit(
                        Diagnostic::error(
                            codes::CANNOT_INFER,
                            format!(
                                "property `{name}` has nothing to return: give it an initialiser or a `get` body",
                            ),
                        )
                        .with_primary(property.span, "no initialiser and no accessor body")
                        .with_help(format!("write `val {name}: .. = ..`, or add `get = ..`")),
                    );
                    continue;
                };
                self.classes[class.0 as usize].fields.push(FieldInfo {
                    name: name.clone(),
                    ty,
                    is_mutable: property.kind == noto_ast::LetKind::Var,
                    initializer: Some(default.id),
                    span: property.span,
                });
                continue;
            }

            if property.default.is_some() {
                self.sink.emit(
                    Diagnostic::error(
                        codes::UNSUPPORTED_CONSTRUCT,
                        "a property with custom accessors cannot also have an initialiser",
                    )
                    .with_primary(
                        property.default.as_ref().expect("just checked").span,
                        "this value could only be reached through storage the accessors cannot name",
                    )
                    .with_help("compute it inside `get` instead"),
                );
                continue;
            }

            let Some(getter) = property.getter.as_ref().filter(|get| get.body.is_some()) else {
                self.sink.emit(
                    Diagnostic::error(
                        codes::CANNOT_INFER,
                        format!("property `{name}` must be readable: give its `get` a body"),
                    )
                    .with_primary(property.span, "no `get` body here"),
                );
                continue;
            };

            let is_mutable = property.kind == noto_ast::LetKind::Var;
            // An accessor follows its class, the way a method does: exporting
            // the class exports the property it can be read through.
            let is_exported = self.classes[class.0 as usize].is_exported;
            let getter_fn =
                self.collect_accessor(class, &class_name, &name, ty, getter, false, is_exported);
            let setter_fn = match &property.setter {
                Some(setter) if setter.body.is_some() => {
                    if !is_mutable {
                        self.sink.emit(
                            Diagnostic::error(
                                codes::REASSIGNED_VAL,
                                format!("a `val` property cannot have a setter"),
                            )
                            .with_primary(setter.span, "this setter can never run")
                            .with_secondary(property.span, "declared `val` here"),
                        );
                        None
                    } else {
                        Some(self.collect_accessor(
                            class,
                            &class_name,
                            &name,
                            ty,
                            setter,
                            true,
                            is_exported,
                        ))
                    }
                }
                // A body-less setter alongside a custom getter has no storage
                // to default to; a `var` computed property simply has no
                // setter until one is written.
                Some(setter) => {
                    self.sink.emit(
                        Diagnostic::error(
                            codes::UNSUPPORTED_CONSTRUCT,
                            "a default `set` needs stored storage, and this property is computed",
                        )
                        .with_primary(setter.span, "give this `set` a body, or remove it"),
                    );
                    None
                }
                None => None,
            };

            self.classes[class.0 as usize].properties.push(crate::analysis::PropertyInfo {
                name,
                ty,
                is_mutable,
                getter: getter_fn,
                setter: setter_fn,
                span: property.span,
            });
        }
    }

    /// Records a `get` or `set` accessor as a function taking the receiver
    /// first, the way a method is one. A setter takes the incoming value
    /// under the conventional name `value` after it.
    fn collect_accessor(
        &mut self,
        class: ClassId,
        class_name: &str,
        property: &str,
        ty: TypeId,
        accessor: &noto_ast::PropertyAccessor,
        is_setter: bool,
        is_exported: bool,
    ) -> FunctionId {
        let id = FunctionId(self.functions.len() as u32);
        let unit = self.store.unit();
        self.functions.push(FunctionInfo {
            name: format!(
                "{class_name}.{}:{property}",
                if is_setter { "set" } else { "get" }
            ),
            module: self.current_module,
            is_exported,
            parameters: Vec::new(),
            result: if is_setter { unit } else { ty },
            locals: Vec::new(),
            body: accessor.body.as_ref().map(|body| body.id),
            type_params: Vec::new(),
            def: None,
            is_lambda: false,
            captures: Vec::new(),
            init_of: None,
            is_async: false,
            span: accessor.span,
        });

        let previous_function = self.current_function.replace(id);
        self.scopes.push();
        let receiver_ty = self.classes[class.0 as usize].ty;
        let receiver =
            self.declare_local(crate::RECEIVER_NAME, receiver_ty, false, true, accessor.span);
        self.functions[id.0 as usize].parameters.push(receiver);
        if is_setter {
            let value = self.declare_local(crate::SETTER_VALUE_NAME, ty, false, true, accessor.span);
            self.functions[id.0 as usize].parameters.push(value);
        }
        self.scopes.pop();
        self.current_function = previous_function;
        id
    }

    /// Synthesises `Class.<init>`, the function a construction call becomes
    /// when any field carries an initialiser.
    ///
    /// Its parameters are the primary constructor's, so an initialiser may
    /// read them — `class Person(val name: String) { val greeting = .. }`.
    /// `this` is deliberately not in scope: the object does not exist until
    /// the function allocates it.
    fn collect_init(&mut self, class: ClassId, decl: &TypeDeclItem) {
        if !self.classes[class.0 as usize]
            .fields
            .iter()
            .any(|field| field.initializer.is_some())
        {
            return;
        }

        let class_name = self.classes[class.0 as usize].name.clone();
        let class_ty = self.classes[class.0 as usize].ty;
        let is_exported = self.classes[class.0 as usize].is_exported;

        let id = FunctionId(self.functions.len() as u32);
        self.functions.push(FunctionInfo {
            name: format!("{class_name}.<init>"),
            module: self.current_module,
            is_exported,
            parameters: Vec::new(),
            result: class_ty,
            locals: Vec::new(),
            body: None,
            type_params: Vec::new(),
            def: None,
            is_lambda: false,
            captures: Vec::new(),
            init_of: Some(class),
            is_async: false,
            span: self.classes[class.0 as usize].span,
        });

        let previous_function = self.current_function.replace(id);
        self.scopes.push();
        for param in &decl.primary_params {
            let ty = match &param.ty {
                Some(ty) => self.resolve_type(ty),
                None => self.store.error(),
            };
            let local = self.declare_local(&param.name.name, ty, false, true, param.span);
            self.functions[id.0 as usize].parameters.push(local);
        }
        self.scopes.pop();
        self.current_function = previous_function;

        self.classes[class.0 as usize].init = Some(id);
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
                .with_primary(function.type_params[0].span, "not implemented in Noto 0.12"),
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
            type_params: Vec::new(),
            def: None,
            module: self.current_module,
            // A method follows its class: exporting the class exports them.
            is_exported: self.classes[class.0 as usize].is_exported,
            parameters: Vec::new(),
            result,
            locals: Vec::new(),
            body: function.body.as_ref().map(|body| body.id),
            is_lambda: false,
            captures: Vec::new(),
            init_of: None,
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

    /// Resolves the types of the data an enum's cases carry.
    ///
    /// Split from registering the enum for the same reason a class's fields
    /// are: a case may carry a value of a type declared further down, or in
    /// another module.
    fn collect_enum_fields(&mut self, decl: &noto_ast::EnumItem) {
        let Some(id) = self.own_enum(&decl.name.name) else {
            // `declare_enum` reported why this declaration is not an enum.
            return;
        };

        for (index, case) in decl.cases.iter().enumerate() {
            if index >= self.enums[id.0 as usize].cases.len() {
                break;
            }
            let mut fields: Vec<FieldInfo> = Vec::new();
            for field in &case.fields {
                if let Some(default) = &field.default {
                    self.sink.emit(
                        Diagnostic::error(
                            codes::UNSUPPORTED_CONSTRUCT,
                            "default values for case data are not supported by this compiler yet",
                        )
                        .with_primary(default.span, "not implemented in Noto 0.12"),
                    );
                }
                let ty = match &field.ty {
                    Some(ty) => self.resolve_type(ty),
                    None => {
                        self.sink.emit(
                            Diagnostic::error(
                                codes::CANNOT_INFER,
                                format!("`{}` needs a declared type", field.name.name),
                            )
                            .with_primary(field.span, "no type to infer from here"),
                        );
                        self.store.error()
                    }
                };
                if let Some(previous) = fields.iter().find(|seen| seen.name == field.name.name) {
                    let previous = previous.span;
                    self.sink.emit(
                        Diagnostic::error(
                            codes::DUPLICATE_NAME,
                            format!(
                                "`{}` is carried twice by `{}.{}`",
                                field.name.name, decl.name.name, case.name.name
                            ),
                        )
                        .with_primary(field.name.span, "named again here")
                        .with_secondary(previous, "first here"),
                    );
                    continue;
                }
                fields.push(FieldInfo {
                    name: field.name.name.clone(),
                    ty,
                    is_mutable: false,
                    // A case's data is initialised by the values passed to it.
                    initializer: None,
                    span: field.span,
                });
            }
            self.enums[id.0 as usize].cases[index].fields = fields;
        }
    }

    fn report_unsupported(&mut self, item: &Item) {
        let name = item.describe();
        self.sink.emit(
            Diagnostic::error(
                codes::UNSUPPORTED_CONSTRUCT,
                format!("`{name}` declarations are not supported by this compiler yet"),
            )
            .with_primary(item.span, "not implemented in Noto 0.12")
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
                .with_primary(receiver.span, "not implemented in Noto 0.12"),
            );
            return;
        }
        for parameter in &function.type_params {
            if let Some(bound) = parameter.bounds.first() {
                self.sink.emit(
                    Diagnostic::error(
                        codes::UNSUPPORTED_CONSTRUCT,
                        "bounds on a type parameter are not supported by this compiler yet",
                    )
                    .with_primary(bound.span, "not implemented in Noto 0.12")
                    .with_note("a type parameter permits only moving the value around"),
                );
            }
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

        // The type parameters are in scope for the signature and, later, for
        // the body: a `T` in a result type is the same `T` a local declares.
        let (def, type_params) = self.declare_type_params(&name, &function.type_params);
        let result = match &function.result {
            Some(ty) => self.resolve_type(ty),
            None => self.store.unit(),
        };

        self.functions.push(FunctionInfo {
            name: name.clone(),
            type_params: type_params.clone(),
            def,
            module: self.current_module,
            is_exported: item.modifiers.is_exported,
            parameters: Vec::new(),
            result,
            locals: Vec::new(),
            body: function.body.as_ref().map(|body| body.id),
            is_lambda: false,
            captures: Vec::new(),
            init_of: None,
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
        self.leave_type_params(def);

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
                .with_primary(span, "not implemented in Noto 0.12"),
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
            type_params: Vec::new(),
            def: None,
            is_lambda: false,
            captures: Vec::new(),
            init_of: None,
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
                        .with_primary(ty.span, "not implemented in Noto 0.12")
                        .with_note("generic functions are; generic classes are not"),
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
            TypeExprKind::List(inner) => {
                let element = self.resolve_type(inner);
                self.store.intern(Type::List(element))
            }
            TypeExprKind::Error => self.store.error(),
        }
    }

    /// Registers a declaration's type parameters and puts them in scope.
    ///
    /// Returns the declaration they belong to, so two functions each
    /// declaring a `T` never see each other's.
    pub(crate) fn declare_type_params(
        &mut self,
        owner: &str,
        params: &[noto_ast::TypeParam],
    ) -> (Option<noto_types::DefId>, Vec<String>) {
        if params.is_empty() {
            return (None, Vec::new());
        }

        let def = self.store.declare(owner.to_string(), noto_types::DefKind::Function);
        let mut names = Vec::new();
        let mut scope = std::collections::HashMap::new();
        for (index, parameter) in params.iter().enumerate() {
            let name = parameter.name.name.clone();
            if scope.contains_key(&name) {
                self.sink.emit(
                    Diagnostic::error(
                        codes::DUPLICATE_NAME,
                        format!("`{name}` is declared twice as a type parameter"),
                    )
                    .with_primary(parameter.name.span, "redeclared here"),
                );
                continue;
            }
            let ty = self.store.intern(Type::Parameter {
                def,
                index: index as u32,
                name: name.clone(),
            });
            scope.insert(name.clone(), ty);
            names.push(name);
        }

        self.type_scope.push(scope);
        (Some(def), names)
    }

    /// Takes a declaration's type parameters back out of scope.
    pub(crate) fn leave_type_params(&mut self, def: Option<noto_types::DefId>) {
        if def.is_some() {
            self.type_scope.pop();
        }
    }

    /// Puts an already-registered declaration's type parameters back in scope,
    /// which is what checking its body needs.
    pub(crate) fn enter_type_params(
        &mut self,
        def: Option<noto_types::DefId>,
        names: &[String],
    ) {
        let Some(def) = def else { return };
        let mut scope = std::collections::HashMap::new();
        for (index, name) in names.iter().enumerate() {
            let ty = self.store.intern(Type::Parameter {
                def,
                index: index as u32,
                name: name.clone(),
            });
            scope.insert(name.clone(), ty);
        }
        self.type_scope.push(scope);
    }

    /// Looks a type name up among the built-in types and declared classes.
    fn resolve_type_name(&mut self, name: &str, span: Span) -> TypeId {
        // A type parameter shadows everything: inside `fn f<Int>(..)` the
        // name means the parameter, however strange that is to write.
        for scope in self.type_scope.iter().rev() {
            if let Some(ty) = scope.get(name) {
                return *ty;
            }
        }
        if let Some(primitive) = Primitive::from_name(name) {
            return self.store.primitive(primitive);
        }
        if let Some(id) = self.lookup_type(name) {
            return self.classes[id.0 as usize].ty;
        }
        if let Some(id) = self.lookup_enum(name) {
            return self.enums[id.0 as usize].ty;
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
