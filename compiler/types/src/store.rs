//! Interning of types.

use crate::{DefId, DefKind, Primitive, Type};
use std::collections::HashMap;

/// A handle to an interned [`Type`].
///
/// Two structurally equal types always get the same id, so type equality is an
/// integer comparison.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct TypeId(u32);

impl TypeId {
    /// The raw index, for use as a key in dense side tables.
    pub fn index(self) -> usize {
        self.0 as usize
    }
}

/// Owns every type in a compilation and hands out [`TypeId`]s.
pub struct TypeStore {
    types: Vec<Type>,
    interned: HashMap<Type, TypeId>,
    /// The primitives, interned up front so that lookups are free.
    well_known: WellKnown,
    /// What kind of declaration each [`DefId`] names.
    definition_kinds: Vec<DefKind>,
    /// The name of every declared type, indexed by [`DefId`].
    ///
    /// A [`Type::Named`] carries only its `DefId`; the rest of what a
    /// declaration means belongs to semantic analysis. The name is the one
    /// part the store needs, because it renders types into diagnostics.
    definitions: Vec<String>,
}

/// Ids of the types the compiler refers to by name.
#[derive(Clone, Copy, Debug)]
struct WellKnown {
    error: TypeId,
    unit: TypeId,
    nothing: TypeId,
    any: TypeId,
    string: TypeId,
    bool: TypeId,
    int: TypeId,
    float64: TypeId,
    char: TypeId,
    byte: TypeId,
}

impl Default for TypeStore {
    fn default() -> Self {
        Self::new()
    }
}

impl TypeStore {
    /// Creates a store with the built-in types already interned.
    pub fn new() -> Self {
        let mut store = TypeStore {
            types: Vec::new(),
            interned: HashMap::new(),
            definitions: Vec::new(),
            definition_kinds: Vec::new(),
            // Filled in immediately below; the placeholder is never observed.
            well_known: WellKnown {
                error: TypeId(0),
                unit: TypeId(0),
                nothing: TypeId(0),
                any: TypeId(0),
                string: TypeId(0),
                bool: TypeId(0),
                int: TypeId(0),
                float64: TypeId(0),
                char: TypeId(0),
                byte: TypeId(0),
            },
        };

        // `Error` is interned first so that TypeId(0) is always the error type,
        // which makes an uninitialised side table read as "unknown".
        let error = store.intern(Type::Error);
        let unit = store.intern(Type::Unit);
        let nothing = store.intern(Type::Nothing);
        let any = store.intern(Type::Any);
        let string = store.intern(Type::String);
        let bool = store.intern(Type::Primitive(Primitive::Bool));
        let int = store.intern(Type::Primitive(Primitive::Int));
        let float64 = store.intern(Type::Primitive(Primitive::Float64));
        let char = store.intern(Type::Primitive(Primitive::Char));
        let byte = store.intern(Type::Primitive(Primitive::Byte));

        store.well_known =
            WellKnown { error, unit, nothing, any, string, bool, int, float64, char, byte };
        store
    }

    /// Interns a type, returning the id it shares with every equal type.
    pub fn intern(&mut self, ty: Type) -> TypeId {
        if let Some(id) = self.interned.get(&ty) {
            return *id;
        }
        let id = TypeId(self.types.len() as u32);
        self.types.push(ty.clone());
        self.interned.insert(ty, id);
        id
    }

    /// Registers a declared type and returns the id that names it.
    pub fn declare(&mut self, name: impl Into<String>, kind: DefKind) -> DefId {
        self.definitions.push(name.into());
        self.definition_kinds.push(kind);
        DefId(self.definitions.len() as u32 - 1)
    }

    /// What kind of declaration an id names.
    pub fn definition_kind(&self, def: DefId) -> DefKind {
        self.definition_kinds.get(def.0 as usize).copied().unwrap_or(DefKind::Class)
    }

    /// The name a declaration was given, or `?` for one that never existed.
    pub fn definition_name(&self, def: DefId) -> &str {
        self.definitions.get(def.0 as usize).map(String::as_str).unwrap_or("?")
    }

    /// Looks up an interned type.
    pub fn get(&self, id: TypeId) -> &Type {
        &self.types[id.index()]
    }

    /// The number of distinct types interned.
    pub fn len(&self) -> usize {
        self.types.len()
    }

    /// Whether nothing has been interned. Never true in practice, since the
    /// built-ins are interned on construction.
    pub fn is_empty(&self) -> bool {
        self.types.is_empty()
    }

    // --- well-known types -------------------------------------------------

