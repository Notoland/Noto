//! What a test run produced, and how it reads on a terminal.

use noto_diagnostics::RenderStyle;
use std::fmt::Write;

/// How one test ended.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// The process exited with status zero.
    Passed,
    /// An `assert` failed.
    Failed,
    /// The process ended some other way — a signal, or an explicit status.
    ///
    /// `None` means the process was killed by a signal and has no exit code.
    Errored(Option<i32>),
    /// The test never ran: it could not be compiled, written or spawned.
    NotBuilt(String),
}

impl Outcome {
    /// Whether this counts as a pass.
    pub fn is_pass(&self) -> bool {
        matches!(self, Outcome::Passed)
    }

    /// The word printed after the test's name.
    pub fn label(&self) -> String {
        match self {
            Outcome::Passed => "ok".to_string(),
            Outcome::Failed => "FAILED".to_string(),
            Outcome::Errored(Some(code)) => format!("FAILED (exit status {code})"),
            Outcome::Errored(None) => "FAILED (killed by a signal)".to_string(),
            Outcome::NotBuilt(reason) => format!("ERROR ({reason})"),
        }
    }
}

/// One test and how it ended.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TestResult {
    /// The description written after `test`.
    pub name: String,
    /// How it ended.
    pub outcome: Outcome,
}

/// Every test that ran, in declaration order.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Report {
    /// The results, in the order the tests were declared.
    pub results: Vec<TestResult>,
}

impl Report {
    /// How many tests passed.
    pub fn passed(&self) -> usize {
        self.results.iter().filter(|result| result.outcome.is_pass()).count()
    }

    /// How many tests did not pass, for any reason.
    pub fn failed(&self) -> usize {
        self.results.len() - self.passed()
    }

    /// Whether every test that ran passed.
    ///
    /// A file with no tests at all succeeds: there is nothing to report and
    /// nothing went wrong, which is what makes `noto test` usable in a script
    /// before any test has been written.
    pub fn is_success(&self) -> bool {
        self.failed() == 0
    }

    /// Renders the run the way `cargo test` does, one line per test.
    pub fn render(&self, style: RenderStyle) -> String {
        let palette = Palette::for_style(style);
        let mut out = String::new();

        let count = self.results.len();
        let _ = writeln!(out, "running {count} test{}", plural(count));
        for result in &self.results {
            let colour =
                if result.outcome.is_pass() { palette.pass } else { palette.fail };
            let _ = writeln!(
                out,
                "test {} ... {colour}{}{}",
                result.name,
                result.outcome.label(),
                palette.reset
            );
        }

        let (verdict, colour) = if self.is_success() {
            ("ok", palette.pass)
        } else {
            ("FAILED", palette.fail)
        };
        let _ = writeln!(
            out,
            "\ntest result: {colour}{verdict}{}. {} passed; {} failed",
            palette.reset,
            self.passed(),
            self.failed()
        );
        out
    }
}

struct Palette {
    pass: &'static str,
    fail: &'static str,
    reset: &'static str,
}

impl Palette {
    fn for_style(style: RenderStyle) -> Self {
        match style {
            RenderStyle::Plain => Palette { pass: "", fail: "", reset: "" },
            RenderStyle::Ansi => {
                Palette { pass: "\x1b[32m", fail: "\x1b[31m", reset: "\x1b[0m" }
            }
        }
    }
}

fn plural(count: usize) -> &'static str {
    if count == 1 {
        ""
    } else {
        "s"
    }
}
