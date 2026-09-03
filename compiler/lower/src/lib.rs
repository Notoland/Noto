//! Lowering the Noto AST to Noto IR.
//!
//! This phase runs after type checking and turns the tree the programmer wrote
//! into the flat, block-structured form the backend consumes. It is where the
//! language's conveniences are paid for:
//!
//! - `if`, `when`, `while`, `for` and `loop` become basic blocks and branches;
//! - `&&`, `||` and `?:` become branches, preserving short-circuit evaluation;
//! - string interpolation becomes a chain of `to_string` and `concat` calls;
//! - `for i in a..b` becomes an explicit counter and comparison.
//!
//! Lowering assumes the program type-checked. Anything it cannot represent is
//! reported as a diagnostic rather than lowered incorrectly, so a compiler bug
//! surfaces as a clear error instead of wrong machine code.

#![deny(missing_docs)]

mod expr;
mod stmt;

use noto_ast::{ItemKind, Module, NodeId};
use noto_diagnostics::{codes, Diagnostic, DiagnosticSink};
use noto_ir::{
    Block, BlockId, Const, FuncId, Function, Inst, InstKind, IrType, Operand, Program, Slot,
    SlotId, Terminator, ValueId,
};
use noto_semantic::{Analysis, FunctionId, LocalId};
use noto_span::Span;
use noto_types::{Primitive, Type, TypeId};
use std::collections::HashMap;

/// Lowers an analysed module to Noto IR.
///
/// The caller must have checked that analysis produced no errors; lowering a
/// program that failed to type-check is not meaningful.
pub fn lower(module: &Module, analysis: &Analysis, sink: &mut DiagnosticSink) -> Program {
    let mut program = Program::new();
    let mut function_ids = HashMap::new();

    // Every function gets an id before any body is lowered, so a call can name
    // a function declared later in the file.
    for (index, info) in analysis.functions.iter().enumerate() {
        let semantic_id = FunctionId(index as u32);
        let ir_id = FuncId(program.functions.len() as u32);
        function_ids.insert(semantic_id, ir_id);

        let result = lower_type(&analysis.store, info.result);
        program.functions.push(Function {
            id: ir_id,
            name: info.name.clone(),
            parameters: Vec::new(),
            slots: Vec::new(),
            result,
            blocks: Vec::new(),
            value_types: Vec::new(),
            span: info.span,
        });
    }

    program.entry = analysis.entry.and_then(|entry| function_ids.get(&entry).copied());

    let bodies = collect_bodies(module);
    for (index, info) in analysis.functions.iter().enumerate() {
        let semantic_id = FunctionId(index as u32);
        let ir_id = function_ids[&semantic_id];

        let Some(body_id) = info.body else { continue };
        let Some(body) = bodies.get(&body_id) else {
            sink.emit(
                Diagnostic::fatal(
                    codes::UNSUPPORTED_CONSTRUCT,
                    format!("the body of `{}` could not be lowered", info.name),
                )
                .with_primary(info.span, "internal compiler error: body not found"),
            );
            continue;
        };

        let mut builder = Builder::new(analysis, &mut program, sink, &function_ids, ir_id);
        builder.lower_function(semantic_id, body);
    }

    program
}

/// Indexes every block that serves as a function or test body.
fn collect_bodies(module: &Module) -> HashMap<NodeId, &noto_ast::Block> {
    let mut bodies = HashMap::new();
    for item in &module.items {
        match &item.kind {
            ItemKind::Fn(function) => {
                if let Some(body) = &function.body {
                    bodies.insert(body.id, body);
                }
            }
            ItemKind::Test(test) => {
                bodies.insert(test.body.id, &test.body);
            }
            _ => {}
        }
    }
    bodies
}