    /// The error type.
    pub fn error(&self) -> TypeId {
        self.well_known.error
    }
    /// `Unit`.
    pub fn unit(&self) -> TypeId {
        self.well_known.unit
    }
    /// `Nothing`.
    pub fn nothing(&self) -> TypeId {
        self.well_known.nothing
    }
    /// `Any`.
    pub fn any(&self) -> TypeId {
        self.well_known.any
    }
    /// `String`.
    pub fn string(&self) -> TypeId {
        self.well_known.string
    }
    /// `Bool`.
    pub fn bool(&self) -> TypeId {
        self.well_known.bool
    }
    /// `Int`.
    pub fn int(&self) -> TypeId {
        self.well_known.int
    }
    /// `Float64`.
    pub fn float64(&self) -> TypeId {
        self.well_known.float64
    }
    /// `Char`.
    pub fn char(&self) -> TypeId {
        self.well_known.char
    }
    /// `Byte`.
    pub fn byte(&self) -> TypeId {
        self.well_known.byte
    }

    /// Interns a primitive type.
    pub fn primitive(&mut self, primitive: Primitive) -> TypeId {
        self.intern(Type::Primitive(primitive))
    }

    /// Interns `T?`, collapsing `T??` to `T?`.
    pub fn nullable(&mut self, inner: TypeId) -> TypeId {
        if self.get(inner).is_nullable() || inner == self.error() {
            return inner;
        }
        // `Nothing?` is how `null` on its own is typed: the only value it can
        // hold is null.
        self.intern(Type::Nullable(inner))
    }

    /// Strips one level of `?`, leaving other types alone.
    pub fn unwrap_nullable(&self, id: TypeId) -> TypeId {
        match self.get(id) {
            Type::Nullable(inner) => *inner,
            _ => id,
        }
    }

    /// Whether the type admits `null`.
    pub fn is_nullable(&self, id: TypeId) -> bool {
        self.get(id).is_nullable()
    }

    // --- relations --------------------------------------------------------

    /// Whether a value of type `from` may be used where `to` is expected.
    ///
    /// Noto has no implicit numeric conversions: `Int32` is not assignable to
    /// `Int64` and the programmer writes `.toInt64()`. The only implicit moves
    /// are the ones that cannot lose information or surprise the reader —
    /// `Nothing` into anything, `T` into `T?`, and anything into `Any`.
    pub fn is_assignable(&self, from: TypeId, to: TypeId) -> bool {
        if from == to {
            return true;
        }
        let (from_ty, to_ty) = (self.get(from), self.get(to));

        // The error type absorbs everything so that one mistake produces one
        // diagnostic instead of a cascade.
        if from_ty.is_error() || to_ty.is_error() {
            return true;
        }

        // `Nothing` never produces a value, so it fits anywhere.
        if from_ty.is_never() {
            return true;
        }

        match (from_ty, to_ty) {
            // `T?` fits in `U?` exactly when `T` fits in `U`, which is what
            // makes `null` — a `Nothing?` — assignable to every nullable type.
            (Type::Nullable(from_inner), Type::Nullable(to_inner)) => {
                self.is_assignable(*from_inner, *to_inner)
            }
            // `T` may be used where `T?` is expected, but not the reverse.
            (_, Type::Nullable(inner)) => self.is_assignable(from, *inner),
            // `Any` holds any non-null value.
            (_, Type::Any) => !from_ty.is_nullable(),
            // A list is invariant in its element: a `[Int]` is not a
            // `[Any]`, because writing through the second would break the
            // first.
            (Type::List(from_element), Type::List(to_element)) => from_element == to_element,
            (Type::Tuple(from_items), Type::Tuple(to_items)) => {
                from_items.len() == to_items.len()
                    && from_items
                        .iter()
                        .zip(to_items)
                        .all(|(a, b)| self.is_assignable(*a, *b))
            }
            (
                Type::Function { parameters: from_params, result: from_result, is_async: from_async },
                Type::Function { parameters: to_params, result: to_result, is_async: to_async },
            ) => {
                // Parameters are contravariant and the result covariant, which
                // is what makes a more permissive function usable in place of a
                // stricter one.
                from_async == to_async
                    && from_params.len() == to_params.len()
                    && from_params
                        .iter()
                        .zip(to_params)
                        .all(|(from_param, to_param)| self.is_assignable(*to_param, *from_param))
                    && self.is_assignable(*from_result, *to_result)
            }
            _ => false,
        }
    }

    /// The narrowest type that both `a` and `b` can be used as.
    ///
    /// Used to give an `if`/`when` a single type when its branches differ.
    /// Returns `None` when the two have nothing useful in common, which the
    /// caller reports as a mismatch.
    pub fn join(&mut self, a: TypeId, b: TypeId) -> Option<TypeId> {
        if a == b {
            return Some(a);
        }
        if self.get(a).is_error() || self.get(b).is_error() {
            return Some(self.error());
        }
        // A branch that never returns does not constrain the result.
        if self.get(a).is_never() {
            return Some(b);
        }
        if self.get(b).is_never() {
            return Some(a);
        }

        // If the difference is only nullability, the answer is the nullable
        // form of the shared type.
        let (a_inner, b_inner) = (self.unwrap_nullable(a), self.unwrap_nullable(b));
        if a_inner == b_inner {
            return Some(self.nullable(a_inner));
        }

        if self.is_assignable(a, b) {
            return Some(b);
        }
        if self.is_assignable(b, a) {
            return Some(a);
        }
        None
    }

