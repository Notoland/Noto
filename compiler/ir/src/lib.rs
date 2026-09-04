//! Noto IR: the language's own intermediate representation.
//!
//! Noto IR sits between the type checker and the native backend. It is a
//! typed, three-address form organised into basic blocks: every instruction
//! either produces one value or performs one effect, and control flow is
//! explicit in each block's terminator. Source-level constructs — `when`,
//! `for`, string interpolation, short-circuit operators — are gone by this
//! point, leaving a small vocabulary that a backend can lower mechanically.
//!
//! The IR is deliberately independent of any target: it has no registers, no
//! stack layout and no calling convention. Those belong to `noto-codegen`.
//! It is equally independent of the AST, which is what lets the optimizer and
//! future backends work without pulling in the front end.
//!
//! Locals are addressed by slot and read with [`Inst::LoadLocal`] rather than
//! being in SSA form. Keeping the memory operations explicit makes lowering
//! straightforward and correct; promoting them to values is an optimizer pass,
//! not a precondition.

#![deny(missing_docs)]

mod display;
mod types;

pub use types::IrType;

use noto_span::Span;

/// Identifies a function within a [`Program`].
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct FuncId(pub u32);

/// Identifies a basic block within a [`Function`].
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct BlockId(pub u32);

/// Identifies a value produced by an instruction.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct ValueId(pub u32);

/// Identifies a local slot within a [`Function`].
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct SlotId(pub u32);

/// Identifies a string in the program's constant pool.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct StringId(pub u32);

/// A compile-time constant.
#[derive(Clone, PartialEq, Debug)]
pub enum Const {
    /// An integer of the given width.
    Int {
        /// The value, sign-extended to 128 bits.
        value: i128,
        /// The type it is stored as.
        ty: IrType,
    },
    /// A boolean.
    Bool(bool),
    /// A Unicode scalar value.
    Char(char),
    /// A string from the constant pool.
    Str(StringId),
    /// The absence of a value.
    Null,
    /// The single value of the `Unit` type.
    Unit,
}

impl Const {
    /// The type of the constant.
    pub fn ty(&self) -> IrType {
        match self {
            Const::Int { ty, .. } => *ty,
            Const::Bool(_) => IrType::Bool,
            Const::Char(_) => IrType::Char,
            Const::Str(_) => IrType::Str,
            Const::Null => IrType::Ptr,
            Const::Unit => IrType::Unit,
        }
    }
}

/// An instruction input: either a value already computed or a constant.
#[derive(Clone, PartialEq, Debug)]
pub enum Operand {
    /// The result of an earlier instruction in the same function.
    Value(ValueId),
    /// A literal.
    Const(Const),
}

impl Operand {
    /// The value this operand names, if it names one.
    pub fn as_value(&self) -> Option<ValueId> {
        match self {
            Operand::Value(id) => Some(*id),
            Operand::Const(_) => None,
        }
    }
}

/// A binary operation.
///
/// Signedness is part of the operation rather than of the type, so the backend
/// never has to consult the type table to pick an instruction.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum BinOp {
    /// Integer addition.
    Add,
    /// Integer subtraction.
    Sub,
    /// Integer multiplication.
    Mul,
    /// Signed division.
    SDiv,
    /// Unsigned division.
    UDiv,
    /// Signed remainder.
    SRem,
    /// Unsigned remainder.
    URem,
    /// Bitwise and.
    And,
    /// Bitwise or.
    Or,
    /// Bitwise exclusive or.
    Xor,
    /// Left shift.
    Shl,
    /// Arithmetic right shift, for signed values.
    AShr,
    /// Logical right shift, for unsigned values.
    LShr,
    /// Equality.
    Eq,
    /// Inequality.
    Ne,
    /// Signed less than.
    SLt,
    /// Signed less than or equal.
    SLe,
    /// Signed greater than.
    SGt,
    /// Signed greater than or equal.
    SGe,
    /// Unsigned less than.
    ULt,
    /// Unsigned less than or equal.
    ULe,
    /// Unsigned greater than.
    UGt,
    /// Unsigned greater than or equal.
    UGe,
}

impl BinOp {
    /// Whether the operation produces a `Bool` regardless of its operands.
    pub fn is_comparison(self) -> bool {
        use BinOp::*;
        matches!(self, Eq | Ne | SLt | SLe | SGt | SGe | ULt | ULe | UGt | UGe)
    }

