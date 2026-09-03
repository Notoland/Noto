//! Patterns, used by `when` arms and destructuring bindings.

use crate::{Expr, Ident, NodeId, Path, TypeExpr};
use noto_span::Span;

/// A pattern as written in source.
#[derive(Clone, PartialEq, Debug)]
pub struct Pattern {
    /// What the pattern matches.
    pub kind: PatternKind,
    /// Where it appeared.
    pub span: Span,
    /// The node's id.
    pub id: NodeId,
}

/// The shapes a pattern can take.
#[derive(Clone, PartialEq, Debug)]
pub enum PatternKind {
    /// `_`: matches anything and binds nothing.
    Wildcard,
    /// `name`: binds the scrutinee to a new name.
    Binding {
        /// The name being bound.
        name: Ident,
        /// A nested pattern the value must also match, as in `n @ 0..9`.
        subpattern: Option<Box<Pattern>>,
    },
    /// A constant the scrutinee must equal, such as a literal or an enum case.
    Value(Box<Expr>),
    /// `0..12` or `0..=12`.
    Range {
        /// The lower bound; `None` for an open range.
        start: Option<Box<Expr>>,
        /// The upper bound; `None` for an open range.
        end: Option<Box<Expr>>,
        /// Whether the upper bound is included.
        inclusive: bool,
    },
    /// `is String`: matches when the runtime type conforms.
    Type(TypeExpr),
    /// `Color.Red` or `Result.Success(value)`: an enum case, with optional
    /// bindings for its associated data.
    EnumCase {
        /// The case being matched.
        path: Path,
        /// Patterns for the associated values, if the case carries any.
        fields: Option<Vec<Pattern>>,
    },
    /// `(a, b)`: matches a tuple element-wise.
    Tuple(Vec<Pattern>),
    /// `User(name, age)`: destructures a data class or struct.
    Destructure {
        /// The type being destructured.
        path: Path,
        /// One pattern per field, in declaration order.
        fields: Vec<Pattern>,
    },
    /// `null`: matches the absence of a value.
    Null,
    /// A pattern the parser could not read; already reported.
    Error,
}

impl Pattern {
    /// Whether the pattern matches every possible value on its own.
    ///
    /// Exhaustiveness checking for sealed hierarchies lives in
    /// `noto-semantic`; this only answers the syntactic question.
    pub fn is_irrefutable(&self) -> bool {
        match &self.kind {
            PatternKind::Wildcard => true,
            PatternKind::Binding { subpattern, .. } => {
                subpattern.as_ref().is_none_or(|p| p.is_irrefutable())
            }
            PatternKind::Tuple(items) => items.iter().all(Pattern::is_irrefutable),
            _ => false,
        }
    }

    /// Collects every name the pattern binds, in source order.
    pub fn bindings(&self) -> Vec<&Ident> {
        let mut out = Vec::new();
        self.collect_bindings(&mut out);
        out
    }

    fn collect_bindings<'a>(&'a self, out: &mut Vec<&'a Ident>) {
        match &self.kind {
            PatternKind::Binding { name, subpattern } => {
                out.push(name);
                if let Some(inner) = subpattern {
                    inner.collect_bindings(out);
                }
            }
            PatternKind::Tuple(items) | PatternKind::Destructure { fields: items, .. } => {
                for item in items {
                    item.collect_bindings(out);
                }
            }
            PatternKind::EnumCase { fields: Some(fields), .. } => {
                for field in fields {
                    field.collect_bindings(out);
                }
            }
            _ => {}
        }
    }
}
