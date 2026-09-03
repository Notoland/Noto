//! Built-in scalar types.

/// The width of an integer type.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub enum IntWidth {
    /// 8 bits.
    W8,
    /// 16 bits.
    W16,
    /// 32 bits.
    W32,
    /// 64 bits.
    W64,
}

impl IntWidth {
    /// The number of bits.
    pub fn bits(self) -> u32 {
        match self {
            IntWidth::W8 => 8,
            IntWidth::W16 => 16,
            IntWidth::W32 => 32,
            IntWidth::W64 => 64,
        }
    }

    /// The number of bytes.
    pub fn bytes(self) -> u32 {
        self.bits() / 8
    }
}

/// The width of a floating point type.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub enum FloatWidth {
    /// 32 bits.
    W32,
    /// 64 bits.
    W64,
}

impl FloatWidth {
    /// The number of bits.
    pub fn bits(self) -> u32 {
        match self {
            FloatWidth::W32 => 32,
            FloatWidth::W64 => 64,
        }
    }

    /// The number of bytes.
    pub fn bytes(self) -> u32 {
        self.bits() / 8
    }
}

/// A built-in scalar type.
///
/// `Int` and `UInt` are 64-bit everywhere Noto runs. Fixing the width means a
/// program that is correct on one target is correct on all of them; code that
/// needs a pointer-sized integer says so with an explicit width.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Primitive {
    /// `Int`: a 64-bit signed integer. The default for integer literals.
    Int,
    /// `Int8`
    Int8,
    /// `Int16`
    Int16,
    /// `Int32`
    Int32,
    /// `Int64`, the same type as `Int` under a name that states its width.
    Int64,
    /// `UInt`: a 64-bit unsigned integer.
    UInt,
    /// `UInt8`
    UInt8,
    /// `UInt16`
    UInt16,
    /// `UInt32`
    UInt32,
    /// `UInt64`
    UInt64,
    /// `Float32`
    Float32,
    /// `Float64`: the default for floating point literals.
    Float64,
    /// `Bool`
    Bool,
    /// `Char`: one Unicode scalar value.
    Char,
    /// `Byte`: a raw 8-bit value, distinct from `UInt8` in intent.
    Byte,
}

impl Primitive {
    /// Looks a primitive up by the name used in source.
    pub fn from_name(name: &str) -> Option<Primitive> {
        use Primitive::*;
        Some(match name {
            "Int" => Int,
            "Int8" => Int8,
            "Int16" => Int16,
            "Int32" => Int32,
            "Int64" => Int64,
            "UInt" => UInt,
            "UInt8" => UInt8,
            "UInt16" => UInt16,
            "UInt32" => UInt32,
            "UInt64" => UInt64,
            "Float32" => Float32,
            "Float64" => Float64,
            "Bool" => Bool,
            "Char" => Char,
            "Byte" => Byte,
            _ => return None,
        })
    }