    /// The mnemonic used when printing the IR.
    pub fn mnemonic(self) -> &'static str {
        use BinOp::*;
        match self {
            Add => "add",
            Sub => "sub",
            Mul => "mul",
            SDiv => "sdiv",
            UDiv => "udiv",
            SRem => "srem",
            URem => "urem",
            And => "and",
            Or => "or",
            Xor => "xor",
            Shl => "shl",
            AShr => "ashr",
            LShr => "lshr",
            Eq => "eq",
            Ne => "ne",
            SLt => "slt",
            SLe => "sle",
            SGt => "sgt",
            SGe => "sge",
            ULt => "ult",
            ULe => "ule",
            UGt => "ugt",
            UGe => "uge",
        }
    }
}

/// A unary operation.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum UnOp {
    /// Two's complement negation.
    Neg,
    /// Bitwise complement.
    Not,
    /// Logical negation of a `Bool`.
    LogicalNot,
}

impl UnOp {
    /// The mnemonic used when printing the IR.
    pub fn mnemonic(self) -> &'static str {
        match self {
            UnOp::Neg => "neg",
            UnOp::Not => "not",
            UnOp::LogicalNot => "lnot",
        }
    }
}

/// An operation the runtime provides.
///
/// Intrinsics are the boundary between generated code and the Noto runtime.
/// Each maps to exactly one runtime routine, so adding a capability to the
/// language means adding one variant here and one routine to `noto-runtime`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Intrinsic {
    /// Writes a string to standard output.
    PrintString,
    /// Writes a string followed by a line break.
    PrintlnString,
    /// Writes an integer in decimal.
    PrintInt,
    /// Writes an integer in decimal followed by a line break.
    PrintlnInt,
    /// Writes `true` or `false`.
    PrintBool,
    /// Writes `true` or `false` followed by a line break.
    PrintlnBool,
    /// Writes a line break.
    PrintlnEmpty,
    /// Formats an integer as a string.
    IntToString,
    /// Formats a boolean as a string.
    BoolToString,
    /// Joins two strings, producing a new one.
    StringConcat,
    /// The length of a string in bytes.
    StringLength,
    /// Whether two strings hold the same bytes.
    StringEquals,
    /// Aborts unless an index is inside `0..length`.
    IndexCheck,
    /// Aborts the program unless the argument is `true`.
    Assert,
    /// Ends the process with the given exit status.
    Exit,
}

impl Intrinsic {
    /// The name used when printing the IR and when naming the runtime routine.
    pub fn name(self) -> &'static str {
        use Intrinsic::*;
        match self {
            PrintString => "print_string",
            PrintlnString => "println_string",
            PrintInt => "print_int",
            PrintlnInt => "println_int",
            PrintBool => "print_bool",
            PrintlnBool => "println_bool",
            PrintlnEmpty => "println_empty",
            IntToString => "int_to_string",
            BoolToString => "bool_to_string",
            StringConcat => "string_concat",
            StringLength => "string_length",
            StringEquals => "string_equals",
            IndexCheck => "index_check",
            Assert => "assert",
            Exit => "exit",
        }
    }

    /// The type this intrinsic produces, or [`IrType::Unit`] for an effect.
    pub fn result(self) -> IrType {
        use Intrinsic::*;
        match self {
            IntToString | BoolToString | StringConcat => IrType::Str,
            StringLength => IrType::I64,
            StringEquals => IrType::Bool,
            IndexCheck => IrType::Unit,
            _ => IrType::Unit,
        }
    }
}

/// One instruction.
#[derive(Clone, PartialEq, Debug)]
pub struct Inst {
    /// What the instruction does.
    pub kind: InstKind,
    /// The source location it came from, for debug information and runtime
    /// diagnostics.
    pub span: Span,
}

impl Inst {
    /// Builds an instruction.
    pub fn new(kind: InstKind, span: Span) -> Self {
        Inst { kind, span }
    }

    /// The value the instruction defines, if it defines one.
    pub fn dest(&self) -> Option<ValueId> {
        match &self.kind {
            InstKind::Const { dest, .. }
            | InstKind::LoadLocal { dest, .. }
            | InstKind::Unary { dest, .. }
            | InstKind::Binary { dest, .. }
            | InstKind::Cast { dest, .. } => Some(*dest),
            InstKind::Alloc { dest, .. } | InstKind::Load { dest, .. } => Some(*dest),
            InstKind::Call { dest, .. } | InstKind::Intrinsic { dest, .. } => *dest,
            InstKind::StoreLocal { .. } | InstKind::Store { .. } => None,
        }
    }
}

