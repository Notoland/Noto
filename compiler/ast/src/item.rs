//! Top-level and member declarations.

use crate::{Block, Expr, Ident, NodeId, Path, TypeExpr};
use noto_span::Span;

/// How widely a declaration is visible.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Visibility {
    /// Visible everywhere, including to other packages.
    Public,
    /// Visible only inside the declaring module. The default.
    #[default]
    Private,
    /// Visible to the declaring type and its subtypes.
    Protected,
    /// Visible everywhere inside the declaring package.
    Internal,
}

impl Visibility {
    /// The source spelling of the modifier.
    pub fn as_str(self) -> &'static str {
        match self {
            Visibility::Public => "public",
            Visibility::Private => "private",
            Visibility::Protected => "protected",
            Visibility::Internal => "internal",
        }
    }
}

/// The modifier set written in front of a declaration.
#[derive(Clone, PartialEq, Debug, Default)]
pub struct Modifiers {
    /// The visibility, defaulting to [`Visibility::Private`].
    pub visibility: Visibility,
    /// Whether the visibility was written explicitly.
    pub visibility_explicit: bool,
    /// `abstract`
    pub is_abstract: bool,
    /// `sealed`
    pub is_sealed: bool,
    /// `override`
    pub is_override: bool,
    /// `async`
    pub is_async: bool,
    /// `export`
    pub is_exported: bool,
    /// The span covering every modifier written.
    pub span: Option<Span>,
}

/// An `@name(args)` annotation.
#[derive(Clone, PartialEq, Debug)]
pub struct Attribute {
    /// The attribute name.
    pub name: Ident,
    /// Its arguments, if it was written with any.
    pub arguments: Vec<Expr>,
    /// Where it appeared, `@` included.
    pub span: Span,
}

/// A declared type parameter such as `T: Comparable`.
#[derive(Clone, PartialEq, Debug)]
pub struct TypeParam {
    /// The parameter name.
    pub name: Ident,
    /// Bounds the argument must satisfy.
    pub bounds: Vec<TypeExpr>,
    /// Where it appeared.
    pub span: Span,
    /// The node's id.
    pub id: NodeId,
}

/// A function or constructor parameter.
#[derive(Clone, PartialEq, Debug)]
pub struct Param {
    /// The parameter name.
    pub name: Ident,
    /// Its declared type. Lambdas may leave this out and have it inferred.
    pub ty: Option<TypeExpr>,
    /// The default value, making the parameter optional at call sites.
    pub default: Option<Expr>,
    /// Where it appeared.
    pub span: Span,
    /// The node's id.
    pub id: NodeId,
}

/// A function declaration.
#[derive(Clone, PartialEq, Debug)]
pub struct FnItem {
    /// The function name.
    pub name: Ident,
    /// The receiver type, when this is an extension function such as
    /// `fn String.isValidEmail()`.
    pub receiver: Option<TypeExpr>,
    /// Its type parameters.
    pub type_params: Vec<TypeParam>,
    /// Its parameters.
    pub params: Vec<Param>,
    /// The declared result type. `None` means `Unit` unless the body is a
    /// single expression, in which case it is inferred.
    pub result: Option<TypeExpr>,
    /// The body. `None` for an abstract or interface method.
    pub body: Option<Block>,
    /// Whether the function is `async`.
    pub is_async: bool,
    /// The node's id.
    pub id: NodeId,
}

/// Which flavour of type a declaration introduces.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ClassKind {
    /// `class`: a reference type with identity.
    Class,
    /// `struct`: a value type, copied on assignment.
    Struct,
    /// `data class`: a class whose identity is its contents.
    DataClass,
    /// `data struct`: a value type whose identity is its contents.
    DataStruct,
}

impl ClassKind {
    /// The source spelling of the keyword sequence.
    pub fn as_str(self) -> &'static str {
        match self {
            ClassKind::Class => "class",
            ClassKind::Struct => "struct",
            ClassKind::DataClass => "data class",
            ClassKind::DataStruct => "data struct",
        }
    }

    /// Whether values of the type are copied rather than referenced.
    pub fn is_value_type(self) -> bool {
        matches!(self, ClassKind::Struct | ClassKind::DataStruct)
    }

    /// Whether the compiler derives equality, hashing, text and copy members.
    pub fn is_data(self) -> bool {
        matches!(self, ClassKind::DataClass | ClassKind::DataStruct)
    }
}

