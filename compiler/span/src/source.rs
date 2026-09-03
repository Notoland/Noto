//! Source files and the map that owns them.

use crate::{BytePos, Span};

/// Identifies a [`SourceFile`] inside a [`SourceMap`].
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct FileId(u32);

impl FileId {
    /// The id used by [`Span::dummy`]; never refers to a real file.
    pub const DUMMY: FileId = FileId(u32::MAX);

    /// Builds a file id from a raw index. Intended for tests and for the
    /// `SourceMap` itself.
    pub const fn from_index(index: u32) -> Self {
        FileId(index)
    }

    /// The raw index behind this id.
    pub const fn index(self) -> u32 {
        self.0
    }
}

/// A human-facing 1-based position inside a source file.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct LineCol {
    /// 1-based line number.
    pub line: u32,
    /// 1-based column, counted in Unicode scalar values rather than bytes so
    /// that the number matches what an editor shows.
    pub column: u32,
}

/// A single unit of Noto source together with a precomputed line table.
pub struct SourceFile {
    id: FileId,
    name: String,
    text: String,
    /// Byte offset of the first character of each line.
    line_starts: Vec<BytePos>,
}

impl SourceFile {
    fn new(id: FileId, name: String, text: String) -> Self {
        let mut line_starts = vec![0];
        for (offset, byte) in text.bytes().enumerate() {
            if byte == b'\n' {
                line_starts.push(offset as BytePos + 1);
            }
        }
        SourceFile { id, name, text, line_starts }
    }

    /// The id this file was registered under.
    pub fn id(&self) -> FileId {
        self.id
    }

    /// The display name of the file, usually its path.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// The complete source text.
    pub fn text(&self) -> &str {
        &self.text
    }

    /// The length of the file in bytes.
    pub fn len(&self) -> u32 {
        self.text.len() as u32
    }

    /// Returns `true` for an empty file.
    pub fn is_empty(&self) -> bool {
        self.text.is_empty()
    }

    /// The source text a span covers, or `None` if the span is out of bounds.
    pub fn slice(&self, span: Span) -> Option<&str> {
        self.text.get(span.start as usize..span.end as usize)
    }

    /// Converts a byte offset into a 1-based line/column pair.
    ///
    /// Offsets past the end of the file are clamped to the last position, so
    /// this never panics on a malformed span.
    pub fn line_col(&self, pos: BytePos) -> LineCol {
        let pos = pos.min(self.len());
        let line_index = match self.line_starts.binary_search(&pos) {
            Ok(exact) => exact,
            Err(next) => next - 1,
        };
        let line_start = self.line_starts[line_index] as usize;
        let column = self.text[line_start..pos as usize].chars().count() as u32 + 1;
        LineCol { line: line_index as u32 + 1, column }
    }

    /// The text of a 1-based line, without its trailing newline.
    pub fn line_text(&self, line: u32) -> Option<&str> {
        let index = line.checked_sub(1)? as usize;
        let start = *self.line_starts.get(index)? as usize;
        let end = self
            .line_starts
            .get(index + 1)
            .map(|next| *next as usize - 1)
            .unwrap_or(self.text.len());
        Some(self.text[start..end].trim_end_matches('\r'))
    }

    /// The number of lines in the file.
    pub fn line_count(&self) -> u32 {
        self.line_starts.len() as u32
    }
}

/// Owns every source file taking part in a compilation.
#[derive(Default)]
pub struct SourceMap {
    files: Vec<SourceFile>,
}

impl SourceMap {
    /// Creates an empty source map.
    pub fn new() -> Self {
        SourceMap { files: Vec::new() }
    }

    /// Registers a file and returns its id.
    pub fn add(&mut self, name: impl Into<String>, text: impl Into<String>) -> FileId {
        let id = FileId(self.files.len() as u32);
        self.files.push(SourceFile::new(id, name.into(), text.into()));
        id
    }

    /// Looks a file up by id.
    pub fn file(&self, id: FileId) -> Option<&SourceFile> {
        self.files.get(id.0 as usize)
    }

    /// Iterates over every registered file.
    pub fn files(&self) -> impl Iterator<Item = &SourceFile> {
        self.files.iter()
    }

    /// The source text a span covers, if the file is known.
    pub fn slice(&self, span: Span) -> Option<&str> {
        self.file(span.file)?.slice(span)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn line_col_for_a_multiline_file() {
        let mut map = SourceMap::new();
        let id = map.add("a.noto", "fn main() {\n    println(\"hi\")\n}\n");
        let file = map.file(id).unwrap();

        assert_eq!(file.line_col(0), LineCol { line: 1, column: 1 });
        assert_eq!(file.line_col(11), LineCol { line: 1, column: 12 });
        assert_eq!(file.line_col(12), LineCol { line: 2, column: 1 });
        assert_eq!(file.line_col(16), LineCol { line: 2, column: 5 });
        assert_eq!(file.line_count(), 4);
    }

    #[test]
    fn columns_count_characters_not_bytes() {
        let mut map = SourceMap::new();
        // "João" is 5 bytes but 4 characters.
        let id = map.add("a.noto", "val n = \"João\"|");
        let file = map.file(id).unwrap();
        let bar = file.text().find('|').unwrap() as u32;
        assert_eq!(file.line_col(bar).column, 15);
    }

    #[test]
    fn line_text_strips_the_newline() {
        let mut map = SourceMap::new();
        let id = map.add("a.noto", "one\ntwo\nthree");
        let file = map.file(id).unwrap();
        assert_eq!(file.line_text(1), Some("one"));
        assert_eq!(file.line_text(2), Some("two"));
        assert_eq!(file.line_text(3), Some("three"));
        assert_eq!(file.line_text(4), None);
    }

    #[test]
    fn slicing_a_span_returns_the_source_text() {
        let mut map = SourceMap::new();
        let id = map.add("a.noto", "val answer = 42");
        assert_eq!(map.slice(Span::new(id, 13, 15)), Some("42"));
    }

    #[test]
    fn positions_past_the_end_are_clamped() {
        let mut map = SourceMap::new();
        let id = map.add("a.noto", "ab\n");
        let file = map.file(id).unwrap();
        assert_eq!(file.line_col(9999), LineCol { line: 2, column: 1 });
    }
}
