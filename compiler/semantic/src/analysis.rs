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

/// Identifies a module within one compilation.
///
/// `ModuleId(0)` is always the root — the file the compiler was pointed at.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Default)]
pub struct ModuleId(pub u32);

impl ModuleId {
    /// The module the compiler was pointed at.
    pub const ROOT: ModuleId = ModuleId(0);
}

/// Identifies a declared class.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct ClassId(pub u32);

/// Identifies a declared enum.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct EnumId(pub u32);

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
    /// A method called through a receiver.
    Method(FunctionId),
    /// A module bound by an import, used as a namespace.
    Module(ModuleId),
    /// A declared enum, named as a type or as the namespace of its cases.
    Enum(EnumId),
    /// One case of an enum.
    EnumCase {
        /// The enum it belongs to.
        enum_id: EnumId,
        /// Its position in the case list, which is also its tag.
        index: u32,
    },
    /// A field read or written through a receiver.
    Field {
        /// The class the field belongs to.
        class: ClassId,
        /// Its position in the class's field list, which is also its slot.
        index: u32,
    },
    /// A property read or written through a receiver. Reading calls its
    /// getter; writing, when it has a setter, calls that.
    Property {
        /// The class the property belongs to.
        class: ClassId,
        /// Its position in the class's property list.
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
    /// The module that declares it.
    pub module: ModuleId,
    /// Whether it is visible to a module that imports this one.
    pub is_exported: bool,
    /// Its parameters, in declaration order.
    pub parameters: Vec<LocalId>,
    /// The declared or inferred result type.
    pub result: TypeId,
    /// Every local it declares, parameters first.
    pub locals: Vec<LocalId>,
    /// The AST node of its body, or `None` for an abstract declaration.
    pub body: Option<NodeId>,
    /// The class this function initialises, when it is a synthesised
    /// `Class.<init>`. Such a function has no body block; lowering builds it
    /// from the field initialisers instead.
    pub init_of: Option<ClassId>,
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
    /// The module that declares it.
    pub module: ModuleId,
    /// Whether it is visible to a module that imports this one.
    pub is_exported: bool,
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
    /// The expression that initialises it, for a field declared in the class
    /// body. Constructor parameters have none: the argument initialises them.
    pub initializer: Option<NodeId>,
    /// Where it was declared.
    pub span: Span,
}

/// One property of a class: a member read like a field but backed by
/// accessors.
#[derive(Clone, Debug)]
pub struct PropertyInfo {
    /// The name as written.
    pub name: String,
    /// Its declared type.
    pub ty: TypeId,
    /// Whether it may be written at all, from `val` or `var`.
    pub is_mutable: bool,
    /// The function a read calls, taking the receiver.
    pub getter: FunctionId,
    /// The function a write calls, taking the receiver and the value; `None`
    /// when the property cannot be assigned to.
    pub setter: Option<FunctionId>,
    /// Where it was declared.
    pub span: Span,
}

/// One method of a class.
#[derive(Clone, Debug)]
pub struct MethodInfo {
    /// The name as written, without the class prefix.
    pub name: String,
    /// The function it was checked and lowered as.
    pub function: FunctionId,
}

/// A checked class declaration.
#[derive(Clone, Debug)]
pub struct ClassInfo {
    /// The name as written.
    pub name: String,
    /// The module that declares it.
    pub module: ModuleId,
    /// Whether it is visible to a module that imports this one.
    pub is_exported: bool,
    /// Its fields, in declaration order. The order is the object's layout.
    pub fields: Vec<FieldInfo>,
    /// How many of those fields are primary constructor parameters. A
    /// construction call takes exactly this many arguments; the fields after
    /// them are declared in the class body and carry their own initialisers.
    pub primary_count: u32,
    /// Its properties, in declaration order.
    pub properties: Vec<PropertyInfo>,
    /// Its methods, in declaration order.
    pub methods: Vec<MethodInfo>,
    /// The synthesised `Class.<init>` that runs the body fields' initialisers,
    /// present when any field has one.
    pub init: Option<FunctionId>,
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

    /// Looks a method up by name.
    pub fn method(&self, name: &str) -> Option<&MethodInfo> {
        self.methods.iter().find(|method| method.name == name)
    }

    /// Looks a property up by name, returning its index and what is known of
    /// it.
    pub fn property(&self, name: &str) -> Option<(u32, &PropertyInfo)> {
        self.properties
            .iter()
            .position(|property| property.name == name)
            .map(|index| (index as u32, &self.properties[index]))
    }
}

/// One case of an enum.
#[derive(Clone, Debug)]
pub struct EnumCaseInfo {
    /// The name as written.
    pub name: String,
    /// The data it carries, in declaration order; empty for a plain case.
    pub fields: Vec<FieldInfo>,
    /// Where it was declared.
    pub span: Span,
}

/// A checked enum declaration.
#[derive(Clone, Debug)]
pub struct EnumInfo {
    /// The name as written.
    pub name: String,
    /// The module that declares it.
    pub module: ModuleId,
    /// Whether it is visible to a module that imports this one.
    pub is_exported: bool,
    /// Its cases, in declaration order. A case's position is its tag.
    pub cases: Vec<EnumCaseInfo>,
    /// Whether any case carries data, which decides how a value is
    /// represented: a pointer when it does, the tag itself when it does not.
    pub has_data: bool,
    /// The type that names it.
    pub ty: TypeId,
    /// The declaration id carried by that type.
    pub def: DefId,
    /// Where it was declared.
    pub span: Span,
}

impl EnumInfo {
    /// The most fields any one case carries, which sizes the payload.
    pub fn widest_case(&self) -> usize {
        self.cases.iter().map(|case| case.fields.len()).max().unwrap_or(0)
    }

    /// Looks a case up by name, returning its tag and what is known of it.
    pub fn case(&self, name: &str) -> Option<(u32, &EnumCaseInfo)> {
        self.cases
            .iter()
            .position(|case| case.name == name)
            .map(|index| (index as u32, &self.cases[index]))
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
    /// Every enum, indexed by [`EnumId`].
    pub enums: Vec<EnumInfo>,
    /// The name of every module, indexed by [`ModuleId`]; the root's is empty.
    pub modules: Vec<String>,
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

    /// Looks an enum up by id.
    pub fn enum_at(&self, id: EnumId) -> &EnumInfo {
        &self.enums[id.0 as usize]
    }

    /// The enum a type names, if it names one.
    pub fn enum_of(&self, ty: TypeId) -> Option<(EnumId, &EnumInfo)> {
        let def = self.store.get(ty).as_def()?;
        self.enums
            .iter()
            .position(|item| item.def == def)
            .map(|index| (EnumId(index as u32), &self.enums[index]))
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
