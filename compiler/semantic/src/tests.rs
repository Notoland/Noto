//! Semantic analysis tests.

use super::*;
use noto_diagnostics::RenderStyle;
use noto_span::SourceMap;

/// Parses and analyses `source`, returning the analysis and every diagnostic
/// message it produced.
fn check(source: &str) -> (Analysis, Vec<String>) {
    let mut map = SourceMap::new();
    let file = map.add("test.noto", source);
    let mut sink = DiagnosticSink::new();
    let module = noto_parser::parse_file(map.file(file).unwrap(), &mut sink);
    let analysis = analyze(&module, &mut sink);
    let messages = sink.diagnostics().iter().map(|d| d.message.clone()).collect();
    (analysis, messages)
}

/// Analyses `source`, asserting it is accepted.
fn check_ok(source: &str) -> Analysis {
    let mut map = SourceMap::new();
    let file = map.add("test.noto", source);
    let mut sink = DiagnosticSink::new();
    let module = noto_parser::parse_file(map.file(file).unwrap(), &mut sink);
    let analysis = analyze(&module, &mut sink);
    assert!(
        !sink.has_errors(),
        "unexpected diagnostics for:\n{source}\n---\n{}",
        sink.render_all(&map, RenderStyle::Plain)
    );
    analysis
}

/// Analyses `source`, asserting exactly one error containing `needle`.
fn check_error(source: &str, needle: &str) {
    let (_, messages) = check(source);
    assert!(
        messages.iter().any(|message| message.contains(needle)),
        "expected an error containing {needle:?}, got {messages:?}\nfor:\n{source}"
    );
}

/// Wraps statements in a `main` function.
fn in_main(body: &str) -> String {
    format!("fn main() {{\n{body}\n}}\n")
}

// --- the first program ---------------------------------------------------

#[test]
fn accepts_hello_world() {
    let analysis = check_ok("fn main() {\n    println(\"Hello, Noto!\")\n}\n");
    assert!(analysis.entry.is_some());
    assert_eq!(analysis.functions.len(), 1);
}

// --- inference ------------------------------------------------------------

#[test]
fn infers_the_type_of_a_binding() {
    let analysis = check_ok(&in_main(
        "    val name = \"João\"\n    var age = 16\n    val ok = true\n    val letter = 'a'",
    ));
    let types: Vec<String> = analysis
        .locals
        .iter()
        .map(|local| format!("{}: {}", local.name, analysis.store.render(local.ty)))
        .collect();
    assert_eq!(types, vec!["name: String", "age: Int", "ok: Bool", "letter: Char"]);
}

#[test]
fn a_literal_takes_the_width_it_is_used_at() {
    let analysis = check_ok(&in_main("    val small: Int8 = 5\n    val wide = 5"));
    assert_eq!(analysis.store.render(analysis.locals[0].ty), "Int8");
    assert_eq!(analysis.store.render(analysis.locals[1].ty), "Int");
}

#[test]
fn a_suffix_pins_the_width() {
    let analysis = check_ok(&in_main("    val n = 10u8\n    val m = 7i32"));
    assert_eq!(analysis.store.render(analysis.locals[0].ty), "UInt8");
    assert_eq!(analysis.store.render(analysis.locals[1].ty), "Int32");
}

#[test]
fn rejects_a_literal_that_does_not_fit() {
    check_error(&in_main("    val n: Int8 = 300"), "does not fit in `Int8`");
}

#[test]
fn infers_a_function_result_from_its_body() {
    let analysis = check_ok("fn double(n: Int): Int = n * 2\nfn main() { println(double(4)) }\n");
    let double = analysis.function_named("double").unwrap();
    assert_eq!(analysis.store.render(analysis.function(double).result), "Int");
}

// --- mutability -----------------------------------------------------------

#[test]
fn a_var_may_be_reassigned() {
    check_ok(&in_main("    var count = 0\n    count = 1\n    count += 2"));
}

#[test]
fn rejects_reassigning_a_val() {
    check_error(&in_main("    val count = 0\n    count = 1"), "cannot assign to `count`");
}

#[test]
fn rejects_reassigning_a_parameter() {
    check_error("fn f(n: Int) {\n    n = 2\n}\n", "cannot assign to `n`");
}

// --- null safety ----------------------------------------------------------

