//! Checking what an `import` binds.
//!
//! The driver has already found the file an import names; what is left is
//! whether the names it asks for exist, whether they are exported, and
//! whether what it binds collides with something already there.
//!
//! Nothing here copies a binding into a scope. An imported name is resolved
//! by following the import to the module that declares it, which is what
//! lets signatures be collected before imports are checked.

use crate::{Checker, Import, ModuleId, ModuleInput};
use noto_diagnostics::{codes, Diagnostic};
use std::collections::HashMap;

impl Checker<'_> {
    /// Checks every import of every module.
    pub(crate) fn check_imports(&mut self, modules: &[ModuleInput]) {
        for (index, module) in modules.iter().enumerate() {
            self.current_module = ModuleId(index as u32);
            self.check_module_imports(module.imports);
        }
    }

    fn check_module_imports(&mut self, imports: &[Import]) {
        let mut bound: HashMap<String, noto_span::Span> = HashMap::new();

        for import in imports {
            if let Some(name) = &import.binding {
                self.check_binding(name, import.span, &mut bound);
            }
            for selected in &import.names {
                self.check_selected(import, selected, &mut bound);
            }
        }
    }

    /// Checks the name a plain import binds as a namespace.
    fn check_binding(
        &mut self,
        name: &str,
        span: noto_span::Span,
        bound: &mut HashMap<String, noto_span::Span>,
    ) {
        if let Some(previous) = bound.get(name).copied() {
            self.report_collision(name, span, previous);
            return;
        }
        if self.module_names[self.current_module.0 as usize].contains_key(name) {
            self.sink.emit(
                Diagnostic::error(
                    codes::DUPLICATE_NAME,
                    format!("`{name}` is already declared in this module"),
                )
                .with_primary(span, "this import binds the same name")
                .with_help(format!("rename the import: `as {name}Module`")),
            );
            return;
        }
        bound.insert(name.to_string(), span);
    }

    /// Checks one name a selective import asks for.
    fn check_selected(
        &mut self,
        import: &Import,
        selected: &noto_ast::Ident,
        bound: &mut HashMap<String, noto_span::Span>,
    ) {
        let name = &selected.name;
        let target = import.module.0 as usize;
        let declared = self.module_names[target].contains_key(name)
            || self.module_types[target].contains_key(name);

        if !declared {
            let known = self.export_list(import.module);
            self.sink.emit(
                Diagnostic::error(
                    codes::UNKNOWN_NAME,
                    format!("`{}` declares no `{name}`", import.path),
                )
                .with_primary(selected.span, "not declared there")
                .with_note(known),
            );
            return;
        }

        if !self.exported[target].contains(name) {
            // The name exists, so the fix is one keyword rather than a
            // different name; say that instead of listing the exports.
            self.sink.emit(
                Diagnostic::error(
                    codes::UNKNOWN_NAME,
                    format!("`{name}` is private to `{}`", import.path),
                )
                .with_primary(selected.span, "declared there, but not exported")
                .with_help(format!("write `export` on its declaration in `{}`", import.path)),
            );
            return;
        }

        if let Some(previous) = bound.get(name).copied() {
            self.report_collision(name, selected.span, previous);
            return;
        }
        if self.module_names[self.current_module.0 as usize].contains_key(name) {
            self.sink.emit(
                Diagnostic::error(
                    codes::DUPLICATE_NAME,
                    format!("`{name}` is already declared in this module"),
                )
                .with_primary(selected.span, "this import brings in the same name")
                .with_help("import the module itself and use `module.name`"),
            );
            return;
        }
        bound.insert(name.clone(), selected.span);
    }

    fn report_collision(
        &mut self,
        name: &str,
        span: noto_span::Span,
        previous: noto_span::Span,
    ) {
        self.sink.emit(
            Diagnostic::error(
                codes::DUPLICATE_NAME,
                format!("`{name}` is bound by two imports"),
            )
            .with_primary(span, "bound again here")
            .with_secondary(previous, "first bound here")
            .with_help("rename one of them with `as`"),
        );
    }

    /// The names a module exports, for a diagnostic about one it does not.
    fn export_list(&self, module: ModuleId) -> String {
        let mut names: Vec<&str> =
            self.exported[module.0 as usize].iter().map(String::as_str).collect();
        if names.is_empty() {
            return "it exports nothing".to_string();
        }
        names.sort_unstable();
        format!("it exports {}", names.join(", "))
    }
}
