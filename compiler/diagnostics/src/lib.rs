//! Diagnostic construction and rendering for the Noto compiler.
//!
//! Noto treats error messages as part of the language's surface: every
//! diagnostic carries a stable code, a primary span, optional secondary
//! labels and optional help/note text. Rendering is separated from
//! construction so that the same diagnostic can be printed to a terminal,
//! serialised for the language server, or asserted on in tests.

#![deny(missing_docs)]

mod render;

pub use render::{render, RenderStyle};

use noto_span::{SourceMap, Span};

/// How serious a diagnostic is.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub enum Severity {
    /// A note attached to another diagnostic or emitted on its own.
    Note,
    /// Something suspicious that does not stop compilation.
    Warning,
    /// A compile error. Compilation continues to find more errors but no
    /// output is produced.
    Error,
    /// An error that makes further analysis meaningless.
    Fatal,
}

impl Severity {
    /// The lowercase word used when printing this severity.
    pub fn label(self) -> &'static str {
        match self {
            Severity::Note => "note",
            Severity::Warning => "warning",
            Severity::Error => "error",
            Severity::Fatal => "fatal",
        }
    }

    /// Whether a diagnostic of this severity prevents producing output.
    pub fn is_error(self) -> bool {
        matches!(self, Severity::Error | Severity::Fatal)
    }
}

/// A stable identifier for a class of diagnostic, e.g. `NOTO0102`.
///
/// Codes are stable across releases so that they can be suppressed, looked up
/// in the documentation and matched on by tooling. The numeric ranges are
/// allocated per compiler phase; see `docs/design/diagnostics.md`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Code(pub u16);

impl std::fmt::Display for Code {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "NOTO{:04}", self.0)
    }
}

/// A span annotated with an explanatory message.
#[derive(Clone, Debug)]
pub struct Label {
    /// Where the label points.
    pub span: Span,
    /// What to say about that location.
    pub message: String,
    /// Primary labels are underlined with `^`, secondary ones with `-`.
    pub primary: bool,
}

impl Label {
    /// A primary label: the place the diagnostic is really about.
    pub fn primary(span: Span, message: impl Into<String>) -> Self {
        Label { span, message: message.into(), primary: true }
    }

    /// A secondary label: context that explains the primary one.
    pub fn secondary(span: Span, message: impl Into<String>) -> Self {
        Label { span, message: message.into(), primary: false }
    }
}

/// A single compiler message.
#[derive(Clone, Debug)]
pub struct Diagnostic {
    /// How serious the message is.
    pub severity: Severity,
    /// The stable diagnostic code.
    pub code: Code,
    /// The one-line summary shown next to the severity.
    pub message: String,
    /// Spans this diagnostic points at, primary first.
    pub labels: Vec<Label>,
    /// Suggestions for fixing the problem.
    pub helps: Vec<String>,
    /// Additional context that is not a suggestion.
    pub notes: Vec<String>,
}

impl Diagnostic {
    /// Starts building a diagnostic with the given severity.
    pub fn new(severity: Severity, code: Code, message: impl Into<String>) -> Self {
        Diagnostic {
            severity,
            code,
            message: message.into(),
            labels: Vec::new(),
            helps: Vec::new(),
            notes: Vec::new(),
        }
    }

    /// Starts an error diagnostic.
    pub fn error(code: Code, message: impl Into<String>) -> Self {
        Diagnostic::new(Severity::Error, code, message)
    }

    /// Starts a warning diagnostic.
    pub fn warning(code: Code, message: impl Into<String>) -> Self {
        Diagnostic::new(Severity::Warning, code, message)
    }

    /// Starts a fatal diagnostic.
    pub fn fatal(code: Code, message: impl Into<String>) -> Self {
        Diagnostic::new(Severity::Fatal, code, message)
    }

    /// Adds a primary label.
    pub fn with_primary(mut self, span: Span, message: impl Into<String>) -> Self {
        self.labels.push(Label::primary(span, message));
        self
    }

    /// Adds a secondary label.
    pub fn with_secondary(mut self, span: Span, message: impl Into<String>) -> Self {
        self.labels.push(Label::secondary(span, message));
        self
    }

    /// Adds a `help:` line.
    pub fn with_help(mut self, help: impl Into<String>) -> Self {
        self.helps.push(help.into());
        self
    }

    /// Adds a `note:` line.
    pub fn with_note(mut self, note: impl Into<String>) -> Self {
        self.notes.push(note.into());
        self
    }

    /// The span the diagnostic is primarily about, if it has one.
    pub fn primary_span(&self) -> Option<Span> {
        self.labels
            .iter()
            .find(|label| label.primary)
            .or_else(|| self.labels.first())
            .map(|label| label.span)
    }
}

/// Collects diagnostics produced across compiler phases.
///
/// The compiler never prints as it goes: phases push into a sink, the driver
/// decides what to do with the result. That keeps phases usable from the
/// language server, where diagnostics are data rather than terminal output.
#[derive(Default)]
pub struct DiagnosticSink {
    diagnostics: Vec<Diagnostic>,
}