    /// Whether `pattern`, which may mention type parameters, matches `actual`.
    ///
    /// Every parameter it meets is bound in `bound`, and a parameter already
    /// bound must meet the same type again — which is what makes
    /// `fn pair<T>(a: T, b: T)` reject two different types.
    pub fn unify(
        &self,
        pattern: TypeId,
        actual: TypeId,
        bound: &mut std::collections::HashMap<(DefId, u32), TypeId>,
    ) -> bool {
        if self.get(pattern).is_error() || self.get(actual).is_error() {
            return true;
        }
        if let Type::Parameter { def, index, .. } = self.get(pattern) {
            let key = (*def, *index);
            return match bound.get(&key) {
                Some(already) => *already == actual,
                None => {
                    bound.insert(key, actual);
                    true
                }
            };
        }

        match (self.get(pattern), self.get(actual)) {
            (Type::List(left), Type::List(right)) => self.unify(*left, *right, bound),
            (Type::Nullable(left), Type::Nullable(right)) => self.unify(*left, *right, bound),
            // A plain value where a nullable is wanted is still a match, and
            // the parameter binds to what is actually there.
            (Type::Nullable(left), _) => self.unify(*left, actual, bound),
            (Type::Tuple(left), Type::Tuple(right)) => {
                left.len() == right.len()
                    && left.iter().zip(right).all(|(a, b)| self.unify(*a, *b, bound))
            }
            (
                Type::Function { parameters: left, result: left_result, .. },
                Type::Function { parameters: right, result: right_result, .. },
            ) => {
                left.len() == right.len()
                    && left.iter().zip(right).all(|(a, b)| self.unify(*a, *b, bound))
                    && self.unify(*left_result, *right_result, bound)
            }
            _ => self.is_assignable(actual, pattern),
        }
    }

    /// Replaces every type parameter in `ty` with what it was bound to.
    pub fn substitute(
        &mut self,
        ty: TypeId,
        bound: &std::collections::HashMap<(DefId, u32), TypeId>,
    ) -> TypeId {
        match self.get(ty).clone() {
            Type::Parameter { def, index, .. } => {
                bound.get(&(def, index)).copied().unwrap_or(ty)
            }
            Type::List(element) => {
                let element = self.substitute(element, bound);
                self.intern(Type::List(element))
            }
            Type::Nullable(inner) => {
                let inner = self.substitute(inner, bound);
                self.nullable(inner)
            }
            Type::Tuple(items) => {
                let items = items.iter().map(|item| self.substitute(*item, bound)).collect();
                self.intern(Type::Tuple(items))
            }
            Type::Function { parameters, result, is_async } => {
                let parameters =
                    parameters.iter().map(|p| self.substitute(*p, bound)).collect();
                let result = self.substitute(result, bound);
                self.intern(Type::Function { parameters, result, is_async })
            }
            _ => ty,
        }
    }

    /// Whether a type mentions any type parameter.
    pub fn is_generic(&self, ty: TypeId) -> bool {
        match self.get(ty) {
            Type::Parameter { .. } => true,
            Type::List(inner) | Type::Nullable(inner) => self.is_generic(*inner),
            Type::Tuple(items) => items.iter().any(|item| self.is_generic(*item)),
            Type::Function { parameters, result, .. } => {
                parameters.iter().any(|p| self.is_generic(*p)) || self.is_generic(*result)
            }
            _ => false,
        }
    }

