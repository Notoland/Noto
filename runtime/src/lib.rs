//! The Noto runtime contract.
//!
//! A Noto program is a native executable with no external runtime: there is no
//! virtual machine, no interpreter and no dependency on libc. What a program
//! still needs — writing to standard output, formatting a number, allocating a
//! string — is provided by a small set of routines emitted into the executable
//! itself.
//!
//! This crate defines the *contract* those routines follow: their names, their
//! signatures, the calling convention, and the memory layout of the values
//! they operate on. The machine code implementing the contract is emitted by
//! the backend in `noto-codegen`, because it is necessarily
//! target-specific. Keeping the contract here means a second backend
//! implements a written-down interface rather than guessing at one.
//!
//! # Memory layout
//!
//! A `String` value is a pointer to a string object:
//!
//! ```text
//! offset 0: i64   length in bytes
//! offset 8: u8[]  the bytes, UTF-8, not zero-terminated
//! ```
//!
//! A null pointer represents `null`. Strings are immutable, so a literal can
//! live in read-only memory and be shared.

#![deny(missing_docs)]

/// The calling convention Noto uses on every target that has a C ABI.
///
/// Noto follows the platform C ABI so that FFI is a matter of declaration
/// rather than translation, and so that a debugger can walk a Noto stack with
/// no special support.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum CallingConvention {
    /// The System V AMD64 ABI, used on Linux, macOS and the BSDs on x86-64.
    SystemVAmd64,
}

impl CallingConvention {
    /// The registers integer and pointer arguments are passed in, in order.
    pub fn integer_argument_registers(self) -> &'static [&'static str] {
        match self {
            CallingConvention::SystemVAmd64 => &["rdi", "rsi", "rdx", "rcx", "r8", "r9"],
        }
    }

    /// How many arguments can be passed in registers before the stack is used.
    pub fn register_argument_count(self) -> usize {
        self.integer_argument_registers().len()
    }
}

/// Byte offset of a string object's length field.
pub const STRING_LENGTH_OFFSET: i32 = 0;

/// Byte offset of a string object's first byte.
pub const STRING_DATA_OFFSET: i32 = 8;

/// Byte offset of a list's length.
pub const LIST_LENGTH_OFFSET: i32 = 0;

/// Byte offset of a list's capacity: how many elements its buffer holds.
pub const LIST_CAPACITY_OFFSET: i32 = 8;

/// Byte offset of the pointer to a list's elements.
///
/// The elements live in a separate block so that growing a list can replace
/// that block without invalidating the pointer everything holding the list
/// already has.
pub const LIST_DATA_OFFSET: i32 = 16;

/// The size of a list's header, in bytes.
pub const LIST_HEADER_SIZE: u32 = 24;

/// How many elements a list's first buffer holds.
///
/// Small enough that a list of two or three costs little, large enough that
/// the first few pushes do not each reallocate.
pub const LIST_INITIAL_CAPACITY: i64 = 4;

/// The alignment every heap allocation is given.
///
/// Eight bytes is enough for every value Noto stores today and keeps the bump
/// allocator's arithmetic to a single mask.
pub const HEAP_ALIGN: u64 = 8;

/// The smallest region the allocator requests from the operating system.
///
/// Allocations larger than this get their own region sized to fit, so a large
/// string is never refused.
pub const HEAP_CHUNK_SIZE: u64 = 1 << 20;

/// The exit status a failed `assert` ends the process with.
///
/// It is distinct from the statuses a program is likely to return itself, so a
/// test runner can tell an assertion failure from an ordinary non-zero exit.
pub const ASSERT_FAILURE_STATUS: i32 = 101;

/// The exit status used when the process runs out of memory.
pub const OUT_OF_MEMORY_STATUS: i32 = 102;

/// A routine the runtime provides to generated code.
///
/// The order of this list is the order the backend emits the routines in, and
/// [`Routine::all`] is what the backend iterates over. Adding a capability to
/// the language means adding a variant here and its machine code in the
/// backend.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub enum Routine {
    /// Program entry: sets up the heap, calls `main`, exits with its result.
    Start,
    /// `write(fd, pointer, length)`.
    Write,
    /// `exit(status)`. Never returns.
    Exit,
    /// `alloc(size) -> pointer`. Never returns null; aborts if it cannot.
    Alloc,
    /// `print_string(string)`.
    PrintString,
    /// `println_string(string)`.
    PrintlnString,
    /// `print_int(value)`.
    PrintInt,
    /// `println_int(value)`.
    PrintlnInt,
    /// `print_bool(value)`.
    PrintBool,
    /// `println_bool(value)`.
    PrintlnBool,
    /// `println_empty()`.
    PrintlnEmpty,
    /// `int_to_string(value) -> string`.
    IntToString,
    /// `bool_to_string(value) -> string`.
    BoolToString,
    /// `string_concat(left, right) -> string`.
    StringConcat,
    /// `string_length(string) -> length`.
    StringLength,
    /// `string_byte_at(string, index) -> byte`. The index is checked.
    StringByteAt,
    /// `string_slice(string, start, end) -> string`. Both bounds are checked.
    StringSlice,
    /// `list_push(list, value)`. Appends one element, growing the buffer
    /// when it is full.
    ListPush,
    /// `index_check(index, length)`. Ends the process when the index is
    /// outside `0..length`.
    ///
    /// Reading past a list would otherwise return whatever the allocator
    /// last put there, which is the class of bug a language is supposed to
    /// make impossible.
    IndexCheck,
    /// `string_equals(left, right) -> bool`.
    ///
    /// Compares contents, not addresses. Two strings built different ways
    /// from the same bytes are equal, which is the only definition a user
    /// can reason about.
    StringEquals,
    /// `assert(condition)`. Ends the process when the condition is false.
    Assert,
    /// `newline()`: writes a single line break.
    Newline,
}

