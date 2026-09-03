//! The Noto command line interface.
//!
//! Every command here is a thin shell over `noto-driver`: the CLI parses
//! arguments, asks the driver how far compilation should go, prints the
//! diagnostics it produced and, for `run` and `build`, writes the executable.

use noto_codegen::EXECUTABLE_MODE;
use noto_diagnostics::RenderStyle;
use noto_driver::{compile_path, read_source, summary, CompileOptions, Stage};
use noto_test_runner::TestOptions;
use noto_span::SourceMap;
use std::io::IsTerminal;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match Command::parse(&args) {
        Ok(Command::Version) => {
            println!("noto {}", env!("CARGO_PKG_VERSION"));
            ExitCode::SUCCESS
        }
        Ok(Command::Help) => {
            print_help();
            ExitCode::SUCCESS
        }
        Ok(command) => run(command),
        Err(error) => {
            eprintln!("error: {error}");
            eprintln!("usage: noto <command> [options] <file.noto> — try `noto help`");
            ExitCode::from(2)
        }
    }
}

enum Command {
    Run {
        input: PathBuf,
    },
    Build {
        input: PathBuf,
        output: Option<PathBuf>,
        emit: Emit,
    },
    Check {
        input: PathBuf,
    },
    Test {
        input: PathBuf,
        filter: Option<String>,
    },
    Lint {
        input: PathBuf,
        deny_warnings: bool,
    },
    Fmt {
        input: PathBuf,
        mode: FmtMode,
    },
    Version,
    Help,
}

/// What `noto fmt` does with the formatted text.
#[derive(Clone, Copy, PartialEq)]
enum FmtMode {
    /// Rewrite the file in place.
    Write,
    /// Report whether the file is already formatted and change nothing.
    Check,
    /// Print the formatted text and change nothing.
    Stdout,
}

/// What to do with the compilation product besides diagnostics.
enum Emit {
    /// The command's default behaviour.
    Default,
    /// `--emit=ir`: stop at Noto IR and print its textual form.
    Ir,
}

impl Command {
    fn parse(args: &[String]) -> Result<Self, String> {
        let Some(command) = args.first() else {
            return Ok(Command::Help);
        };
        let rest = &args[1..];
        match command.as_str() {
            "version" | "--version" | "-V" if rest.is_empty() => Ok(Command::Version),
            "help" | "--help" | "-h" => Ok(Command::Help),
            "run" => Ok(Command::Run {
                input: input_of(rest)?,
            }),
            "check" => Ok(Command::Check {
                input: input_of(rest)?,
            }),
            "fmt" => {
                let mut mode = FmtMode::Write;
                let mut input = None;
                for arg in rest {
                    match arg.as_str() {
                        "--check" => mode = FmtMode::Check,
                        "--stdout" => mode = FmtMode::Stdout,
                        other => input = Some(PathBuf::from(other)),
                    }
                }
                Ok(Command::Fmt {
                    input: input.ok_or("`fmt` needs an input file")?,
                    mode,
                })
            }
            "lint" => {
                let mut deny_warnings = false;
                let mut input = None;
                for arg in rest {
                    match arg.as_str() {
                        "-D" | "--deny-warnings" => deny_warnings = true,
                        other => input = Some(PathBuf::from(other)),
                    }
                }
                Ok(Command::Lint {
                    input: input.ok_or("`lint` needs an input file")?,
                    deny_warnings,
                })
            }
            "test" => {
                let mut filter = None;
                let mut input = None;
                let mut rest = rest.iter();
                while let Some(arg) = rest.next() {
                    match arg.as_str() {
                        "--filter" => {
                            let value = rest
                                .next()
                                .ok_or_else(|| "`--filter` needs a value".to_string())?;
                            filter = Some(value.clone());
                        }
                        other => input = Some(PathBuf::from(other)),
                    }
                }
                Ok(Command::Test {
                    input: input.ok_or("`test` needs an input file")?,
                    filter,
                })
            }
            "build" => {
                let mut output = None;
                let mut emit = Emit::Default;
                let mut input = None;
                let mut rest = rest.iter();
                while let Some(arg) = rest.next() {
                    match arg.as_str() {
                        "-o" | "--output" => {
                            let value = rest
                                .next()
                                .ok_or_else(|| "`--output` needs a value".to_string())?;
                            output = Some(PathBuf::from(value));
                        }
                        "--emit=ir" => emit = Emit::Ir,
                        other => input = Some(PathBuf::from(other)),
                    }
                }
                Ok(Command::Build {
                    input: input.ok_or("`build` needs an input file")?,
                    output,
                    emit,
                })
            }
            other => Err(format!("unknown command `{other}`")),
        }
    }
}