/// The kinds of instruction Noto IR defines.
#[derive(Clone, PartialEq, Debug)]
pub enum InstKind {
    /// Materialises a constant.
    Const {
        /// Where the result goes.
        dest: ValueId,
        /// The constant.
        value: Const,
    },
    /// Reads a local slot.
    LoadLocal {
        /// Where the result goes.
        dest: ValueId,
        /// The slot to read.
        slot: SlotId,
    },
    /// Writes a local slot.
    StoreLocal {
        /// The slot to write.
        slot: SlotId,
        /// The value to store.
        value: Operand,
    },
    /// Applies a unary operation.
    Unary {
        /// Where the result goes.
        dest: ValueId,
        /// The operation.
        op: UnOp,
        /// The operand.
        operand: Operand,
    },
    /// Applies a binary operation.
    Binary {
        /// Where the result goes.
        dest: ValueId,
        /// The operation.
        op: BinOp,
        /// The left operand.
        left: Operand,
        /// The right operand.
        right: Operand,
    },
    /// Converts a value between numeric types.
    Cast {
        /// Where the result goes.
        dest: ValueId,
        /// The value to convert.
        operand: Operand,
        /// The type to convert to.
        to: IrType,
    },
    /// Calls a function declared in this program.
    Call {
        /// Where the result goes, or `None` when it produces `Unit`.
        dest: Option<ValueId>,
        /// The function to call.
        callee: FuncId,
        /// The arguments, in parameter order.
        arguments: Vec<Operand>,
    },
    /// Calls a runtime routine.
    Intrinsic {
        /// Where the result goes, or `None` when it produces `Unit`.
        dest: Option<ValueId>,
        /// Which routine.
        which: Intrinsic,
        /// The arguments.
        arguments: Vec<Operand>,
    },
    /// Reserves `size` bytes of heap and produces a pointer to them.
    ///
    /// The memory is not initialised: whoever allocates an object writes
    /// every one of its fields before the pointer escapes.
    Alloc {
        /// Where the pointer goes.
        dest: ValueId,
        /// How many bytes to reserve.
        size: u32,
    },
    /// Reads a value from `address + offset`.
    Load {
        /// Where the result goes.
        dest: ValueId,
        /// The base pointer.
        address: Operand,
        /// The byte offset added to it.
        offset: u32,
    },
    /// Writes a value to `address + offset`.
    Store {
        /// The base pointer.
        address: Operand,
        /// The byte offset added to it.
        offset: u32,
        /// The value to write.
        value: Operand,
    },
}

/// How control leaves a basic block.
#[derive(Clone, PartialEq, Debug)]
pub enum Terminator {
    /// Continues at another block.
    Jump(BlockId),
    /// Continues at one of two blocks depending on a `Bool`.
    Branch {
        /// The condition.
        condition: Operand,
        /// Taken when the condition is true.
        then_block: BlockId,
        /// Taken when the condition is false.
        else_block: BlockId,
    },
    /// Leaves the function.
    Return(Option<Operand>),
    /// States that control never reaches this point.
    ///
    /// Emitted after a call that cannot return, and used as a placeholder for
    /// a block the lowering has not terminated yet.
    Unreachable,
}

impl Terminator {
    /// The blocks control may continue at.
    pub fn successors(&self) -> Vec<BlockId> {
        match self {
            Terminator::Jump(target) => vec![*target],
            Terminator::Branch { then_block, else_block, .. } => vec![*then_block, *else_block],
            Terminator::Return(_) | Terminator::Unreachable => Vec::new(),
        }
    }
}

/// A basic block: a straight run of instructions ending in one terminator.
#[derive(Clone, Debug)]
pub struct Block {
    /// Its id within the function.
    pub id: BlockId,
    /// A short label used when printing the IR.
    pub label: String,
    /// The instructions, in execution order.
    pub instructions: Vec<Inst>,
    /// How control leaves the block.
    pub terminator: Terminator,
}

/// A local slot: a named place a function can read and write.
#[derive(Clone, Debug)]
pub struct Slot {
    /// The name it had in source, for debug information.
    pub name: String,
    /// Its type.
    pub ty: IrType,
    /// Whether it holds one of the function's parameters.
    pub is_parameter: bool,
}

