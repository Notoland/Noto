//! Lowering statements and blocks.

use crate::{Builder, LoopTargets};
use noto_ast::{Block, LetKind, Pattern, PatternKind, Stmt, StmtKind};
use noto_ir::{BinOp, Const, InstKind, IrType, Operand, Terminator};
use noto_semantic::Resolution;

impl Builder<'_> {
    /// Lowers a block, returning the value of its trailing expression.
    pub(crate) fn lower_block(&mut self, block: &Block) -> Option<Operand> {
        let mut value = None;
        for (index, stmt) in block.statements.iter().enumerate() {
            let is_last = index + 1 == block.statements.len();
            let stmt_value = self.lower_stmt(stmt);
            if is_last {
                value = stmt_value;
            }
            if self.current_block_is_terminated() {
                break;
            }
        }
        value
    }

    /// Lowers one statement, returning its value when it has one.
    fn lower_stmt(&mut self, stmt: &Stmt) -> Option<Operand> {
        match &stmt.kind {
            StmtKind::Let { kind, pattern, value, .. } => {
                self.lower_let(*kind, pattern, value.as_ref());
                None
            }
            StmtKind::Expr(expr) => Some(self.lower_expr(expr)),
            StmtKind::While { condition, body } => {
                self.lower_while(condition, body);
                None
            }
            StmtKind::Loop { body } => {
                self.lower_loop(body);
                None
            }
            StmtKind::For { pattern, iterable, body } => {
                self.lower_for(pattern, iterable, body);
                None
            }
            StmtKind::Defer { value } => {
                // Running deferred work on every exit path needs the scope
                // tracking that lands with the memory model; until then a
                // `defer` that would silently not run is an error, not a
                // no-op.
                self.unsupported(value.span, "`defer`");
                None
            }
            StmtKind::Item(item) => {
                self.unsupported(item.span, "declarations inside a function body");
                None
            }
            StmtKind::Error => None,
        }
    }

    fn lower_let(&mut self, _kind: LetKind, pattern: &Pattern, value: Option<&noto_ast::Expr>) {
        let Some(value) = value else { return };
        let operand = self.lower_expr(value);
        self.store_pattern(pattern, operand, value.span);
    }

    /// Writes a value into the slots a binding pattern names.
    fn store_pattern(&mut self, pattern: &Pattern, value: Operand, span: noto_span::Span) {
        match &pattern.kind {
            PatternKind::Wildcard => {}
            PatternKind::Binding { .. } => {
                let Some(Resolution::Local(local)) = self.analysis.resolution(pattern.id) else {
                    return;
                };
                let Some(slot) = self.slot_of(local) else { return };
                self.push(InstKind::StoreLocal { slot, value }, span);
            }
            _ => {
                self.unsupported(pattern.span, "this destructuring pattern");
            }
        }
    }

    /// `while cond { body }` becomes three blocks: the test, the body, and
    /// what follows.
    fn lower_while(&mut self, condition: &noto_ast::Expr, body: &Block) {
        let test = self.new_block("while_test");
        let body_block = self.new_block("while_body");
        let exit = self.new_block("while_exit");

        self.set_terminator(Terminator::Jump(test));
        self.switch_to(test);
        let condition_value = self.lower_expr(condition);
        self.set_terminator(Terminator::Branch {
            condition: condition_value,
            then_block: body_block,
            else_block: exit,
        });

        self.switch_to(body_block);
        self.loops.push(LoopTargets { continue_block: test, break_block: exit });
        self.lower_block(body);
        self.loops.pop();
        self.set_terminator(Terminator::Jump(test));

        self.switch_to(exit);
    }

    /// `loop { body }` runs until a `break`.
    fn lower_loop(&mut self, body: &Block) {
        let body_block = self.new_block("loop_body");
        let exit = self.new_block("loop_exit");

        self.set_terminator(Terminator::Jump(body_block));
        self.switch_to(body_block);
        self.loops.push(LoopTargets { continue_block: body_block, break_block: exit });
        self.lower_block(body);
        self.loops.pop();
        self.set_terminator(Terminator::Jump(body_block));

        self.switch_to(exit);
    }

    /// `for i in a..b { body }` becomes a counter, a test and an increment.
    ///
    /// The upper bound is evaluated once, before the loop, so a range whose end
    /// is a call does not call it on every iteration.
    fn lower_for(&mut self, pattern: &Pattern, iterable: &noto_ast::Expr, body: &Block) {
        if matches!(
            self.analysis.store.get(self.analysis.type_of(iterable.id)),
            noto_types::Type::List(_)
        ) {
            self.lower_for_list(pattern, iterable, body);
            return;
        }

        let noto_ast::ExprKind::Range { start, end, inclusive } = &iterable.kind else {
            self.unsupported(iterable.span, "iterating over this value");
            return;
        };
        let (Some(start), Some(end)) = (start, end) else {
            self.unsupported(iterable.span, "an open-ended range in a `for`");
            return;
        };

        let span = iterable.span;
        let start_value = self.lower_expr(start);
        let end_value = self.lower_expr(end);

        // The bound is copied into a slot so that the body cannot change it.
        let bound_slot = self.add_temp_slot("for$end", IrType::I64);
        self.push(InstKind::StoreLocal { slot: bound_slot, value: end_value }, span);

        let Some(counter_slot) = self.pattern_slot(pattern) else {
            // `for _ in ..` still has to run the loop the right number of
            // times, so the counter gets an anonymous slot.
            let slot = self.add_temp_slot("for$i", IrType::I64);
            self.lower_for_with_slot(slot, start_value, bound_slot, *inclusive, body, span);
            return;
        };

        self.lower_for_with_slot(counter_slot, start_value, bound_slot, *inclusive, body, span);
    }

    /// Lowers `for x in xs` over a list.
    ///
    /// The list is evaluated once into a slot, and so is its length: a body
    /// that reassigns the binding it walked cannot change what is walked, and
    /// the length is read before the first element rather than at every step.
    fn lower_for_list(
        &mut self,
        pattern: &Pattern,
        iterable: &noto_ast::Expr,
        body: &Block,
    ) {
        let span = iterable.span;
        let element_ty = match self.analysis.store.get(self.analysis.type_of(iterable.id)) {
            noto_types::Type::List(element) => {
                crate::lower_type(&self.analysis.store, *element)
            }
            _ => {
                self.unsupported(span, "iterating over this value");
                return;
            }
        };

        let list_slot = self.add_temp_slot("for$list", IrType::Ptr);
        let list = self.lower_expr(iterable);
        self.push(InstKind::StoreLocal { slot: list_slot, value: list }, span);

        let list_value =
            self.emit_value(IrType::Ptr, span, |dest| InstKind::LoadLocal { dest, slot: list_slot });
        let length = self.emit_value(IrType::I64, span, |dest| InstKind::Load {
            dest,
            address: list_value,
            offset: 0,
        });
        let bound_slot = self.add_temp_slot("for$end", IrType::I64);
        self.push(InstKind::StoreLocal { slot: bound_slot, value: length }, span);

        let index_slot = self.add_temp_slot("for$i", IrType::I64);
        self.push(
            InstKind::StoreLocal {
                slot: index_slot,
                value: Operand::Const(Const::Int { value: 0, ty: IrType::I64 }),
            },
            span,
        );

        let test = self.new_block("for_test");
        let body_block = self.new_block("for_body");
        let step = self.new_block("for_step");
        let exit = self.new_block("for_exit");

        self.set_terminator(Terminator::Jump(test));
        self.switch_to(test);
        let index = self.emit_value(IrType::I64, span, |dest| InstKind::LoadLocal {
            dest,
            slot: index_slot,
        });
        let bound = self.emit_value(IrType::I64, span, |dest| InstKind::LoadLocal {
            dest,
            slot: bound_slot,
        });
        let condition = self.emit_value(IrType::Bool, span, |dest| InstKind::Binary {
            dest,
            op: BinOp::SLt,
            left: index,
            right: bound,
        });
        self.set_terminator(Terminator::Branch {
            condition,
            then_block: body_block,
            else_block: exit,
        });

        self.switch_to(body_block);
        // The element is read here rather than in the test, so an empty list
        // never reads past its length.
        if let Some(slot) = self.pattern_slot(pattern) {
            let list_value = self.emit_value(IrType::Ptr, span, |dest| InstKind::LoadLocal {
                dest,
                slot: list_slot,
            });
            let index = self.emit_value(IrType::I64, span, |dest| InstKind::LoadLocal {
                dest,
                slot: index_slot,
            });
            let scaled = self.emit_value(IrType::I64, span, |dest| InstKind::Binary {
                dest,
                op: BinOp::Mul,
                left: index,
                right: Operand::Const(Const::Int {
                    value: crate::FIELD_SIZE as i128,
                    ty: IrType::I64,
                }),
            });
            let address = self.emit_value(IrType::Ptr, span, |dest| InstKind::Binary {
                dest,
                op: BinOp::Add,
                left: list_value,
                right: scaled,
            });
            let value = self.emit_value(element_ty, span, |dest| InstKind::Load {
                dest,
                address,
                offset: crate::FIELD_SIZE,
            });
            self.push(InstKind::StoreLocal { slot, value }, span);
        }

        self.loops.push(LoopTargets { continue_block: step, break_block: exit });
        self.lower_block(body);
        self.loops.pop();
        if !self.current_block_is_terminated() {
            self.set_terminator(Terminator::Jump(step));
        }

        self.switch_to(step);
        let index = self.emit_value(IrType::I64, span, |dest| InstKind::LoadLocal {
            dest,
            slot: index_slot,
        });
        let next = self.emit_value(IrType::I64, span, |dest| InstKind::Binary {
            dest,
            op: BinOp::Add,
            left: index,
            right: Operand::Const(Const::Int { value: 1, ty: IrType::I64 }),
        });
        self.push(InstKind::StoreLocal { slot: index_slot, value: next }, span);
        self.set_terminator(Terminator::Jump(test));

        self.switch_to(exit);
    }

    fn lower_for_with_slot(
        &mut self,
        counter: noto_ir::SlotId,
        start: Operand,
        bound: noto_ir::SlotId,
        inclusive: bool,
        body: &Block,
        span: noto_span::Span,
    ) {
        self.push(InstKind::StoreLocal { slot: counter, value: start }, span);

        let test = self.new_block("for_test");
        let body_block = self.new_block("for_body");
        let step = self.new_block("for_step");
        let exit = self.new_block("for_exit");

        self.set_terminator(Terminator::Jump(test));
        self.switch_to(test);
        let current = self.emit_value(IrType::I64, span, |dest| InstKind::LoadLocal {
            dest,
            slot: counter,
        });
        let limit =
            self.emit_value(IrType::I64, span, |dest| InstKind::LoadLocal { dest, slot: bound });
        let op = if inclusive { BinOp::SLe } else { BinOp::SLt };
        let keep_going = self.emit_value(IrType::Bool, span, |dest| InstKind::Binary {
            dest,
            op,
            left: current,
            right: limit,
        });
        self.set_terminator(Terminator::Branch {
            condition: keep_going,
            then_block: body_block,
            else_block: exit,
        });

        self.switch_to(body_block);
        // `continue` must still advance the counter, so it targets the step
        // block rather than the test.
        self.loops.push(LoopTargets { continue_block: step, break_block: exit });
        self.lower_block(body);
        self.loops.pop();
        self.set_terminator(Terminator::Jump(step));

        self.switch_to(step);
        let value = self.emit_value(IrType::I64, span, |dest| InstKind::LoadLocal {
            dest,
            slot: counter,
        });
        let next = self.emit_value(IrType::I64, span, |dest| InstKind::Binary {
            dest,
            op: BinOp::Add,
            left: value,
            right: Operand::Const(Const::Int { value: 1, ty: IrType::I64 }),
        });
        self.push(InstKind::StoreLocal { slot: counter, value: next }, span);
        self.set_terminator(Terminator::Jump(test));

        self.switch_to(exit);
    }

    /// The slot a simple binding pattern names.
    fn pattern_slot(&mut self, pattern: &Pattern) -> Option<noto_ir::SlotId> {
        let Some(Resolution::Local(local)) = self.analysis.resolution(pattern.id) else {
            return None;
        };
        self.slot_of(local)
    }

    /// Adds a slot the source did not name, used for loop bookkeeping.
    pub(crate) fn add_temp_slot(&mut self, name: &str, ty: IrType) -> noto_ir::SlotId {
        let function = self.function_mut();
        let id = noto_ir::SlotId(function.slots.len() as u32);
        function.slots.push(noto_ir::Slot {
            name: format!("{name}{}", id.0),
            ty,
            is_parameter: false,
        });
        id
    }
}