impl Routine {
    /// Every routine, in emission order.
    pub fn all() -> &'static [Routine] {
        use Routine::*;
        &[
            Start,
            Write,
            Exit,
            Alloc,
            PrintString,
            PrintlnString,
            PrintInt,
            PrintlnInt,
            PrintBool,
            PrintlnBool,
            PrintlnEmpty,
            IntToString,
            BoolToString,
            StringConcat,
            StringLength,
            StringEquals,
            StringByteAt,
            StringSlice,
            ListPush,
            IndexCheck,
            Assert,
            Newline,
        ]
    }

    /// The symbol name the routine is emitted under.
    pub fn symbol(self) -> &'static str {
        use Routine::*;
        match self {
            Start => "_start",
            Write => "noto_rt_write",
            Exit => "noto_rt_exit",
            Alloc => "noto_rt_alloc",
            PrintString => "noto_rt_print_string",
            PrintlnString => "noto_rt_println_string",
            PrintInt => "noto_rt_print_int",
            PrintlnInt => "noto_rt_println_int",
            PrintBool => "noto_rt_print_bool",
            PrintlnBool => "noto_rt_println_bool",
            PrintlnEmpty => "noto_rt_println_empty",
            IntToString => "noto_rt_int_to_string",
            BoolToString => "noto_rt_bool_to_string",
            StringConcat => "noto_rt_string_concat",
            StringLength => "noto_rt_string_length",
            StringEquals => "noto_rt_string_equals",
            StringByteAt => "noto_rt_string_byte_at",
            StringSlice => "noto_rt_string_slice",
            IndexCheck => "noto_rt_index_check",
            ListPush => "noto_rt_list_push",
            Assert => "noto_rt_assert",
            Newline => "noto_rt_newline",
        }
    }

    /// How many arguments the routine takes.
    pub fn arity(self) -> usize {
        use Routine::*;
        match self {
            Start | PrintlnEmpty | Newline => 0,
            Exit | Alloc | PrintString | PrintlnString | PrintInt | PrintlnInt | PrintBool
            | PrintlnBool | IntToString | BoolToString | StringLength | Assert => 1,
            StringConcat | StringEquals | IndexCheck | ListPush | StringByteAt => 2,
            StringSlice | Write => 3,
        }
    }

    /// Whether the routine returns to its caller.
    pub fn returns(self) -> bool {
        !matches!(self, Routine::Start | Routine::Exit)
    }
}

/// The text an out-of-range index writes before ending the process.
pub const INDEX_FAILURE_MESSAGE: &str = "noto: index out of range\n";

/// The status an out-of-range index ends the process with.
///
/// It is distinct from an assertion failure so that a test runner can tell a
/// bad index from a failed expectation.
pub const INDEX_FAILURE_STATUS: i32 = 102;

/// The text a failed assertion writes before ending the process.
pub const ASSERT_FAILURE_MESSAGE: &str = "noto: assertion failed\n";

/// The text written when an allocation cannot be satisfied.
pub const OUT_OF_MEMORY_MESSAGE: &str = "noto: out of memory\n";

/// The text `true` formats to.
pub const TRUE_TEXT: &str = "true";

/// The text `false` formats to.
pub const FALSE_TEXT: &str = "false";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_routine_has_a_distinct_symbol() {
        let mut symbols: Vec<&str> = Routine::all().iter().map(|r| r.symbol()).collect();
        let count = symbols.len();
        symbols.sort_unstable();
        symbols.dedup();
        assert_eq!(symbols.len(), count, "routine symbols must be unique");
    }

    #[test]
    fn the_entry_point_is_named_start() {
        assert_eq!(Routine::Start.symbol(), "_start");
        assert!(!Routine::Start.returns());
        assert!(!Routine::Exit.returns());
        assert!(Routine::Alloc.returns());
    }

    #[test]
    fn no_routine_needs_more_registers_than_the_abi_provides() {
        let limit = CallingConvention::SystemVAmd64.register_argument_count();
        for routine in Routine::all() {
            assert!(routine.arity() <= limit, "{routine:?} takes too many arguments");
        }
    }
}