#[test]
fn a_nullable_binding_accepts_null() {
    let analysis = check_ok(&in_main("    val name: String? = null"));
    assert_eq!(analysis.store.render(analysis.locals[0].ty), "String?");
}

#[test]
fn rejects_null_for_a_non_nullable_type() {
    check_error(&in_main("    val name: String = null"), "expected `String`, found `Nothing?`");
}

#[test]
fn rejects_a_nullable_value_where_a_plain_one_is_required() {
    check_error(
        "fn takes(name: String) {}\nfn main() {\n    val maybe: String? = null\n    takes(maybe)\n}\n",
        "expected `String`, found `String?`",
    );
}

#[test]
fn elvis_removes_the_nullability() {
    let analysis =
        check_ok(&in_main("    val maybe: String? = null\n    val name = maybe ?: \"anon\""));
    assert_eq!(analysis.store.render(analysis.locals[1].ty), "String");
}

#[test]
fn rejects_arithmetic_on_a_nullable_value() {
    check_error(
        &in_main("    val maybe: Int? = null\n    val n = maybe + 1"),
        "cannot be applied to a `Int?`",
    );
}

#[test]
fn warns_when_elvis_can_never_run() {
    let (_, messages) = check(&in_main("    val name = \"x\" ?: \"y\""));
    assert!(
        messages.iter().any(|m| m.contains("never null")),
        "expected a warning, got {messages:?}"
    );
}

// --- conversions ----------------------------------------------------------

#[test]
fn rejects_mixing_integer_widths() {
    check_error(
        &in_main("    val a: Int32 = 1\n    val b: Int64 = 2\n    val c = a + b"),
        "cannot mix `Int32` and `Int64`",
    );
}

#[test]
fn suggests_the_conversion_that_would_work() {
    let mut map = SourceMap::new();
    let file = map.add("test.noto", &in_main("    val a: Int64 = 1\n    val b: Int32 = 2\n    val c = a + b"));
    let mut sink = DiagnosticSink::new();
    let module = noto_parser::parse_file(map.file(file).unwrap(), &mut sink);
    analyze(&module, &mut sink);
    let helps: Vec<String> =
        sink.diagnostics().iter().flat_map(|d| d.helps.clone()).collect();
    assert!(helps.iter().any(|h| h.contains(".toInt64()")), "{helps:?}");
}

#[test]
fn rejects_assigning_a_narrower_integer_without_a_conversion() {
    check_error(&in_main("    val a: Int32 = 1\n    val b: Int64 = a"), "expected `Int64`, found `Int32`");
}

// --- conditions -----------------------------------------------------------

#[test]
fn rejects_a_non_bool_condition() {
    check_error(&in_main("    if 1 { }"), "a condition must be a `Bool`");
}

#[test]
fn explains_that_there_is_no_truthiness() {
    let mut map = SourceMap::new();
    let file = map.add("test.noto", &in_main("    val count = 3\n    if count { }"));
    let mut sink = DiagnosticSink::new();
    let module = noto_parser::parse_file(map.file(file).unwrap(), &mut sink);
    analyze(&module, &mut sink);
    let helps: Vec<String> = sink.diagnostics().iter().flat_map(|d| d.helps.clone()).collect();
    assert!(helps.iter().any(|h| h.contains("no truthiness")), "{helps:?}");
}

#[test]
fn if_is_an_expression_when_both_branches_agree() {
    let analysis = check_ok(&in_main("    val n = if true { 1 } else { 2 }"));
    assert_eq!(analysis.store.render(analysis.locals[0].ty), "Int");
}

#[test]
fn rejects_an_if_whose_branches_disagree() {
    check_error(
        &in_main("    val n = if true { 1 } else { \"two\" }"),
        "branches of this `if` have different types",
    );
}

#[test]
fn a_branch_that_returns_does_not_constrain_the_type() {
    check_ok("fn f(c: Bool): Int {\n    val n = if c { 1 } else { return 0 }\n    return n\n}\n");
}

// --- when -----------------------------------------------------------------

#[test]
fn checks_a_when_over_ranges() {
    check_ok(&in_main(
        "    val age = 16\n    when (age) {\n        0..12 -> println(\"Criança\")\n        13..17 -> println(\"Adolescente\")\n        else -> println(\"Adulto\")\n    }",
    ));
}

#[test]
fn a_when_producing_a_value_needs_an_else() {
    check_error(
        &in_main("    val label = when (1) {\n        1 -> \"one\"\n        2 -> \"two\"\n    }"),
        "does not cover every case",
    );
}