impl DiagnosticSink {
    /// Creates an empty sink.
    pub fn new() -> Self {
        DiagnosticSink { diagnostics: Vec::new() }
    }

    /// Records a diagnostic.
    pub fn emit(&mut self, diagnostic: Diagnostic) {
        self.diagnostics.push(diagnostic);
    }

    /// Records every diagnostic from an iterator.
    pub fn extend(&mut self, diagnostics: impl IntoIterator<Item = Diagnostic>) {
        self.diagnostics.extend(diagnostics);
    }

    /// Whether any error or fatal diagnostic has been recorded.
    pub fn has_errors(&self) -> bool {
        self.diagnostics.iter().any(|d| d.severity.is_error())
    }

    /// The number of error and fatal diagnostics.
    pub fn error_count(&self) -> usize {
        self.diagnostics.iter().filter(|d| d.severity.is_error()).count()
    }

    /// The number of warnings.
    pub fn warning_count(&self) -> usize {
        self.diagnostics.iter().filter(|d| d.severity == Severity::Warning).count()
    }

    /// Every recorded diagnostic, in emission order.
    pub fn diagnostics(&self) -> &[Diagnostic] {
        &self.diagnostics
    }

    /// Whether nothing has been recorded.
    pub fn is_empty(&self) -> bool {
        self.diagnostics.is_empty()
    }

    /// Takes ownership of the recorded diagnostics, leaving the sink empty.
    pub fn take(&mut self) -> Vec<Diagnostic> {
        std::mem::take(&mut self.diagnostics)
    }

    /// Sorts diagnostics by source position so output is deterministic
    /// regardless of the order phases discovered problems in.
    pub fn sort(&mut self) {
        self.diagnostics.sort_by_key(|d| {
            let span = d.primary_span().unwrap_or(Span::dummy());
            (span.file.index(), span.start, span.end, d.code.0)
        });
    }

    /// Renders every diagnostic against `map`.
    pub fn render_all(&self, map: &SourceMap, style: RenderStyle) -> String {
        let mut out = String::new();
        for diagnostic in &self.diagnostics {
            out.push_str(&render(diagnostic, map, style));
            out.push('\n');
        }
        out
    }
}

/// Diagnostic code ranges, one block per compiler phase.
pub mod codes {
    use super::Code;

    // 0001-0099: driver, files and command line.
    /// A source file could not be read.
    pub const CANNOT_READ_FILE: Code = Code(1);
    /// The entry point `fn main` is missing.
    pub const NO_MAIN: Code = Code(2);
    /// The file does not use the `.noto` extension.
    pub const BAD_EXTENSION: Code = Code(3);
    /// Modules import each other in a cycle.
    pub const IMPORT_CYCLE: Code = Code(4);

    // 0100-0199: lexer.
    /// A character that cannot start any Noto token.
    pub const UNEXPECTED_CHARACTER: Code = Code(100);
    /// A string literal was not closed before end of file or line.
    pub const UNTERMINATED_STRING: Code = Code(101);
    /// A block comment was not closed before end of file.
    pub const UNTERMINATED_COMMENT: Code = Code(102);
    /// An escape sequence the language does not define.
    pub const INVALID_ESCAPE: Code = Code(103);
    /// A numeric literal the lexer could not make sense of.
    pub const INVALID_NUMBER: Code = Code(104);
    /// A character literal that does not hold exactly one character.
    pub const INVALID_CHAR_LITERAL: Code = Code(105);
    /// A numeric literal too large for its type.
    pub const NUMBER_OUT_OF_RANGE: Code = Code(106);
    /// An interpolation `${ ... }` that was never closed.
    pub const UNTERMINATED_INTERPOLATION: Code = Code(107);

    // 0200-0299: parser.
    /// The parser found a token it did not expect.
    pub const UNEXPECTED_TOKEN: Code = Code(200);
    /// A construct that is valid syntax but not allowed in this position.
    pub const UNEXPECTED_CONSTRUCT: Code = Code(201);
    /// A modifier that cannot apply to the following declaration.
    pub const INVALID_MODIFIER: Code = Code(202);
    /// A `when` arm list that ends with something other than `else`.
    pub const MALFORMED_WHEN: Code = Code(203);

    // 0300-0399: name resolution.
    /// A name that is not in scope.
    pub const UNKNOWN_NAME: Code = Code(300);
    /// A name declared twice in the same scope.
    pub const DUPLICATE_NAME: Code = Code(301);
    /// A type name that is not in scope.
    pub const UNKNOWN_TYPE: Code = Code(302);
    /// Assignment to something that is not assignable.
    pub const NOT_ASSIGNABLE: Code = Code(303);
    /// Reassignment of a `val` binding.
    pub const REASSIGNED_VAL: Code = Code(304);
    /// A `break` or `continue` outside any loop.
    pub const OUTSIDE_LOOP: Code = Code(305);
    /// A `return` outside any function.
    pub const OUTSIDE_FUNCTION: Code = Code(306);
    /// A local read before it is initialised.
    pub const USED_BEFORE_INIT: Code = Code(307);

