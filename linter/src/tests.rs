use super::*;
use noto_diagnostics::{codes, Code, DiagnosticSink};
use noto_driver::{compile_source, CompileOptions, Stage};

/// Lints a source file and returns the codes it reported, in the order they
/// were emitted.
fn codes_of(source: &str) -> Vec<Code> {
    lints(source).iter().map(|diagnostic| diagnostic.code).collect()
}

fn lints(source: &str) -> Vec<Diagnostic> {
    let mut sink = DiagnosticSink::new();
    let options = CompileOptions {
        stage: Stage::Check,
        allow_no_main: true,
        ..CompileOptions::default()
    };
    let (_, compilation) = compile_source("lint.noto", source, &options, &mut sink);
    assert!(!sink.has_errors(), "the source must compile: {:?}", sink.diagnostics());

    let module = compilation.module.expect("parsed");
    let analysis = compilation.analysis.expect("analysed");
    let mut sink = DiagnosticSink::new();
    lint(&module, &analysis, &mut sink);
    assert!(!sink.has_errors(), "every lint is a warning");
    sink.diagnostics().to_vec()
}

#[test]
fn a_binding_that_is_never_read_is_reported() {
    assert_eq!(
        codes_of("fn main() {\n    val unused = 1\n}\n"),
        [codes::UNUSED_BINDING]
    );
}

#[test]
fn a_binding_that_is_read_is_not_reported() {
    assert!(codes_of("fn main() {\n    val n = 1\n    println(n)\n}\n").is_empty());
}

#[test]
fn a_leading_underscore_opts_out() {
    assert!(codes_of("fn main() {\n    val _unused = 1\n}\n").is_empty());
    assert!(codes_of("fn helper(_ignored: Int): Int = 1\nfn main() { println(helper(2)) }\n")
        .is_empty());
}

#[test]
fn an_unused_parameter_is_reported_as_a_parameter() {
    let lints = lints("fn helper(extra: Int): Int = 1\nfn main() { println(helper(2)) }\n");
    assert_eq!(lints.len(), 1);
    assert_eq!(lints[0].code, codes::UNUSED_BINDING);
    assert!(lints[0].message.contains("parameter `extra`"), "{}", lints[0].message);
}

#[test]
fn a_var_that_is_never_reassigned_is_reported() {
    let lints = lints("fn main() {\n    var n = 1\n    println(n)\n}\n");
    assert_eq!(lints.len(), 1);
    assert_eq!(lints[0].code, codes::VAR_NEVER_REASSIGNED);
    assert!(lints[0].helps.iter().any(|help| help.contains("`val`")));
}

#[test]
fn a_var_that_is_reassigned_is_not_reported() {
    assert!(codes_of("fn main() {\n    var n = 1\n    n = 2\n    println(n)\n}\n").is_empty());
}

#[test]
fn a_compound_assignment_counts_as_both_a_read_and_a_write() {
    assert!(codes_of("fn main() {\n    var total = 0\n    total += 1\n    println(total)\n}\n")
        .is_empty());
}

#[test]
fn a_write_alone_does_not_count_as_a_use() {
    assert_eq!(
        codes_of("fn main() {\n    var n = 1\n    n = 2\n}\n"),
        [codes::UNUSED_BINDING],
        "assigning to a binding nothing reads does not make it used"
    );
}

#[test]
fn a_val_is_never_reported_as_never_reassigned() {
    assert!(codes_of("fn main() {\n    val n = 1\n    println(n)\n}\n").is_empty());
}

