//! A read-only walker over the tree.
//!
//! Phases that only need to observe the AST — the linter, the language
//! server's symbol index, the test collector — implement [`Visitor`] and
//! override the handful of methods they care about. Each `visit_*` method
//! defaults to the matching `walk_*` free function, so overriding one and
//! calling `walk_*` from it keeps the traversal going.
//!
//! The lifetime is the tree's, so a visitor may keep references into what it
//! walks — collecting every lambda body, say — rather than only counting
//! what it sees.

use crate::{
    Block, Expr, ExprKind, Item, ItemKind, Literal, Module, Pattern, PatternKind, Stmt, StmtKind,
    StringSegment, TypeExpr, TypeExprKind, WhenArm,
};

/// A read-only traversal over the AST.
#[allow(unused_variables)]
pub trait Visitor<'ast>: Sized {
    /// Visits a whole module.
    fn visit_module(&mut self, module: &'ast Module) {
        walk_module(self, module);
    }
    /// Visits a declaration.
    fn visit_item(&mut self, item: &'ast Item) {
        walk_item(self, item);
    }
    /// Visits a block.
    fn visit_block(&mut self, block: &'ast Block) {
        walk_block(self, block);
    }
    /// Visits a statement.
    fn visit_stmt(&mut self, stmt: &'ast Stmt) {
        walk_stmt(self, stmt);
    }
    /// Visits an expression.
    fn visit_expr(&mut self, expr: &'ast Expr) {
        walk_expr(self, expr);
    }
    /// Visits a pattern.
    fn visit_pattern(&mut self, pattern: &'ast Pattern) {
        walk_pattern(self, pattern);
    }
    /// Visits a type expression.
    fn visit_type(&mut self, ty: &'ast TypeExpr) {
        walk_type(self, ty);
    }
    /// Visits one arm of a `when`.
    fn visit_when_arm(&mut self, arm: &'ast WhenArm) {
        walk_when_arm(self, arm);
    }
}

/// Walks every item in a module.
pub fn walk_module<'ast, V: Visitor<'ast>>(visitor: &mut V, module: &'ast Module) {
    for item in &module.items {
        visitor.visit_item(item);
    }
}

/// Walks the children of a declaration.
pub fn walk_item<'ast, V: Visitor<'ast>>(visitor: &mut V, item: &'ast Item) {
    match &item.kind {
        ItemKind::Fn(func) => {
            if let Some(receiver) = &func.receiver {
                visitor.visit_type(receiver);
            }
            for param in &func.params {
                if let Some(ty) = &param.ty {
                    visitor.visit_type(ty);
                }
                if let Some(default) = &param.default {
                    visitor.visit_expr(default);
                }
            }
            if let Some(result) = &func.result {
                visitor.visit_type(result);
            }
            if let Some(body) = &func.body {
                visitor.visit_block(body);
            }
        }
        ItemKind::TypeDecl(decl) => {
            for field in decl.primary_params.iter().chain(&decl.fields) {
                if let Some(ty) = &field.ty {
                    visitor.visit_type(ty);
                }
                if let Some(default) = &field.default {
                    visitor.visit_expr(default);
                }
            }
            for property in &decl.properties {
                if let Some(ty) = &property.ty {
                    visitor.visit_type(ty);
                }
                if let Some(default) = &property.default {
                    visitor.visit_expr(default);
                }
                for accessor in [&property.getter, &property.setter].into_iter().flatten() {
                    if let Some(body) = &accessor.body {
                        visitor.visit_block(body);
                    }
                }
            }
            for base in decl.base.iter().chain(&decl.interfaces) {
                visitor.visit_type(base);
            }
            for method in &decl.methods {
                visitor.visit_item(method);
            }
        }
        ItemKind::Interface(interface) => {
            for base in &interface.interfaces {
                visitor.visit_type(base);
            }
            for method in &interface.methods {
                visitor.visit_item(method);
            }
        }
        ItemKind::Enum(enum_item) => {
            for case in &enum_item.cases {
                for field in &case.fields {
                    if let Some(ty) = &field.ty {
                        visitor.visit_type(ty);
                    }
                }
                if let Some(value) = &case.value {
                    visitor.visit_expr(value);
                }
            }
            for method in &enum_item.methods {
                visitor.visit_item(method);
            }
        }
        ItemKind::Const(item) => {
            if let Some(ty) = &item.ty {
                visitor.visit_type(ty);
            }
            visitor.visit_expr(&item.value);
        }
        ItemKind::Test(test) => visitor.visit_block(&test.body),
        ItemKind::Import(_) | ItemKind::Error => {}
    }
}

/// Walks every statement in a block.
pub fn walk_block<'ast, V: Visitor<'ast>>(visitor: &mut V, block: &'ast Block) {
    for stmt in &block.statements {
        visitor.visit_stmt(stmt);
    }
}

/// Walks the children of a statement.
pub fn walk_stmt<'ast, V: Visitor<'ast>>(visitor: &mut V, stmt: &'ast Stmt) {
    match &stmt.kind {
        StmtKind::Let { pattern, ty, value, .. } => {
            visitor.visit_pattern(pattern);
            if let Some(ty) = ty {
                visitor.visit_type(ty);
            }
            if let Some(value) = value {
                visitor.visit_expr(value);
            }
        }
        StmtKind::Expr(expr) => visitor.visit_expr(expr),
        StmtKind::While { condition, body } => {
            visitor.visit_expr(condition);
            visitor.visit_block(body);
        }
        StmtKind::Loop { body } => visitor.visit_block(body),
        StmtKind::For { pattern, iterable, body } => {
            visitor.visit_pattern(pattern);
            visitor.visit_expr(iterable);
            visitor.visit_block(body);
        }
        StmtKind::Defer { value } => visitor.visit_expr(value),
        StmtKind::Item(item) => visitor.visit_item(item),
        StmtKind::Error => {}
    }
}