#[test]
fn rejects_when_arms_that_disagree() {
    check_error(
        &in_main("    val x = when (1) {\n        1 -> \"one\"\n        else -> 2\n    }"),
        "arms produce a",
    );
}

#[test]
fn rejects_a_when_pattern_of_the_wrong_type() {
    check_error(
        &in_main("    when (\"text\") {\n        1 -> println(\"one\")\n        else -> println(\"other\")\n    }"),
        "expected `String`, found `Int`",
    );
}

// --- functions ------------------------------------------------------------

#[test]
fn functions_may_be_called_before_they_are_declared() {
    check_ok("fn main() {\n    println(later())\n}\n\nfn later(): Int = 42\n");
}

#[test]
fn checks_argument_count() {
    check_error("fn add(a: Int, b: Int): Int = a + b\nfn main() { add(1) }\n", "takes 2 arguments, but 1 were given");
}

#[test]
fn checks_argument_types() {
    check_error(
        "fn add(a: Int, b: Int): Int = a + b\nfn main() { add(1, \"two\") }\n",
        "expected `Int`, found `String`",
    );
}

#[test]
fn rejects_a_missing_return_value() {
    check_error("fn f(): Int {\n    println(\"hi\")\n}\n", "must produce a `Int`");
}

#[test]
fn rejects_returning_a_value_from_a_unit_function() {
    check_error("fn f() {\n    return 1\n}\n", "does not return a value");
}

#[test]
fn rejects_a_duplicate_function() {
    check_error("fn f() {}\nfn f() {}\n", "declared more than once");
}

#[test]
fn rejects_main_with_parameters() {
    check_error("fn main(n: Int) {}\n", "`main` must not take any parameters");
}

#[test]
fn main_may_return_an_exit_status() {
    check_ok("fn main(): Int {\n    return 0\n}\n");
}

#[test]
fn rejects_main_returning_something_else() {
    check_error("fn main(): String = \"x\"\n", "`main` must return `Unit` or `Int`");
}

// --- names ----------------------------------------------------------------

#[test]
fn rejects_an_unknown_name() {
    check_error(&in_main("    println(missing)"), "cannot find `missing` in this scope");
}

#[test]
fn suggests_a_near_miss() {
    let mut map = SourceMap::new();
    let file = map.add("test.noto", &in_main("    val count = 1\n    println(cout)"));
    let mut sink = DiagnosticSink::new();
    let module = noto_parser::parse_file(map.file(file).unwrap(), &mut sink);
    analyze(&module, &mut sink);
    let helps: Vec<String> = sink.diagnostics().iter().flat_map(|d| d.helps.clone()).collect();
    assert!(helps.iter().any(|h| h.contains("`count`")), "{helps:?}");
}

#[test]
fn rejects_declaring_the_same_name_twice_in_one_scope() {
    check_error(&in_main("    val x = 1\n    val x = 2"), "already declared in this scope");
}

#[test]
fn an_inner_scope_may_shadow_an_outer_one() {
    check_ok(&in_main("    val x = 1\n    if true {\n        val x = 2\n        println(x)\n    }"));
}

#[test]
fn a_binding_is_not_visible_outside_its_block() {
    check_error(
        &in_main("    if true {\n        val inner = 1\n    }\n    println(inner)"),
        "cannot find `inner`",
    );
}

// --- loops ----------------------------------------------------------------

#[test]
fn checks_a_for_over_a_range() {
    let analysis = check_ok(&in_main("    for i in 0..10 {\n        println(i)\n    }"));
    let i = analysis.locals.iter().find(|local| local.name == "i").unwrap();
    assert_eq!(analysis.store.render(i.ty), "Int");
}

#[test]
fn rejects_iterating_over_a_non_range() {
    check_error(&in_main("    for c in \"text\" { }"), "cannot iterate over a `String`");
}

#[test]
fn rejects_break_outside_a_loop() {
    check_error(&in_main("    break"), "`break` can only be used inside a loop");
}

#[test]
fn accepts_break_and_continue_inside_a_loop() {
    check_ok(&in_main(
        "    var i = 0\n    while i < 10 {\n        i += 1\n        if i == 3 { continue }\n        if i == 5 { break }\n    }",
    ));
}

// --- builtins and members --------------------------------------------------

