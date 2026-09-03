//! The machine-level types Noto IR works in.

/// A type at the IR level.
///
/// The rich source type system is gone by this point: what remains is what the
/// machine has to know — how many bits a value occupies and how to interpret
/// them. Nullability and named types are erased during lowering, which is what
/// keeps the backend small.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum IrType {
    /// An 8-bit integer.
    I8,
    /// A 16-bit integer.
    I16,
    /// A 32-bit integer.
    I32,
    /// A 64-bit integer.
    I64,
    /// An 8-bit unsigned integer.
    U8,
    /// A 16-bit unsigned integer.
    U16,
    /// A 32-bit unsigned integer.
    U32,
    /// A 64-bit unsigned integer.
    U64,
    /// A 32-bit float.
    F32,
    /// A 64-bit float.
    F64,
    /// A one-byte boolean holding 0 or 1.
    Bool,
    /// A Unicode scalar value, stored in 32 bits.
    Char,
    /// A pointer to a string object.
    Str,
    /// An untyped pointer.
    Ptr,
    /// The type of an expression that produces nothing.
    Unit,
}

impl IrType {
    /// How many bytes a value of this type occupies.
    pub fn size(self) -> u32 {
        use IrType::*;
        match self {
            I8 | U8 | Bool => 1,
            I16 | U16 => 2,
            I32 | U32 | F32 | Char => 4,
            I64 | U64 | F64 | Str | Ptr => 8,
            Unit => 0,
        }
    }

    /// The alignment a value of this type requires.
    pub fn align(self) -> u32 {
        self.size().max(1)
    }

    /// Whether the type is an integer.
    pub fn is_integer(self) -> bool {
        use IrType::*;
        matches!(self, I8 | I16 | I32 | I64 | U8 | U16 | U32 | U64)
    }

    /// Whether the type is a signed integer.
    pub fn is_signed(self) -> bool {
        use IrType::*;
        matches!(self, I8 | I16 | I32 | I64)
    }

    /// Whether the type is a float.
    pub fn is_float(self) -> bool {
        matches!(self, IrType::F32 | IrType::F64)
    }

    /// Whether the type is held in a pointer-sized machine word.
    pub fn is_pointer(self) -> bool {
        matches!(self, IrType::Str | IrType::Ptr)
    }

    /// Whether the type carries no data.
    pub fn is_unit(self) -> bool {
        matches!(self, IrType::Unit)
    }

    /// The name used when printing the IR.
    pub fn name(self) -> &'static str {
        use IrType::*;
        match self {
            I8 => "i8",
            I16 => "i16",
            I32 => "i32",
            I64 => "i64",
            U8 => "u8",
            U16 => "u16",
            U32 => "u32",
            U64 => "u64",
            F32 => "f32",
            F64 => "f64",
            Bool => "bool",
            Char => "char",
            Str => "str",
            Ptr => "ptr",
            Unit => "unit",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes_match_the_widths() {
        assert_eq!(IrType::I8.size(), 1);
        assert_eq!(IrType::I32.size(), 4);
        assert_eq!(IrType::I64.size(), 8);
        assert_eq!(IrType::Bool.size(), 1);
        assert_eq!(IrType::Char.size(), 4);
        assert_eq!(IrType::Str.size(), 8, "a string is a pointer to its object");
        assert_eq!(IrType::Unit.size(), 0);
    }

    #[test]
    fn signedness_is_part_of_the_type() {
        assert!(IrType::I32.is_signed());
        assert!(!IrType::U32.is_signed());
        assert!(IrType::U32.is_integer());
        assert!(!IrType::Bool.is_integer());
    }

    #[test]
    fn unit_never_occupies_space() {
        assert!(IrType::Unit.is_unit());
        assert_eq!(IrType::Unit.size(), 0);
        assert_eq!(IrType::Unit.align(), 1, "alignment is never zero");
    }
}