/// A stored field.
#[derive(Clone, PartialEq, Debug)]
pub struct Field {
    /// The modifiers written on it.
    pub modifiers: Modifiers,
    /// Whether the field may be reassigned.
    pub kind: crate::LetKind,
    /// The field name.
    pub name: Ident,
    /// Its declared type.
    pub ty: Option<TypeExpr>,
    /// Its default value.
    pub default: Option<Expr>,
    /// Where it appeared.
    pub span: Span,
    /// The node's id.
    pub id: NodeId,
}

/// A `get`/`set` accessor body.
#[derive(Clone, PartialEq, Debug)]
pub struct PropertyAccessor {
    /// The modifiers written on the accessor, which may narrow the property's
    /// own visibility.
    pub modifiers: Modifiers,
    /// The body. `None` declares the accessor without defining it, which asks
    /// the compiler to supply the default implementation.
    pub body: Option<Block>,
    /// Where it appeared.
    pub span: Span,
    /// The node's id.
    pub id: NodeId,
}

/// A property: a member accessed like a field but backed by accessors.
#[derive(Clone, PartialEq, Debug)]
pub struct Property {
    /// The modifiers written on it.
    pub modifiers: Modifiers,
    /// Whether the property is settable at all.
    pub kind: crate::LetKind,
    /// The property name.
    pub name: Ident,
    /// Its declared type.
    pub ty: Option<TypeExpr>,
    /// Its initialiser.
    pub default: Option<Expr>,
    /// The `get` accessor.
    pub getter: Option<PropertyAccessor>,
    /// The `set` accessor.
    pub setter: Option<PropertyAccessor>,
    /// Where it appeared.
    pub span: Span,
    /// The node's id.
    pub id: NodeId,
}

/// A `class`, `struct` or `data class` declaration.
#[derive(Clone, PartialEq, Debug)]
pub struct TypeDeclItem {
    /// Which flavour was declared.
    pub class_kind: ClassKind,
    /// The type name.
    pub name: Ident,
    /// Its type parameters.
    pub type_params: Vec<TypeParam>,
    /// The primary constructor parameters, written after the name.
    pub primary_params: Vec<Field>,
    /// The base class, if one was named. Noto allows at most one.
    pub base: Option<TypeExpr>,
    /// The interfaces the type implements.
    pub interfaces: Vec<TypeExpr>,
    /// Fields declared in the body.
    pub fields: Vec<Field>,
    /// Properties declared in the body.
    pub properties: Vec<Property>,
    /// Methods declared in the body.
    pub methods: Vec<Item>,
    /// The node's id.
    pub id: NodeId,
}

/// An `interface` declaration.
#[derive(Clone, PartialEq, Debug)]
pub struct InterfaceItem {
    /// The interface name.
    pub name: Ident,
    /// Its type parameters.
    pub type_params: Vec<TypeParam>,
    /// The interfaces it extends.
    pub interfaces: Vec<TypeExpr>,
    /// Properties it requires.
    pub properties: Vec<Property>,
    /// Methods it requires, with or without default bodies.
    pub methods: Vec<Item>,
    /// The node's id.
    pub id: NodeId,
}

/// One case of an enum.
#[derive(Clone, PartialEq, Debug)]
pub struct EnumCase {
    /// The case name.
    pub name: Ident,
    /// Associated data, as in `Success(value: Int)`.
    pub fields: Vec<Field>,
    /// An explicit value, as in `Red = 1`.
    pub value: Option<Expr>,
    /// Where it appeared.
    pub span: Span,
    /// The node's id.
    pub id: NodeId,
}

/// An `enum` declaration.
#[derive(Clone, PartialEq, Debug)]
pub struct EnumItem {
    /// The enum name.
    pub name: Ident,
    /// Its type parameters.
    pub type_params: Vec<TypeParam>,
    /// The interfaces it implements.
    pub interfaces: Vec<TypeExpr>,
    /// Its cases, in declaration order.
    pub cases: Vec<EnumCase>,
    /// Methods declared in the body.
    pub methods: Vec<Item>,
    /// The node's id.
    pub id: NodeId,
}

