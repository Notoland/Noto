//! Expressions.

use crate::{Block, Ident, NodeId, Param, Path, Pattern, TypeExpr};
use noto_lexer::NumericSuffix;
use noto_span::Span;

/// An expression as written in source.
#[derive(Clone, PartialEq, Debug)]
pub struct Expr {
    /// What the expression computes.
    pub kind: ExprKind,
    /// Where it appeared.
    pub span: Span,
    /// The node's id.
    pub id: NodeId,
}

/// A literal value.
#[derive(Clone, PartialEq, Debug)]
pub enum Literal {
    /// An integer literal together with its optional width suffix.
    Int {
        /// The magnitude as written.
        value: u128,
        /// The suffix, if the programmer pinned a width.
        suffix: Option<NumericSuffix>,
    },
    /// A floating point literal.
    Float {
        /// The value.
        value: f64,
        /// The suffix, if the programmer pinned a width.
        suffix: Option<NumericSuffix>,
    },
    /// A string literal, possibly interpolated.
    Str(Vec<StringSegment>),
    /// A character literal.
    Char(char),
    /// `true` or `false`.
    Bool(bool),
    /// `null`.
    Null,
}

/// One piece of a string literal.
#[derive(Clone, PartialEq, Debug)]
pub enum StringSegment {
    /// Literal text with escapes already resolved.
    Text(String),
    /// An interpolated expression.
    Interpolation(Box<Expr>),
}

/// A binary operator.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum BinaryOp {
    /// `+`
    Add,
    /// `-`
    Sub,
    /// `*`
    Mul,
    /// `/`
    Div,
    /// `%`
    Rem,
    /// `==`
    Eq,
    /// `!=`
    Ne,
    /// `<`
    Lt,
    /// `<=`
    Le,
    /// `>`
    Gt,
    /// `>=`
    Ge,
    /// `&&`
    And,
    /// `||`
    Or,
    /// `&`
    BitAnd,
    /// `|`
    BitOr,
    /// `^`
    BitXor,
    /// `<<`
    Shl,
    /// `>>`
    Shr,
    /// `?:` — evaluates the right side only when the left is null.
    Elvis,
    /// `in`
    In,
}

impl BinaryOp {
    /// The source spelling of the operator.
    pub fn as_str(self) -> &'static str {
        use BinaryOp::*;
        match self {
            Add => "+",
            Sub => "-",
            Mul => "*",
            Div => "/",
            Rem => "%",
            Eq => "==",
            Ne => "!=",
            Lt => "<",
            Le => "<=",
            Gt => ">",
            Ge => ">=",
            And => "&&",
            Or => "||",
            BitAnd => "&",
            BitOr => "|",
            BitXor => "^",
            Shl => "<<",
            Shr => ">>",
            Elvis => "?:",
            In => "in",
        }
    }

    /// Whether the operator produces a `Bool` regardless of its operands.
    pub fn is_comparison(self) -> bool {
        use BinaryOp::*;
        matches!(self, Eq | Ne | Lt | Le | Gt | Ge | In)
    }

    /// Whether the operator only evaluates its right operand conditionally.
    pub fn is_short_circuit(self) -> bool {
        matches!(self, BinaryOp::And | BinaryOp::Or | BinaryOp::Elvis)
    }
}

/// A prefix operator.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum UnaryOp {
    /// `-`
    Neg,
    /// `!`
    Not,
    /// `~`
    BitNot,
}

impl UnaryOp {
    /// The source spelling of the operator.
    pub fn as_str(self) -> &'static str {
        match self {
            UnaryOp::Neg => "-",
            UnaryOp::Not => "!",
            UnaryOp::BitNot => "~",
        }
    }
}

/// One argument in a call.
#[derive(Clone, PartialEq, Debug)]
pub struct Argument {
    /// The parameter name, when the call uses named arguments.
    pub name: Option<Ident>,
    /// The value being passed.
    pub value: Expr,
    /// The span of the whole argument, name included.
    pub span: Span,
}

/// A function or method call.
#[derive(Clone, PartialEq, Debug)]
pub struct CallExpr {
    /// What is being called.
    pub callee: Box<Expr>,
    /// The arguments, in source order.
    pub arguments: Vec<Argument>,
    /// Explicit type arguments, as in `parse<Int>(text)`.
    pub type_arguments: Vec<TypeExpr>,
}

/// An anonymous function.
#[derive(Clone, PartialEq, Debug)]
pub struct LambdaExpr {
    /// The declared parameters. A trailing lambda with none uses the implicit
    /// `it` binding instead.
    pub parameters: Vec<Param>,
    /// The declared result type, if written.
    pub result: Option<TypeExpr>,
    /// The body.
    pub body: Block,
    /// Whether the lambda was written with `async`.
    pub is_async: bool,
    /// Whether it was written as a trailing `{ ... }` block on a call.
    pub is_trailing: bool,
}

