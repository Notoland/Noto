use super::*;
use noto_diagnostics::{DiagnosticSink, RenderStyle};
use noto_driver::{compile_source, CompileOptions, Stage};

/// Lowers a source file the way `noto test` does: far enough for IR, with a
/// missing `main` allowed.
fn lower(source: &str) -> Program {
    let mut sink = DiagnosticSink::new();
    let options =
        CompileOptions { stage: Stage::Ir, allow_no_main: true, ..CompileOptions::default() };
    let (_, compilation) = compile_source("tests.noto", source, &options, &mut sink);
    assert!(!sink.has_errors(), "{:?}", sink.diagnostics());
    compilation.ir.expect("lowering produced a program")
}

fn report_of(source: &str) -> Report {
    let mut program = lower(source);
    run(&mut program, &TestOptions::default())
}

#[test]
fn discovery_strips_the_mangled_prefix_and_keeps_declaration_order() {
    let program = lower(
        "test \"adds numbers\" {\n    assert(1 + 1 == 2)\n}\n\
         test \"compares text\" {\n    assert(\"a\" != \"b\")\n}\n",
    );
    let names: Vec<String> = discover(&program).into_iter().map(|test| test.name).collect();
    assert_eq!(names, ["adds numbers", "compares text"]);
}

#[test]
fn an_ordinary_function_is_not_a_test() {
    let program = lower("fn helper(): Int = 1\ntest \"uses it\" {\n    assert(helper() == 1)\n}\n");
    assert_eq!(discover(&program).len(), 1);
}

#[test]
fn a_filter_matches_any_part_of_a_name() {
    assert!(selects(None, "anything"));
    assert!(selects(Some("adds"), "adds numbers"));
    assert!(selects(Some("numbers"), "adds numbers"));
    assert!(!selects(Some("parses"), "adds numbers"));
}

#[test]
fn a_report_counts_every_kind_of_failure_against_the_run() {
    let report = Report {
        results: vec![
            TestResult { name: "a".into(), outcome: Outcome::Passed },
            TestResult { name: "b".into(), outcome: Outcome::Failed },
            TestResult { name: "c".into(), outcome: Outcome::Errored(Some(3)) },
            TestResult { name: "d".into(), outcome: Outcome::NotBuilt("no backend".into()) },
        ],
    };
    assert_eq!(report.passed(), 1);
    assert_eq!(report.failed(), 3);
    assert!(!report.is_success());
}

#[test]
fn a_run_with_no_tests_succeeds() {
    let report = Report::default();
    assert!(report.is_success());
    assert!(report.render(RenderStyle::Plain).contains("running 0 tests"));
}

#[test]
fn the_plain_rendering_carries_no_escape_sequences() {
    let report = Report {
        results: vec![TestResult { name: "adds".into(), outcome: Outcome::Failed }],
    };
    let rendered = report.render(RenderStyle::Plain);
    assert!(!rendered.contains('\x1b'), "{rendered}");
    assert!(rendered.contains("test adds ... FAILED"));
    assert!(rendered.contains("test result: FAILED. 0 passed; 1 failed"));
}

// The remaining tests execute the emitted binaries, so they only run where the
// backend can produce something this machine will load.
#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
mod execution {
    use super::*;

    #[test]
    fn a_passing_test_and_a_failing_one_are_told_apart() {
        let report = report_of(
            "test \"holds\" {\n    assert(1 + 1 == 2)\n}\n\
             test \"does not hold\" {\n    assert(1 + 1 == 3)\n}\n",
        );
        assert_eq!(report.results[0].outcome, Outcome::Passed);
        assert_eq!(report.results[1].outcome, Outcome::Failed);
        assert_eq!(report.passed(), 1);
        assert!(!report.is_success());
    }

    #[test]
    fn a_failing_test_does_not_stop_the_ones_after_it() {
        let report = report_of(
            "test \"fails\" {\n    assert(false)\n}\n\
             test \"still runs\" {\n    assert(true)\n}\n",
        );
        assert_eq!(report.results.len(), 2);
        assert_eq!(report.results[1].outcome, Outcome::Passed);
    }

    #[test]
    fn a_filter_narrows_the_run() {
        let mut program = lower(
            "test \"parses a number\" {\n    assert(true)\n}\n\
             test \"lowers a call\" {\n    assert(true)\n}\n",
        );
        let options = TestOptions { filter: Some("parses".into()), ..TestOptions::default() };
        let report = run(&mut program, &options);
        assert_eq!(report.results.len(), 1);
        assert_eq!(report.results[0].name, "parses a number");
    }

    #[test]
    fn the_program_entry_point_is_left_as_it_was_found() {
        let mut program =
            lower("fn main() {\n    println(\"hi\")\n}\ntest \"t\" {\n    assert(true)\n}\n");
        let before = program.entry;
        run(&mut program, &TestOptions::default());
        assert_eq!(program.entry, before);
    }

    #[test]
    fn a_test_calling_a_function_runs_it() {
        let report = report_of(
            "fn double(n: Int): Int = n * 2\n\
             test \"doubles\" {\n    assert(double(21) == 42)\n}\n",
        );
        assert_eq!(report.results[0].outcome, Outcome::Passed);
    }
}