    /// The name used in source.
    pub fn name(self) -> &'static str {
        use Primitive::*;
        match self {
            Int => "Int",
            Int8 => "Int8",
            Int16 => "Int16",
            Int32 => "Int32",
            Int64 => "Int64",
            UInt => "UInt",
            UInt8 => "UInt8",
            UInt16 => "UInt16",
            UInt32 => "UInt32",
            UInt64 => "UInt64",
            Float32 => "Float32",
            Float64 => "Float64",
            Bool => "Bool",
            Char => "Char",
            Byte => "Byte",
        }
    }

    /// Whether this is an integer type.
    pub fn is_integer(self) -> bool {
        self.int_width().is_some()
    }

    /// Whether this is a number.
    pub fn is_numeric(self) -> bool {
        self.is_integer() || self.float_width().is_some()
    }

    /// Whether this is a signed integer.
    pub fn is_signed(self) -> bool {
        use Primitive::*;
        matches!(self, Int | Int8 | Int16 | Int32 | Int64)
    }

    /// The integer width, if this is an integer.
    pub fn int_width(self) -> Option<IntWidth> {
        use Primitive::*;
        Some(match self {
            Int8 | UInt8 | Byte => IntWidth::W8,
            Int16 | UInt16 => IntWidth::W16,
            Int32 | UInt32 => IntWidth::W32,
            Int | Int64 | UInt | UInt64 => IntWidth::W64,
            _ => return None,
        })
    }

    /// The floating point width, if this is a float.
    pub fn float_width(self) -> Option<FloatWidth> {
        match self {
            Primitive::Float32 => Some(FloatWidth::W32),
            Primitive::Float64 => Some(FloatWidth::W64),
            _ => None,
        }
    }

    /// How many bytes a value of this type occupies.
    pub fn size(self) -> u32 {
        use Primitive::*;
        match self {
            Bool => 1,
            // A `Char` holds a Unicode scalar value, which needs 21 bits.
            Char => 4,
            other => other
                .int_width()
                .map(IntWidth::bytes)
                .or_else(|| other.float_width().map(FloatWidth::bytes))
                .unwrap_or(8),
        }
    }

    /// The alignment a value of this type requires, in bytes.
    pub fn align(self) -> u32 {
        self.size()
    }

    /// The inclusive range of values an integer type can hold.
    pub fn int_range(self) -> Option<(i128, i128)> {
        let width = self.int_width()?;
        let bits = width.bits();
        Some(if self.is_signed() {
            let max = (1i128 << (bits - 1)) - 1;
            (-(1i128 << (bits - 1)), max)
        } else {
            (0, (1i128 << bits) - 1)
        })
    }

    /// Whether a value of `self` fits in `target` without losing information.
    ///
    /// Noto never applies these widenings on its own — a conversion is always
    /// written with `.toInt32()` and friends. The relation is used to explain
    /// in a diagnostic which conversion would be safe.
    pub fn widens_to(self, target: Primitive) -> bool {
        if self == target {
            return true;
        }
        match (self.int_range(), target.int_range()) {
            (Some((low, high)), Some((target_low, target_high))) => {
                target_low <= low && high <= target_high
            }
            (Some(_), None) => target.float_width().is_some(),
            (None, _) => match (self.float_width(), target.float_width()) {
                (Some(from), Some(to)) => from <= to,
                _ => false,
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_round_trip() {
        for name in [
            "Int", "Int8", "Int16", "Int32", "Int64", "UInt", "UInt8", "UInt16", "UInt32",
            "UInt64", "Float32", "Float64", "Bool", "Char", "Byte",
        ] {
            let primitive = Primitive::from_name(name).unwrap_or_else(|| panic!("{name}"));
            assert_eq!(primitive.name(), name);
        }
        assert_eq!(Primitive::from_name("String"), None, "String is not a primitive");
    }

    #[test]
    fn int_is_sixty_four_bits_everywhere() {
        assert_eq!(Primitive::Int.size(), 8);
        assert_eq!(Primitive::UInt.size(), 8);
        assert_eq!(Primitive::Int.int_width(), Some(IntWidth::W64));
    }

    #[test]
    fn integer_ranges_are_exact() {
        assert_eq!(Primitive::Int8.int_range(), Some((-128, 127)));
        assert_eq!(Primitive::UInt8.int_range(), Some((0, 255)));
        assert_eq!(Primitive::Int32.int_range(), Some((-2_147_483_648, 2_147_483_647)));
        assert_eq!(Primitive::Int64.int_range(), Some((i64::MIN as i128, i64::MAX as i128)));
        assert_eq!(Primitive::Bool.int_range(), None);
    }

    #[test]
    fn widening_follows_the_ranges() {
        assert!(Primitive::Int8.widens_to(Primitive::Int32));
        assert!(Primitive::UInt8.widens_to(Primitive::Int16));
        assert!(!Primitive::Int32.widens_to(Primitive::Int8));
        assert!(!Primitive::Int64.widens_to(Primitive::UInt64), "negatives do not fit");
        assert!(!Primitive::UInt64.widens_to(Primitive::Int64), "the top half does not fit");
        assert!(Primitive::Float32.widens_to(Primitive::Float64));
        assert!(!Primitive::Float64.widens_to(Primitive::Float32));
        assert!(!Primitive::Bool.widens_to(Primitive::Int));
    }

    #[test]
    fn sizes_match_the_widths() {
        assert_eq!(Primitive::Bool.size(), 1);
        assert_eq!(Primitive::Byte.size(), 1);
        assert_eq!(Primitive::Char.size(), 4);
        assert_eq!(Primitive::Float32.size(), 4);
        assert_eq!(Primitive::Float64.size(), 8);
    }
}
