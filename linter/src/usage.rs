//! What is declared and never used.
//!
//! Every lint here needs the same fact — where each name is mentioned — so
//! they share one walk. Reads and writes are told apart at the assignment:
//! `x = 1` writes `x` without reading it, while `x += 1` does both.

use crate::{is_ignored, Found, TEST_PREFIX};
use noto_ast::visit::{self, Visitor};
use noto_ast::{Expr, ExprKind, Module};
use noto_diagnostics::{codes, Diagnostic};
use noto_semantic::{Analysis, ConstId, FunctionId, LocalId, Resolution};
use std::collections::HashSet;

/// Reports every declaration nothing uses.
pub(crate) fn check(module: &Module, analysis: &Analysis, found: &mut Found) {
    let mut usage = Usage {
        analysis,
        reads: HashSet::new(),
        writes: HashSet::new(),
        called: HashSet::new(),
        constants: HashSet::new(),
    };
    usage.visit_module(module);
    locals(analysis, &usage, found);
    functions(analysis, &usage, found);
    constants(analysis, &usage, found);
}

/// Reports functions nothing calls.
///
/// The entry point is used by definition and a test body is run by
/// `noto test`, so neither can be dead. A function called only from another
/// dead function still counts as used: reachability is the optimizer's
/// analysis, and a lint that guesses at it would be wrong in both directions.
fn functions(analysis: &Analysis, usage: &Usage, found: &mut Found) {
    for (index, function) in analysis.functions.iter().enumerate() {
        let id = FunctionId(index as u32);
        let is_test = function.name.starts_with(TEST_PREFIX);
        if is_test
            || analysis.entry == Some(id)
            || is_ignored(&function.name)
            || usage.called.contains(&id)
        {
            continue;
        }
        found.push(
            Diagnostic::warning(
                codes::UNUSED_FUNCTION,
                format!("function `{}` is never called", function.name),
            )
            .with_primary(function.span, "declared here and never called")
            .with_help(format!(
                "remove it, or rename it to `_{}` to say that is deliberate",
                function.name
            )),
        );
    }
}

/// Reports constants nothing reads.
fn constants(analysis: &Analysis, usage: &Usage, found: &mut Found) {
    for (index, constant) in analysis.constants.iter().enumerate() {
        let id = ConstId(index as u32);
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
fn locals(analysis: &Analysis, usage: &Usage, found: &mut Found) {
    for (index, local) in analysis.locals.iter().enumerate() {
        let id = LocalId(index as u32);
        if is_ignored(&local.name) {
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
            ExprKind::Path(_) => match self.analysis.resolution(expr.id) {
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
            _ => visit::walk_expr(self, expr),
        }
    }
}
