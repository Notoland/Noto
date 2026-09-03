//! Lowering expressions.

use crate::{lower_type, Builder};
use noto_ast::{BinaryOp, Expr, ExprKind, Literal, StringSegment, UnaryOp};
use noto_ir::{BinOp, Const, InstKind, Intrinsic, IrType, Operand, Terminator, UnOp};
use noto_semantic::{Builtin, Resolution};
use noto_span::Span;

impl Builder<'_> {
    /// Lowers an expression, returning the value it produces.
    pub(crate) fn lower_expr(&mut self, expr: &Expr) -> Operand {
        match &expr.kind {
            ExprKind::Literal(literal) => self.lower_literal(literal, expr),
            ExprKind::Path(_) => self.lower_path(expr),
            ExprKind::Unary { op, operand } => self.lower_unary(*op, operand, expr),
            ExprKind::Binary { op, left, right, .. } => self.lower_binary(*op, left, right, expr),
            ExprKind::Assign { target, value, op, .. } => {
                self.lower_assign(target, value, *op, expr.span)
            }
            ExprKind::If { condition, then_branch, else_branch } => {
                self.lower_if(condition, then_branch, else_branch.as_deref(), expr)
            }
            ExprKind::When { scrutinee, arms } => {
                self.lower_when(scrutinee.as_deref(), arms, expr)
            }
            ExprKind::Block(block) => {
                self.lower_block(block).unwrap_or(Operand::Const(Const::Unit))
            }
            ExprKind::Call(call) => self.lower_call(call, expr),
            ExprKind::Member { receiver, name, .. } => self.lower_member(receiver, name, expr),
            ExprKind::Return(value) => {
                let value = value.as_deref().map(|value| self.lower_expr(value));
                let result = self.function().result;
                let terminator = if result.is_unit() {
                    Terminator::Return(None)
                } else {
                    Terminator::Return(Some(value.unwrap_or(Operand::Const(Const::Unit))))
                };
                self.set_terminator(terminator);
                Operand::Const(Const::Unit)
            }
            ExprKind::Break => {
                if let Some(targets) = self.loops.last().copied() {
                    self.set_terminator(Terminator::Jump(targets.break_block));
                }
                Operand::Const(Const::Unit)
            }
            ExprKind::Continue => {
                if let Some(targets) = self.loops.last().copied() {
                    self.set_terminator(Terminator::Jump(targets.continue_block));
                }
                Operand::Const(Const::Unit)
            }
            ExprKind::Tuple(items) if items.is_empty() => Operand::Const(Const::Unit),
            ExprKind::Range { .. } => {
                // A range only exists as part of a `for` or a `when` arm, both
                // of which consume it before this point.
                self.unsupported(expr.span, "a range used as a value")
            }
            _ => self.unsupported(expr.span, "this expression"),
        }
    }

    fn lower_literal(&mut self, literal: &Literal, expr: &Expr) -> Operand {
        let ty = self.type_of(expr.id);
        match literal {
            Literal::Int { value, .. } => {
                Operand::Const(Const::Int { value: *value as i128, ty })
            }
            Literal::Bool(value) => Operand::Const(Const::Bool(*value)),
            Literal::Char(value) => Operand::Const(Const::Char(*value)),
            Literal::Null => Operand::Const(Const::Null),
            Literal::Str(segments) => self.lower_string(segments, expr.span),
            Literal::Float { .. } => {
                self.unsupported(expr.span, "floating point arithmetic")
            }
        }
    }

    /// Builds an interpolated string by converting each part and joining them.
    ///
    /// A literal with no interpolation is a single constant, so the common case
    /// costs nothing at runtime.
    fn lower_string(&mut self, segments: &[StringSegment], span: Span) -> Operand {
        let mut result: Option<Operand> = None;

        for segment in segments {
            let piece = match segment {
                StringSegment::Text(text) => {
                    if text.is_empty() && segments.len() > 1 {
                        continue;
                    }
                    let id = self.program.intern_string(text);
                    Operand::Const(Const::Str(id))
                }
                StringSegment::Interpolation(inner) => {
                    let value = self.lower_expr(inner);
                    self.to_string(value, inner, span)
                }
            };

            result = Some(match result {
                None => piece,
                Some(left) => self.emit_value(IrType::Str, span, |dest| InstKind::Intrinsic {
                    dest: Some(dest),
                    which: Intrinsic::StringConcat,
                    arguments: vec![left, piece],
                }),
            });
        }

        result.unwrap_or_else(|| {
            let id = self.program.intern_string("");
            Operand::Const(Const::Str(id))
        })
    }

    /// Converts a value to its text form for interpolation.
    fn to_string(&mut self, value: Operand, expr: &Expr, span: Span) -> Operand {
        let ty = self.analysis.type_of(expr.id);
        let Some(builtin) = noto_semantic::builtins::to_string_for(&self.analysis.store, ty) else {
            return self.unsupported(expr.span, "interpolating this type");
        };
        match builtin {
            Builtin::StringToString => value,
            Builtin::IntToString => {
                self.emit_value(IrType::Str, span, |dest| InstKind::Intrinsic {
                    dest: Some(dest),
                    which: Intrinsic::IntToString,
                    arguments: vec![value],
                })
            }
            Builtin::BoolToString => {
                self.emit_value(IrType::Str, span, |dest| InstKind::Intrinsic {
                    dest: Some(dest),
                    which: Intrinsic::BoolToString,
                    arguments: vec![value],
                })
            }
            _ => self.unsupported(expr.span, "interpolating this type"),
        }
    }

    fn lower_path(&mut self, expr: &Expr) -> Operand {
        match self.analysis.resolution(expr.id) {
            Some(Resolution::Local(local)) => {
                let Some(slot) = self.slot_of(local) else {
                    return self.unsupported(expr.span, "this binding");
                };
                let ty = self.function().slot(slot).ty;
                self.emit_value(ty, expr.span, |dest| InstKind::LoadLocal { dest, slot })
            }
            Some(Resolution::Const(id)) => {
                let constant = self.analysis.constant(id);
                let ty = lower_type(&self.analysis.store, constant.ty);
                match constant.value.clone() {
                    noto_semantic::ConstValue::Int(value) => {
                        Operand::Const(Const::Int { value, ty })
                    }
                    noto_semantic::ConstValue::Bool(value) => Operand::Const(Const::Bool(value)),
                    noto_semantic::ConstValue::Char(value) => Operand::Const(Const::Char(value)),
                    noto_semantic::ConstValue::Str(text) => {
                        let id = self.program.intern_string(&text);
                        Operand::Const(Const::Str(id))
                    }
                    noto_semantic::ConstValue::Error => Operand::Const(Const::Unit),
                }
            }
            _ => self.unsupported(expr.span, "using a function as a value"),
        }
    }

    fn lower_unary(&mut self, op: UnaryOp, operand: &Expr, expr: &Expr) -> Operand {
        let value = self.lower_expr(operand);
        let ty = self.type_of(expr.id);
        let op = match op {
            UnaryOp::Neg => UnOp::Neg,
            UnaryOp::Not => UnOp::LogicalNot,
            UnaryOp::BitNot => UnOp::Not,
        };
        self.emit_value(ty, expr.span, |dest| InstKind::Unary { dest, op, operand: value })
    }

    fn lower_binary(&mut self, op: BinaryOp, left: &Expr, right: &Expr, expr: &Expr) -> Operand {
        // `&&`, `||` and `?:` only evaluate their right side sometimes, so they
        // become branches rather than instructions.
        match op {
            BinaryOp::And => return self.lower_short_circuit(left, right, expr.span, false),
            BinaryOp::Or => return self.lower_short_circuit(left, right, expr.span, true),
            BinaryOp::Elvis => return self.lower_elvis(left, right, expr.span),
            _ => {}
        }

        let operand_ty = self.type_of(left.id);
        let left_value = self.lower_expr(left);

        // `String + String` is a runtime call, not a machine instruction.
        if op == BinaryOp::Add && operand_ty == IrType::Str {
            let right_value = self.lower_expr(right);
            return self.emit_value(IrType::Str, expr.span, |dest| InstKind::Intrinsic {
                dest: Some(dest),
                which: Intrinsic::StringConcat,
                arguments: vec![left_value, right_value],
            });
        }

        let right_value = self.lower_expr(right);
        let Some(ir_op) = binary_op(op, operand_ty) else {
            return self.unsupported(expr.span, "this operator");
        };
        let result_ty = if ir_op.is_comparison() { IrType::Bool } else { self.type_of(expr.id) };

        self.emit_value(result_ty, expr.span, |dest| InstKind::Binary {
            dest,
            op: ir_op,
            left: left_value,
            right: right_value,
        })
    }

    /// Lowers `&&` and `||` into a branch that skips the right operand.
    ///
    /// `short_circuit_on` is the value of the left operand that makes the right
    /// one unnecessary: `true` for `||`, `false` for `&&`.
    fn lower_short_circuit(
        &mut self,
        left: &Expr,
        right: &Expr,
        span: Span,
        short_circuit_on: bool,
    ) -> Operand {
        let result = self.add_temp_slot("cond$", IrType::Bool);

        let left_value = self.lower_expr(left);
        self.push(InstKind::StoreLocal { slot: result, value: left_value.clone() }, span);

        let evaluate_right = self.new_block("sc_rhs");
        let join = self.new_block("sc_join");
        let (then_block, else_block) = if short_circuit_on {
            (join, evaluate_right)
        } else {
            (evaluate_right, join)
        };
        self.set_terminator(Terminator::Branch { condition: left_value, then_block, else_block });

        self.switch_to(evaluate_right);
        let right_value = self.lower_expr(right);
        self.push(InstKind::StoreLocal { slot: result, value: right_value }, span);
        self.set_terminator(Terminator::Jump(join));

        self.switch_to(join);
        self.emit_value(IrType::Bool, span, |dest| InstKind::LoadLocal { dest, slot: result })
    }

    /// Lowers `a ?: b` into a null check that skips `b` when `a` has a value.
    fn lower_elvis(&mut self, left: &Expr, right: &Expr, span: Span) -> Operand {
        let ty = self.type_of(left.id);
        let result = self.add_temp_slot("elvis$", ty);

        let left_value = self.lower_expr(left);
        self.push(InstKind::StoreLocal { slot: result, value: left_value.clone() }, span);

        // Null is the null pointer, so the test is a comparison against zero.
        let is_null = self.emit_value(IrType::Bool, span, |dest| InstKind::Binary {
            dest,
            op: BinOp::Eq,
            left: left_value,
            right: Operand::Const(Const::Null),
        });

        let default_block = self.new_block("elvis_default");
        let join = self.new_block("elvis_join");
        self.set_terminator(Terminator::Branch {
            condition: is_null,
            then_block: default_block,
            else_block: join,
        });

        self.switch_to(default_block);
        let right_value = self.lower_expr(right);
        self.push(InstKind::StoreLocal { slot: result, value: right_value }, span);
        self.set_terminator(Terminator::Jump(join));

        self.switch_to(join);
        self.emit_value(ty, span, |dest| InstKind::LoadLocal { dest, slot: result })
    }

    fn lower_assign(
        &mut self,
        target: &Expr,
        value: &Expr,
        op: Option<BinaryOp>,
        span: Span,
    ) -> Operand {
        let Some(Resolution::Local(local)) = self.analysis.resolution(target.id) else {
            return self.unsupported(target.span, "assigning to this target");
        };
        let Some(slot) = self.slot_of(local) else {
            return self.unsupported(target.span, "assigning to this target");
        };
        let ty = self.function().slot(slot).ty;

        let stored = match op {
            None => self.lower_expr(value),
            Some(op) => {
                // `a += b` reads the slot, applies the operator and writes back.
                let current =
                    self.emit_value(ty, span, |dest| InstKind::LoadLocal { dest, slot });
                let right = self.lower_expr(value);

                if op == BinaryOp::Add && ty == IrType::Str {
                    self.emit_value(IrType::Str, span, |dest| InstKind::Intrinsic {
                        dest: Some(dest),
                        which: Intrinsic::StringConcat,
                        arguments: vec![current, right],
                    })
                } else {
                    let Some(ir_op) = binary_op(op, ty) else {
                        return self.unsupported(span, "this compound assignment");
                    };
                    self.emit_value(ty, span, |dest| InstKind::Binary {
                        dest,
                        op: ir_op,
                        left: current,
                        right,
                    })
                }
            }
        };

        self.push(InstKind::StoreLocal { slot, value: stored }, span);
        Operand::Const(Const::Unit)
    }

    fn lower_if(
        &mut self,
        condition: &Expr,
        then_branch: &noto_ast::Block,
        else_branch: Option<&Expr>,
        expr: &Expr,
    ) -> Operand {
        let span = expr.span;
        let ty = self.type_of(expr.id);
        // An `if` used as a statement has no value to carry, so no slot is
        // allocated for one.
        let result = (!ty.is_unit()).then(|| self.add_temp_slot("if$", ty));

        let condition_value = self.lower_expr(condition);
        let then_block = self.new_block("if_then");
        // An `if` with no `else` branches straight to the join, so no empty
        // block is created for the branch that does not exist.
        let else_block = else_branch.map(|_| self.new_block("if_else"));
        let join = self.new_block("if_join");

        self.set_terminator(Terminator::Branch {
            condition: condition_value,
            then_block,
            else_block: else_block.unwrap_or(join),
        });

        self.switch_to(then_block);
        let then_value = self.lower_block(then_branch);
        if let (Some(slot), Some(value)) = (result, then_value) {
            self.push(InstKind::StoreLocal { slot, value }, span);
        }
        self.set_terminator(Terminator::Jump(join));

        if let (Some(else_branch), Some(else_block)) = (else_branch, else_block) {
            self.switch_to(else_block);
            let else_value = self.lower_expr(else_branch);
            if let Some(slot) = result {
                self.push(InstKind::StoreLocal { slot, value: else_value }, span);
            }
            self.set_terminator(Terminator::Jump(join));
        }

        self.switch_to(join);
        match result {
            Some(slot) => {
                self.emit_value(ty, span, |dest| InstKind::LoadLocal { dest, slot })
            }
            None => Operand::Const(Const::Unit),
        }
    }

    /// Lowers `when` into a chain of tests, one arm at a time.
    fn lower_when(
        &mut self,
        scrutinee: Option<&Expr>,
        arms: &[noto_ast::WhenArm],
        expr: &Expr,
    ) -> Operand {
        let span = expr.span;
        let ty = self.type_of(expr.id);
        let result = (!ty.is_unit()).then(|| self.add_temp_slot("when$", ty));

        // The subject is evaluated once and kept in a slot, so an arm testing
        // it several times does not re-run the expression.
        let subject = scrutinee.map(|scrutinee| {
            let subject_ty = self.type_of(scrutinee.id);
            let slot = self.add_temp_slot("when$subject", subject_ty);
            let value = self.lower_expr(scrutinee);
            self.push(InstKind::StoreLocal { slot, value }, span);
            (slot, subject_ty)
        });

        let join = self.new_block("when_join");

        for arm in arms {
            let body_block = self.new_block("when_arm");
            let next = self.new_block("when_next");

            if arm.is_else {
                self.set_terminator(Terminator::Jump(body_block));
            } else {
                let test = self.lower_arm_test(arm, subject, span);
                self.set_terminator(Terminator::Branch {
                    condition: test,
                    then_block: body_block,
                    else_block: next,
                });
            }

            self.switch_to(body_block);
            let value = self.lower_expr(&arm.body);
            if let Some(slot) = result {
                self.push(InstKind::StoreLocal { slot, value }, arm.span);
            }
            self.set_terminator(Terminator::Jump(join));

            self.switch_to(next);
            if arm.is_else {
                // Nothing can follow `else`; the block stays unreachable.
                self.set_terminator(Terminator::Jump(join));
            }
        }

        self.set_terminator(Terminator::Jump(join));
        self.switch_to(join);

        match result {
            Some(slot) => {
                self.emit_value(ty, span, |dest| InstKind::LoadLocal { dest, slot })
            }
            None => Operand::Const(Const::Unit),
        }
    }

    /// Builds the condition that decides whether one `when` arm is taken.
    fn lower_arm_test(
        &mut self,
        arm: &noto_ast::WhenArm,
        subject: Option<(noto_ir::SlotId, IrType)>,
        span: Span,
    ) -> Operand {
        let mut test: Option<Operand> = None;

        for pattern in &arm.patterns {
            let Some((slot, subject_ty)) = subject else {
                return self.unsupported(pattern.span, "this pattern");
            };
            let matched = self.lower_pattern_test(pattern, slot, subject_ty, span);
            test = Some(match test {
                None => matched,
                // Several patterns on one arm match if any of them does.
                Some(previous) => {
                    self.emit_value(IrType::Bool, span, |dest| InstKind::Binary {
                        dest,
                        op: BinOp::Or,
                        left: previous,
                        right: matched,
                    })
                }
            });
        }

        let has_pattern_test = test.is_some();
        let mut condition = test.unwrap_or(Operand::Const(Const::Bool(true)));

        if let Some(guard) = &arm.guard {
            let guard_value = self.lower_expr(&guard.condition);
            condition = if has_pattern_test {
                self.emit_value(IrType::Bool, span, |dest| InstKind::Binary {
                    dest,
                    op: BinOp::And,
                    left: condition,
                    right: guard_value,
                })
            } else {
                guard_value
            };
        }

        condition
    }

    /// Builds the test for one pattern against the subject slot.
    fn lower_pattern_test(
        &mut self,
        pattern: &noto_ast::Pattern,
        slot: noto_ir::SlotId,
        ty: IrType,
        span: Span,
    ) -> Operand {
        use noto_ast::PatternKind;

        match &pattern.kind {
            PatternKind::Wildcard => Operand::Const(Const::Bool(true)),
            PatternKind::Value(value) => {
                let subject =
                    self.emit_value(ty, span, |dest| InstKind::LoadLocal { dest, slot });
                let expected = self.lower_expr(value);
                self.emit_value(IrType::Bool, span, |dest| InstKind::Binary {
                    dest,
                    op: BinOp::Eq,
                    left: subject,
                    right: expected,
                })
            }
            PatternKind::Range { start, end, inclusive } => {
                let mut test: Option<Operand> = None;

                if let Some(start) = start {
                    let subject =
                        self.emit_value(ty, span, |dest| InstKind::LoadLocal { dest, slot });
                    let low = self.lower_expr(start);
                    test = Some(self.emit_value(IrType::Bool, span, |dest| InstKind::Binary {
                        dest,
                        op: if ty.is_signed() { BinOp::SGe } else { BinOp::UGe },
                        left: subject,
                        right: low,
                    }));
                }
                if let Some(end) = end {
                    let subject =
                        self.emit_value(ty, span, |dest| InstKind::LoadLocal { dest, slot });
                    let high = self.lower_expr(end);
                    let op = match (*inclusive, ty.is_signed()) {
                        (true, true) => BinOp::SLe,
                        (false, true) => BinOp::SLt,
                        (true, false) => BinOp::ULe,
                        (false, false) => BinOp::ULt,
                    };
                    let upper = self.emit_value(IrType::Bool, span, |dest| InstKind::Binary {
                        dest,
                        op,
                        left: subject,
                        right: high,
                    });
                    test = Some(match test {
                        None => upper,
                        Some(lower) => {
                            self.emit_value(IrType::Bool, span, |dest| InstKind::Binary {
                                dest,
                                op: BinOp::And,
                                left: lower,
                                right: upper,
                            })
                        }
                    });
                }

                test.unwrap_or(Operand::Const(Const::Bool(true)))
            }
            PatternKind::Binding { subpattern: None, .. } => {
                // A bare name matches anything and binds the subject to it.
                if let Some(Resolution::Local(local)) = self.analysis.resolution(pattern.id) {
                    if let Some(target) = self.slot_of(local) {
                        let subject =
                            self.emit_value(ty, span, |dest| InstKind::LoadLocal { dest, slot });
                        self.push(InstKind::StoreLocal { slot: target, value: subject }, span);
                    }
                }
                Operand::Const(Const::Bool(true))
            }
            PatternKind::Null => {
                let subject =
                    self.emit_value(ty, span, |dest| InstKind::LoadLocal { dest, slot });
                self.emit_value(IrType::Bool, span, |dest| InstKind::Binary {
                    dest,
                    op: BinOp::Eq,
                    left: subject,
                    right: Operand::Const(Const::Null),
                })
            }
            _ => self.unsupported(pattern.span, "this pattern"),
        }
    }

    fn lower_call(&mut self, call: &noto_ast::CallExpr, expr: &Expr) -> Operand {
        // A builtin is resolved on the callee node by the type checker.
        if let Some(Resolution::Builtin(builtin)) = self.analysis.resolution(call.callee.id) {
            return self.lower_builtin_call(builtin, call, expr);
        }

        let Some(Resolution::Function(function)) = self.analysis.resolution(call.callee.id) else {
            return self.unsupported(expr.span, "calling this value");
        };
        let Some(callee) = self.func_id_of(function) else {
            return self.unsupported(expr.span, "calling this function");
        };

        let arguments: Vec<Operand> =
            call.arguments.iter().map(|argument| self.lower_expr(&argument.value)).collect();
        let result = self.program.function(callee).result;

        if result.is_unit() {
            self.push(InstKind::Call { dest: None, callee, arguments }, expr.span);
            Operand::Const(Const::Unit)
        } else {
            self.emit_value(result, expr.span, |dest| InstKind::Call {
                dest: Some(dest),
                callee,
                arguments,
            })
        }
    }

    fn lower_builtin_call(
        &mut self,
        builtin: Builtin,
        call: &noto_ast::CallExpr,
        expr: &Expr,
    ) -> Operand {
        let mut arguments = Vec::new();

        // A method's receiver becomes its first argument.
        if builtin.is_method() {
            if let ExprKind::Member { receiver, .. } = &call.callee.kind {
                arguments.push(self.lower_expr(receiver));
            }
        }
        for argument in &call.arguments {
            arguments.push(self.lower_expr(&argument.value));
        }

        // `"x".toString()` is the identity, so it is not worth a call.
        if builtin == Builtin::StringToString {
            return arguments.into_iter().next().unwrap_or(Operand::Const(Const::Unit));
        }

        let which = intrinsic_for(builtin);
        let result = which.result();

        if result.is_unit() {
            self.push(InstKind::Intrinsic { dest: None, which, arguments }, expr.span);
            Operand::Const(Const::Unit)
        } else {
            self.emit_value(result, expr.span, |dest| InstKind::Intrinsic {
                dest: Some(dest),
                which,
                arguments,
            })
        }
    }

    fn lower_member(&mut self, receiver: &Expr, _name: &noto_ast::Ident, expr: &Expr) -> Operand {
        let Some(Resolution::Builtin(builtin)) = self.analysis.resolution(expr.id) else {
            return self.unsupported(expr.span, "reading this member");
        };
        let value = self.lower_expr(receiver);
        let which = intrinsic_for(builtin);
        self.emit_value(which.result(), expr.span, |dest| InstKind::Intrinsic {
            dest: Some(dest),
            which,
            arguments: vec![value],
        })
    }
}

