//! The x86-64 backend.
//!
//! Noto IR is turned into machine code one function at a time. The strategy is
//! deliberately simple and predictable: every local slot and every IR value
//! gets a stack slot in the frame, operands are loaded into scratch registers,
//! the operation is performed, and the result is written back.
//!
//! That produces more memory traffic than a register allocator would, and it
//! is the right starting point: it is easy to verify against the IR, it can
//! never run out of registers, and it gives the optimizer something correct to
//! improve. Register allocation is a pass over this, not a rewrite of it.

pub mod encode;
pub mod runtime;

use crate::elf::{self, Image};
use crate::{CodegenError, Target};
use encode::{Assembler, Cond, Label, Reference, Reg};
use noto_ir::{
    BinOp, BlockId, Const, FuncId, Function, InstKind, Intrinsic, IrType, Operand, Program,
    Terminator, UnOp,
};
use noto_runtime::{Routine, STRING_DATA_OFFSET};
use std::collections::HashMap;

/// The register the first operand is loaded into.
const LEFT: Reg = Reg::Rax;
/// The register the second operand is loaded into.
const RIGHT: Reg = Reg::Rcx;

/// Every value and slot occupies one 8-byte stack cell.
///
/// Packing narrower types would shrink frames but complicates every access;
/// values are kept in their canonical extended form instead, which is what the
/// comparison and division instructions expect.
const CELL: i32 = 8;

/// Compiles a program to a Linux x86-64 executable.
pub fn compile(program: &Program, target: Target) -> Result<Vec<u8>, CodegenError> {
    debug_assert!(target.is_supported());

    let mut assembler = Assembler::new();
    let mut rodata = Vec::new();
    let runtime_data = runtime::append_data(&mut rodata);
    let string_offsets = append_strings(&mut rodata, program);

    let runtime_labels = runtime::RuntimeLabels::new(&mut assembler);
    let function_labels: HashMap<FuncId, Label> =
        program.functions.iter().map(|f| (f.id, assembler.label())).collect();

    let main_returns_status =
        program.entry.map(|id| !program.function(id).result.is_unit()).unwrap_or(false);
    let main_label = program.entry.and_then(|id| function_labels.get(&id).copied());

    runtime::emit(
        &mut assembler,
        &runtime_labels,
        &runtime_data,
        main_label,
        main_returns_status,
    );

    for function in &program.functions {
        let mut generator = FunctionGenerator::new(
            &mut assembler,
            function,
            &function_labels,
            &runtime_labels,
            &string_offsets,
        )?;
        generator.emit()?;
    }

    let (text, relocations, symbols) = assembler.finish();
    let entry_offset = symbols
        .get(Routine::Start.symbol())
        .copied()
        .ok_or_else(|| CodegenError::Internal("the entry point was never emitted".into()))?;

    let layout = elf::layout(text.len() as u64, rodata.len() as u64);
    let mut text = text;
    patch_relocations(&mut text, &relocations, &layout);

    Ok(elf::write(&Image {
        text,
        rodata,
        data: vec![0; runtime::globals::SIZE as usize],
        bss_size: 0,
        entry_offset: entry_offset as u64,
    }))
}

/// Lays the program's string constants out in read-only memory.
fn append_strings(rodata: &mut Vec<u8>, program: &Program) -> Vec<u32> {
    let mut offsets = Vec::with_capacity(program.strings.len());
    for text in &program.strings {
        while rodata.len() % 8 != 0 {
            rodata.push(0);
        }
        offsets.push(rodata.len() as u32);
        rodata.extend_from_slice(&(text.len() as u64).to_le_bytes());
        rodata.extend_from_slice(text.as_bytes());
    }
    offsets
}

/// Fills in the RIP-relative displacements now that addresses are known.
fn patch_relocations(text: &mut [u8], relocations: &[encode::Relocation], layout: &elf::Layout) {
    for relocation in relocations {
        let target = match relocation.reference {
            Reference::RoData(offset) => layout.rodata_address + offset as u64,
            Reference::Data(offset) => layout.data_address + offset as u64,
        };
        let next = layout.text_address + relocation.next_instruction as u64;
        let displacement = (target as i64 - next as i64) as i32;
        let at = relocation.at as usize;
        text[at..at + 4].copy_from_slice(&displacement.to_le_bytes());
    }
}