/// A function in Noto IR.
#[derive(Clone, Debug)]
pub struct Function {
    /// Its id within the program.
    pub id: FuncId,
    /// The symbol name it is emitted under.
    pub name: String,
    /// The slots holding its parameters, in declaration order.
    pub parameters: Vec<SlotId>,
    /// Every local slot, parameters included.
    pub slots: Vec<Slot>,
    /// Its result type.
    pub result: IrType,
    /// Its basic blocks. The first is the entry block.
    pub blocks: Vec<Block>,
    /// The type of every value the function defines, indexed by [`ValueId`].
    pub value_types: Vec<IrType>,
    /// Where it was declared.
    pub span: Span,
}

impl Function {
    /// The block control starts at.
    pub fn entry_block(&self) -> BlockId {
        BlockId(0)
    }

    /// Looks a block up.
    pub fn block(&self, id: BlockId) -> &Block {
        &self.blocks[id.0 as usize]
    }

    /// Looks a block up for modification.
    pub fn block_mut(&mut self, id: BlockId) -> &mut Block {
        &mut self.blocks[id.0 as usize]
    }

    /// Looks a slot up.
    pub fn slot(&self, id: SlotId) -> &Slot {
        &self.slots[id.0 as usize]
    }

    /// The type of a value.
    pub fn value_type(&self, id: ValueId) -> IrType {
        self.value_types[id.0 as usize]
    }

    /// The type of an operand.
    pub fn operand_type(&self, operand: &Operand) -> IrType {
        match operand {
            Operand::Value(id) => self.value_type(*id),
            Operand::Const(value) => value.ty(),
        }
    }
}

/// A whole program in Noto IR.
#[derive(Clone, Debug, Default)]
pub struct Program {
    /// Every function, indexed by [`FuncId`].
    pub functions: Vec<Function>,
    /// The string constant pool, indexed by [`StringId`].
    pub strings: Vec<String>,
    /// The entry point, if the program has one.
    pub entry: Option<FuncId>,
}

impl Program {
    /// Creates an empty program.
    pub fn new() -> Self {
        Program::default()
    }

    /// Looks a function up.
    pub fn function(&self, id: FuncId) -> &Function {
        &self.functions[id.0 as usize]
    }

    /// Looks a function up for modification.
    pub fn function_mut(&mut self, id: FuncId) -> &mut Function {
        &mut self.functions[id.0 as usize]
    }

    /// Looks a string constant up.
    pub fn string(&self, id: StringId) -> &str {
        &self.strings[id.0 as usize]
    }

    /// Adds a string to the pool, reusing an equal one if it is already there.
    pub fn intern_string(&mut self, text: &str) -> StringId {
        if let Some(index) = self.strings.iter().position(|existing| existing == text) {
            return StringId(index as u32);
        }
        self.strings.push(text.to_string());
        StringId(self.strings.len() as u32 - 1)
    }

    /// Finds a function by name.
    pub fn function_named(&self, name: &str) -> Option<&Function> {
        self.functions.iter().find(|function| function.name == name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strings_are_interned_once() {
        let mut program = Program::new();
        let a = program.intern_string("hello");
        let b = program.intern_string("hello");
        let c = program.intern_string("world");
        assert_eq!(a, b);
        assert_ne!(a, c);
        assert_eq!(program.strings.len(), 2);
        assert_eq!(program.string(a), "hello");
    }

    #[test]
    fn terminators_report_their_successors() {
        assert_eq!(Terminator::Jump(BlockId(3)).successors(), vec![BlockId(3)]);
        assert_eq!(
            Terminator::Branch {
                condition: Operand::Const(Const::Bool(true)),
                then_block: BlockId(1),
                else_block: BlockId(2),
            }
            .successors(),
            vec![BlockId(1), BlockId(2)]
        );
        assert!(Terminator::Return(None).successors().is_empty());
        assert!(Terminator::Unreachable.successors().is_empty());
    }

    #[test]
    fn comparisons_are_classified() {
        assert!(BinOp::SLt.is_comparison());
        assert!(BinOp::Eq.is_comparison());
        assert!(!BinOp::Add.is_comparison());
    }

    #[test]
    fn constants_know_their_type() {
        assert_eq!(Const::Int { value: 1, ty: IrType::I32 }.ty(), IrType::I32);
        assert_eq!(Const::Bool(true).ty(), IrType::Bool);
        assert_eq!(Const::Str(StringId(0)).ty(), IrType::Str);
        assert_eq!(Const::Unit.ty(), IrType::Unit);
    }
}
