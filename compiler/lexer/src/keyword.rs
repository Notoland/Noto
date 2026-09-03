//! The reserved word set of the Noto language.

/// A reserved word.
///
/// Noto reserves a small, fixed vocabulary. Words listed as *reserved for
/// future use* are rejected as identifiers today so that adding them later is
/// not a breaking change; see `docs/design/lexer.md`.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Keyword {
    /// `abstract`
    Abstract,
    /// `as`
    As,
    /// `async`
    Async,
    /// `await`
    Await,
    /// `break`
    Break,
    /// `class`
    Class,
    /// `const`
    Const,
    /// `continue`
    Continue,
    /// `data`
    Data,
    /// `defer`
    Defer,
    /// `else`
    Else,
    /// `enum`
    Enum,
    /// `export`
    Export,
    /// `false`
    False,
    /// `fn`
    Fn,
    /// `for`
    For,
    /// `get`
    Get,
    /// `if`
    If,
    /// `import`
    Import,
    /// `in`
    In,
    /// `init`
    Init,
    /// `interface`
    Interface,
    /// `internal`
    Internal,
    /// `is`
    Is,
    /// `loop`
    Loop,
    /// `null`
    Null,
    /// `override`
    Override,
    /// `private`
    Private,
    /// `protected`
    Protected,
    /// `public`
    Public,
    /// `return`
    Return,
    /// `sealed`
    Sealed,
    /// `set`
    Set,
    /// `struct`
    Struct,
    /// `super`
    Super,
    /// `test`
    Test,
    /// `this`
    This,
    /// `true`
    True,
    /// `unsafe`
    Unsafe,
    /// `val`
    Val,
    /// `var`
    Var,
    /// `when`
    When,
    /// `while`
    While,

    /// A word reserved for a future version of the language.
    Reserved(&'static str),
}

/// Words with no meaning yet, rejected as identifiers so that giving them
/// meaning later stays source compatible.
pub const RESERVED_FOR_FUTURE: &[&str] = &[
    "actor", "impl", "macro", "match", "module", "mut", "operator", "package", "static", "trait",
    "type", "typealias", "use", "where", "yield",
];

impl Keyword {
    /// Looks a word up in the reserved vocabulary.
    pub fn from_str(word: &str) -> Option<Keyword> {
        use Keyword::*;
        let keyword = match word {
            "abstract" => Abstract,
            "as" => As,
            "async" => Async,
            "await" => Await,
            "break" => Break,
            "class" => Class,
            "const" => Const,
            "continue" => Continue,
            "data" => Data,
            "defer" => Defer,
            "else" => Else,
            "enum" => Enum,
            "export" => Export,
            "false" => False,
            "fn" => Fn,
            "for" => For,
            "get" => Get,
            "if" => If,
            "import" => Import,
            "in" => In,
            "init" => Init,
            "interface" => Interface,
            "internal" => Internal,
            "is" => Is,
            "loop" => Loop,
            "null" => Null,
            "override" => Override,
            "private" => Private,
            "protected" => Protected,
            "public" => Public,
            "return" => Return,
            "sealed" => Sealed,
            "set" => Set,
            "struct" => Struct,
            "super" => Super,
            "test" => Test,
            "this" => This,
            "true" => True,
            "unsafe" => Unsafe,
            "val" => Val,
            "var" => Var,
            "when" => When,
            "while" => While,
            other => {
                let index = RESERVED_FOR_FUTURE.iter().position(|w| *w == other)?;
                Reserved(RESERVED_FOR_FUTURE[index])
            }
        };
        Some(keyword)
    }

    /// The exact source spelling of the word.
    pub fn as_str(self) -> &'static str {
        use Keyword::*;
        match self {
            Abstract => "abstract",
            As => "as",
            Async => "async",
            Await => "await",
            Break => "break",
            Class => "class",
            Const => "const",
            Continue => "continue",
            Data => "data",
            Defer => "defer",
            Else => "else",
            Enum => "enum",
            Export => "export",
            False => "false",
            Fn => "fn",
            For => "for",
            Get => "get",
            If => "if",
            Import => "import",
            In => "in",
            Init => "init",
            Interface => "interface",
            Internal => "internal",
            Is => "is",
            Loop => "loop",
            Null => "null",
            Override => "override",
            Private => "private",
            Protected => "protected",
            Public => "public",
            Return => "return",
            Sealed => "sealed",
            Set => "set",
            Struct => "struct",
            Super => "super",
            Test => "test",
            This => "this",
            True => "true",
            Unsafe => "unsafe",
            Val => "val",
            Var => "var",
            When => "when",
            While => "while",
            Reserved(word) => word,
        }
    }

    /// Whether the word is a visibility modifier.
    pub fn is_visibility(self) -> bool {
        use Keyword::*;
        matches!(self, Public | Private | Protected | Internal)
    }

    /// Whether the word may appear in the modifier list of a declaration.
    pub fn is_modifier(self) -> bool {
        use Keyword::*;
        self.is_visibility() || matches!(self, Abstract | Sealed | Override | Async | Data | Const)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_keyword_round_trips() {
        for word in [
            "abstract", "as", "async", "await", "break", "class", "const", "continue", "data",
            "defer", "else", "enum", "export", "false", "fn", "for", "get", "if", "import", "in",
            "init", "interface", "internal", "is", "loop", "null", "override", "private",
            "protected", "public", "return", "sealed", "set", "struct", "super", "test", "this",
            "true", "unsafe", "val", "var", "when", "while",
        ] {
            let keyword = Keyword::from_str(word).unwrap_or_else(|| panic!("`{word}` not reserved"));
            assert_eq!(keyword.as_str(), word);
        }
    }

    #[test]
    fn future_words_are_reserved() {
        for word in RESERVED_FOR_FUTURE {
            assert_eq!(Keyword::from_str(word), Some(Keyword::Reserved(word)));
        }
    }

    #[test]
    fn ordinary_words_are_not_keywords() {
        for word in ["name", "user", "Int", "println", "value", "result", "option"] {
            assert_eq!(Keyword::from_str(word), None);
        }
    }
}