/// A `const` declaration.
#[derive(Clone, PartialEq, Debug)]
pub struct ConstItem {
    /// The constant name.
    pub name: Ident,
    /// Its declared type.
    pub ty: Option<TypeExpr>,
    /// Its value, which must be computable at compile time.
    pub value: Expr,
    /// The node's id.
    pub id: NodeId,
}

/// An `import` declaration.
#[derive(Clone, PartialEq, Debug)]
pub struct ImportItem {
    /// The module or item being imported.
    pub path: Path,
    /// The names selected from it, as in `import std.io { File, Path }`.
    /// Empty means the whole module is imported under its own name.
    pub names: Vec<Ident>,
    /// A local alias, as in `import std.collections as coll`.
    pub alias: Option<Ident>,
    /// The node's id.
    pub id: NodeId,
}

/// A `test "name" { .. }` declaration.
#[derive(Clone, PartialEq, Debug)]
pub struct TestItem {
    /// The test's description.
    pub name: String,
    /// The span of the description literal.
    pub name_span: Span,
    /// The test body.
    pub body: Block,
    /// The node's id.
    pub id: NodeId,
}

/// A declaration.
#[derive(Clone, PartialEq, Debug)]
pub struct Item {
    /// What is being declared.
    pub kind: ItemKind,
    /// The modifiers written on it.
    pub modifiers: Modifiers,
    /// The attributes written on it.
    pub attributes: Vec<Attribute>,
    /// The doc comment written above it, lines joined with `\n`.
    pub doc: Option<String>,
    /// Where it appeared, modifiers and attributes included.
    pub span: Span,
}

/// The shapes a declaration can take.
#[derive(Clone, PartialEq, Debug)]
pub enum ItemKind {
    /// A function.
    Fn(FnItem),
    /// A class, struct or data class.
    TypeDecl(TypeDeclItem),
    /// An interface.
    Interface(InterfaceItem),
    /// An enum.
    Enum(EnumItem),
    /// A constant.
    Const(ConstItem),
    /// An import.
    Import(ImportItem),
    /// A test.
    Test(TestItem),
    /// A declaration the parser could not read; already reported.
    Error,
}

impl Item {
    /// The declared name, for items that have one.
    pub fn name(&self) -> Option<&Ident> {
        match &self.kind {
            ItemKind::Fn(item) => Some(&item.name),
            ItemKind::TypeDecl(item) => Some(&item.name),
            ItemKind::Interface(item) => Some(&item.name),
            ItemKind::Enum(item) => Some(&item.name),
            ItemKind::Const(item) => Some(&item.name),
            ItemKind::Import(_) | ItemKind::Test(_) | ItemKind::Error => None,
        }
    }

    /// The item's node id.
    pub fn id(&self) -> NodeId {
        match &self.kind {
            ItemKind::Fn(item) => item.id,
            ItemKind::TypeDecl(item) => item.id,
            ItemKind::Interface(item) => item.id,
            ItemKind::Enum(item) => item.id,
            ItemKind::Const(item) => item.id,
            ItemKind::Import(item) => item.id,
            ItemKind::Test(item) => item.id,
            ItemKind::Error => NodeId::DUMMY,
        }
    }

    /// A word describing the kind of declaration, for diagnostics.
    pub fn describe(&self) -> &'static str {
        match &self.kind {
            ItemKind::Fn(_) => "function",
            ItemKind::TypeDecl(item) => item.class_kind.as_str(),
            ItemKind::Interface(_) => "interface",
            ItemKind::Enum(_) => "enum",
            ItemKind::Const(_) => "constant",
            ItemKind::Import(_) => "import",
            ItemKind::Test(_) => "test",
            ItemKind::Error => "declaration",
        }
    }

    /// Whether an attribute with this name is written on the item.
    pub fn has_attribute(&self, name: &str) -> bool {
        self.attributes.iter().any(|attribute| attribute.name.name == name)
    }
}
