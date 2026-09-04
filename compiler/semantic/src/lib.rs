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
mod imports;
mod scope;

pub use analysis::{
    Analysis, ClassId, ClassInfo, ConstId, ConstInfo, ConstValue, EnumCaseInfo, EnumId, EnumInfo,
    FieldInfo, FunctionId, FunctionInfo, LocalId, LocalInfo, MethodInfo, ModuleId, PropertyInfo,
    Resolution, TestInfo,
};

/// The name the receiver of a method is bound to.
///
/// It is a keyword rather than an identifier, so nothing the user writes can
/// collide with it; binding it as an ordinary local is what lets `this` be
/// looked up, type checked and lowered like any other parameter.
pub const RECEIVER_NAME: &str = "this";
/// The name the incoming value of a `set` accessor is bound to.
///
/// The grammar gives a setter no parameter list, so the value arrives under
/// this conventional name — the same one C# uses — rather than one each
/// property would have to repeat.
pub const SETTER_VALUE_NAME: &str = "value";
pub use builtins::Builtin;

use noto_ast::{Ident, Module};
use noto_diagnostics::DiagnosticSink;
use noto_span::Span;
use noto_types::{TypeId, TypeStore};
use scope::Scopes;
use std::collections::{HashMap, HashSet};

/// One `import`, with the module it names already found.
///
/// The driver owns finding files; analysis only needs to know which module an
/// import reaches and under what name.
#[derive(Clone, Debug)]
pub struct Import {
    /// The module it names.
    pub module: ModuleId,
    /// The dotted path as written, for diagnostics.
    pub path: String,
    /// The namespace it binds, or `None` for a selective import.
    pub binding: Option<String>,
    /// The names selected from it; empty unless it is a selective import.
    pub names: Vec<Ident>,
    /// Where it was written.
    pub span: Span,
}

/// One module handed to analysis.
pub struct ModuleInput<'a> {
    /// Its dotted name; empty for the root.
    pub name: &'a str,
    /// Its parsed contents.
    pub ast: &'a Module,
    /// What it imports.
    pub imports: &'a [Import],
}

/// Analyses a parsed module.
///
/// An [`Analysis`] is always returned. When `sink` holds errors afterwards the
/// results are still structurally valid but contain error types, so later
/// phases must check the sink before lowering.
pub fn analyze(module: &Module, sink: &mut DiagnosticSink) -> Analysis {
    analyze_program(&[ModuleInput { name: "", ast: module, imports: &[] }], sink)
}

