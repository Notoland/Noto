//! What semantic analysis produces.

use crate::builtins::Builtin;
use noto_ast::NodeId;
use noto_span::Span;
use noto_types::{DefId, TypeId, TypeStore};
use std::collections::HashMap;

/// Identifies a local binding within a compilation.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct LocalId(pub u32);

/// Identifies a checked function.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct FunctionId(pub u32);

/// Identifies a compile-time constant.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct ConstId(pub u32);

/// Identifies a declared class.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct ClassId(pub u32);

/// What a name in the program refers to.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Resolution {
    /// A local binding or parameter.
    Local(LocalId),
    /// A function declared in the program.
    Function(FunctionId),
    /// A compile-time constant.
    Const(ConstId),
    /// A declared class, named either as a type or as its constructor.
    Class(ClassId),
    /// A field read or written through a receiver.
    Field {
        /// The class the field belongs to.
        class: ClassId,
        /// Its position in the class's field list, which is also its slot.
        index: u32,
    },
    /// An operation provided by the compiler.
    Builtin(Builtin),
    /// A name that could not be resolved; already reported.
    Error,
}

/// A local binding.
#[derive(Clone, Debug)]
pub struct LocalInfo {
    /// The name as written.
    pub name: String,
    /// Its type.
    pub ty: TypeId,
    /// Whether it may be reassigned.
    pub is_mutable: bool,
    /// Whether it is a parameter of its function.
    pub is_parameter: bool,
    /// The function it belongs to.
    pub function: FunctionId,
    /// Where it was declared.
    pub span: Span,
}

/// A checked function.
#[derive(Clone, Debug)]
pub struct FunctionInfo {
    /// The name as written.
    pub name: String,
    /// Its parameters, in declaration order.
    pub parameters: Vec<LocalId>,
    /// The declared or inferred result type.
    pub result: TypeId,
    /// Every local it declares, parameters first.
    pub locals: Vec<LocalId>,
    /// The AST node of its body, or `None` for an abstract declaration.
    pub body: Option<NodeId>,
    /// Whether it was declared `async`.
    pub is_async: bool,
    /// Where it was declared.
    pub span: Span,
}

/// A checked constant.
#[derive(Clone, Debug)]
pub struct ConstInfo {
    /// The name as written.
    pub name: String,
    /// Its type.
    pub ty: TypeId,
    /// The value it was folded to.
    pub value: ConstValue,
    /// Where it was declared.
    pub span: Span,
}

/// The value of a constant, computed at compile time.
#[derive(Clone, PartialEq, Debug)]
pub enum ConstValue {
    /// An integer.
    Int(i128),
    /// A boolean.
    Bool(bool),
    /// Text.
    Str(String),
    /// A character.
    Char(char),
    /// A value that could not be folded; already reported.
    Error,
}

/// One field of a class.
#[derive(Clone, Debug)]
pub struct FieldInfo {
    /// The name as written.
    pub name: String,
    /// Its declared type.
    pub ty: TypeId,
    /// Whether it may be reassigned, from `val` or `var`.
    pub is_mutable: bool,
    /// Where it was declared.
    pub span: Span,
}

/// A checked class declaration.
#[derive(Clone, Debug)]
pub struct ClassInfo {
    /// The name as written.
    pub name: String,
    /// Its fields, in declaration order. The order is the object's layout.
    pub fields: Vec<FieldInfo>,
    /// The type that names it.
    pub ty: TypeId,
    /// The declaration id carried by that type.
    pub def: DefId,
    /// Where it was declared.
    pub span: Span,
}

impl ClassInfo {
    /// Looks a field up by name, returning its index and what is known of it.
    pub fn field(&self, name: &str) -> Option<(u32, &FieldInfo)> {
        self.fields
            .iter()
            .position(|field| field.name == name)
            .map(|index| (index as u32, &self.fields[index]))
    }
}

/// A checked test declaration.
#[derive(Clone, Debug)]
pub struct TestInfo {
    /// The description written after `test`.
    pub name: String,
    /// The function the test body was checked as.
    pub function: FunctionId,
    /// Where it was declared.
    pub span: Span,
}

/// Everything semantic analysis learned about a module.
///
/// The AST is never modified; results are looked up by [`NodeId`]. That keeps
/// the tree usable by tooling and lets analysis be re-run on an edited file
/// without rebuilding anything else.
pub struct Analysis {
    /// The type of every expression, keyed by node id.
    pub types: HashMap<NodeId, TypeId>,
    /// What every name refers to, keyed by the node that mentions it.
    pub resolutions: HashMap<NodeId, Resolution>,
    /// Every local binding, indexed by [`LocalId`].
    pub locals: Vec<LocalInfo>,
    /// Every function, indexed by [`FunctionId`].
    pub functions: Vec<FunctionInfo>,
    /// Every constant, indexed by [`ConstId`].
    pub constants: Vec<ConstInfo>,
    /// Every class, indexed by [`ClassId`].
    pub classes: Vec<ClassInfo>,
    /// Every test.
    pub tests: Vec<TestInfo>,
    /// The program's entry point, if it declares one.
    pub entry: Option<FunctionId>,
    /// The interned types the rest of the compiler shares.
    pub store: TypeStore,
}

impl Analysis {
    /// The class a type names, if it names one.
    ///
    /// A [`Type::Named`](noto_types::Type::Named) carries only a `DefId`;
    /// this is how the rest of the compiler gets from a type back to the
    /// declaration. The scan is linear because a program has few classes and
    /// a second index would be one more thing to keep in step.
    pub fn class_of(&self, ty: TypeId) -> Option<(ClassId, &ClassInfo)> {
        let def = self.store.get(ty).as_def()?;
        self.classes
            .iter()
            .position(|class| class.def == def)
            .map(|index| (ClassId(index as u32), &self.classes[index]))
    }

    /// Looks a class up by id.
    pub fn class(&self, id: ClassId) -> &ClassInfo {
        &self.classes[id.0 as usize]
    }

    /// The type recorded for a node, or the error type if there is none.
    pub fn type_of(&self, id: NodeId) -> TypeId {
        self.types.get(&id).copied().unwrap_or_else(|| self.store.error())
    }

    /// What a node's name resolved to.
    pub fn resolution(&self, id: NodeId) -> Option<Resolution> {
        self.resolutions.get(&id).copied()
    }

    /// Looks a local up.
    pub fn local(&self, id: LocalId) -> &LocalInfo {
        &self.locals[id.0 as usize]
    }

    /// Looks a function up.
    pub fn function(&self, id: FunctionId) -> &FunctionInfo {
        &self.functions[id.0 as usize]
    }

    /// Looks a constant up.
    pub fn constant(&self, id: ConstId) -> &ConstInfo {
        &self.constants[id.0 as usize]
    }

    /// Finds a function by name.
    pub fn function_named(&self, name: &str) -> Option<FunctionId> {
        self.functions
            .iter()
            .position(|function| function.name == name)
            .map(|index| FunctionId(index as u32))
    }
}
