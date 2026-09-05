//! The Noto type system.
//!
//! This crate owns the *representation* of types and the rules that relate
//! them — assignability, nullability, numeric width. It knows nothing about
//! the AST or about how a program is checked; `noto-semantic` builds types
//! from source and asks the questions defined here.
//!
//! Types are interned into a [`TypeStore`] and referred to by [`TypeId`], so
//! comparing two types is an integer comparison and a type can be attached to
//! an AST node without cloning it.

#![deny(missing_docs)]

mod primitive;
mod store;

pub use primitive::{FloatWidth, IntWidth, Primitive};
pub use store::{TypeId, TypeStore};

/// A structural description of a type.
///
/// Named types (`class`, `struct`, `enum`, `interface`) are represented by a
/// [`Type::Named`] carrying a [`DefId`]; their contents live in the definition
/// table owned by `noto-semantic`. That keeps this crate free of declaration
/// data while still letting it answer questions about type identity.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub enum Type {
    /// A built-in scalar such as `Int` or `Bool`.
    Primitive(Primitive),
    /// `String`.
    ///
    /// Strings are built in rather than a library type because the compiler
    /// creates them for literals and interpolation.
    String,
    /// A user-declared type, possibly with type arguments applied.
    Named {
        /// Which declaration this names.
        def: DefId,
        /// The type arguments, empty for a non-generic type.
        arguments: Vec<TypeId>,
    },
    /// `T?`: `T` extended with the absence of a value.
    Nullable(TypeId),
    /// `[T]`: a fixed-length sequence of `T`.
    ///
    /// Built in rather than a library type because the compiler creates one
    /// for every list literal, and because indexing it is a machine operation
    /// rather than a call.
    List(TypeId),
    /// `(A, B)`: an anonymous product.
    Tuple(Vec<TypeId>),
    /// The type of a function value.
    Function {
        /// Parameter types in declaration order.
        parameters: Vec<TypeId>,
        /// The result type.
        result: TypeId,
        /// Whether calling it produces a `Task`.
        is_async: bool,
    },
    /// A type parameter standing for an argument not yet supplied.
    Parameter {
        /// The declaration that introduced it.
        def: DefId,
        /// Its position in that declaration's parameter list.
        index: u32,
        /// Its name, kept for diagnostics.
        name: String,
    },
    /// `Unit`: the result of a function that returns nothing useful.
    ///
    /// `Unit` has exactly one value, so returning it carries no information
    /// but is still a value — unlike `Nothing`.
    Unit,
    /// `Nothing`: the type of an expression that never produces a value.
    ///
    /// `return`, `break` and a call that always panics have this type. It is a
    /// subtype of every type, which is what lets `val x: Int = if c { 1 } else
    /// { return }` typecheck.
    Nothing,
    /// `Any`: the supertype of every non-null type.
    Any,
    /// A type the compiler could not work out. Errors involving it are
    /// suppressed so that one mistake does not cascade.
    Error,
}

/// What kind of declaration a [`DefId`] names.
///
/// The type store keeps this because a [`Type::Named`] carries only an id,
/// and the difference between a class and an enum decides everything from
/// how a value of it is represented to what a pattern may match.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum DefKind {
    /// A `class`: a reference to fields on the heap.
    Class,
    /// An `enum` whose cases carry no data: a value of it is its tag.
    Enum,
    /// A `fn`, which owns the type parameters it declares.
    Function,
    /// An `enum` some case of which carries data: a value of it is a pointer
    /// to its tag followed by that case's fields.
    EnumWithData,
}

/// Identifies a declaration in the definition table.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct DefId(pub u32);

impl DefId {
    /// A placeholder for a declaration that could not be resolved.
    pub const ERROR: DefId = DefId(u32::MAX);
}

impl Type {
    /// Whether the type admits `null`.
    pub fn is_nullable(&self) -> bool {
        matches!(self, Type::Nullable(_))
    }

    /// Whether this is the error type.
    pub fn is_error(&self) -> bool {
        matches!(self, Type::Error)
    }

    /// Whether this type has no values, so control never continues past an
    /// expression of this type.
    pub fn is_never(&self) -> bool {
        matches!(self, Type::Nothing)
    }

    /// Whether the type is a built-in number.
    pub fn is_numeric(&self) -> bool {
        matches!(self, Type::Primitive(primitive) if primitive.is_numeric())
    }

    /// Whether the type is a built-in integer.
    pub fn is_integer(&self) -> bool {
        matches!(self, Type::Primitive(primitive) if primitive.is_integer())
    }

    /// The primitive behind this type, if it is one.
    pub fn as_primitive(&self) -> Option<Primitive> {
        match self {
            Type::Primitive(primitive) => Some(*primitive),
            _ => None,
        }
    }

    /// The declaration this type names, if it names one.
    pub fn as_def(&self) -> Option<DefId> {
        match self {
            Type::Named { def, .. } => Some(*def),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nothing_and_unit_are_different() {
        assert!(Type::Nothing.is_never());
        assert!(!Type::Unit.is_never());
    }

    #[test]
    fn numeric_classification() {
        assert!(Type::Primitive(Primitive::Int).is_numeric());
        assert!(Type::Primitive(Primitive::Int).is_integer());
        assert!(Type::Primitive(Primitive::Float64).is_numeric());
        assert!(!Type::Primitive(Primitive::Float64).is_integer());
        assert!(!Type::Primitive(Primitive::Bool).is_numeric());
        assert!(!Type::String.is_numeric());
    }
}
