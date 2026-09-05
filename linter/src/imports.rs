//! Imports nothing uses.
//!
//! An unused import is not just clutter: it is a dependency the module does
//! not have, and it is what makes a build graph larger than the program.
//!
//! What counts as used is decided from what analysis recorded, not from the
//! text. A qualified name records the module it went through on its receiver;
//! a selectively imported name records the declaration it reached. Both are
//! looked up here rather than matched by spelling, so a local that happens to
//! share a name with an import does not keep it alive.

use crate::Found;
use noto_ast::visit::{self, Visitor};
use noto_ast::{Expr, ExprKind, Module};
use noto_diagnostics::{codes, Diagnostic};
use noto_semantic::{Analysis, Import, ModuleId, Resolution};
use std::collections::HashSet;

/// Reports the imports of one module that nothing in it uses.
pub(crate) fn check(
    module: &Module,
    imports: &[Import],
    analysis: &Analysis,
    found: &mut Found,
) {
    if imports.is_empty() {
        return;
    }

    let mut usage = Usage {
        analysis,
        modules: HashSet::new(),
        names: HashSet::new(),
        types: HashSet::new(),
    };
    usage.visit_module(module);

    for import in imports {
        if let Some(binding) = &import.binding {
            let qualified = format!("{binding}.");
            let used = usage.modules.contains(&import.module)
                || usage.types.iter().any(|mention| mention.starts_with(&qualified));
            if !used {
                found.push(
                    Diagnostic::warning(
                        codes::UNUSED_IMPORT,
                        format!("nothing uses `{}`", import.path),
                    )
                    .with_primary(import.span, "this import is never used")
                    .with_help("remove it"),
                );
            }
            continue;
        }

        // A selective import is used one name at a time; the import as a
        // whole is dead only when none of them is used.
        let unused: Vec<&str> = import
            .names
            .iter()
            .filter(|selected| {
                !usage.names.contains(&(import.module, selected.name.clone()))
                    && !usage.types.contains(&selected.name)
            })
            .map(|selected| selected.name.as_str())
            .collect();

        if unused.len() == import.names.len() {
            found.push(
                Diagnostic::warning(
                    codes::UNUSED_IMPORT,
                    format!("nothing uses `{}`", import.path),
                )
                .with_primary(import.span, "this import is never used")
                .with_help("remove it"),
            );
        } else if !unused.is_empty() {
            let names: Vec<String> = unused.iter().map(|name| format!("`{name}`")).collect();
            found.push(
                Diagnostic::warning(
                    codes::UNUSED_IMPORT,
                    format!("nothing uses {} from `{}`", names.join(", "), import.path),
                )
                .with_primary(import.span, "imported and never used")
                .with_help("drop the unused names from the import"),
            );
        }
    }
}

struct Usage<'a> {
    analysis: &'a Analysis,
    /// Modules reached through a namespace.
    modules: HashSet<ModuleId>,
    /// Declarations reached by name, as the module they came from and their
    /// name.
    names: HashSet<(ModuleId, String)>,
    /// Every type name written in the module, as written.
    ///
    /// A type mention records no resolution — a type expression resolves to a
    /// `TypeId`, not to a declaration — so this is matched against what an
    /// import binds. That is sound rather than a guess: an import may not
    /// bind a name the module already declares, so a type written `Point`
    /// while `Point` is imported can only be the imported one.
    types: HashSet<String>,
}

impl Usage<'_> {
    /// Records what one resolved name reached, if it reached another module.
    fn record(&mut self, resolution: Resolution) {
        let (module, name) = match resolution {
            Resolution::Module(id) => {
                self.modules.insert(id);
                return;
            }
            Resolution::Function(id) | Resolution::Method(id) => {
                let info = &self.analysis.functions[id.0 as usize];
                (info.module, info.name.clone())
            }
            Resolution::Class(id) => {
                let info = self.analysis.class(id);
                (info.module, info.name.clone())
            }
            Resolution::Field { class, .. } | Resolution::Property { class, .. } => {
                let info = self.analysis.class(class);
                (info.module, info.name.clone())
            }
            Resolution::Const(id) => {
                let info = &self.analysis.constants[id.0 as usize];
                (info.module, info.name.clone())
            }
            Resolution::Enum(id) | Resolution::EnumCase { enum_id: id, .. } => {
                let info = self.analysis.enum_at(id);
                (info.module, info.name.clone())
            }
            Resolution::Local(_)
            | Resolution::Builtin(_)
            | Resolution::ListMethod(_)
            | Resolution::Error => return,
        };
        // A method is `Class.method`; what an import binds is the class.
        let name = name.split('.').next().unwrap_or(&name).to_string();
        self.names.insert((module, name));
    }
}

impl<'ast> Visitor<'ast> for Usage<'_> {
    fn visit_expr(&mut self, expr: &'ast Expr) {
        if let Some(resolution) = self.analysis.resolution(expr.id) {
            self.record(resolution);
        }
        // A call records what it reached on its callee rather than on itself.
        if let ExprKind::Call(call) = &expr.kind {
            if let Some(resolution) = self.analysis.resolution(call.callee.id) {
                self.record(resolution);
            }
        }
        visit::walk_expr(self, expr);
    }

    fn visit_type(&mut self, ty: &'ast noto_ast::TypeExpr) {
        // A type mention keeps an import alive: `fn f(p: point.Point)` needs
        // it as surely as a call does.
        if let noto_ast::TypeExprKind::Named { path, .. } = &ty.kind {
            self.types.insert(path.to_dotted());
        }
        visit::walk_type(self, ty);
    }
}