/// Generates the code of one function.
struct FunctionGenerator<'a> {
    assembler: &'a mut Assembler,
    function: &'a Function,
    function_labels: &'a HashMap<FuncId, Label>,
    runtime_labels: &'a runtime::RuntimeLabels,
    string_offsets: &'a [u32],
    /// The label of each basic block.
    block_labels: Vec<Label>,
    /// Total bytes reserved below the frame pointer.
    frame_size: i32,
    /// Where the first value cell sits, below every slot cell.
    values_base: i32,
}

impl<'a> FunctionGenerator<'a> {
    fn new(
        assembler: &'a mut Assembler,
        function: &'a Function,
        function_labels: &'a HashMap<FuncId, Label>,
        runtime_labels: &'a runtime::RuntimeLabels,
        string_offsets: &'a [u32],
    ) -> Result<Self, CodegenError> {
        let convention = noto_runtime::CallingConvention::SystemVAmd64;
        if function.parameters.len() > convention.register_argument_count() {
            return Err(CodegenError::TooManyParameters {
                function: function.name.clone(),
                limit: convention.register_argument_count(),
                span: function.span,
            });
        }

        let block_labels = function.blocks.iter().map(|_| assembler.label()).collect();
        let slot_bytes = function.slots.len() as i32 * CELL;
        let value_bytes = function.value_types.len() as i32 * CELL;
        // The ABI requires the stack to be 16-byte aligned at every call, and
        // the return address plus the saved frame pointer keep it aligned, so
        // the frame itself only has to round up.
        let frame_size = (slot_bytes + value_bytes + 15) & !15;

        Ok(FunctionGenerator {
            assembler,
            function,
            function_labels,
            runtime_labels,
            string_offsets,
            block_labels,
            frame_size,
            values_base: slot_bytes,
        })
    }

    /// The frame offset of a local slot.
    fn slot_offset(&self, slot: noto_ir::SlotId) -> i32 {
        -(slot.0 as i32 + 1) * CELL
    }

    /// The frame offset of an IR value.
    fn value_offset(&self, value: noto_ir::ValueId) -> i32 {
        -(self.values_base + (value.0 as i32 + 1) * CELL)
    }

    fn emit(&mut self) -> Result<(), CodegenError> {
        let label = self.function_labels[&self.function.id];
        self.assembler.bind(label);
        self.assembler.define_symbol(self.function.name.clone());

        self.assembler.push(Reg::Rbp);
        self.assembler.mov_reg_reg(Reg::Rbp, Reg::Rsp);
        if self.frame_size > 0 {
            self.assembler.sub_imm(Reg::Rsp, self.frame_size);
        }

        // Arguments arrive in registers; the frame is where the rest of the
        // function expects to find them.
        let convention = noto_runtime::CallingConvention::SystemVAmd64;
        let argument_registers = [Reg::Rdi, Reg::Rsi, Reg::Rdx, Reg::Rcx, Reg::R8, Reg::R9];
        let _ = convention;
        for (index, slot) in self.function.parameters.iter().enumerate() {
            let offset = self.slot_offset(*slot);
            self.assembler.mov_mem_reg(Reg::Rbp, offset, argument_registers[index]);
        }

        for (index, block) in self.function.blocks.iter().enumerate() {
            self.assembler.bind(self.block_labels[index]);
            for inst in &block.instructions {
                self.emit_inst(&inst.kind)?;
            }
            self.emit_terminator(&block.terminator, BlockId(index as u32));
        }

        Ok(())
    }

