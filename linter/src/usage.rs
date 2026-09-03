//! What is declared and never used.
//!
//! Every lint here needs the same fact — where each name is mentioned — so
//! they share one walk. Reads and writes are told apart at the assignment:
//! `x = 1` writes `x` without reading it, while `x += 1` does both.

use crate::{is_ignored, Found, TEST_PREFIX};
use noto_ast::visit::{self, Visitor};
use noto_ast::{Expr, ExprKind, Module};
use noto_diagnostics::{codes, Diagnostic};
use noto_semantic::{Analysis, ConstId, FunctionId, LocalId, ModuleId, Resolution};
use std::collections::HashSet;

/// Reports every declaration nothing uses.
///
/// `id` is the module `module` is the AST of. Only its own declarations are
/// reported: the walk sees only this module's bodies, so a function called
/// from somewhere else would look dead from here.
pub(crate) fn check(module: &Module, id: ModuleId, analysis: &Analysis, found: &mut Found) {
    let mut usage = Usage {
        analysis,
        reads: HashSet::new(),
        writes: HashSet::new(),
        called: HashSet::new(),
        constants: HashSet::new(),
    };
    usage.visit_module(module);
    locals(id, analysis, &usage, found);
    functions(id, analysis, &usage, found);
    constants(id, analysis, &usage, found);
}

/// Reports functions nothing calls.
///
/// The entry point is used by definition and a test body is run by
/// `noto test`, so neither can be dead. A function called only from another
/// dead function still counts as used: reachability is the optimizer's
/// analysis, and a lint that guesses at it would be wrong in both directions.
fn functions(module: ModuleId, analysis: &Analysis, usage: &Usage, found: &mut Found) {
    for (index, function) in analysis.functions.iter().enumerate() {
        let id = FunctionId(index as u32);
        // An exported declaration is the module's surface, not its dead
        // code: what calls it is by definition somewhere else.
        if function.module != module || function.is_exported {
            continue;
        }
        let is_test = function.name.starts_with(TEST_PREFIX);
        // A method is named `Class.method`; the opt-out and the suggested
        // rename are about the part the author actually wrote.
        let written = function.name.rsplit('.').next().unwrap_or(&function.name);
        if is_test
            || analysis.entry == Some(id)
            || is_ignored(written)
            || usage.called.contains(&id)
        {
            continue;
        }
        let what = if function.name.contains('.') { "method" } else { "function" };
        found.push(
            Diagnostic::warning(
                codes::UNUSED_FUNCTION,
                format!("{what} `{}` is never called", function.name),
            )
            .with_primary(function.span, "declared here and never called")
            .with_help(format!(
                "remove it, or rename it to `_{written}` to say that is deliberate"
            )),
        );
    }
}

/// Reports constants nothing reads.
fn constants(module: ModuleId, analysis: &Analysis, usage: &Usage, found: &mut Found) {
    for (index, constant) in analysis.constants.iter().enumerate() {
        let id = ConstId(index as u32);
        if constant.module != module || constant.is_exported {
            continue;
        }
        if is_ignored(&constant.name) || usage.constants.contains(&id) {
            continue;
        }
        found.push(
            Diagnostic::warning(
                codes::UNUSED_CONST,
                format!("constant `{}` is never used", constant.name),
            )
            .with_primary(constant.span, "declared here and never read"),
        );
    }
}

/// Reports bindings nothing reads and `var`s nothing reassigns.
fn locals(module: ModuleId, analysis: &Analysis, usage: &Usage, found: &mut Found) {
    for (index, local) in analysis.locals.iter().enumerate() {
        let id = LocalId(index as u32);
        if analysis.functions[local.function.0 as usize].module != module {
            continue;
        }
        // The receiver is bound by the compiler, not written by anyone, so
        // there is no one to tell that it is unused.
        if is_ignored(&local.name) || local.name == noto_semantic::RECEIVER_NAME {
            continue;
        }

        if !usage.reads.contains(&id) {
            let what = if local.is_parameter { "parameter" } else { "binding" };
            found.push(
                Diagnostic::warning(
                    codes::UNUSED_BINDING,
                    format!("{what} `{}` is never used", local.name),
                )
                .with_primary(local.span, "declared here and never read")
                .with_help(format!(
                    "remove it, or rename it to `_{}` to say that is deliberate",
                    local.name
                )),
            );
            // A binding nothing reads is already the more useful message; the
            // `var` lint below would only repeat it.
            continue;
        }

        if local.is_mutable && !local.is_parameter && !usage.writes.contains(&id) {
            found.push(
                Diagnostic::warning(
                    codes::VAR_NEVER_REASSIGNED,
                    format!("`{}` is declared `var` but never reassigned", local.name),
                )
                .with_primary(local.span, "never assigned to after this")
                .with_help("declare it with `val`"),
            );
        }
    }
}

struct Usage<'a> {
    analysis: &'a Analysis,
    reads: HashSet<LocalId>,
    writes: HashSet<LocalId>,
    called: HashSet<FunctionId>,
    constants: HashSet<ConstId>,
}

impl Usage<'_> {
    /// The local an expression names, if it names one directly.
    ///
    /// Only a bare path counts. `items[i]` is an index into whatever `items`
    /// holds, so assigning to it reads `items` like any other expression.
    fn local_of(&self, expr: &Expr) -> Option<LocalId> {
        if !matches!(expr.kind, ExprKind::Path(_)) {
            return None;
        }
        match self.analysis.resolution(expr.id) {
            Some(Resolution::Local(id)) => Some(id),
            _ => None,
        }
    }
}

impl Visitor for Usage<'_> {
    fn visit_expr(&mut self, expr: &Expr) {
        match &expr.kind {
            // `this` names the receiver, which is a local like any other.
            ExprKind::Path(_) | ExprKind::This => match self.analysis.resolution(expr.id) {
                Some(Resolution::Local(id)) => {
                    self.reads.insert(id);
                }
                Some(Resolution::Function(id)) => {
                    self.called.insert(id);
                }
                Some(Resolution::Const(id)) => {
                    self.constants.insert(id);
                }
                _ => {}
            },
            ExprKind::Assign { target, value, op, .. } => {
                match self.local_of(target) {
                    Some(id) => {
                        self.writes.insert(id);
                        // `x += 1` reads `x` before storing back into it.
                        if op.is_some() {
                            self.reads.insert(id);
                        }
                    }
                    None => self.visit_expr(target),
                }
                self.visit_expr(value);
            }
            _ => {
                // A call through a member resolves on its callee rather than
                // on a path: `p.area()` calls a method, `util.double(1)` a
                // function in another module. Both are calls.
                match self.analysis.resolution(expr.id) {
                    Some(Resolution::Method(id)) | Some(Resolution::Function(id)) => {
                        self.called.insert(id);
                    }
                    Some(Resolution::Const(id)) => {
                        self.constants.insert(id);
                    }
                    _ => {}
                }
                visit::walk_expr(self, expr)
            }
        }
    }
}