/// Analyses a whole program: the root module and everything it imports.
///
/// The passes are ordered by what the language forces, not by taste. Class
/// names come first because a signature anywhere may name a class declared
/// anywhere, including in another module. Signatures and fields come next,
/// because a body may call anything. Bodies come last.
///
/// Imports are never copied into a scope. A name that an import brings in is
/// resolved by following the import to the module that declares it, which is
/// what keeps the passes from needing each other's results.
pub fn analyze_program(modules: &[ModuleInput], sink: &mut DiagnosticSink) -> Analysis {
    let mut checker = Checker::new(sink);
    checker.modules = modules.iter().map(|module| module.name.to_string()).collect();
    checker.imports = modules.iter().map(|module| module.imports.to_vec()).collect();
    checker.module_names = vec![HashMap::new(); modules.len()];
    checker.module_types = vec![HashMap::new(); modules.len()];
    checker.module_enums = vec![HashMap::new(); modules.len()];
    checker.exported = vec![HashSet::new(); modules.len()];

    for (index, module) in modules.iter().enumerate() {
        checker.current_module = ModuleId(index as u32);
        checker.declare_classes(module.ast);
    }

    for (index, module) in modules.iter().enumerate() {
        checker.current_module = ModuleId(index as u32);
        checker.scopes.push();
        // The class pass already bound every class name as its constructor;
        // collecting into the same scope keeps them.
        for (name, resolution) in checker.module_names[index].clone() {
            checker.scopes.declare(name, resolution);
        }
        checker.collect_items(module.ast);
        checker.module_names[index] = checker.scopes.take_top();
    }

    checker.check_imports(modules);

    for (index, module) in modules.iter().enumerate() {
        checker.current_module = ModuleId(index as u32);
        checker.scopes.push();
        for (name, resolution) in checker.module_names[index].clone() {
            checker.scopes.declare(name, resolution);
        }
        checker.check_items(module.ast);
        checker.scopes.pop();
    }

    // Only the root's entry point runs; an imported module's `main`, if it
    // has one, is an ordinary function here.
    if let Some(entry) = checker.entry {
        checker.check_entry_signature(entry);
    }

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
    classes: Vec<ClassInfo>,
    enums: Vec<EnumInfo>,
    tests: Vec<TestInfo>,
    /// The module whose declarations are being collected or checked.
    current_module: ModuleId,
    /// Every module's name, indexed by [`ModuleId`].
    modules: Vec<String>,
    /// What every module imports.
    imports: Vec<Vec<Import>>,
    /// Each module's own top-level value names.
    module_names: Vec<HashMap<String, Resolution>>,
    /// Each module's own enum names.
    module_enums: Vec<HashMap<String, EnumId>>,
    /// Each module's own type names.
    ///
    /// Types and values live in separate namespaces: `Point` as a type is
    /// looked up here, `Point` as a constructor through the value scopes.
    module_types: Vec<HashMap<String, ClassId>>,
    /// The names each module exports, across both namespaces.
    exported: Vec<HashSet<String>>,
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
            classes: Vec::new(),
            enums: Vec::new(),
            tests: Vec::new(),
            current_module: ModuleId::ROOT,
            modules: vec![String::new()],
            imports: vec![Vec::new()],
            module_names: vec![HashMap::new()],
            module_types: vec![HashMap::new()],
            module_enums: vec![HashMap::new()],
            exported: vec![HashSet::new()],
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
            classes: self.classes,
            enums: self.enums,
            modules: self.modules,
            tests: self.tests,
            entry: self.entry,
            store: self.store,
        }
    }

    /// The current module's name prefixed to a declaration's.
    ///
    /// Only diagnostics see it, and only for types, where the same class name
    /// in two modules would otherwise render identically.
    fn qualify(&self, name: &str) -> String {
        let module = &self.modules[self.current_module.0 as usize];
        if module.is_empty() {
            name.to_string()
        } else {
            format!("{module}.{name}")
        }
    }

    /// A type declared by the module being checked.
    fn own_type(&self, name: &str) -> Option<ClassId> {
        self.module_types[self.current_module.0 as usize].get(name).copied()
    }

    /// An enum declared by the module being checked.
    fn own_enum(&self, name: &str) -> Option<EnumId> {
        self.module_enums[self.current_module.0 as usize].get(name).copied()
    }

    /// Resolves an enum name: this module's own, then what it imports.
    fn lookup_enum(&self, name: &str) -> Option<EnumId> {
        if let Some(id) = self.own_enum(name) {
            return Some(id);
        }
        match name.split_once('.') {
            Some((namespace, rest)) => {
                let target = self.namespace(namespace)?;
                self.export_enum(target, rest)
            }
            None => self.selective(name).and_then(|target| self.export_enum(target, name)),
        }
    }

    /// An enum `target` exports under `name`.
    fn export_enum(&self, target: ModuleId, name: &str) -> Option<EnumId> {
        let index = target.0 as usize;
        if !self.exported[index].contains(name) {
            return None;
        }
        self.module_enums[index].get(name).copied()
    }

    /// Resolves a type name: this module's own, then what it imports.
    ///
    /// `point.Point` names the module a namespace binds; a bare name may also
    /// come from a selective import.
    fn lookup_type(&self, name: &str) -> Option<ClassId> {
        if let Some(id) = self.own_type(name) {
            return Some(id);
        }
        match name.split_once('.') {
            Some((namespace, rest)) => {
                let target = self.namespace(namespace)?;
                self.export_type(target, rest)
            }
            None => self.selective(name).and_then(|target| self.export_type(target, name)),
        }
    }

    /// The module a namespace name binds, if an import binds one.
    fn namespace(&self, name: &str) -> Option<ModuleId> {
        self.imports[self.current_module.0 as usize]
            .iter()
            .find(|import| import.binding.as_deref() == Some(name))
            .map(|import| import.module)
    }

    /// The module a selective import brings `name` from.
    fn selective(&self, name: &str) -> Option<ModuleId> {
        self.imports[self.current_module.0 as usize]
            .iter()
            .find(|import| import.names.iter().any(|selected| selected.name == name))
            .map(|import| import.module)
    }

    /// A type `target` exports under `name`.
    fn export_type(&self, target: ModuleId, name: &str) -> Option<ClassId> {
        let index = target.0 as usize;
        if !self.exported[index].contains(name) {
            return None;
        }
        self.module_types[index].get(name).copied()
    }

    /// A value `target` exports under `name`.
    fn export_value(&self, target: ModuleId, name: &str) -> Option<Resolution> {
        let index = target.0 as usize;
        if !self.exported[index].contains(name) {
            return None;
        }
        self.module_names[index].get(name).copied()
    }

    /// Resolves a value name: what is in scope, then what an import brings in.
    ///
    /// A module's own declaration wins over an imported one, which falls out
    /// of asking the scopes first.
    fn lookup_value(&self, name: &str) -> Option<Resolution> {
        if let Some(resolution) = self.scopes.lookup(name) {
            return Some(resolution);
        }
        // A namespace is not in any scope: it is the import itself.
        if let Some(module) = self.namespace(name) {
            return Some(Resolution::Module(module));
        }
        self.selective(name).and_then(|target| self.export_value(target, name))
    }

    /// The enum a type names, if it names one.
    fn enum_of(&self, ty: TypeId) -> Option<(EnumId, &analysis::EnumInfo)> {
        let def = self.store.get(ty).as_def()?;
        self.enums
            .iter()
            .position(|item| item.def == def)
            .map(|index| (EnumId(index as u32), &self.enums[index]))
    }

    /// The class a type names, if it names one.
    ///
    /// The scan is linear because a program has few classes; the alternative
    /// is a second index to keep in step with the first.
    fn class_of(&self, ty: TypeId) -> Option<(ClassId, &ClassInfo)> {
        let def = self.store.get(ty).as_def()?;
        self.classes
            .iter()
            .position(|class| class.def == def)
            .map(|index| (ClassId(index as u32), &self.classes[index]))
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