/// The IR operation a source operator becomes at the given operand type.
///
/// Signedness lives in the operation rather than the type, so this is where
/// `<` becomes either `slt` or `ult`.
fn binary_op(op: BinaryOp, ty: IrType) -> Option<BinOp> {
    let signed = ty.is_signed() || ty == IrType::Bool || ty == IrType::Char;
    Some(match op {
        BinaryOp::Add => BinOp::Add,
        BinaryOp::Sub => BinOp::Sub,
        BinaryOp::Mul => BinOp::Mul,
        BinaryOp::Div => {
            if signed {
                BinOp::SDiv
            } else {
                BinOp::UDiv
            }
        }
        BinaryOp::Rem => {
            if signed {
                BinOp::SRem
            } else {
                BinOp::URem
            }
        }
        BinaryOp::Eq => BinOp::Eq,
        BinaryOp::Ne => BinOp::Ne,
        BinaryOp::Lt => {
            if signed {
                BinOp::SLt
            } else {
                BinOp::ULt
            }
        }
        BinaryOp::Le => {
            if signed {
                BinOp::SLe
            } else {
                BinOp::ULe
            }
        }
        BinaryOp::Gt => {
            if signed {
                BinOp::SGt
            } else {
                BinOp::UGt
            }
        }
        BinaryOp::Ge => {
            if signed {
                BinOp::SGe
            } else {
                BinOp::UGe
            }
        }
        BinaryOp::BitAnd => BinOp::And,
        BinaryOp::BitOr => BinOp::Or,
        BinaryOp::BitXor => BinOp::Xor,
        BinaryOp::Shl => BinOp::Shl,
        BinaryOp::Shr => {
            if signed {
                BinOp::AShr
            } else {
                BinOp::LShr
            }
        }
        BinaryOp::And | BinaryOp::Or | BinaryOp::Elvis | BinaryOp::In => return None,
    })
}

/// The runtime routine a builtin maps to.
fn intrinsic_for(builtin: Builtin) -> Intrinsic {
    match builtin {
        Builtin::PrintString => Intrinsic::PrintString,
        Builtin::PrintlnString => Intrinsic::PrintlnString,
        Builtin::PrintInt => Intrinsic::PrintInt,
        Builtin::PrintlnInt => Intrinsic::PrintlnInt,
        Builtin::PrintBool => Intrinsic::PrintBool,
        Builtin::PrintlnBool => Intrinsic::PrintlnBool,
        Builtin::PrintlnEmpty => Intrinsic::PrintlnEmpty,
        Builtin::IntToString | Builtin::StringToString => Intrinsic::IntToString,
        Builtin::BoolToString => Intrinsic::BoolToString,
        Builtin::StringLength => Intrinsic::StringLength,
        Builtin::Assert => Intrinsic::Assert,
    }
}