/// Maps a source type onto its machine representation.
///
/// Nullability is erased: a `String?` is the same machine value as a `String`,
/// with null represented by a null pointer. The type checker has already
/// proved that a null can only reach a place that expects one.
pub fn lower_type(store: &noto_types::TypeStore, ty: TypeId) -> IrType {
    match store.get(ty) {
        Type::Primitive(primitive) => lower_primitive(*primitive),
        Type::String => IrType::Str,
        Type::Nullable(inner) => lower_type(store, *inner),
        Type::Unit | Type::Nothing => IrType::Unit,
        // An object is a reference: a `Point` value is the address of its
        // fields, and assigning one copies the pointer.
        Type::Named { .. } | Type::Tuple(_) | Type::Function { .. } | Type::Any => IrType::Ptr,
        Type::Parameter { .. } | Type::Error => IrType::I64,
    }
}

/// How many bytes one field occupies in an object.
///
/// Every field gets a full machine word, whatever it holds. A `Bool` needs
/// one byte and an `Int32` four, so this wastes space — but it makes a
/// field's offset its index times eight, keeps every access naturally
/// aligned, and lets a load and a store move a whole register. Packing
/// smaller fields together is an optimisation, and one that needs the memory
/// model settled before it changes what an object looks like.
pub const FIELD_SIZE: u32 = 8;

/// The size in bytes of an object with `fields` fields.
///
/// An object with no fields still gets a word, so that two of them have
/// different addresses.
pub fn object_size(fields: usize) -> u32 {
    (fields as u32).max(1) * FIELD_SIZE
}

/// The byte offset of the field at `index`.
pub fn field_offset(index: u32) -> u32 {
    index * FIELD_SIZE
}

fn lower_primitive(primitive: Primitive) -> IrType {
    use Primitive::*;
    match primitive {
        Int | Int64 => IrType::I64,
        Int8 => IrType::I8,
        Int16 => IrType::I16,
        Int32 => IrType::I32,
        UInt | UInt64 => IrType::U64,
        UInt8 | Byte => IrType::U8,
        UInt16 => IrType::U16,
        UInt32 => IrType::U32,
        Float32 => IrType::F32,
        Float64 => IrType::F64,
        Bool => IrType::Bool,
        Char => IrType::Char,
    }
}

/// Builds the IR for one function.
pub(crate) struct Builder<'a> {
    pub(crate) analysis: &'a Analysis,
    pub(crate) program: &'a mut Program,
    pub(crate) sink: &'a mut DiagnosticSink,
    function_ids: &'a HashMap<FunctionId, FuncId>,
    /// The function being built.
    current: FuncId,
    /// The block instructions are appended to.
    pub(crate) block: BlockId,
    /// Which IR slot each semantic local lives in.
    slots: HashMap<LocalId, SlotId>,
    /// The blocks `break` and `continue` jump to, innermost last.
    pub(crate) loops: Vec<LoopTargets>,
}

/// Where `break` and `continue` go inside one loop.
#[derive(Clone, Copy)]
pub(crate) struct LoopTargets {
    /// The block a `continue` jumps to.
    pub(crate) continue_block: BlockId,
    /// The block a `break` jumps to.
    pub(crate) break_block: BlockId,
}

impl<'a> Builder<'a> {
    fn new(
        analysis: &'a Analysis,
        program: &'a mut Program,
        sink: &'a mut DiagnosticSink,
        function_ids: &'a HashMap<FunctionId, FuncId>,
        current: FuncId,
    ) -> Self {
        Builder {
            analysis,
            program,
            sink,
            function_ids,
            current,
            block: BlockId(0),
            slots: HashMap::new(),
            loops: Vec::new(),
        }
    }

    /// Lowers a whole function body.
    fn lower_function(&mut self, semantic_id: FunctionId, body: &noto_ast::Block) {
        let info = self.analysis.function(semantic_id);
        let result = lower_type(&self.analysis.store, info.result);

        // Slots are allocated for every local up front, parameters first, so
        // that a slot id is stable for the whole function.
        let mut parameters = Vec::new();
        for local_id in &info.locals {
            let local = self.analysis.local(*local_id);
            let slot = SlotId(self.function().slots.len() as u32);
            let ty = lower_type(&self.analysis.store, local.ty);
            self.function_mut().slots.push(Slot {
                name: local.name.clone(),
                ty,
                is_parameter: local.is_parameter,
            });
            self.slots.insert(*local_id, slot);
            if local.is_parameter {
                parameters.push(slot);
            }
        }
        self.function_mut().parameters = parameters;

        let entry = self.new_block("entry");
        debug_assert_eq!(entry, BlockId(0), "the entry block must come first");
        self.block = entry;

        let value = self.lower_block(body);

        // A body that falls off its end returns its trailing value, or nothing
        // when the function produces `Unit`.
        if !self.current_block_is_terminated() {
            let terminator = if result.is_unit() {
                Terminator::Return(None)
            } else {
                Terminator::Return(Some(value.unwrap_or(Operand::Const(Const::Unit))))
            };
            self.set_terminator(terminator);
        }
    }

