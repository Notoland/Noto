//! The compilation driver.
//!
//! The driver owns the pipeline: it reads source, runs each phase in order,
//! decides when to stop, and turns the result into files on disk. Every phase
//! it calls is a library that knows nothing about the file system, which is
//! what lets the same phases serve `noto build`, the language server and the
//! test runner.
//!
//! ```text
//! source → lexer → parser → AST → semantic analysis → Noto IR
//!        → optimizer → backend → machine code → executable
//! ```
//!
//! Compilation stops at the first phase that reported an error. Diagnostics
//! from that phase are all reported, so one run shows every problem at that
//! level rather than one at a time.

#![deny(missing_docs)]

mod pipeline;

pub use pipeline::{compile, compile_source, CompileOptions, Compilation, Stage};

use noto_diagnostics::{codes, Diagnostic, DiagnosticSink, RenderStyle};
use noto_span::SourceMap;
use std::path::Path;

/// The file extension Noto source uses.
pub const SOURCE_EXTENSION: &str = "noto";

/// Reads a source file into a [`SourceMap`].
///
/// Reports a diagnostic and returns `None` when the file cannot be read, so
/// the caller handles a missing file the same way as a syntax error.
pub fn read_source(
    map: &mut SourceMap,
    path: &Path,
    sink: &mut DiagnosticSink,
) -> Option<noto_span::FileId> {
    if path.extension().and_then(|extension| extension.to_str()) != Some(SOURCE_EXTENSION) {
        sink.emit(
            Diagnostic::warning(
                codes::BAD_EXTENSION,
                format!("`{}` does not use the `.{SOURCE_EXTENSION}` extension", path.display()),
            )
            .with_note("Noto source files are named `something.noto`"),
        );
    }

    match std::fs::read_to_string(path) {
        Ok(text) => Some(map.add(path.display().to_string(), text)),
        Err(error) => {
            sink.emit(
                Diagnostic::fatal(
                    codes::CANNOT_READ_FILE,
                    format!("cannot read `{}`", path.display()),
                )
                .with_note(error.to_string()),
            );
            None
        }
    }
}

/// Renders every diagnostic in `sink` against `map`.
pub fn render_diagnostics(
    sink: &DiagnosticSink,
    map: &SourceMap,
    style: RenderStyle,
) -> String {
    sink.render_all(map, style)
}

/// A one-line summary of how a compilation went.
pub fn summary(sink: &DiagnosticSink) -> String {
    let errors = sink.error_count();
    let warnings = sink.warning_count();
    match (errors, warnings) {
        (0, 0) => String::new(),
        (0, warnings) => format!("{warnings} warning{}", plural(warnings)),
        (errors, 0) => format!("{errors} error{}", plural(errors)),
        (errors, warnings) => format!(
            "{errors} error{}, {warnings} warning{}",
            plural(errors),
            plural(warnings)
        ),
    }
}

fn plural(count: usize) -> &'static str {
    if count == 1 {
        ""
    } else {
        "s"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn summarises_counts_with_correct_plurals() {
        let mut sink = DiagnosticSink::new();
        assert_eq!(summary(&sink), "");
        sink.emit(Diagnostic::error(codes::TYPE_MISMATCH, "x"));
        assert_eq!(summary(&sink), "1 error");
        sink.emit(Diagnostic::error(codes::TYPE_MISMATCH, "y"));
        assert_eq!(summary(&sink), "2 errors");
        sink.emit(Diagnostic::warning(codes::UNUSED_BINDING, "z"));
        assert_eq!(summary(&sink), "2 errors, 1 warning");
    }

    #[test]
    fn a_missing_file_is_reported_rather_than_panicking() {
        let mut map = SourceMap::new();
        let mut sink = DiagnosticSink::new();
        let result = read_source(&mut map, Path::new("/nonexistent/file.noto"), &mut sink);
        assert!(result.is_none());
        assert!(sink.has_errors());
        assert!(sink.diagnostics()[0].message.contains("cannot read"));
    }
}