    // 0400-0499: type checking.
    /// Two types that were required to match did not.
    pub const TYPE_MISMATCH: Code = Code(400);
    /// A call with the wrong number of arguments.
    pub const ARITY_MISMATCH: Code = Code(401);
    /// A call of something that is not callable.
    pub const NOT_CALLABLE: Code = Code(402);
    /// An operator applied to operand types it is not defined for.
    pub const INVALID_OPERANDS: Code = Code(403);
    /// A member that the receiver type does not have.
    pub const UNKNOWN_MEMBER: Code = Code(404);
    /// A type annotation the compiler could not infer and the user omitted.
    pub const CANNOT_INFER: Code = Code(405);
    /// A nullable value used where a non-null one is required.
    pub const NULLABLE_NOT_ALLOWED: Code = Code(406);
    /// A function that can fall off its end without returning a value.
    pub const MISSING_RETURN: Code = Code(407);
    /// A `when` over a sealed type that does not cover every case.
    pub const NON_EXHAUSTIVE_WHEN: Code = Code(408);
    /// A conversion the language does not perform implicitly.
    pub const NO_IMPLICIT_CONVERSION: Code = Code(409);
    /// A condition that is not `Bool`.
    pub const NON_BOOL_CONDITION: Code = Code(410);
    /// A type is missing a member the interface it lists requires.
    pub const MISSING_INTERFACE_MEMBER: Code = Code(411);
    /// A member meant to implement an interface has the wrong signature.
    pub const INTERFACE_SIGNATURE_MISMATCH: Code = Code(412);
    // 413 is reserved for an unsatisfied bound at a call, which lands with
    // bounds themselves; see RFC 0003.
    /// An interface named where a value type is expected.
    pub const INTERFACE_NOT_A_VALUE: Code = Code(414);
    /// An `interface` body declares storage an interface cannot have.
    pub const INTERFACE_HAS_STORAGE: Code = Code(415);
    /// `Self` written outside an interface or a member implementing one.
    pub const SELF_OUTSIDE_INTERFACE: Code = Code(416);

    // 0500-0599: lowering to Noto IR and code generation.
    /// A construct the current backend cannot lower yet.
    pub const UNSUPPORTED_CONSTRUCT: Code = Code(500);
    /// The target triple is not supported by any registered backend.
    pub const UNSUPPORTED_TARGET: Code = Code(501);
    /// Writing the executable failed.
    pub const CANNOT_WRITE_OUTPUT: Code = Code(502);

    // 0600-0699: lints.
    /// A binding that is never read.
    pub const UNUSED_BINDING: Code = Code(600);
    /// A `var` that is never reassigned and could be a `val`.
    pub const VAR_NEVER_REASSIGNED: Code = Code(601);
    /// Code that can never be reached.
    pub const UNREACHABLE_CODE: Code = Code(602);
    /// An import that is never used.
    pub const UNUSED_IMPORT: Code = Code(603);
    /// A function that is never called, or a property that is never read.
    pub const UNUSED_FUNCTION: Code = Code(604);
    /// A constant that is never read.
    pub const UNUSED_CONST: Code = Code(605);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn code_formats_with_a_stable_prefix() {
        assert_eq!(Code(1).to_string(), "NOTO0001");
        assert_eq!(Code(407).to_string(), "NOTO0407");
    }

    #[test]
    fn sink_counts_by_severity() {
        let mut sink = DiagnosticSink::new();
        assert!(!sink.has_errors());
        sink.emit(Diagnostic::warning(codes::UNUSED_BINDING, "unused"));
        assert!(!sink.has_errors());
        assert_eq!(sink.warning_count(), 1);
        sink.emit(Diagnostic::error(codes::TYPE_MISMATCH, "boom"));
        assert!(sink.has_errors());
        assert_eq!(sink.error_count(), 1);
    }

    #[test]
    fn primary_span_prefers_a_primary_label() {
        let file = noto_span::FileId::from_index(0);
        let d = Diagnostic::error(codes::TYPE_MISMATCH, "mismatch")
            .with_secondary(Span::new(file, 0, 1), "declared here")
            .with_primary(Span::new(file, 10, 12), "found here");
        assert_eq!(d.primary_span(), Some(Span::new(file, 10, 12)));
    }

    #[test]
    fn sort_orders_by_position() {
        let file = noto_span::FileId::from_index(0);
        let mut sink = DiagnosticSink::new();
        sink.emit(Diagnostic::error(codes::TYPE_MISMATCH, "b").with_primary(Span::new(file, 40, 41), ""));
        sink.emit(Diagnostic::error(codes::TYPE_MISMATCH, "a").with_primary(Span::new(file, 4, 5), ""));
        sink.sort();
        assert_eq!(sink.diagnostics()[0].message, "a");
        assert_eq!(sink.diagnostics()[1].message, "b");
    }
}