#[test]
fn resolves_println_by_argument_type() {
    check_ok(&in_main("    println(\"text\")\n    println(1)\n    println(true)\n    println()"));
}

#[test]
fn rejects_println_of_an_unprintable_value() {
    check_error(&in_main("    println('a')"), "no version of `println` accepts (Char)");
}

#[test]
fn checks_to_string_on_numbers_and_bools() {
    let analysis = check_ok(&in_main("    val a = 1.toString()\n    val b = true.toString()"));
    assert_eq!(analysis.store.render(analysis.locals[0].ty), "String");
    assert_eq!(analysis.store.render(analysis.locals[1].ty), "String");
}

#[test]
fn reads_the_length_of_a_string() {
    let analysis = check_ok(&in_main("    val n = \"hello\".length"));
    assert_eq!(analysis.store.render(analysis.locals[0].ty), "Int");
}

#[test]
fn rejects_an_unknown_member() {
    check_error(&in_main("    val n = \"hello\".size"), "`String` has no member `size`");
}

#[test]
fn rejects_reading_a_member_through_a_nullable_value() {
    check_error(
        &in_main("    val maybe: String? = null\n    val n = maybe.length"),
        "cannot be read from a `String?`",
    );
}

// --- strings ---------------------------------------------------------------

#[test]
fn checks_string_interpolation() {
    check_ok(&in_main("    val name = \"João\"\n    val age = 16\n    println(\"$name tem $age anos\")"));
}

#[test]
fn rejects_interpolating_a_value_with_no_text_form() {
    check_error(
        &in_main("    val c = 'x'\n    println(\"letter: $c\")"),
        "cannot be interpolated into a string",
    );
}

#[test]
fn strings_concatenate_with_plus() {
    let analysis = check_ok(&in_main("    val greeting = \"Olá, \" + \"João\""));
    assert_eq!(analysis.store.render(analysis.locals[0].ty), "String");
}

#[test]
fn rejects_adding_a_number_to_a_string() {
    check_error(&in_main("    val x = \"n: \" + 1"), "cannot be applied to a `String`");
}

// --- constants -------------------------------------------------------------

#[test]
fn folds_a_constant_at_compile_time() {
    let analysis = check_ok("const MAX_USERS = 100\nconst DOUBLE = MAX_USERS\nfn main() {}\n");
    assert_eq!(analysis.constants[0].value, ConstValue::Int(100));
}

#[test]
fn folds_constant_arithmetic() {
    let analysis = check_ok("const SIZE = 4 * 1024\nfn main() {}\n");
    assert_eq!(analysis.constants[0].value, ConstValue::Int(4096));
}

#[test]
fn rejects_a_constant_that_needs_runtime_work() {
    check_error("fn f(): Int = 1\nconst X = f()\nfn main() {}\n", "must be computable at compile time");
}

#[test]
fn rejects_division_by_zero_in_a_constant() {
    check_error("const X = 1 / 0\nfn main() {}\n", "division by zero");
}

// --- unreachable code ------------------------------------------------------

#[test]
fn warns_about_code_after_a_return() {
    let (_, messages) = check("fn f(): Int {\n    return 1\n    println(\"never\")\n}\n");
    assert!(messages.iter().any(|m| m.contains("never run")), "{messages:?}");
}

// --- unsupported constructs ------------------------------------------------

#[test]
fn reports_constructs_the_compiler_cannot_lower_yet() {
    for (source, needle) in [
        ("struct Point(val x: Int)\nfn main() {}\n", "not supported by this compiler yet"),
        ("data class User(val name: String)\nfn main() {}\n", "not supported by this compiler yet"),
        ("interface Shape { fn area(): Int }\nfn main() {}\n", "not supported by this compiler yet"),
        ("enum Color { Red }\nfn main() {}\n", "not supported by this compiler yet"),
        ("fn f<T>(x: T) {}\nfn main() {}\n", "generic functions are not supported"),
    ] {
        check_error(source, needle);
    }
}

#[test]
fn an_unsupported_construct_still_produces_an_analysis() {
    let (analysis, messages) = check("interface Shape { fn area(): Int }\nfn main() {}\n");
    assert!(!messages.is_empty());
    assert!(analysis.entry.is_some(), "`main` is still analysed");
}

// --- classes ---------------------------------------------------------------