    fn emit_inst(&mut self, kind: &InstKind) -> Result<(), CodegenError> {
        match kind {
            InstKind::Const { dest, value } => {
                self.load_const(LEFT, value);
                let offset = self.value_offset(*dest);
                self.assembler.mov_mem_reg(Reg::Rbp, offset, LEFT);
            }
            InstKind::LoadLocal { dest, slot } => {
                let source = self.slot_offset(*slot);
                self.assembler.mov_reg_mem(LEFT, Reg::Rbp, source);
                let offset = self.value_offset(*dest);
                self.assembler.mov_mem_reg(Reg::Rbp, offset, LEFT);
            }
            InstKind::StoreLocal { slot, value } => {
                self.load_operand(LEFT, value);
                let offset = self.slot_offset(*slot);
                self.assembler.mov_mem_reg(Reg::Rbp, offset, LEFT);
            }
            InstKind::Unary { dest, op, operand } => {
                self.load_operand(LEFT, operand);
                match op {
                    UnOp::Neg => self.assembler.neg(LEFT),
                    UnOp::Not => self.assembler.not(LEFT),
                    UnOp::LogicalNot => {
                        // `!b` is `b == 0`, which `sete` computes without a
                        // branch.
                        self.assembler.cmp_imm(LEFT, 0);
                        self.assembler.setcc(Cond::Eq, LEFT);
                        self.assembler.movzx8(LEFT, LEFT);
                    }
                }
                let ty = self.function.value_type(*dest);
                self.normalise(LEFT, ty);
                let offset = self.value_offset(*dest);
                self.assembler.mov_mem_reg(Reg::Rbp, offset, LEFT);
            }
            InstKind::Binary { dest, op, left, right } => {
                self.emit_binary(*dest, *op, left, right);
            }
            InstKind::Cast { dest, operand, to } => {
                self.load_operand(LEFT, operand);
                self.normalise(LEFT, *to);
                let offset = self.value_offset(*dest);
                self.assembler.mov_mem_reg(Reg::Rbp, offset, LEFT);
            }
            InstKind::Call { dest, callee, arguments } => {
                self.emit_call_arguments(arguments)?;
                let label = self.function_labels[callee];
                self.assembler.call(label);
                if let Some(dest) = dest {
                    let offset = self.value_offset(*dest);
                    self.assembler.mov_mem_reg(Reg::Rbp, offset, Reg::Rax);
                }
            }
            InstKind::Intrinsic { dest, which, arguments } => {
                self.emit_call_arguments(arguments)?;
                let label = self.runtime_labels.get(routine_for(*which));
                self.assembler.call(label);
                if let Some(dest) = dest {
                    let offset = self.value_offset(*dest);
                    self.assembler.mov_mem_reg(Reg::Rbp, offset, Reg::Rax);
                }
            }
        }
        Ok(())
    }

    /// Loads call arguments into their ABI registers.
    ///
    /// Arguments are loaded last-to-first so that an argument already living
    /// in a register the call needs is not clobbered before it is read; every
    /// operand comes from the frame, so the only hazard is between the
    /// registers themselves.
    fn emit_call_arguments(&mut self, arguments: &[Operand]) -> Result<(), CodegenError> {
        let registers = [Reg::Rdi, Reg::Rsi, Reg::Rdx, Reg::Rcx, Reg::R8, Reg::R9];
        if arguments.len() > registers.len() {
            return Err(CodegenError::TooManyArguments {
                function: self.function.name.clone(),
                limit: registers.len(),
                span: self.function.span,
            });
        }
        for (index, argument) in arguments.iter().enumerate().rev() {
            self.load_operand(registers[index], argument);
        }
        Ok(())
    }

    fn emit_binary(
        &mut self,
        dest: noto_ir::ValueId,
        op: BinOp,
        left: &Operand,
        right: &Operand,
    ) {
        let operand_ty = self.function.operand_type(left);
        self.load_operand(LEFT, left);
        self.load_operand(RIGHT, right);

        match op {
            BinOp::Add => self.assembler.add(LEFT, RIGHT),
            BinOp::Sub => self.assembler.sub(LEFT, RIGHT),
            BinOp::Mul => self.assembler.imul(LEFT, RIGHT),
            BinOp::And => self.assembler.and(LEFT, RIGHT),
            BinOp::Or => self.assembler.or(LEFT, RIGHT),
            BinOp::Xor => self.assembler.xor(LEFT, RIGHT),
            BinOp::Shl => self.assembler.shl_cl(LEFT),
            BinOp::AShr => self.assembler.sar_cl(LEFT),
            BinOp::LShr => self.assembler.shr_cl(LEFT),
            BinOp::SDiv | BinOp::SRem => {
                // idiv divides rdx:rax, so rdx must hold the sign extension.
                self.assembler.cqo();
                self.assembler.idiv(RIGHT);
                if op == BinOp::SRem {
                    self.assembler.mov_reg_reg(LEFT, Reg::Rdx);
                }
            }
            BinOp::UDiv | BinOp::URem => {
                self.assembler.xor(Reg::Rdx, Reg::Rdx);
                self.assembler.div(RIGHT);
                if op == BinOp::URem {
                    self.assembler.mov_reg_reg(LEFT, Reg::Rdx);
                }
            }
            comparison => {
                let condition = match comparison {
                    BinOp::Eq => Cond::Eq,
                    BinOp::Ne => Cond::Ne,
                    BinOp::SLt => Cond::Lt,
                    BinOp::SLe => Cond::Le,
                    BinOp::SGt => Cond::Gt,
                    BinOp::SGe => Cond::Ge,
                    BinOp::ULt => Cond::Below,
                    BinOp::ULe => Cond::BelowEq,
                    BinOp::UGt => Cond::Above,
                    BinOp::UGe => Cond::AboveEq,
                    _ => unreachable!("every arithmetic operation is handled above"),
                };
                self.assembler.cmp(LEFT, RIGHT);
                self.assembler.setcc(condition, LEFT);
                self.assembler.movzx8(LEFT, LEFT);
            }
        }

        if !op.is_comparison() {
            self.normalise(LEFT, operand_ty);
        }
        let offset = self.value_offset(dest);
        self.assembler.mov_mem_reg(Reg::Rbp, offset, LEFT);
    }