    // --- function and block plumbing --------------------------------------

    pub(crate) fn function(&self) -> &Function {
        self.program.function(self.current)
    }

    pub(crate) fn function_mut(&mut self) -> &mut Function {
        self.program.function_mut(self.current)
    }

    /// Starts a new block and returns its id. The insertion point is not moved.
    pub(crate) fn new_block(&mut self, label: &str) -> BlockId {
        let function = self.function_mut();
        let id = BlockId(function.blocks.len() as u32);
        function.blocks.push(Block {
            id,
            label: format!("{label}{}", id.0),
            instructions: Vec::new(),
            terminator: Terminator::Unreachable,
        });
        id
    }

    /// Moves the insertion point.
    pub(crate) fn switch_to(&mut self, block: BlockId) {
        self.block = block;
    }

    /// Whether the current block already ends in a real terminator.
    ///
    /// A block starts out `Unreachable`, which doubles as "not terminated
    /// yet": lowering only overwrites it once, so code after a `return` is
    /// dropped rather than appended to a finished block.
    pub(crate) fn current_block_is_terminated(&self) -> bool {
        !matches!(self.function().block(self.block).terminator, Terminator::Unreachable)
    }

    /// Sets the terminator of the current block, if it does not have one.
    pub(crate) fn set_terminator(&mut self, terminator: Terminator) {
        if self.current_block_is_terminated() {
            return;
        }
        let block = self.block;
        self.function_mut().block_mut(block).terminator = terminator;
    }

    /// Appends an instruction to the current block.
    pub(crate) fn push(&mut self, kind: InstKind, span: Span) {
        if self.current_block_is_terminated() {
            return;
        }
        let block = self.block;
        self.function_mut().block_mut(block).instructions.push(Inst::new(kind, span));
    }

    /// Allocates a value id of the given type.
    pub(crate) fn new_value(&mut self, ty: IrType) -> ValueId {
        let function = self.function_mut();
        let id = ValueId(function.value_types.len() as u32);
        function.value_types.push(ty);
        id
    }

    /// Emits an instruction that produces a value and returns it.
    pub(crate) fn emit_value(
        &mut self,
        ty: IrType,
        span: Span,
        build: impl FnOnce(ValueId) -> InstKind,
    ) -> Operand {
        let dest = self.new_value(ty);
        self.push(build(dest), span);
        Operand::Value(dest)
    }

    /// The IR slot a semantic local lives in.
    pub(crate) fn slot_of(&self, local: LocalId) -> Option<SlotId> {
        self.slots.get(&local).copied()
    }

    /// The IR id of a semantic function.
    pub(crate) fn func_id_of(&self, function: FunctionId) -> Option<FuncId> {
        self.function_ids.get(&function).copied()
    }

    /// The machine type of an AST node, from the type checker's records.
    pub(crate) fn type_of(&self, id: NodeId) -> IrType {
        lower_type(&self.analysis.store, self.analysis.type_of(id))
    }

    /// Reports a construct lowering does not handle.
    pub(crate) fn unsupported(&mut self, span: Span, what: &str) -> Operand {
        self.sink.emit(
            Diagnostic::error(
                codes::UNSUPPORTED_CONSTRUCT,
                format!("{what} cannot be compiled to native code yet"),
            )
            .with_primary(span, "not implemented in Noto 0.3"),
        );
        Operand::Const(Const::Unit)
    }
}

#[cfg(test)]
mod tests;
