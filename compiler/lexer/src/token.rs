//! Tokens produced by the Noto lexer.

use crate::Keyword;
use noto_span::Span;

/// A lexed token together with its source location.
#[derive(Clone, PartialEq, Debug)]
pub struct Token {
    /// What kind of token this is.
    pub kind: TokenKind,
    /// Where it appeared in source.
    pub span: Span,
    /// Whether at least one line break separates this token from the previous
    /// one.
    ///
    /// Noto has no statement terminator: a statement ends at a line break
    /// when the statement is complete. The lexer stays purely lexical and only
    /// records where the breaks were; the parser owns the rule that decides
    /// when one ends a statement.
    pub newline_before: bool,
}

impl Token {
    /// Builds a token.
    pub fn new(kind: TokenKind, span: Span, newline_before: bool) -> Self {
        Token { kind, span, newline_before }
    }

    /// Whether this is the end-of-file marker.
    pub fn is_eof(&self) -> bool {
        self.kind == TokenKind::Eof
    }

    /// The keyword this token holds, if it is one.
    pub fn keyword(&self) -> Option<Keyword> {
        match &self.kind {
            TokenKind::Keyword(keyword) => Some(*keyword),
            _ => None,
        }
    }

    /// The identifier text, if this token is an identifier.
    pub fn ident(&self) -> Option<&str> {
        match &self.kind {
            TokenKind::Ident(name) => Some(name),
            _ => None,
        }
    }
}

/// The base an integer literal was written in.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Base {
    /// `0b1010`
    Binary,
    /// `0o755`
    Octal,
    /// `42`
    Decimal,
    /// `0xff`
    Hex,
}

/// An explicit width/signedness suffix on a numeric literal, as in `10u8`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum NumericSuffix {
    /// `i8`
    I8,
    /// `i16`
    I16,
    /// `i32`
    I32,
    /// `i64`
    I64,
    /// `u8`
    U8,
    /// `u16`
    U16,
    /// `u32`
    U32,
    /// `u64`
    U64,
    /// `f32`
    F32,
    /// `f64`
    F64,
}

impl NumericSuffix {
    /// Parses a suffix spelling.
    pub fn from_str(text: &str) -> Option<NumericSuffix> {
        use NumericSuffix::*;
        Some(match text {
            "i8" => I8,
            "i16" => I16,
            "i32" => I32,
            "i64" => I64,
            "u8" => U8,
            "u16" => U16,
            "u32" => U32,
            "u64" => U64,
            "f32" => F32,
            "f64" => F64,
            _ => return None,
        })
    }

    /// The source spelling of the suffix.
    pub fn as_str(self) -> &'static str {
        use NumericSuffix::*;
        match self {
            I8 => "i8",
            I16 => "i16",
            I32 => "i32",
            I64 => "i64",
            U8 => "u8",
            U16 => "u16",
            U32 => "u32",
            U64 => "u64",
            F32 => "f32",
            F64 => "f64",
        }
    }

    /// Whether the suffix names a floating point type.
    pub fn is_float(self) -> bool {
        matches!(self, NumericSuffix::F32 | NumericSuffix::F64)
    }
}

/// An integer literal with its value already decoded.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct IntLiteral {
    /// The magnitude of the literal. A leading `-` is a unary operator, not
    /// part of the literal, so this is always non-negative.
    pub value: u128,
    /// How it was written.
    pub base: Base,
    /// The suffix, if the programmer pinned a width.
    pub suffix: Option<NumericSuffix>,
}

/// A floating point literal with its value already decoded.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct FloatLiteral {
    /// The decoded value.
    pub value: f64,
    /// The suffix, if the programmer pinned a width.
    pub suffix: Option<NumericSuffix>,
}

/// One piece of a string literal.
///
/// A literal without interpolation is a single [`StringPart::Text`]. `"$name"`
/// and `"${a + b}"` both produce an [`StringPart::Interpolation`] holding the
/// tokens of the embedded expression.
#[derive(Clone, PartialEq, Debug)]
pub enum StringPart {
    /// Literal text with escapes already resolved.
    Text(String),
    /// An embedded expression, lexed but not yet parsed.
    Interpolation {
        /// The tokens of the expression, terminated by [`TokenKind::Eof`].
        tokens: Vec<Token>,
        /// The span of the interpolation including its `$` marker.
        span: Span,
    },
}

/// A string literal, possibly interpolated.
#[derive(Clone, PartialEq, Debug)]
pub struct StringLiteral {
    /// The pieces, in source order.
    pub parts: Vec<StringPart>,
    /// Whether it was written with `"""` delimiters.
    pub multiline: bool,
}

impl StringLiteral {
    /// The text of the literal when it contains no interpolation.
    pub fn as_plain_text(&self) -> Option<&str> {
        match self.parts.as_slice() {
            [] => Some(""),
            [StringPart::Text(text)] => Some(text),
            _ => None,
        }
    }

    /// Whether any part of the literal is an interpolation.
    pub fn is_interpolated(&self) -> bool {
        self.parts.iter().any(|p| matches!(p, StringPart::Interpolation { .. }))
    }
}