/// Walks the children of an expression.
pub fn walk_expr<'ast, V: Visitor<'ast>>(visitor: &mut V, expr: &'ast Expr) {
    match &expr.kind {
        ExprKind::Literal(Literal::Str(segments)) => {
            for segment in segments {
                if let StringSegment::Interpolation(inner) = segment {
                    visitor.visit_expr(inner);
                }
            }
        }
        ExprKind::Literal(_)
        | ExprKind::Path(_)
        | ExprKind::This
        | ExprKind::Super
        | ExprKind::Break
        | ExprKind::Continue
        | ExprKind::Error => {}
        ExprKind::Tuple(items) | ExprKind::ListLiteral(items) => {
            for item in items {
                visitor.visit_expr(item);
            }
        }
        ExprKind::Unary { operand, .. } => visitor.visit_expr(operand),
        ExprKind::Binary { left, right, .. } => {
            visitor.visit_expr(left);
            visitor.visit_expr(right);
        }
        ExprKind::Member { receiver, .. } => visitor.visit_expr(receiver),
        ExprKind::Index { target, index } => {
            visitor.visit_expr(target);
            visitor.visit_expr(index);
        }
        ExprKind::Call(call) => {
            visitor.visit_expr(&call.callee);
            for ty in &call.type_arguments {
                visitor.visit_type(ty);
            }
            for argument in &call.arguments {
                visitor.visit_expr(&argument.value);
            }
        }
        ExprKind::Assign { target, value, .. } => {
            visitor.visit_expr(target);
            visitor.visit_expr(value);
        }
        ExprKind::If { condition, then_branch, else_branch } => {
            visitor.visit_expr(condition);
            visitor.visit_block(then_branch);
            if let Some(else_branch) = else_branch {
                visitor.visit_expr(else_branch);
            }
        }
        ExprKind::When { scrutinee, arms } => {
            if let Some(scrutinee) = scrutinee {
                visitor.visit_expr(scrutinee);
            }
            for arm in arms {
                visitor.visit_when_arm(arm);
            }
        }
        ExprKind::Block(block) | ExprKind::Unsafe(block) => visitor.visit_block(block),
        ExprKind::Lambda(lambda) => {
            for param in &lambda.parameters {
                if let Some(ty) = &param.ty {
                    visitor.visit_type(ty);
                }
            }
            visitor.visit_block(&lambda.body);
        }
        ExprKind::Range { start, end, .. } => {
            if let Some(start) = start {
                visitor.visit_expr(start);
            }
            if let Some(end) = end {
                visitor.visit_expr(end);
            }
        }
        ExprKind::Is { value, ty, .. } | ExprKind::As { value, ty, .. } => {
            visitor.visit_expr(value);
            visitor.visit_type(ty);
        }
        ExprKind::Try(inner) | ExprKind::Await(inner) => visitor.visit_expr(inner),
        ExprKind::Return(value) => {
            if let Some(value) = value {
                visitor.visit_expr(value);
            }
        }
    }
}

/// Walks the children of a pattern.
pub fn walk_pattern<'ast, V: Visitor<'ast>>(visitor: &mut V, pattern: &'ast Pattern) {
    match &pattern.kind {
        PatternKind::Wildcard | PatternKind::Null | PatternKind::Error => {}
        PatternKind::Binding { subpattern, .. } => {
            if let Some(subpattern) = subpattern {
                visitor.visit_pattern(subpattern);
            }
        }
        PatternKind::Value(expr) => visitor.visit_expr(expr),
        PatternKind::Range { start, end, .. } => {
            if let Some(start) = start {
                visitor.visit_expr(start);
            }
            if let Some(end) = end {
                visitor.visit_expr(end);
            }
        }
        PatternKind::Type(ty) => visitor.visit_type(ty),
        PatternKind::EnumCase { fields, .. } => {
            for field in fields.iter().flatten() {
                visitor.visit_pattern(field);
            }
        }
        PatternKind::Tuple(items) | PatternKind::Destructure { fields: items, .. } => {
            for item in items {
                visitor.visit_pattern(item);
            }
        }
    }
}

/// Walks the children of a type expression.
pub fn walk_type<'ast, V: Visitor<'ast>>(visitor: &mut V, ty: &'ast TypeExpr) {
    match &ty.kind {
        TypeExprKind::Named { arguments, .. } => {
            for argument in arguments {
                visitor.visit_type(argument);
            }
        }
        TypeExprKind::Nullable(inner) | TypeExprKind::List(inner) => visitor.visit_type(inner),
        TypeExprKind::Tuple(items) => {
            for item in items {
                visitor.visit_type(item);
            }
        }
        TypeExprKind::Function { parameters, result, .. } => {
            for parameter in parameters {
                visitor.visit_type(parameter);
            }
            visitor.visit_type(result);
        }
        TypeExprKind::Error => {}
    }
}

/// Walks the children of a `when` arm.
pub fn walk_when_arm<'ast, V: Visitor<'ast>>(visitor: &mut V, arm: &'ast WhenArm) {
    for pattern in &arm.patterns {
        visitor.visit_pattern(pattern);
    }
    if let Some(guard) = &arm.guard {
        visitor.visit_expr(&guard.condition);
    }
    visitor.visit_expr(&arm.body);
}