/// Reads the one positional argument shared by `run` and `check`.
fn input_of(args: &[String]) -> Result<PathBuf, String> {
    match args {
        [single] => Ok(PathBuf::from(single)),
        [] => Err("expected a `.noto` file".to_string()),
        _ => Err("expected exactly one input file".to_string()),
    }
}

fn run(command: Command) -> ExitCode {
    // Formatting is the one command that never compiles: it works on the
    // token stream, so it runs before the pipeline is involved at all.
    if let Command::Fmt { input, mode } = &command {
        return fmt(input, *mode);
    }

    let (input, stage) = match &command {
        Command::Version | Command::Help => return ExitCode::SUCCESS,
        Command::Run { input } => (input, Stage::Executable),
        Command::Build { input, emit, .. } => (input, emit_stage(emit)),
        Command::Check { input } => (input, Stage::Check),
        Command::Test { input, .. } => (input, Stage::Ir),
        Command::Lint { input, .. } => (input, Stage::Check),
        Command::Fmt { .. } => unreachable!("handled above"),
    };

    let mut map = SourceMap::new();
    let mut sink = noto_diagnostics::DiagnosticSink::new();

    let options = CompileOptions {
        stage,
        // A file of tests, or a file being linted, is a legitimate program
        // without a `main`.
        allow_no_main: matches!(command, Command::Test { .. } | Command::Lint { .. }),
        ..CompileOptions::default()
    };
    let result = compile_path(&mut map, input, &options, &mut sink);

    // The linter is a phase like any other: it pushes into the same sink, so
    // its warnings are rendered and counted with everything else.
    if matches!(command, Command::Lint { .. }) {
        if let (Some(module), Some(analysis)) = (result.module(), &result.analysis) {
            noto_linter::lint_module(module, result.root_imports(), analysis, &mut sink);
        }
    }

    report(&map, &sink);

    if sink.has_errors() {
        return ExitCode::FAILURE;
    }
    let warnings = sink.warning_count();

    match command {
        Command::Check { .. } => ExitCode::SUCCESS,
        Command::Build { emit: Emit::Ir, .. } => {
            if let Some(program) = result.ir {
                print!("{program}");
            }
            ExitCode::SUCCESS
        }
        Command::Build {
            input,
            output,
            emit: Emit::Default,
        } => {
            let Some(executable) = result.executable else {
                return ExitCode::FAILURE;
            };
            let output = output.unwrap_or_else(|| default_output(&input));
            if let Err(error) = write_executable(&output, &executable) {
                eprintln!("error: cannot write `{}`: {}", output.display(), error);
                return ExitCode::FAILURE;
            }
            println!("wrote {}", output.display());
            ExitCode::SUCCESS
        }
        Command::Run { input } => {
            let Some(executable) = result.executable else {
                return ExitCode::FAILURE;
            };
            let temporary = std::env::temp_dir().join(format!(
                "noto-run-{}-{}",
                input
                    .file_stem()
                    .and_then(|stem| stem.to_str())
                    .unwrap_or("program"),
                std::process::id(),
            ));
            if let Err(error) = write_executable(&temporary, &executable) {
                eprintln!("error: cannot write `{}`: {}", temporary.display(), error);
                return ExitCode::FAILURE;
            }
            let status = std::process::Command::new(&temporary).status();
            let _ = std::fs::remove_file(&temporary);
            match status {
                Ok(status) if status.success() => ExitCode::SUCCESS,
                Ok(status) => ExitCode::from(status.code().unwrap_or(1) as u8),
                Err(error) => {
                    eprintln!("error: cannot run `{}`: {}", temporary.display(), error);
                    ExitCode::FAILURE
                }
            }
        }
        Command::Test { filter, .. } => {
            let Some(mut program) = result.ir else {
                return ExitCode::FAILURE;
            };
            let options = TestOptions {
                filter,
                // `noto test app.noto` runs app's tests; an imported
                // module's are run by pointing at that module.
                file: result.root_file,
                ..TestOptions::default()
            };
            let report = noto_test_runner::run(&mut program, &options);
            print!("{}", report.render(style()));
            if report.is_success() {
                ExitCode::SUCCESS
            } else {
                ExitCode::FAILURE
            }
        }
        Command::Lint { deny_warnings, .. } => {
            if deny_warnings && warnings > 0 {
                ExitCode::FAILURE
            } else {
                ExitCode::SUCCESS
            }
        }
        Command::Fmt { .. } => unreachable!("handled above"),
        Command::Version | Command::Help => ExitCode::SUCCESS,
    }
}