    /// Re-establishes the canonical 64-bit form of a narrower value.
    ///
    /// Arithmetic is always performed at 64 bits, so an `Int8` addition that
    /// overflows leaves bits above the type's width set. Truncating here keeps
    /// every value in the range its type promises, which is what comparisons
    /// and printing depend on.
    fn normalise(&mut self, reg: Reg, ty: IrType) {
        match ty {
            IrType::I8 => self.assembler.movsx8(reg, reg),
            IrType::I16 => self.assembler.movsx16(reg, reg),
            IrType::I32 => self.assembler.movsx32(reg, reg),
            IrType::U8 => self.assembler.movzx8(reg, reg),
            IrType::U16 => self.assembler.movzx16(reg, reg),
            IrType::U32 | IrType::Char => self.assembler.mov32(reg, reg),
            IrType::Bool => {
                self.assembler.cmp_imm(reg, 0);
                self.assembler.setcc(Cond::Ne, reg);
                self.assembler.movzx8(reg, reg);
            }
            _ => {}
        }
    }

    fn load_operand(&mut self, reg: Reg, operand: &Operand) {
        match operand {
            Operand::Value(id) => {
                let offset = self.value_offset(*id);
                self.assembler.mov_reg_mem(reg, Reg::Rbp, offset);
            }
            Operand::Const(value) => self.load_const(reg, value),
        }
    }

    fn load_const(&mut self, reg: Reg, value: &Const) {
        match value {
            Const::Int { value, .. } => self.assembler.mov_reg_imm64(reg, *value as i64),
            Const::Bool(value) => self.assembler.mov_reg_imm64(reg, i64::from(*value)),
            Const::Char(value) => self.assembler.mov_reg_imm64(reg, *value as i64),
            Const::Null | Const::Unit => self.assembler.mov_reg_imm64(reg, 0),
            Const::Str(id) => {
                let offset = self.string_offsets[id.0 as usize];
                self.assembler.lea_rip(reg, Reference::RoData(offset));
            }
        }
    }

    fn emit_terminator(&mut self, terminator: &Terminator, _block: BlockId) {
        match terminator {
            Terminator::Jump(target) => {
                let label = self.block_labels[target.0 as usize];
                self.assembler.jmp(label);
            }
            Terminator::Branch { condition, then_block, else_block } => {
                self.load_operand(LEFT, condition);
                self.assembler.cmp_imm(LEFT, 0);
                self.assembler.jcc(Cond::Ne, self.block_labels[then_block.0 as usize]);
                self.assembler.jmp(self.block_labels[else_block.0 as usize]);
            }
            Terminator::Return(value) => {
                if let Some(value) = value {
                    self.load_operand(Reg::Rax, value);
                }
                self.assembler.mov_reg_reg(Reg::Rsp, Reg::Rbp);
                self.assembler.pop(Reg::Rbp);
                self.assembler.ret();
            }
            Terminator::Unreachable => {
                // A block nothing reaches still needs bytes, and faulting is
                // the safest thing to put there.
                self.assembler.ud2();
            }
        }
    }
}

/// The runtime routine an intrinsic calls.
fn routine_for(intrinsic: Intrinsic) -> Routine {
    match intrinsic {
        Intrinsic::PrintString => Routine::PrintString,
        Intrinsic::PrintlnString => Routine::PrintlnString,
        Intrinsic::PrintInt => Routine::PrintInt,
        Intrinsic::PrintlnInt => Routine::PrintlnInt,
        Intrinsic::PrintBool => Routine::PrintBool,
        Intrinsic::PrintlnBool => Routine::PrintlnBool,
        Intrinsic::PrintlnEmpty => Routine::PrintlnEmpty,
        Intrinsic::IntToString => Routine::IntToString,
        Intrinsic::BoolToString => Routine::BoolToString,
        Intrinsic::StringConcat => Routine::StringConcat,
        Intrinsic::StringLength => Routine::StringLength,
        Intrinsic::Assert => Routine::Assert,
        Intrinsic::Exit => Routine::Exit,
    }
}

/// Silences the unused import when the string data offset is only referenced
/// by the runtime module.
const _: i32 = STRING_DATA_OFFSET;