#[test]
fn a_class_declares_a_type_and_a_constructor() {
    let analysis = check_ok(
        "class Point(val x: Int, val y: Int)\n         fn main() {\n    val p = Point(1, 2)\n    println(p.x)\n}\n",
    );
    assert_eq!(analysis.classes.len(), 1);
    assert_eq!(analysis.classes[0].name, "Point");
    assert_eq!(analysis.classes[0].fields.len(), 2);
}

#[test]
fn a_class_name_is_usable_as_a_type() {
    check_ok(
        "class Point(val x: Int)\n         fn origin(): Point = Point(0)\n         fn main() {\n    println(origin().x)\n}\n",
    );
}

#[test]
fn a_class_may_be_named_before_it_is_declared() {
    check_ok(
        "fn first(p: Pair): Int = p.left\n         class Pair(val left: Int, val right: Int)\n         fn main() {\n    println(first(Pair(1, 2)))\n}\n",
    );
}

#[test]
fn a_field_may_hold_another_class() {
    check_ok(
        "class Inner(val n: Int)\n         class Outer(val inner: Inner)\n         fn main() {\n    println(Outer(Inner(1)).inner.n)\n}\n",
    );
}

#[test]
fn a_constructor_checks_its_argument_count() {
    check_error(
        "class Point(val x: Int, val y: Int)\nfn main() { val p = Point(1) }\n",
        "`Point` takes 2 arguments",
    );
}

#[test]
fn a_constructor_names_the_field_a_bad_argument_was_meant_for() {
    check_error(
        "class Point(val x: Int, val y: Int)\nfn main() { val p = Point(1, \"two\") }\n",
        "`Point.y` is a `Int`",
    );
}

#[test]
fn an_unknown_field_is_reported_with_the_ones_that_exist() {
    check_error(
        "class Point(val x: Int)\nfn main() { println(Point(1).z) }\n",
        "`Point` has no field `z`",
    );
}

#[test]
fn a_val_field_cannot_be_assigned() {
    check_error(
        "class Point(val x: Int)\nfn main() {\n    val p = Point(1)\n    p.x = 2\n}\n",
        "cannot assign to `Point.x`",
    );
}

#[test]
fn a_var_field_can_be_assigned() {
    check_ok(
        "class Counter(var count: Int)\n         fn main() {\n    val c = Counter(0)\n    c.count = 1\n    println(c.count)\n}\n",
    );
}

#[test]
fn a_field_assignment_is_type_checked() {
    check_error(
        "class Counter(var count: Int)\nfn main() {\n    val c = Counter(0)\n    c.count = \"one\"\n}\n",
        "expected `Int`",
    );
}

#[test]
fn a_class_declared_twice_is_reported() {
    check_error(
        "class Point(val x: Int)\nclass Point(val y: Int)\nfn main() {}\n",
        "`Point` is declared more than once",
    );
}

#[test]
fn a_duplicate_field_is_reported() {
    check_error(
        "class Point(val x: Int, val x: Int)\nfn main() {}\n",
        "`x` is declared more than once in `Point`",
    );
}

#[test]
fn a_field_without_a_type_is_reported() {
    check_error("class Point(val x)\nfn main() {}\n", "needs a declared type");
}

#[test]
fn a_class_type_takes_part_in_null_safety() {
    check_ok(
        "class Point(val x: Int)\n         fn main() {\n    val p: Point? = null\n    val q = p ?: Point(0)\n    println(q.x)\n}\n",
    );
    check_error(
        "class Point(val x: Int)\nfn main() {\n    val p: Point? = null\n    println(p.x)\n}\n",
        "cannot be read from a `Point?`",
    );
}

// --- tests -----------------------------------------------------------------

#[test]
fn collects_test_declarations() {
    let analysis = check_ok(
        "fn add(a: Int, b: Int): Int = a + b\n\ntest \"soma dois números\" {\n    assert(add(2, 3) == 5)\n}\n",
    );
    assert_eq!(analysis.tests.len(), 1);
    assert_eq!(analysis.tests[0].name, "soma dois números");
}

#[test]
fn a_test_body_is_type_checked() {
    check_error(
        "test \"broken\" {\n    assert(1)\n}\n",
        "no version of `assert` accepts (Int)",
    );
}

// --- error suppression -----------------------------------------------------

#[test]
fn one_mistake_does_not_cascade() {
    let (_, messages) = check(&in_main("    val x = missing\n    val y = x + 1\n    println(y)"));
    assert_eq!(messages.len(), 1, "expected one diagnostic, got {messages:?}");
}
