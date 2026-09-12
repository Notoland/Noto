//! The functions behind RFC 0003's built-in conformances.
//!
//! `Int: Comparable`, `String: Hashable` and the rest of the fixed table in
//! `docs/rfcs/0003-interfaces-and-bounds.md` name real methods a witness has
//! to point at, and nothing can open `class Int` to write one. So this module
//! builds them directly as Noto IR — the same way `Builder::lower_initializer`
//! builds `Class.<init>` from no source at all — rather than from a body the
//! parser produced.
//!
//! Each is built at most once per program: [`function_for`] checks
//! [`noto_ir::Program::function_named`] before building anything, the same
//! way [`noto_ir::Program::intern_witness`] already dedupes a witness table by
//! name.

use noto_ir::{
    Block, BlockId, Const, FuncId, Function, Inst, InstKind, Intrinsic, IrType, Operand, Program,
    Slot, SlotId, Terminator, ValueId,
};
use noto_semantic::BuiltinType;
use noto_span::Span;
use noto_types::Primitive;

/// The function implementing `ty`'s `member`, building it the first time it
/// is asked for.
///
/// `member` is always `"compareTo"` or `"hash"`: those are the only methods
/// [`BuiltinType`]'s two interfaces, `Comparable` and `Hashable`, declare.
pub(crate) fn function_for(program: &mut Program, ty: BuiltinType, member: &str) -> FuncId {
    let name = symbol(ty, member);
    if let Some(existing) = program.function_named(&name) {
        return existing.id;
    }
    match (ty, member) {
        (BuiltinType::String, "compareTo") => build_string_compare_to(program, name),
        (BuiltinType::String, "hash") => build_string_hash(program, name),
        (BuiltinType::Primitive(primitive), "compareTo") => {
            build_scalar_compare_to(program, name, primitive)
        }
        (BuiltinType::Primitive(primitive), "hash") => build_scalar_hash(program, name, primitive),
        _ => unreachable!(
            "RFC 0003's fixed table only ever asks for `compareTo` or `hash`, got `{member}`"
        ),
    }
}

/// The symbol a built-in conformance function is emitted under, matching the
/// `Class.method` convention an ordinary method's is mangled with.
fn symbol(ty: BuiltinType, member: &str) -> String {
    format!("{}.{member}", ty.name())
}

/// A minimal function builder, independent of [`crate::Builder`] because
/// these functions have no [`noto_semantic::Analysis`] behind them: nothing
/// in checked source ever calls one by name, so there is no
/// [`noto_semantic::FunctionId`] to hang one off. Only a witness ever reaches
/// one, by address.
struct FnBuilder<'a> {
    program: &'a mut Program,
    id: FuncId,
    block: BlockId,
}

impl<'a> FnBuilder<'a> {
    /// Starts a new function named `name`, producing `result`, with an empty
    /// entry block ready to receive instructions.
    fn new(program: &'a mut Program, name: String, result: IrType) -> Self {
        let id = FuncId(program.functions.len() as u32);
        program.functions.push(Function {
            id,
            name,
            parameters: Vec::new(),
            slots: Vec::new(),
            result,
            blocks: Vec::new(),
            value_types: Vec::new(),
            span: Span::dummy(),
        });
        let mut builder = FnBuilder { program, id, block: BlockId(0) };
        let entry = builder.new_block();
        builder.block = entry;
        builder
    }

    fn function(&mut self) -> &mut Function {
        self.program.function_mut(self.id)
    }

    /// Adds a parameter slot, in declaration order.
    fn add_param(&mut self, name: &str, ty: IrType) -> SlotId {
        let function = self.function();
        let id = SlotId(function.slots.len() as u32);
        function.slots.push(Slot { name: name.to_string(), ty, is_parameter: true });
        function.parameters.push(id);
        id
    }

    /// Adds a slot the signature does not name, for loop bookkeeping.
    fn add_temp(&mut self, name: &str, ty: IrType) -> SlotId {
        let function = self.function();
        let id = SlotId(function.slots.len() as u32);
        function.slots.push(Slot { name: name.to_string(), ty, is_parameter: false });
        id
    }

    fn new_block(&mut self) -> BlockId {
        let function = self.function();
        let id = BlockId(function.blocks.len() as u32);
        function.blocks.push(Block {
            id,
            label: format!("b{}", id.0),
            instructions: Vec::new(),
            terminator: Terminator::Unreachable,
        });
        id
    }

    fn switch_to(&mut self, block: BlockId) {
        self.block = block;
    }

    fn push(&mut self, kind: InstKind) {
        let block = self.block;
        self.function().block_mut(block).instructions.push(Inst::new(kind, Span::dummy()));
    }

    fn set_terminator(&mut self, terminator: Terminator) {
        let block = self.block;
        self.function().block_mut(block).terminator = terminator;
    }

