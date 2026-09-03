//! The Noto abstract syntax tree.
//!
//! The AST mirrors the source closely: it keeps the shape the programmer
//! wrote, spans for every node, and no resolved information. Names are still
//! strings here; resolution and typing happen in `noto-semantic`, which
//! annotates the tree through side tables keyed by [`NodeId`] rather than by
//! mutating it. That split keeps the parser free of semantic concerns and lets
//! tooling — the formatter, the linter, the language server — work on an
//! unresolved tree.

#![deny(missing_docs)]

mod expr;
mod item;
mod pattern;
mod stmt;
mod ty;
pub mod visit;

pub use expr::{
    Argument, BinaryOp, CallExpr, Expr, ExprKind, LambdaExpr, Literal, StringSegment, UnaryOp,
    WhenArm, WhenGuard,
};
pub use noto_lexer::NumericSuffix;
pub use item::{
    Attribute, ClassKind, ConstItem, EnumCase, EnumItem, Field, FnItem, Item, ItemKind, ImportItem,
    InterfaceItem, Modifiers, Param, Property, PropertyAccessor, TestItem, TypeDeclItem,
    TypeParam, Visibility,
};
pub use pattern::{Pattern, PatternKind};
pub use stmt::{Block, LetKind, Stmt, StmtKind};
pub use ty::{TypeExpr, TypeExprKind};

use noto_span::Span;

/// A stable identifier for an AST node.
///
/// Every node the compiler may want to annotate carries one. Side tables in
/// later phases — resolved types, resolved names, IR mappings — are keyed by
/// this id, so the tree itself never has to be rewritten.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct NodeId(pub u32);

impl NodeId {
    /// The id used for nodes that were never allocated one.
    pub const DUMMY: NodeId = NodeId(u32::MAX);
}

/// Hands out [`NodeId`]s during parsing.
#[derive(Default, Debug)]
pub struct NodeIdGenerator {
    next: u32,
}

impl NodeIdGenerator {
    /// Creates a generator starting from zero.
    pub fn new() -> Self {
        NodeIdGenerator { next: 0 }
    }

    /// Allocates the next id.
    pub fn next_id(&mut self) -> NodeId {
        let id = NodeId(self.next);
        self.next += 1;
        id
    }

    /// How many ids have been handed out.
    pub fn count(&self) -> usize {
        self.next as usize
    }
}

/// An identifier as written in source.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Ident {
    /// The text of the name.
    pub name: String,
    /// Where it appeared.
    pub span: Span,
}

impl Ident {
    /// Builds an identifier.
    pub fn new(name: impl Into<String>, span: Span) -> Self {
        Ident { name: name.into(), span }
    }
}

impl std::fmt::Display for Ident {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.name)
    }
}

/// A dotted path such as `std.io.File`.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Path {
    /// The segments, in source order. Never empty.
    pub segments: Vec<Ident>,
    /// The span covering the whole path.
    pub span: Span,
}

impl Path {
    /// Builds a path from its segments.
    pub fn new(segments: Vec<Ident>, span: Span) -> Self {
        debug_assert!(!segments.is_empty(), "a path always has at least one segment");
        Path { segments, span }
    }

    /// A single-segment path.
    pub fn single(ident: Ident) -> Self {
        let span = ident.span;
        Path { segments: vec![ident], span }
    }

    /// The last segment, which names the item itself.
    pub fn last(&self) -> &Ident {
        self.segments.last().expect("a path always has at least one segment")
    }

    /// Whether the path has exactly one segment.
    pub fn is_single(&self) -> bool {
        self.segments.len() == 1
    }

    /// The dotted spelling of the path.
    pub fn to_dotted(&self) -> String {
        self.segments.iter().map(|s| s.name.as_str()).collect::<Vec<_>>().join(".")
    }
}

/// One parsed source file.
#[derive(Debug)]
pub struct Module {
    /// The items declared at the top level, in source order.
    pub items: Vec<Item>,
    /// Where the whole file lives.
    pub span: Span,
    /// The module's own node id.
    pub id: NodeId,
}

impl Module {
    /// Every function declared at the top level.
    pub fn functions(&self) -> impl Iterator<Item = &FnItem> {
        self.items.iter().filter_map(|item| match &item.kind {
            ItemKind::Fn(func) => Some(func),
            _ => None,
        })
    }

    /// Looks up a top-level function by name.
    pub fn function(&self, name: &str) -> Option<&FnItem> {
        self.functions().find(|f| f.name.name == name)
    }

    /// Every test declared at the top level.
    pub fn tests(&self) -> impl Iterator<Item = &TestItem> {
        self.items.iter().filter_map(|item| match &item.kind {
            ItemKind::Test(test) => Some(test),
            _ => None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use noto_span::FileId;

    fn span() -> Span {
        Span::new(FileId::from_index(0), 0, 1)
    }

    #[test]
    fn node_ids_are_unique_and_sequential() {
        let mut generator = NodeIdGenerator::new();
        assert_eq!(generator.next_id(), NodeId(0));
        assert_eq!(generator.next_id(), NodeId(1));
        assert_eq!(generator.next_id(), NodeId(2));
        assert_eq!(generator.count(), 3);
    }

    #[test]
    fn paths_render_dotted() {
        let path = Path::new(
            vec![Ident::new("std", span()), Ident::new("io", span()), Ident::new("File", span())],
            span(),
        );
        assert_eq!(path.to_dotted(), "std.io.File");
        assert_eq!(path.last().name, "File");
        assert!(!path.is_single());
    }

    #[test]
    fn a_single_segment_path_is_its_own_last() {
        let path = Path::single(Ident::new("println", span()));
        assert!(path.is_single());
        assert_eq!(path.last().name, "println");
    }
}
