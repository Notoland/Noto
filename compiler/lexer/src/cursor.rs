//! A character cursor over UTF-8 source text.

use noto_span::BytePos;

/// Reads characters from source while tracking the byte offset.
pub struct Cursor<'src> {
    text: &'src str,
    /// Byte offset of the next character to read.
    pos: usize,
}

impl<'src> Cursor<'src> {
    /// Starts at the beginning of `text`.
    pub fn new(text: &'src str) -> Self {
        Cursor { text, pos: 0 }
    }

    /// The byte offset of the next character.
    pub fn pos(&self) -> BytePos {
        self.pos as BytePos
    }

    /// The next character without consuming it.
    pub fn peek(&self) -> Option<char> {
        self.text[self.pos..].chars().next()
    }

    /// The character `n` positions ahead without consuming anything.
    pub fn peek_nth(&self, n: usize) -> Option<char> {
        self.text[self.pos..].chars().nth(n)
    }

    /// Consumes and returns the next character.
    pub fn bump(&mut self) -> Option<char> {
        let ch = self.peek()?;
        self.pos += ch.len_utf8();
        Some(ch)
    }

    /// Consumes the next character if it equals `expected`.
    pub fn eat(&mut self, expected: char) -> bool {
        if self.peek() == Some(expected) {
            self.pos += expected.len_utf8();
            true
        } else {
            false
        }
    }

    /// Consumes characters while `predicate` holds.
    pub fn eat_while(&mut self, mut predicate: impl FnMut(char) -> bool) {
        while let Some(ch) = self.peek() {
            if !predicate(ch) {
                break;
            }
            self.pos += ch.len_utf8();
        }
    }

    /// The source text between two byte offsets.
    pub fn slice(&self, start: BytePos, end: BytePos) -> &'src str {
        &self.text[start as usize..end as usize]
    }

    /// Whether the text at the cursor starts with `prefix`.
    pub fn starts_with(&self, prefix: &str) -> bool {
        self.text[self.pos..].starts_with(prefix)
    }

    /// Advances past `prefix` if it is present.
    pub fn eat_str(&mut self, prefix: &str) -> bool {
        if self.starts_with(prefix) {
            self.pos += prefix.len();
            true
        } else {
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn walks_multibyte_characters() {
        let mut cursor = Cursor::new("Joãoz");
        assert_eq!(cursor.bump(), Some('J'));
        assert_eq!(cursor.bump(), Some('o'));
        assert_eq!(cursor.bump(), Some('ã'));
        assert_eq!(cursor.pos(), 4);
        assert_eq!(cursor.bump(), Some('o'));
        assert!(cursor.eat('z'));
        assert_eq!(cursor.peek(), None);
        assert_eq!(cursor.bump(), None);
    }

    #[test]
    fn eat_while_stops_at_the_predicate() {
        let mut cursor = Cursor::new("abc123");
        cursor.eat_while(|c| c.is_ascii_alphabetic());
        assert_eq!(cursor.pos(), 3);
        assert_eq!(cursor.peek(), Some('1'));
    }

    #[test]
    fn eat_str_matches_whole_prefixes_only() {
        let mut cursor = Cursor::new("\"\"\"text");
        assert!(!cursor.eat_str("\"\"\"\""));
        assert!(cursor.eat_str("\"\"\""));
        assert_eq!(cursor.pos(), 3);
    }
}
