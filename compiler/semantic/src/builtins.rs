//! The built-in functions and methods the compiler knows about.
//!
//! These are the operations the compiler must understand itself: printing,
//! converting a value to text, and the assertion used by the test runner.
//! Everything else belongs in the standard library, written in Noto. Each
//! builtin maps to one Noto IR intrinsic, which the backend turns into a call
//! into the runtime.

use noto_types::{Primitive, Type, TypeId, TypeStore};

/// An operation implemented by the compiler and its runtime.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Builtin {
    /// `print(value: String)`
    PrintString,
    /// `println(value: String)`
    PrintlnString,
    /// `print(value: Int)`
    PrintInt,
    /// `println(value: Int)`
    PrintlnInt,
    /// `print(value: Bool)`
    PrintBool,
    /// `println(value: Bool)`
    PrintlnBool,
    /// `println()`
    PrintlnEmpty,
    /// `Int.toString(): String`
    IntToString,
    /// `Bool.toString(): String`
    BoolToString,
    /// `String.toString(): String`, which returns the receiver unchanged.
    StringToString,
    /// `String.length: Int`
    StringLength,
    /// `[T].length: Int`
    ListLength,
    /// `assert(condition: Bool)`
    Assert,
}

impl Builtin {
    /// The name this builtin is written with in source.
    pub fn name(self) -> &'static str {
        use Builtin::*;
        match self {
            PrintString | PrintInt | PrintBool => "print",
            PrintlnString | PrintlnInt | PrintlnBool | PrintlnEmpty => "println",
            IntToString | BoolToString | StringToString => "toString",
            StringLength | ListLength => "length",
            Assert => "assert",
        }
    }

    /// The parameter types this overload accepts.
    pub fn parameters(self, store: &TypeStore) -> Vec<TypeId> {
        use Builtin::*;
        match self {
            PrintString | PrintlnString => vec![store.string()],
            PrintInt | PrintlnInt => vec![store.int()],
            PrintBool | PrintlnBool | Assert => vec![store.bool()],
            PrintlnEmpty | IntToString | BoolToString | StringToString | StringLength
            | ListLength => Vec::new(),
        }
    }

    /// The type this builtin produces.
    pub fn result(self, store: &TypeStore) -> TypeId {
        use Builtin::*;
        match self {
            IntToString | BoolToString | StringToString => store.string(),
            StringLength | ListLength => store.int(),
            _ => store.unit(),
        }
    }

    /// Whether the builtin is called as a method on a receiver.
    pub fn is_method(self) -> bool {
        use Builtin::*;
        matches!(self, IntToString | BoolToString | StringToString | StringLength | ListLength)
    }

    /// Whether the builtin is read as a property rather than called.
    pub fn is_property(self) -> bool {
        matches!(self, Builtin::StringLength | Builtin::ListLength)
    }
}

/// Every free function the compiler provides.
pub const FREE_FUNCTIONS: &[Builtin] = &[
    Builtin::PrintString,
    Builtin::PrintlnString,
    Builtin::PrintInt,
    Builtin::PrintlnInt,
    Builtin::PrintBool,
    Builtin::PrintlnBool,
    Builtin::PrintlnEmpty,
    Builtin::Assert,
];

/// The overloads of a free function with the given name.
pub fn free_overloads(name: &str) -> Vec<Builtin> {
    FREE_FUNCTIONS.iter().copied().filter(|builtin| builtin.name() == name).collect()
}

/// The builtin member `name` on `receiver`, if there is one.
pub fn member(store: &TypeStore, receiver: TypeId, name: &str) -> Option<Builtin> {
    let ty = store.get(receiver);
    match (ty, name) {
        (Type::String, "toString") => Some(Builtin::StringToString),
        (Type::String, "length") => Some(Builtin::StringLength),
        (Type::List(_), "length") => Some(Builtin::ListLength),
        (Type::Primitive(Primitive::Bool), "toString") => Some(Builtin::BoolToString),
        (Type::Primitive(primitive), "toString") if primitive.is_integer() => {
            Some(Builtin::IntToString)
        }
        _ => None,
    }
}

/// The builtin that converts `ty` to text, used when lowering interpolation.
pub fn to_string_for(store: &TypeStore, ty: TypeId) -> Option<Builtin> {
    match store.get(ty) {
        Type::String => Some(Builtin::StringToString),
        Type::Primitive(Primitive::Bool) => Some(Builtin::BoolToString),
        Type::Primitive(primitive) if primitive.is_integer() => Some(Builtin::IntToString),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn println_has_one_overload_per_printable_type() {
        let overloads = free_overloads("println");
        assert_eq!(overloads.len(), 4);
        assert!(overloads.contains(&Builtin::PrintlnEmpty));
    }

    #[test]
    fn to_string_is_found_on_every_integer_width() {
        let mut store = TypeStore::new();
        for primitive in [Primitive::Int, Primitive::Int32, Primitive::UInt8] {
            let ty = store.primitive(primitive);
            assert_eq!(member(&store, ty, "toString"), Some(Builtin::IntToString));
        }
        let string = store.string();
        assert_eq!(member(&store, string, "length"), Some(Builtin::StringLength));
        assert_eq!(member(&store, string, "nope"), None);
    }
}
