//! Statements and blocks.

use crate::{Expr, Ident, NodeId, Pattern, TypeExpr};
use noto_span::Span;

/// Whether a binding may be reassigned.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum LetKind {
    /// `val`: the binding keeps whatever it was initialised with.
    ///
    /// This constrains the binding, not the value: a `val` holding a mutable
    /// object can still have that object's contents changed.
    Val,
    /// `var`: the binding may be reassigned.
    Var,
}

impl LetKind {
    /// The source spelling of the keyword.
    pub fn as_str(self) -> &'static str {
        match self {
            LetKind::Val => "val",
            LetKind::Var => "var",
        }
    }
}

/// A statement.
#[derive(Clone, PartialEq, Debug)]
pub struct Stmt {
    /// What the statement does.
    pub kind: StmtKind,
    /// Where it appeared.
    pub span: Span,
    /// The node's id.
    pub id: NodeId,
}

/// The shapes a statement can take.
#[derive(Clone, PartialEq, Debug)]
pub enum StmtKind {
    /// `val name: T = value` or `var name = value`.
    Let {
        /// Whether the binding is reassignable.
        kind: LetKind,
        /// What the binding names. A plain name is a `Binding` pattern; a
        /// destructuring binding is a tuple or destructure pattern.
        pattern: Pattern,
        /// The declared type, if written.
        ty: Option<TypeExpr>,
        /// The initialiser. Optional so that a declaration can be initialised
        /// on every path of a following `if`.
        value: Option<Expr>,
    },
    /// An expression evaluated for its effect or as the block's value.
    Expr(Expr),
    /// `while cond { .. }`.
    While {
        /// The condition.
        condition: Expr,
        /// The body.
        body: Block,
    },
    /// `loop { .. }`: repeats until a `break`.
    Loop {
        /// The body.
        body: Block,
    },
    /// `for name in iterable { .. }`.
    For {
        /// The loop variable pattern.
        pattern: Pattern,
        /// What is being iterated.
        iterable: Expr,
        /// The body.
        body: Block,
    },
    /// `defer expr`: runs on every exit from the enclosing scope.
    Defer {
        /// What to run.
        value: Expr,
    },
    /// An item declared inside a block, such as a local function.
    Item(Box<crate::Item>),
    /// A statement the parser could not read; already reported.
    Error,
}

/// A `{ .. }` block.
///
/// A block evaluates to its trailing expression when it has one, which is how
/// `if`, `when` and lambdas produce values without a `return`.
#[derive(Clone, PartialEq, Debug)]
pub struct Block {
    /// The statements, in source order.
    pub statements: Vec<Stmt>,
    /// Where the block appeared, braces included.
    pub span: Span,
    /// The node's id.
    pub id: NodeId,
}

impl Block {
    /// Builds a block.
    pub fn new(statements: Vec<Stmt>, span: Span, id: NodeId) -> Self {
        Block { statements, span, id }
    }

    /// An empty block.
    pub fn empty(span: Span, id: NodeId) -> Self {
        Block { statements: Vec::new(), span, id }
    }

    /// Whether the block has no statements.
    pub fn is_empty(&self) -> bool {
        self.statements.is_empty()
    }

    /// The trailing expression the block evaluates to, if it has one.
    ///
    /// Only a bare expression in final position counts; a `val` or a `while`
    /// leaves the block with no value.
    pub fn tail_expr(&self) -> Option<&Expr> {
        match self.statements.last() {
            Some(Stmt { kind: StmtKind::Expr(expr), .. }) => Some(expr),
            _ => None,
        }
    }
}

/// A binding introduced by a `val`/`var` statement with a simple name.
///
/// Convenience view used by the linter and the language server.
pub struct SimpleBinding<'a> {
    /// Whether it can be reassigned.
    pub kind: LetKind,
    /// The bound name.
    pub name: &'a Ident,
    /// The declared type, if written.
    pub ty: Option<&'a TypeExpr>,
    /// The initialiser, if written.
    pub value: Option<&'a Expr>,
}

impl Stmt {
    /// Views this statement as a simple `val`/`var` binding, if it is one.
    pub fn as_simple_binding(&self) -> Option<SimpleBinding<'_>> {
        let StmtKind::Let { kind, pattern, ty, value } = &self.kind else { return None };
        let crate::PatternKind::Binding { name, subpattern: None } = &pattern.kind else {
            return None;
        };
        Some(SimpleBinding { kind: *kind, name, ty: ty.as_ref(), value: value.as_ref() })
    }
}
