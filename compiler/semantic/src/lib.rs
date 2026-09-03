//! Semantic analysis and type checking for Noto.
//!
//! This phase runs after parsing and before lowering to Noto IR. It resolves
//! every name, gives every expression a type, and enforces the rules the
//! grammar cannot: null safety, mutability, exhaustiveness, and the absence of
//! implicit conversions.
//!
//! Analysis proceeds in two passes over the module. The first collects the
//! signature of every top-level declaration, so that a function may call one
//! declared further down the file. The second checks the bodies against those
//! signatures.
//!
//! Nothing here mutates the AST: results are recorded in [`Analysis`], keyed by
//! [`NodeId`](noto_ast::NodeId).

#![deny(missing_docs)]

pub mod analysis;
pub mod builtins;
mod check;
mod collect;
mod scope;

pub use analysis::{
    Analysis, ConstId, ConstInfo, ConstValue, FunctionId, FunctionInfo, LocalId, LocalInfo,
    Resolution, TestInfo,
};
pub use builtins::Builtin;

use noto_ast::Module;
use noto_diagnostics::DiagnosticSink;
use noto_types::{TypeId, TypeStore};
use scope::Scopes;
use std::collections::HashMap;

/// Analyses a parsed module.
///
/// An [`Analysis`] is always returned. When `sink` holds errors afterwards the
/// results are still structurally valid but contain error types, so later
/// phases must check the sink before lowering.
pub fn analyze(module: &Module, sink: &mut DiagnosticSink) -> Analysis {
    let mut checker = Checker::new(sink);
    checker.collect_items(module);
    checker.check_items(module);
    checker.finish()
}

/// The type checker's working state.
struct Checker<'sink> {
    sink: &'sink mut DiagnosticSink,
    store: TypeStore,
    scopes: Scopes,
    types: HashMap<noto_ast::NodeId, TypeId>,
    resolutions: HashMap<noto_ast::NodeId, Resolution>,
    locals: Vec<LocalInfo>,
    functions: Vec<FunctionInfo>,
    constants: Vec<ConstInfo>,
    tests: Vec<TestInfo>,
    entry: Option<FunctionId>,
    /// The function whose body is being checked.
    current_function: Option<FunctionId>,
    /// The result type the enclosing function must produce.
    expected_result: TypeId,
}

impl<'sink> Checker<'sink> {
    fn new(sink: &'sink mut DiagnosticSink) -> Self {
        let store = TypeStore::new();
        let expected_result = store.unit();
        Checker {
            sink,
            store,
            scopes: Scopes::new(),
            types: HashMap::new(),
            resolutions: HashMap::new(),
            locals: Vec::new(),
            functions: Vec::new(),
            constants: Vec::new(),
            tests: Vec::new(),
            entry: None,
            current_function: None,
            expected_result,
        }
    }

    fn finish(self) -> Analysis {
        Analysis {
            types: self.types,
            resolutions: self.resolutions,
            locals: self.locals,
            functions: self.functions,
            constants: self.constants,
            tests: self.tests,
            entry: self.entry,
            store: self.store,
        }
    }

    /// Records the type of an expression node.
    fn record_type(&mut self, id: noto_ast::NodeId, ty: TypeId) -> TypeId {
        self.types.insert(id, ty);
        ty
    }

    /// Records what a name refers to.
    fn record_resolution(&mut self, id: noto_ast::NodeId, resolution: Resolution) {
        self.resolutions.insert(id, resolution);
    }

    /// Declares a local and returns its id.
    fn declare_local(
        &mut self,
        name: &str,
        ty: TypeId,
        is_mutable: bool,
        is_parameter: bool,
        span: noto_span::Span,
    ) -> LocalId {
        let function = self.current_function.expect("a local belongs to a function");
        let id = LocalId(self.locals.len() as u32);
        self.locals.push(LocalInfo {
            name: name.to_string(),
            ty,
            is_mutable,
            is_parameter,
            function,
            span,
        });
        self.functions[function.0 as usize].locals.push(id);
        self.scopes.declare(name, Resolution::Local(id));
        id
    }
}

#[cfg(test)]
mod tests;
