//! The Noto test runner.
//!
//! A `test "name" { ... }` declaration is checked and lowered like any other
//! function, under the mangled name `test$name`. What remains is deciding how
//! to execute them, and this crate answers that with one process per test:
//! the program is compiled once, then the backend is asked for one executable
//! per test with that test as the entry point.
//!
//! One process per test is what makes the result trustworthy. A failing
//! `assert` terminates the process — there is no unwinding in Noto and no
//! runtime to catch it — so a single binary running every test in turn would
//! stop at the first failure and hide the rest. Separate processes also mean
//! a test that corrupts memory or loops cannot take the others with it.
//!
//! ```text
//! source → compile once → Noto IR → for each test: set entry, emit, execute
//! ```
//!
//! The exit status is the whole protocol: `0` passed, [`ASSERT_FAILURE_STATUS`]
//! means an assertion failed, anything else is a test that ended some other
//! way and is reported with its status.

#![deny(missing_docs)]

mod report;

pub use report::{Outcome, Report, TestResult};

use noto_codegen::{Target, EXECUTABLE_MODE};
use noto_ir::{FuncId, Program};
use noto_span::FileId;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

/// The prefix `compiler/semantic` gives a lowered test body.
///
/// It keeps tests out of the ordinary namespace; stripping it recovers the
/// description the user wrote.
pub const TEST_PREFIX: &str = "test$";

/// Settings for one test run.
#[derive(Clone, Debug)]
pub struct TestOptions {
    /// Only run tests whose name contains this text.
    pub filter: Option<String>,
    /// Only run tests declared in this file.
    ///
    /// A program is many modules, and `noto test app.noto` means app's
    /// tests: an imported module's are run by pointing at that module. A
    /// test's name is only its description, so a failure from a module the
    /// user did not name would be a line with nothing to attribute it to.
    pub file: Option<FileId>,
    /// What to generate the per-test executables for.
    pub target: Target,
    /// Where the per-test executables are written.
    pub directory: PathBuf,
}

impl Default for TestOptions {
    fn default() -> Self {
        TestOptions {
            filter: None,
            file: None,
            target: Target::host(),
            directory: std::env::temp_dir(),
        }
    }
}

/// A test found in a lowered program.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Test {
    /// The description written after `test`, without the mangled prefix.
    pub name: String,
    /// The function to enter.
    pub function: FuncId,
}

/// Finds every test in a lowered program, in declaration order.
pub fn discover(program: &Program) -> Vec<Test> {
    program
        .functions
        .iter()
        .filter_map(|function| {
            let name = function.name.strip_prefix(TEST_PREFIX)?;
            Some(Test { name: name.to_string(), function: function.id })
        })
        .collect()
}

/// Whether a test is selected by an optional filter.
///
/// A missing filter selects everything; otherwise the filter is matched as a
/// substring, which is what makes `noto test --filter parse` useful without
/// asking anyone to remember a whole description.
pub fn selects(filter: Option<&str>, name: &str) -> bool {
    match filter {
        None => true,
        Some(filter) => name.contains(filter),
    }
}

/// Compiles and runs every test in `program`.
///
/// `program` is borrowed mutably because each test is emitted with its own
/// entry point; the original entry is restored before returning, so the
/// program is left exactly as it was handed over.
pub fn run(program: &mut Program, options: &TestOptions) -> Report {
    let tests: Vec<Test> = discover(program)
        .into_iter()
        .filter(|test| selects(options.filter.as_deref(), &test.name))
        .filter(|test| match options.file {
            Some(file) => program.function(test.function).span.file == file,
            None => true,
        })
        .collect();

    // Every run gets its own directory. Two runs in one process would
    // otherwise write the same path, and writing over a file that is already
    // executing fails with `Text file busy`.
    let directory = match scratch_directory(&options.directory) {
        Ok(directory) => directory,
        Err(error) => {
            return Report {
                results: tests
                    .into_iter()
                    .map(|test| TestResult {
                        name: test.name,
                        outcome: Outcome::NotBuilt(error.clone()),
                    })
                    .collect(),
            }
        }
    };

    let original_entry = program.entry;
    let mut results = Vec::with_capacity(tests.len());

    for (index, test) in tests.iter().enumerate() {
        program.entry = Some(test.function);
        let path = directory.join(index.to_string());
        let outcome = build_and_run(program, options.target, &path);
        let _ = std::fs::remove_file(&path);
        results.push(TestResult { name: test.name.clone(), outcome });
    }

    program.entry = original_entry;
    let _ = std::fs::remove_dir(&directory);
    Report { results }
}

/// Creates a directory this run alone writes into.
///
/// The process id is not enough on its own: one process can run tests more
/// than once, and two runs at once — which is what a parallel test harness
/// does — must not share a directory. A counter separates them; a clock
/// reading would not, because two runs can start inside one tick of it.
fn scratch_directory(base: &Path) -> Result<PathBuf, String> {
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(0);

    let run = NEXT.fetch_add(1, Ordering::Relaxed);
    let directory = base.join(format!("noto-test-{}-{run}", std::process::id()));
    std::fs::create_dir_all(&directory)
        .map(|()| directory.clone())
        .map_err(|error| format!("cannot create `{}`: {error}", directory.display()))
}

/// Emits one test as an executable, runs it and reads its exit status.
fn build_and_run(program: &Program, target: Target, path: &Path) -> Outcome {
    let executable = match noto_codegen::compile(program, target) {
        Ok(executable) => executable,
        Err(error) => return Outcome::NotBuilt(error.to_string()),
    };

    if let Err(error) = write_executable(path, &executable) {
        return Outcome::NotBuilt(format!("cannot write `{}`: {error}", path.display()));
    }

    match std::process::Command::new(path).status() {
        Ok(status) if status.success() => Outcome::Passed,
        Ok(status) => match status.code() {
            Some(code) if code == noto_runtime::ASSERT_FAILURE_STATUS => Outcome::Failed,
            code => Outcome::Errored(code),
        },
        Err(error) => Outcome::NotBuilt(format!("cannot run `{}`: {error}", path.display())),
    }
}

fn write_executable(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    std::fs::write(path, bytes)?;
    let mut permissions = std::fs::metadata(path)?.permissions();
    permissions.set_mode(EXECUTABLE_MODE);
    std::fs::set_permissions(path, permissions)
}

#[cfg(test)]
mod tests;