#[test]
fn unreachable_code_is_left_to_the_type_checker() {
    // Semantic analysis already reports NOTO0602 from the `Nothing` type, and
    // catches cases a syntactic lint would miss. The linter must not repeat it.
    let mut sink = DiagnosticSink::new();
    let options = CompileOptions {
        stage: Stage::Check,
        allow_no_main: true,
        ..CompileOptions::default()
    };
    let (_, compilation) = compile_source(
        "lint.noto",
        "fn main() {\n    return\n    println(\"never\")\n}\n",
        &options,
        &mut sink,
    );
    assert!(
        sink.diagnostics().iter().any(|d| d.code == codes::UNREACHABLE_CODE),
        "the checker reports it"
    );

    let mut lints = DiagnosticSink::new();
    lint(
        &compilation.module.expect("parsed"),
        &compilation.analysis.expect("analysed"),
        &mut lints,
    );
    assert!(
        !lints.diagnostics().iter().any(|d| d.code == codes::UNREACHABLE_CODE),
        "and the linter does not report it again"
    );
}

#[test]
fn a_function_that_is_never_called_is_reported() {
    let lints = lints("fn helper(): Int = 1\nfn main() {\n    println(2)\n}\n");
    assert_eq!(lints.len(), 1);
    assert_eq!(lints[0].code, codes::UNUSED_FUNCTION);
    assert!(lints[0].message.contains("`helper`"), "{}", lints[0].message);
}

#[test]
fn a_called_function_is_not_reported() {
    assert!(codes_of("fn helper(): Int = 1\nfn main() {\n    println(helper())\n}\n")
        .is_empty());
}

#[test]
fn main_is_never_dead_code() {
    assert!(codes_of("fn main() {\n    println(1)\n}\n").is_empty());
}

#[test]
fn a_test_body_is_never_dead_code() {
    assert!(codes_of("test \"t\" {\n    assert(true)\n}\n").is_empty());
}

#[test]
fn a_function_called_only_from_a_test_is_used() {
    assert!(codes_of(
        "fn double(n: Int): Int = n * 2\ntest \"t\" {\n    assert(double(1) == 2)\n}\n"
    )
    .is_empty());
}

#[test]
fn a_method_that_is_never_called_is_reported_as_a_method() {
    let lints = lints(
        "class Rect(val side: Int) {\n    fn area(): Int = this.side\n}\n         fn main() {\n    println(Rect(1).side)\n}\n",
    );
    assert_eq!(lints.len(), 1);
    assert_eq!(lints[0].code, codes::UNUSED_FUNCTION);
    assert!(lints[0].message.contains("method `Rect.area`"), "{}", lints[0].message);
    assert!(
        lints[0].helps.iter().any(|help| help.contains("`_area`")),
        "the rename is about the name the author wrote: {:?}",
        lints[0].helps
    );
}

#[test]
fn a_method_that_is_called_is_not_reported() {
    assert!(codes_of(
        "class Rect(val side: Int) {\n    fn area(): Int = this.side\n}\n         fn main() {\n    println(Rect(1).area())\n}\n"
    )
    .is_empty());
}

#[test]
fn an_underscore_opts_a_method_out() {
    assert!(codes_of(
        "class Rect(val side: Int) {\n    fn _area(): Int = this.side\n}\n         fn main() {\n    println(Rect(1).side)\n}\n"
    )
    .is_empty());
}

#[test]
fn the_receiver_is_never_reported_as_unused() {
    // Nobody wrote `this`, so there is nobody to tell that it is unused.
    assert!(codes_of(
        "class Rect(val side: Int) {\n    fn constant(): Int = 1\n}\n         fn main() {\n    println(Rect(1).constant())\n}\n"
    )
    .is_empty());
}

#[test]
fn a_constant_that_is_never_read_is_reported() {
    let lints = lints("const LIMIT: Int = 10\nfn main() {\n    println(1)\n}\n");
    assert_eq!(lints.len(), 1);
    assert_eq!(lints[0].code, codes::UNUSED_CONST);
}

#[test]
fn a_constant_that_is_read_is_not_reported() {
    assert!(codes_of("const LIMIT: Int = 10\nfn main() {\n    println(LIMIT)\n}\n").is_empty());
}

#[test]
fn a_test_body_is_linted_like_any_other_function() {
    assert_eq!(
        codes_of("test \"t\" {\n    val unused = 1\n    assert(true)\n}\n"),
        [codes::UNUSED_BINDING]
    );
}
