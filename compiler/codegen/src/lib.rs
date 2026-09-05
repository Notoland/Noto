//! Native code generation for Noto.
//!
//! This crate turns Noto IR into a runnable executable. It owns everything
//! target-specific: instruction encoding, the calling convention, the stack
//! frame layout and the object file format.
//!
//! Targets are selected through [`Target`], and each backend lives in its own
//! module behind the same entry point. Only Linux x86-64 is implemented today;
//! the shape is what lets a second one be added without touching the phases
//! above.

#![deny(missing_docs)]

pub mod elf;
pub mod x86_64;

mod target;

pub use target::{Architecture, OperatingSystem, Target};

use noto_diagnostics::{codes, Diagnostic};
use noto_ir::Program;
use noto_span::Span;

/// Why code generation could not finish.
#[derive(Clone, Debug)]
pub enum CodegenError {
    /// No backend is registered for the requested target.
    UnsupportedTarget(Target),
    /// A function declares more parameters than the ABI passes in registers.
    TooManyParameters {
        /// The function's name.
        function: String,
        /// How many parameters the ABI can pass.
        limit: usize,
        /// Where the function was declared.
        span: Span,
    },
    /// A call passes more arguments than the ABI passes in registers.
    TooManyArguments {
        /// The name of the function containing the call.
        function: String,
        /// How many arguments the ABI can pass.
        limit: usize,
        /// Where the containing function was declared.
        span: Span,
    },
    /// The backend reached a state that should be impossible.
    Internal(String),
}

impl CodegenError {
    /// Renders the error as a compiler diagnostic.
    pub fn to_diagnostic(&self) -> Diagnostic {
        match self {
            CodegenError::UnsupportedTarget(target) => Diagnostic::error(
                codes::UNSUPPORTED_TARGET,
                format!("no backend for the target `{target}`"),
            )
            .with_note(format!(
                "this compiler can generate code for: {}",
                Target::supported()
                    .iter()
                    .map(Target::to_string)
                    .collect::<Vec<_>>()
                    .join(", ")
            )),
            CodegenError::TooManyParameters { function, limit, span } => {
                let mut diagnostic = Diagnostic::error(
                    codes::UNSUPPORTED_CONSTRUCT,
                    format!("`{function}` takes more than {limit} parameters"),
                )
                .with_primary(*span, "too many parameters")
                .with_note("passing arguments on the stack is not implemented in Noto 0.9")
                .with_help("group the extra parameters into a single value");
                // A method's receiver is passed like any other argument, so it
                // spends one of the registers the author did not write.
                if function.contains('.') {
                    diagnostic = diagnostic
                        .with_note("a method's receiver counts as one of them");
                }
                diagnostic
            }
            CodegenError::TooManyArguments { function, limit, span } => Diagnostic::error(
                codes::UNSUPPORTED_CONSTRUCT,
                format!("a call in `{function}` passes more than {limit} arguments"),
            )
            .with_primary(*span, "too many arguments")
            .with_note("passing arguments on the stack is not implemented in Noto 0.9"),
            CodegenError::Internal(message) => Diagnostic::fatal(
                codes::UNSUPPORTED_CONSTRUCT,
                format!("internal compiler error: {message}"),
            )
            .with_note("this is a bug in the Noto compiler; please report it"),
        }
    }
}

impl std::fmt::Display for CodegenError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.to_diagnostic().message)
    }
}

impl std::error::Error for CodegenError {}

/// Compiles a program into a runnable executable image for `target`.
pub fn compile(program: &Program, target: Target) -> Result<Vec<u8>, CodegenError> {
    if !target.is_supported() {
        return Err(CodegenError::UnsupportedTarget(target));
    }
    match target.architecture {
        Architecture::X86_64 => x86_64::compile(program, target),
    }
}

/// The Unix permission bits an executable is written with.
pub const EXECUTABLE_MODE: u32 = 0o755;
