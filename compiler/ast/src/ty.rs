//! Type expressions as written in source.
//!
//! These are syntax, not semantics: `Int` here is the *name* `Int`, not the
//! resolved integer type. `noto-semantic` maps them onto `noto-types` values.

use crate::{NodeId, Path};
use noto_span::Span;

/// A type as it appears in source.
#[derive(Clone, PartialEq, Debug)]
pub struct TypeExpr {
    /// What the type expression says.
    pub kind: TypeExprKind,
    /// Where it appeared.
    pub span: Span,
    /// The node's id.
    pub id: NodeId,
}

/// The shapes a source type expression can take.
#[derive(Clone, PartialEq, Debug)]
pub enum TypeExprKind {
    /// A named type, possibly generic: `Int`, `List<String>`, `std.io.File`.
    Named {
        /// The name of the type.
        path: Path,
        /// Its type arguments, empty when the type is not generic.
        arguments: Vec<TypeExpr>,
    },
    /// `T?`: the type extended with the absence of a value.
    Nullable(Box<TypeExpr>),
    /// `(Int, String)`: an anonymous product type.
    Tuple(Vec<TypeExpr>),
    /// `fn(Int, Int): Int`: the type of a function value.
    Function {
        /// Parameter types.
        parameters: Vec<TypeExpr>,
        /// The result type.
        result: Box<TypeExpr>,
        /// Whether the function is `async`.
        is_async: bool,
    },
    /// `[Int]`: shorthand for `List<Int>`.
    List(Box<TypeExpr>),
    /// A type the parser could not read; already reported.
    Error,
}

impl TypeExpr {
    /// Builds a plain named type with no arguments.
    pub fn named(path: Path, id: NodeId) -> Self {
        let span = path.span;
        TypeExpr { kind: TypeExprKind::Named { path, arguments: Vec::new() }, span, id }
    }

    /// Whether the type is written as nullable.
    pub fn is_nullable(&self) -> bool {
        matches!(self.kind, TypeExprKind::Nullable(_))
    }

    /// The name of a simple named type, if this is one.
    pub fn simple_name(&self) -> Option<&str> {
        match &self.kind {
            TypeExprKind::Named { path, arguments } if arguments.is_empty() && path.is_single() => {
                Some(&path.last().name)
            }
            _ => None,
        }
    }

    /// Renders the type the way it was written, for diagnostics.
    pub fn render(&self) -> String {
        match &self.kind {
            TypeExprKind::Named { path, arguments } => {
                let base = path.to_dotted();
                if arguments.is_empty() {
                    base
                } else {
                    let args: Vec<String> = arguments.iter().map(TypeExpr::render).collect();
                    format!("{base}<{}>", args.join(", "))
                }
            }
            TypeExprKind::Nullable(inner) => format!("{}?", inner.render()),
            TypeExprKind::Tuple(items) => {
                let items: Vec<String> = items.iter().map(TypeExpr::render).collect();
                format!("({})", items.join(", "))
            }
            TypeExprKind::Function { parameters, result, is_async } => {
                let params: Vec<String> = parameters.iter().map(TypeExpr::render).collect();
                let prefix = if *is_async { "async fn" } else { "fn" };
                format!("{prefix}({}): {}", params.join(", "), result.render())
            }
            TypeExprKind::List(inner) => format!("[{}]", inner.render()),
            TypeExprKind::Error => "<error>".to_string(),
        }
    }
}