/// Runs `noto fmt`.
fn fmt(input: &Path, mode: FmtMode) -> ExitCode {
    let mut map = SourceMap::new();
    let mut sink = noto_diagnostics::DiagnosticSink::new();
    let Some(file) = read_source(&mut map, input, &mut sink) else {
        report(&map, &sink);
        return ExitCode::FAILURE;
    };

    let source = map.file(file).expect("just read");
    let Some(formatted) = noto_formatter::format(source, &mut sink) else {
        report(&map, &sink);
        return ExitCode::FAILURE;
    };
    report(&map, &sink);

    let unchanged = formatted == source.text();
    match mode {
        FmtMode::Stdout => {
            print!("{formatted}");
            ExitCode::SUCCESS
        }
        FmtMode::Check => {
            if unchanged {
                ExitCode::SUCCESS
            } else {
                println!("{} is not formatted", input.display());
                ExitCode::FAILURE
            }
        }
        FmtMode::Write => {
            if unchanged {
                return ExitCode::SUCCESS;
            }
            if let Err(error) = std::fs::write(input, &formatted) {
                eprintln!("error: cannot write `{}`: {}", input.display(), error);
                return ExitCode::FAILURE;
            }
            println!("formatted {}", input.display());
            ExitCode::SUCCESS
        }
    }
}

/// `--emit=ir` stops the pipeline before the backend.
fn emit_stage(emit: &Emit) -> Stage {
    match emit {
        Emit::Ir => Stage::Ir,
        Emit::Default => Stage::Executable,
    }
}

/// `hello.noto` becomes `hello` in the same directory.
fn default_output(input: &Path) -> PathBuf {
    input.with_extension("")
}

fn write_executable(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    std::fs::write(path, bytes)?;
    let mut permissions = std::fs::metadata(path)?.permissions();
    permissions.set_mode(EXECUTABLE_MODE);
    std::fs::set_permissions(path, permissions)
}

/// Prints diagnostics and, when there is anything to count, the one-line
/// summary, the way rustc does.
fn report(map: &SourceMap, sink: &noto_diagnostics::DiagnosticSink) {
    print!("{}", sink.render_all(map, style()));
    let summary = summary(sink);
    if !summary.is_empty() {
        println!("{summary}");
    }
}

/// Colour only when a terminal is there to read it.
fn style() -> RenderStyle {
    if std::io::stdout().is_terminal() {
        RenderStyle::Ansi
    } else {
        RenderStyle::Plain
    }
}

fn print_help() {
    println!(
        "noto {} — the Noto compiler driver

usage: noto <command> [options] <file.noto>

commands:
  run <file.noto>          compile to a temporary executable and run it
  build <file.noto>        write the executable next to the source
    -o, --output <path>    where to write it instead
    --emit=ir              print the textual Noto IR instead
  check <file.noto>        parse and analyse, report diagnostics only
  test <file.noto>         compile and run every `test` declaration
    --filter <text>        only run tests whose name contains <text>
  lint <file.noto>         report what is legal but probably not meant
    -D, --deny-warnings    exit non-zero when any lint fires
  fmt <file.noto>          format the file in place
    --check                exit non-zero if it is not already formatted
    --stdout               print the formatted text instead of writing it
  version                  print the version
  help                     print this message",
        env!("CARGO_PKG_VERSION")
    );
}