/// The kinds of token the Noto lexer produces.
#[derive(Clone, PartialEq, Debug)]
pub enum TokenKind {
    /// An identifier such as `userName`.
    Ident(String),
    /// A reserved word.
    Keyword(Keyword),
    /// An integer literal.
    Int(IntLiteral),
    /// A floating point literal.
    Float(FloatLiteral),
    /// A string literal.
    Str(StringLiteral),
    /// A character literal.
    Char(char),
    /// A documentation comment, `/// ...`, kept for tooling.
    DocComment(String),

    // --- delimiters -----------------------------------------------------
    /// `(`
    LParen,
    /// `)`
    RParen,
    /// `{`
    LBrace,
    /// `}`
    RBrace,
    /// `[`
    LBracket,
    /// `]`
    RBracket,

    // --- punctuation ----------------------------------------------------
    /// `,`
    Comma,
    /// `.`
    Dot,
    /// `:`
    Colon,
    /// `::`
    ColonColon,
    /// `;`
    Semicolon,
    /// `->`
    Arrow,
    /// `=>`
    FatArrow,
    /// `@`
    At,
    /// `_`
    Underscore,
    /// `..`
    DotDot,
    /// `..=`
    DotDotEq,

    // --- operators ------------------------------------------------------
    /// `+`
    Plus,
    /// `-`
    Minus,
    /// `*`
    Star,
    /// `/`
    Slash,
    /// `%`
    Percent,
    /// `=`
    Eq,
    /// `==`
    EqEq,
    /// `!=`
    BangEq,
    /// `<`
    Lt,
    /// `<=`
    LtEq,
    /// `>`
    Gt,
    /// `>=`
    GtEq,
    /// `!`
    Bang,
    /// `&&`
    AmpAmp,
    /// `||`
    PipePipe,
    /// `&`
    Amp,
    /// `|`
    Pipe,
    /// `^`
    Caret,
    /// `~`
    Tilde,
    /// `<<`
    Shl,
    /// `>>`
    Shr,
    /// `+=`
    PlusEq,
    /// `-=`
    MinusEq,
    /// `*=`
    StarEq,
    /// `/=`
    SlashEq,
    /// `%=`
    PercentEq,
    /// `&=`
    AmpEq,
    /// `|=`
    PipeEq,
    /// `^=`
    CaretEq,
    /// `<<=`
    ShlEq,
    /// `>>=`
    ShrEq,
    /// `?`
    Question,
    /// `?.`
    QuestionDot,
    /// `?:`
    Elvis,

    /// End of input.
    Eof,
}

impl TokenKind {
    /// A short human-readable description used in parser diagnostics.
    pub fn describe(&self) -> String {
        use TokenKind::*;
        match self {
            Ident(name) => format!("identifier `{name}`"),
            Keyword(keyword) => format!("keyword `{}`", keyword.as_str()),
            Int(_) => "integer literal".to_string(),
            Float(_) => "float literal".to_string(),
            Str(_) => "string literal".to_string(),
            Char(_) => "character literal".to_string(),
            DocComment(_) => "doc comment".to_string(),
            Eof => "end of file".to_string(),
            other => format!("`{}`", other.symbol().unwrap_or("?")),
        }
    }

    /// The source spelling for fixed tokens; `None` for tokens whose text
    /// varies.
    pub fn symbol(&self) -> Option<&'static str> {
        use TokenKind::*;
        Some(match self {
            LParen => "(",
            RParen => ")",
            LBrace => "{",
            RBrace => "}",
            LBracket => "[",
            RBracket => "]",
            Comma => ",",
            Dot => ".",
            Colon => ":",
            ColonColon => "::",
            Semicolon => ";",
            Arrow => "->",
            FatArrow => "=>",
            At => "@",
            Underscore => "_",
            DotDot => "..",
            DotDotEq => "..=",
            Plus => "+",
            Minus => "-",
            Star => "*",
            Slash => "/",
            Percent => "%",
            Eq => "=",
            EqEq => "==",
            BangEq => "!=",
            Lt => "<",
            LtEq => "<=",
            Gt => ">",
            GtEq => ">=",
            Bang => "!",
            AmpAmp => "&&",
            PipePipe => "||",
            Amp => "&",
            Pipe => "|",
            Caret => "^",
            Tilde => "~",
            Shl => "<<",
            Shr => ">>",
            PlusEq => "+=",
            MinusEq => "-=",
            StarEq => "*=",
            SlashEq => "/=",
            PercentEq => "%=",
            AmpEq => "&=",
            PipeEq => "|=",
            CaretEq => "^=",
            ShlEq => "<<=",
            ShrEq => ">>=",
            Question => "?",
            QuestionDot => "?.",
            Elvis => "?:",
            _ => return None,
        })
    }

    /// Whether a line break directly after this token can end a statement.
    ///
    /// A statement never ends on a dangling operator or an open delimiter, so
    /// `a +\n b` is one expression while `a\n+ b` is two statements.
    pub fn can_end_statement(&self) -> bool {
        use TokenKind::*;
        matches!(
            self,
            Ident(_)
                | Int(_)
                | Float(_)
                | Str(_)
                | Char(_)
                | RParen
                | RBrace
                | RBracket
                | Question
                | Underscore
        ) || matches!(
            self,
            Keyword(
                crate::Keyword::True
                    | crate::Keyword::False
                    | crate::Keyword::Null
                    | crate::Keyword::This
                    | crate::Keyword::Super
                    | crate::Keyword::Break
                    | crate::Keyword::Continue
                    | crate::Keyword::Return
            )
        )
    }
}