    fn new_value(&mut self, ty: IrType) -> ValueId {
        let function = self.function();
        let id = ValueId(function.value_types.len() as u32);
        function.value_types.push(ty);
        id
    }

    fn emit(&mut self, ty: IrType, build: impl FnOnce(ValueId) -> InstKind) -> Operand {
        let dest = self.new_value(ty);
        self.push(build(dest));
        Operand::Value(dest)
    }

    fn load(&mut self, slot: SlotId, ty: IrType) -> Operand {
        self.emit(ty, |dest| InstKind::LoadLocal { dest, slot })
    }

    fn store(&mut self, slot: SlotId, value: Operand) {
        self.push(InstKind::StoreLocal { slot, value });
    }

    fn binary(&mut self, ty: IrType, op: noto_ir::BinOp, left: Operand, right: Operand) -> Operand {
        self.emit(ty, |dest| InstKind::Binary { dest, op, left, right })
    }

    fn int_const(value: i128) -> Operand {
        Operand::Const(Const::Int { value, ty: IrType::I64 })
    }

    fn ret(&mut self, value: Operand) {
        self.set_terminator(Terminator::Return(Some(value)));
    }

    fn finish(self) -> FuncId {
        self.id
    }
}

/// `compareTo(other: Self): Int` for any of the primitives RFC 0003 makes
/// `Comparable` — every integer width and `Char`, but not `Bool` or a float.
///
/// Three-way comparison is built from the same signed/unsigned `<` and `>`
/// the language already has for this type, rather than from a subtraction:
/// `UInt64.compareTo` on two values more than `Int64::MAX` apart would give a
/// subtraction the wrong sign, and this has no such limit.
fn build_scalar_compare_to(program: &mut Program, name: String, primitive: Primitive) -> FuncId {
    use noto_ir::BinOp;

    let ty = crate::lower_primitive(primitive);
    // The same rule `binary_op` in `expr.rs` uses: `Char` compares as signed
    // too, and every value of it is non-negative, so the two agree.
    let signed = ty.is_signed() || ty == IrType::Char;
    let (lt, gt) = if signed { (BinOp::SLt, BinOp::SGt) } else { (BinOp::ULt, BinOp::UGt) };

    let mut b = FnBuilder::new(program, name, IrType::I64);
    let this = b.add_param("this", ty);
    let other = b.add_param("other", ty);

    let this_val = b.load(this, ty);
    let other_val = b.load(other, ty);
    let is_lt = b.binary(IrType::Bool, lt, this_val.clone(), other_val.clone());

    let lt_block = b.new_block();
    let check_gt = b.new_block();
    b.set_terminator(Terminator::Branch { condition: is_lt, then_block: lt_block, else_block: check_gt });

    b.switch_to(lt_block);
    b.ret(FnBuilder::int_const(-1));

    b.switch_to(check_gt);
    let is_gt = b.binary(IrType::Bool, gt, this_val, other_val);
    let gt_block = b.new_block();
    let eq_block = b.new_block();
    b.set_terminator(Terminator::Branch { condition: is_gt, then_block: gt_block, else_block: eq_block });

    b.switch_to(gt_block);
    b.ret(FnBuilder::int_const(1));

    b.switch_to(eq_block);
    b.ret(FnBuilder::int_const(0));

    b.finish()
}

/// `hash(): Int` for any primitive RFC 0003 makes `Hashable` — everything but
/// a float.
///
/// The identity function: every value here is already a machine word, so
/// returning it unchanged (loaded as `Int` rather than as its own, possibly
/// narrower, type) is a valid hash and the cheapest one. Codegen moves a
/// slot's full eight bytes regardless of the type it was declared with — a
/// parameter arrives that way and nothing narrows it afterwards — so the
/// bits this loads are already the correctly extended `Int` a narrower type's
/// value would produce.
fn build_scalar_hash(program: &mut Program, name: String, primitive: Primitive) -> FuncId {
    let ty = crate::lower_primitive(primitive);
    let mut b = FnBuilder::new(program, name, IrType::I64);
    let this = b.add_param("this", ty);
    let value = b.load(this, IrType::I64);
    b.ret(value);
    b.finish()
}