    /// Renders a type the way it is written in source.
    pub fn render(&self, id: TypeId) -> String {
        match self.get(id) {
            Type::Primitive(primitive) => primitive.name().to_string(),
            Type::String => "String".to_string(),
            Type::Named { def, arguments } => {
                let name = self.definition_name(*def).to_string();
                if arguments.is_empty() {
                    name
                } else {
                    let args: Vec<String> = arguments.iter().map(|a| self.render(*a)).collect();
                    format!("{name}<{}>", args.join(", "))
                }
            }
            Type::List(element) => format!("[{}]", self.render(*element)),
            Type::Nullable(inner) => format!("{}?", self.render(*inner)),
            Type::Tuple(items) => {
                let items: Vec<String> = items.iter().map(|i| self.render(*i)).collect();
                format!("({})", items.join(", "))
            }
            Type::Function { parameters, result, is_async } => {
                let params: Vec<String> = parameters.iter().map(|p| self.render(*p)).collect();
                let prefix = if *is_async { "async fn" } else { "fn" };
                format!("{prefix}({}): {}", params.join(", "), self.render(*result))
            }
            Type::Parameter { name, .. } => name.clone(),
            Type::Unit => "Unit".to_string(),
            Type::Nothing => "Nothing".to_string(),
            Type::Any => "Any".to_string(),
            Type::Error => "?".to_string(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn equal_types_share_an_id() {
        let mut store = TypeStore::new();
        let a = store.primitive(Primitive::Int32);
        let b = store.primitive(Primitive::Int32);
        assert_eq!(a, b);
        assert_ne!(a, store.int());
    }

    #[test]
    fn nullability_does_not_stack() {
        let mut store = TypeStore::new();
        let string = store.string();
        let once = store.nullable(string);
        let twice = store.nullable(once);
        assert_eq!(once, twice);
        assert_eq!(store.unwrap_nullable(once), string);
        assert_eq!(store.render(once), "String?");
    }

    #[test]
    fn a_value_is_assignable_to_its_nullable_form_but_not_the_reverse() {
        let mut store = TypeStore::new();
        let string = store.string();
        let nullable = store.nullable(string);
        assert!(store.is_assignable(string, nullable));
        assert!(!store.is_assignable(nullable, string));
    }

    #[test]
    fn integers_do_not_convert_implicitly() {
        let mut store = TypeStore::new();
        let small = store.primitive(Primitive::Int32);
        let big = store.primitive(Primitive::Int64);
        assert!(!store.is_assignable(small, big), "Noto asks for `.toInt64()`");
        assert!(!store.is_assignable(big, small));
    }

    #[test]
    fn null_fits_every_nullable_type() {
        let mut store = TypeStore::new();
        let nothing = store.nothing();
        // `null` on its own has the type `Nothing?`.
        let null = store.nullable(nothing);
        let string = store.string();
        let nullable_string = store.nullable(string);
        assert!(store.is_assignable(null, nullable_string));
        assert!(!store.is_assignable(null, string));
        assert!(!store.is_assignable(nullable_string, null));
    }

    #[test]
    fn nothing_fits_anywhere() {
        let store = TypeStore::new();
        let nothing = store.nothing();
        assert!(store.is_assignable(nothing, store.int()));
        assert!(store.is_assignable(nothing, store.string()));
        assert!(!store.is_assignable(store.int(), nothing));
    }

    #[test]
    fn any_accepts_non_null_values_only() {
        let mut store = TypeStore::new();
        let any = store.any();
        let string = store.string();
        let nullable = store.nullable(string);
        assert!(store.is_assignable(string, any));
        assert!(!store.is_assignable(nullable, any));
    }

    #[test]
    fn errors_are_assignable_in_both_directions() {
        let store = TypeStore::new();
        let error = store.error();
        assert!(store.is_assignable(error, store.int()));
        assert!(store.is_assignable(store.int(), error));
    }

    #[test]
    fn joining_branches_finds_a_common_type() {
        let mut store = TypeStore::new();
        let int = store.int();
        let string = store.string();
        let nullable_int = store.nullable(int);

        assert_eq!(store.join(int, int), Some(int));
        assert_eq!(store.join(int, nullable_int), Some(nullable_int));
        assert_eq!(store.join(int, store.nothing()), Some(int));
        assert_eq!(store.join(int, string), None);
    }

    #[test]
    fn function_parameters_are_contravariant() {
        let mut store = TypeStore::new();
        let any = store.any();
        let string = store.string();
        let unit = store.unit();

        let takes_any =
            store.intern(Type::Function { parameters: vec![any], result: unit, is_async: false });
        let takes_string =
            store.intern(Type::Function { parameters: vec![string], result: unit, is_async: false });

        // A function that accepts anything can stand in for one that accepts
        // only strings; the reverse would let a non-string through.
        assert!(store.is_assignable(takes_any, takes_string));
        assert!(!store.is_assignable(takes_string, takes_any));
    }

    #[test]
    fn tuples_compare_element_wise() {
        let mut store = TypeStore::new();
        let int = store.int();
        let string = store.string();
        let a = store.intern(Type::Tuple(vec![int, string]));
        let b = store.intern(Type::Tuple(vec![int, string]));
        let c = store.intern(Type::Tuple(vec![string, int]));
        assert_eq!(a, b);
        assert!(!store.is_assignable(a, c));
    }

    #[test]
    fn rendering_matches_the_source_spelling() {
        let mut store = TypeStore::new();
        let int = store.int();
        let nullable = store.nullable(int);
        let function =
            store.intern(Type::Function { parameters: vec![int, int], result: int, is_async: false });
        assert_eq!(store.render(int), "Int");
        assert_eq!(store.render(nullable), "Int?");
        assert_eq!(store.render(function), "fn(Int, Int): Int");
        assert_eq!(store.render(store.unit()), "Unit");
    }
}
