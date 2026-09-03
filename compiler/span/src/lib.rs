//! Source positions and source maps for the Noto compiler.
//!
//! Every token, AST node, type and IR instruction in the compiler carries a
//! [`Span`] so that diagnostics can point back at the exact bytes the user
//! wrote. Positions are stored as byte offsets into a single [`SourceFile`];
//! translation to human-facing line/column pairs happens only when a
//! diagnostic is rendered, which keeps the hot paths cheap.

#![deny(missing_docs)]

mod source;

pub use source::{FileId, LineCol, SourceFile, SourceMap};

/// A byte offset into a source file.
///
/// Noto source is always valid UTF-8, and every span boundary the compiler
/// produces lies on a character boundary.
pub type BytePos = u32;

/// A half-open byte range `[start, end)` within a single source file.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct Span {
    /// Offset of the first byte covered by the span.
    pub start: BytePos,
    /// Offset one past the last byte covered by the span.
    pub end: BytePos,
    /// The file the span belongs to.
    pub file: FileId,
}

impl Span {
    /// Creates a span covering `[start, end)` in `file`.
    pub const fn new(file: FileId, start: BytePos, end: BytePos) -> Self {
        Span { start, end, file }
    }

    /// A zero-width span at `pos`, used to point *between* two tokens.
    pub const fn at(file: FileId, pos: BytePos) -> Self {
        Span { start: pos, end: pos, file }
    }

    /// A placeholder span for compiler-synthesised nodes with no source text.
    pub const fn dummy() -> Self {
        Span { start: 0, end: 0, file: FileId::DUMMY }
    }

    /// Returns `true` if this span carries no source location.
    pub fn is_dummy(&self) -> bool {
        self.file == FileId::DUMMY && self.start == 0 && self.end == 0
    }

    /// The number of bytes the span covers.
    pub fn len(&self) -> u32 {
        self.end.saturating_sub(self.start)
    }

    /// Returns `true` if the span covers no bytes.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// The smallest span covering both `self` and `other`.
    ///
    /// If the two spans belong to different files, `self` is returned
    /// unchanged; joining across files is never meaningful.
    pub fn to(self, other: Span) -> Span {
        if self.file != other.file {
            return self;
        }
        Span {
            start: self.start.min(other.start),
            end: self.end.max(other.end),
            file: self.file,
        }
    }

    /// Returns `true` if `pos` falls inside the span.
    pub fn contains(&self, pos: BytePos) -> bool {
        pos >= self.start && pos < self.end
    }
}

impl std::fmt::Debug for Span {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}:{}..{}", self.file.index(), self.start, self.end)
    }
}

/// A value paired with the span of the source text it came from.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Spanned<T> {
    /// The wrapped value.
    pub node: T,
    /// Where the value appeared in source.
    pub span: Span,
}

impl<T> Spanned<T> {
    /// Pairs `node` with `span`.
    pub const fn new(node: T, span: Span) -> Self {
        Spanned { node, span }
    }

    /// Applies `f` to the contained value, preserving the span.
    pub fn map<U>(self, f: impl FnOnce(T) -> U) -> Spanned<U> {
        Spanned { node: f(self.node), span: self.span }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn f() -> FileId {
        FileId::from_index(1)
    }

    #[test]
    fn span_join_takes_the_outer_bounds() {
        let a = Span::new(f(), 4, 8);
        let b = Span::new(f(), 12, 16);
        assert_eq!(a.to(b), Span::new(f(), 4, 16));
        assert_eq!(b.to(a), Span::new(f(), 4, 16));
    }

    #[test]
    fn span_join_across_files_keeps_the_receiver() {
        let a = Span::new(FileId::from_index(1), 4, 8);
        let b = Span::new(FileId::from_index(2), 0, 2);
        assert_eq!(a.to(b), a);
    }

    #[test]
    fn dummy_span_is_recognised() {
        assert!(Span::dummy().is_dummy());
        assert!(!Span::new(f(), 0, 0).is_dummy());
    }

    #[test]
    fn contains_is_half_open() {
        let s = Span::new(f(), 3, 5);
        assert!(!s.contains(2));
        assert!(s.contains(3));
        assert!(s.contains(4));
        assert!(!s.contains(5));
    }
}