/// `compareTo(other: Self): Int` for `String`: lexicographic order over
/// unsigned bytes, the same as C's `memcmp` followed by `strcmp`'s length
/// tie-break — a prefix of the other string sorts first.
///
/// Built as a loop over `StringLength`/`StringByteAt` rather than a new
/// runtime routine: the standard library is written in Noto wherever the
/// language can express the thing, and a byte-comparison loop is something
/// `std/string.noto` could write if `String` were a class it could attach a
/// method to.
fn build_string_compare_to(program: &mut Program, name: String) -> FuncId {
    use noto_ir::BinOp;

    let mut b = FnBuilder::new(program, name, IrType::I64);
    let this = b.add_param("this", IrType::Str);
    let other = b.add_param("other", IrType::Str);

    let this_val = b.load(this, IrType::Str);
    let other_val = b.load(other, IrType::Str);
    let la = b.emit(IrType::I64, |dest| InstKind::Intrinsic {
        dest: Some(dest),
        which: Intrinsic::StringLength,
        arguments: vec![this_val.clone()],
    });
    let lb = b.emit(IrType::I64, |dest| InstKind::Intrinsic {
        dest: Some(dest),
        which: Intrinsic::StringLength,
        arguments: vec![other_val.clone()],
    });

    let i_slot = b.add_temp("i", IrType::I64);
    b.store(i_slot, FnBuilder::int_const(0));

    let test = b.new_block();
    let body = b.new_block();
    let step = b.new_block();
    let return_diff = b.new_block();
    let tail = b.new_block();

    b.set_terminator(Terminator::Jump(test));

    b.switch_to(test);
    let i = b.load(i_slot, IrType::I64);
    let within_this = b.binary(IrType::Bool, BinOp::SLt, i.clone(), la.clone());
    let within_other = b.binary(IrType::Bool, BinOp::SLt, i.clone(), lb.clone());
    let keep_going = b.binary(IrType::Bool, BinOp::And, within_this, within_other);
    b.set_terminator(Terminator::Branch { condition: keep_going, then_block: body, else_block: tail });

    b.switch_to(body);
    let ca = b.emit(IrType::I64, |dest| InstKind::Intrinsic {
        dest: Some(dest),
        which: Intrinsic::StringByteAt,
        arguments: vec![this_val.clone(), i.clone()],
    });
    let cb = b.emit(IrType::I64, |dest| InstKind::Intrinsic {
        dest: Some(dest),
        which: Intrinsic::StringByteAt,
        arguments: vec![other_val.clone(), i.clone()],
    });
    let differs = b.binary(IrType::Bool, BinOp::Ne, ca.clone(), cb.clone());
    b.set_terminator(Terminator::Branch { condition: differs, then_block: return_diff, else_block: step });

    b.switch_to(return_diff);
    let diff = b.binary(IrType::I64, BinOp::Sub, ca, cb);
    b.ret(diff);

    b.switch_to(step);
    let next = b.binary(IrType::I64, BinOp::Add, i, FnBuilder::int_const(1));
    b.store(i_slot, next);
    b.set_terminator(Terminator::Jump(test));

    b.switch_to(tail);
    let length_diff = b.binary(IrType::I64, BinOp::Sub, la, lb);
    b.ret(length_diff);

    b.finish()
}

/// `hash(): Int` for `String`: the classic `h = h * 31 + byte` accumulation
/// over every byte, the same recurrence `java.lang.String.hashCode` uses.
fn build_string_hash(program: &mut Program, name: String) -> FuncId {
    use noto_ir::BinOp;

    let mut b = FnBuilder::new(program, name, IrType::I64);
    let this = b.add_param("this", IrType::Str);
    let this_val = b.load(this, IrType::Str);
    let n = b.emit(IrType::I64, |dest| InstKind::Intrinsic {
        dest: Some(dest),
        which: Intrinsic::StringLength,
        arguments: vec![this_val.clone()],
    });

    let h_slot = b.add_temp("h", IrType::I64);
    let i_slot = b.add_temp("i", IrType::I64);
    b.store(h_slot, FnBuilder::int_const(0));
    b.store(i_slot, FnBuilder::int_const(0));

    let test = b.new_block();
    let body = b.new_block();
    let tail = b.new_block();

    b.set_terminator(Terminator::Jump(test));

    b.switch_to(test);
    let i = b.load(i_slot, IrType::I64);
    let keep_going = b.binary(IrType::Bool, BinOp::SLt, i.clone(), n);
    b.set_terminator(Terminator::Branch { condition: keep_going, then_block: body, else_block: tail });

    b.switch_to(body);
    let h = b.load(h_slot, IrType::I64);
    let byte = b.emit(IrType::I64, |dest| InstKind::Intrinsic {
        dest: Some(dest),
        which: Intrinsic::StringByteAt,
        arguments: vec![this_val.clone(), i.clone()],
    });
    let scaled = b.binary(IrType::I64, BinOp::Mul, h, FnBuilder::int_const(31));
    let next_h = b.binary(IrType::I64, BinOp::Add, scaled, byte);
    b.store(h_slot, next_h);
    let next_i = b.binary(IrType::I64, BinOp::Add, i, FnBuilder::int_const(1));
    b.store(i_slot, next_i);
    b.set_terminator(Terminator::Jump(test));

    b.switch_to(tail);
    let result = b.load(h_slot, IrType::I64);
    b.ret(result);

    b.finish()
}