/// A condition attached to a `when` arm.
#[derive(Clone, PartialEq, Debug)]
pub struct WhenGuard {
    /// The `Bool` expression that must hold for the arm to be taken.
    pub condition: Expr,
}

/// One arm of a `when` expression.
#[derive(Clone, PartialEq, Debug)]
pub struct WhenArm {
    /// The patterns this arm matches; several patterns share one body.
    pub patterns: Vec<Pattern>,
    /// An extra condition the arm requires.
    pub guard: Option<WhenGuard>,
    /// What the arm evaluates to.
    pub body: Expr,
    /// Whether this is the `else` arm.
    pub is_else: bool,
    /// The span of the whole arm.
    pub span: Span,
}

/// The shapes an expression can take.
#[derive(Clone, PartialEq, Debug)]
pub enum ExprKind {
    /// A literal value.
    Literal(Literal),
    /// A reference to a name in scope.
    Path(Path),
    /// `this`.
    This,
    /// `super`.
    Super,
    /// `(a, b)`: a tuple. A parenthesised single expression is not a tuple.
    Tuple(Vec<Expr>),
    /// `[1, 2, 3]`: a list.
    ListLiteral(Vec<Expr>),
    /// A prefix operator applied to an operand.
    Unary {
        /// The operator.
        op: UnaryOp,
        /// The operand.
        operand: Box<Expr>,
    },
    /// A binary operator applied to two operands.
    Binary {
        /// The operator.
        op: BinaryOp,
        /// The left operand.
        left: Box<Expr>,
        /// The right operand.
        right: Box<Expr>,
        /// The span of the operator itself, for precise diagnostics.
        op_span: Span,
    },
    /// `receiver.name`, or `receiver?.name` when `safe` is set.
    Member {
        /// The value the member is read from.
        receiver: Box<Expr>,
        /// The member name.
        name: Ident,
        /// Whether the access short-circuits on null.
        safe: bool,
    },
    /// `target[index]`.
    Index {
        /// The value being indexed.
        target: Box<Expr>,
        /// The index.
        index: Box<Expr>,
    },
    /// A call.
    Call(CallExpr),
    /// `target = value`, including the compound forms such as `+=`.
    Assign {
        /// Where the value is stored.
        target: Box<Expr>,
        /// The value.
        value: Box<Expr>,
        /// The arithmetic applied first, for `+=` and friends.
        op: Option<BinaryOp>,
        /// The span of the assignment operator.
        op_span: Span,
    },
    /// `if cond { .. } else { .. }`, an expression in Noto.
    If {
        /// The condition.
        condition: Box<Expr>,
        /// The branch taken when the condition holds.
        then_branch: Block,
        /// The branch taken otherwise.
        else_branch: Option<Box<Expr>>,
    },
    /// `when (value) { .. }`, or `when { .. }` with no scrutinee.
    When {
        /// The value being matched, if the `when` has one.
        scrutinee: Option<Box<Expr>>,
        /// The arms, in source order.
        arms: Vec<WhenArm>,
    },
    /// A `{ .. }` block used as an expression.
    Block(Block),
    /// A lambda.
    Lambda(Box<LambdaExpr>),
    /// `start..end` or `start..=end`.
    Range {
        /// The lower bound.
        start: Option<Box<Expr>>,
        /// The upper bound.
        end: Option<Box<Expr>>,
        /// Whether the upper bound is included.
        inclusive: bool,
    },
    /// `value is Type` or `value !is Type`.
    Is {
        /// The value being tested.
        value: Box<Expr>,
        /// The type it is tested against.
        ty: TypeExpr,
        /// Whether the test is negated.
        negated: bool,
    },
    /// `value as Type` or `value as? Type`.
    As {
        /// The value being converted.
        value: Box<Expr>,
        /// The target type.
        ty: TypeExpr,
        /// Whether the conversion yields null instead of failing.
        safe: bool,
    },
    /// `expr?`: propagates a null or an error to the caller.
    Try(Box<Expr>),
    /// `await expr`.
    Await(Box<Expr>),
    /// `unsafe { .. }`.
    Unsafe(Block),
    /// `return expr`.
    Return(Option<Box<Expr>>),
    /// `break`.
    Break,
    /// `continue`.
    Continue,
    /// An expression the parser could not read; already reported.
    Error,
}

impl Expr {
    /// Whether the expression can appear on the left of an assignment.
    ///
    /// This is the syntactic half of the check; whether the target is a `val`
    /// is decided during name resolution.
    pub fn is_assignable(&self) -> bool {
        matches!(
            self.kind,
            ExprKind::Path(_)
                | ExprKind::Index { .. }
                | ExprKind::Member { safe: false, .. }
        )
    }

    /// Whether the expression ends in a block, which changes how a following
    /// line break is read.
    pub fn ends_with_block(&self) -> bool {
        matches!(
            self.kind,
            ExprKind::Block(_)
                | ExprKind::If { .. }
                | ExprKind::When { .. }
                | ExprKind::Unsafe(_)
        )
    }
}
