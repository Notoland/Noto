//! The Noto linter.
//!
//! The linter reports what is legal but probably not what was meant: a
//! binding nothing reads, a `var` nothing reassigns, a function nothing
//! calls.
//! Every lint is a warning — `noto lint` never rejects a program the compiler
//! accepts — and every one carries a stable `NOTO06nn` code so it can be
//! looked up and, once suppression exists, silenced.
//!
//! It runs after semantic analysis and reads its results rather than
//! recomputing them: which name refers to which binding is the type checker's
//! answer, and asking it twice is how the two drift apart. Like every other
//! phase, the linter pushes into a [`DiagnosticSink`] and prints nothing.
//!
//! Implemented today:
//!
//! | Code | Lint |
//! |---|---|
//! | `NOTO0600` | a binding that is never read |
//! | `NOTO0601` | a `var` that is never reassigned |
//! | `NOTO0603` | an import that nothing uses |
//! | `NOTO0604` | a function that is never called |
//! | `NOTO0605` | a constant that is never read |
//!
//! One code in the range is not the linter's: `NOTO0602`, unreachable code,
//! is emitted by semantic analysis, where it falls out of type checking for
//! free and catches more than a syntactic lint could.

#![deny(missing_docs)]

mod imports;
mod usage;

use noto_ast::Module;
use noto_diagnostics::{Diagnostic, DiagnosticSink};
use noto_semantic::Analysis;

/// The prefix that marks a binding as deliberately unused.
///
/// Naming something `_total` is how the author says the binding is there on
/// purpose and nothing reads it; the linter takes them at their word.
pub const IGNORED_PREFIX: &str = "_";

/// Runs every lint over an analysed module.
///
/// Diagnostics come out in source order regardless of which lint produced
/// them, which is what makes the output read like the file.
pub fn lint(module: &Module, analysis: &Analysis, sink: &mut DiagnosticSink) {
    lint_module(module, &[], analysis, sink)
}

/// Runs every lint over one module of a program, named by its id.

/// Runs every lint over one module of a program.
///
/// `imports` is what that module imports, which the unused-import lint needs
/// and nothing else does.
pub fn lint_module(
    module: &Module,
    imports: &[noto_semantic::Import],
    analysis: &Analysis,
    sink: &mut DiagnosticSink,
) {
    lint_one(module, noto_semantic::ModuleId::ROOT, imports, analysis, sink)
}

/// Runs every lint over the module with the given id.
pub fn lint_one(
    module: &Module,
    id: noto_semantic::ModuleId,
    imports: &[noto_semantic::Import],
    analysis: &Analysis,
    sink: &mut DiagnosticSink,
) {
    let mut found = Vec::new();
    usage::check(module, id, analysis, &mut found);
    imports::check(module, imports, analysis, &mut found);

    found.sort_by_key(|diagnostic| {
        diagnostic.labels.first().map(|label| label.span.start).unwrap_or(0)
    });
    for diagnostic in found {
        sink.emit(diagnostic);
    }
}

/// The prefix `compiler/semantic` gives a lowered test body.
///
/// A test is run by `noto test` rather than called from the program, so it is
/// never dead code.
pub(crate) const TEST_PREFIX: &str = "test$";

/// Whether a name opts out of the unused-binding lints.
pub(crate) fn is_ignored(name: &str) -> bool {
    name.starts_with(IGNORED_PREFIX)
}

/// The type every lint produces.
pub(crate) type Found = Vec<Diagnostic>;

#[cfg(test)]
mod tests;
