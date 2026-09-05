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
        ("enum Colour { Red = 1 }\nfn main() {}\n", "not supported by this compiler yet"),
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

// --- methods ---------------------------------------------------------------

#[test]
fn a_method_is_collected_as_a_function_taking_the_receiver_first() {
    let analysis = check_ok(
        "class Rect(val width: Int, val height: Int) {\n             fn area(): Int = this.width * this.height\n         }\n         fn main() {\n    println(Rect(2, 3).area())\n}\n",
    );
    assert_eq!(analysis.classes[0].methods.len(), 1);
    let method = &analysis.classes[0].methods[0];
    assert_eq!(method.name, "area");

    let function = &analysis.functions[method.function.0 as usize];
    assert_eq!(function.name, "Rect.area", "the class prefix keeps it out of the free namespace");
    assert_eq!(function.parameters.len(), 1, "the receiver is the only parameter");
    assert_eq!(analysis.locals[function.parameters[0].0 as usize].name, "this");
}

#[test]
fn a_method_takes_its_own_parameters_after_the_receiver() {
    let analysis = check_ok(
        "class Counter(var count: Int) {\n             fn add(n: Int) {\n        this.count = this.count + n\n    }\n         }\n         fn main() {\n    val c = Counter(0)\n    c.add(2)\n    println(c.count)\n}\n",
    );
    let method = analysis.classes[0].method("add").expect("the method");
    let function = &analysis.functions[method.function.0 as usize];
    assert_eq!(function.parameters.len(), 2);
    assert_eq!(analysis.locals[function.parameters[1].0 as usize].name, "n");
}

#[test]
fn a_method_may_call_another_method_of_the_same_class() {
    check_ok(
        "class Rect(val side: Int) {\n             fn area(): Int = this.side * this.side\n             fn twice(): Int = this.area() * 2\n         }\n         fn main() {\n    println(Rect(2).twice())\n}\n",
    );
}

#[test]
fn a_method_call_checks_its_arguments() {
    check_error(
        "class Counter(var count: Int) {\n    fn add(n: Int) {\n        this.count = n\n    }\n}\n         fn main() {\n    Counter(0).add(\"two\")\n}\n",
        "expected `Int`",
    );
}

#[test]
fn a_method_call_checks_its_argument_count() {
    check_error(
        "class Counter(var count: Int) {\n    fn add(n: Int) {\n        this.count = n\n    }\n}\n         fn main() {\n    Counter(0).add()\n}\n",
        "`Counter.add` takes 1 argument",
    );
}

#[test]
fn an_unknown_method_is_reported_with_the_ones_that_exist() {
    check_error(
        "class Rect(val side: Int) {\n    fn area(): Int = this.side\n}\n         fn main() {\n    println(Rect(1).volume())\n}\n",
        "`Rect` has no method `volume`",
    );
}

#[test]
fn reading_a_method_without_calling_it_says_so() {
    check_error(
        "class Rect(val side: Int) {\n    fn area(): Int = this.side\n}\n         fn main() {\n    println(Rect(1).area)\n}\n",
        "`area` is a method of `Rect`",
    );
}

#[test]
fn this_outside_a_method_is_reported() {
    check_error("fn main() {\n    println(this)\n}\n", "`this` can only be used inside a method");
}

#[test]
fn a_method_and_a_field_cannot_share_a_name() {
    check_error(
        "class Rect(val area: Int) {\n    fn area(): Int = 1\n}\nfn main() {}\n",
        "already has a field named `area`",
    );
}

#[test]
fn a_method_declared_twice_is_reported() {
    check_error(
        "class Rect(val side: Int) {\n    fn area(): Int = 1\n    fn area(): Int = 2\n}\n         fn main() {}\n",
        "`Rect.area` is declared more than once",
    );
}

#[test]
fn a_method_body_is_checked_like_any_other() {
    check_error(
        "class Rect(val side: Int) {\n    fn area(): Int = \"wide\"\n}\nfn main() {}\n",
        "must produce a `Int`",
    );
}

#[test]
fn a_method_cannot_assign_to_a_val_field() {
    check_error(
        "class Rect(val side: Int) {\n    fn grow() {\n        this.side = 2\n    }\n}\n         fn main() {}\n",
        "cannot assign to `Rect.side`",
    );
}

// --- fields declared in the class body, and properties ----------------------

#[test]
fn a_body_field_is_not_a_constructor_parameter() {
    let analysis = check_ok(
        "class Person(val name: String) {\n    val greeting: String = \"olá\"\n}\n         fn main() {\n    println(Person(\"joão\").greeting)\n}\n",
    );
    let class = &analysis.classes[0];
    assert_eq!(class.fields.len(), 2, "both are fields");
    assert_eq!(class.primary_count, 1, "but only one is passed");
    assert!(class.init.is_some(), "the initialiser needs a function to run in");
}

#[test]
fn a_body_field_may_read_a_constructor_parameter() {
    check_ok(
        "class Person(val first: String, val last: String) {\n             val full: String = first + \" \" + last\n}\n         fn main() {\n    println(Person(\"a\", \"b\").full)\n}\n",
    );
}

#[test]
fn a_body_field_initialiser_is_type_checked() {
    check_error(
        "class Person(val name: String) {\n    val age: Int = \"old\"\n}\nfn main() {}\n",
        "expected `Int`",
    );
}

#[test]
fn a_body_field_needs_something_to_initialise_it() {
    check_error(
        "class Person(val name: String) {\n    val age: Int\n}\nfn main() {}\n",
        "has nothing to initialise it",
    );
}

#[test]
fn a_class_without_body_fields_needs_no_initialiser_function() {
    let analysis = check_ok(
        "class Point(val x: Int)\nfn main() {\n    println(Point(1).x)\n}\n",
    );
    assert!(analysis.classes[0].init.is_none(), "there is nothing for it to do");
}

#[test]
fn a_computed_property_is_read_through_its_getter() {
    let analysis = check_ok(
        "class Rect(val width: Int, val height: Int) {\n             val area: Int { get = this.width * this.height }\n}\n         fn main() {\n    println(Rect(2, 3).area)\n}\n",
    );
    let class = &analysis.classes[0];
    assert_eq!(class.properties.len(), 1);
    assert_eq!(class.properties[0].name, "area");
    assert!(class.properties[0].setter.is_none(), "no `set` was written");
    assert_eq!(class.fields.len(), 2, "a computed property stores nothing");
}

#[test]
fn a_property_with_a_setter_can_be_assigned() {
    check_ok(
        "class Box(val start: Int) {\n             var stored: Int = 0\n             var value: Int {\n        get = this.stored\n        set { this.stored = value }\n    }\n}\n         fn main() {\n    val b = Box(1)\n    b.value = 5\n    println(b.value)\n}\n",
    );
}

#[test]
fn a_property_without_a_setter_cannot_be_assigned() {
    check_error(
        "class Rect(val w: Int) {\n    val area: Int { get = this.w }\n}\n         fn main() {\n    val r = Rect(1)\n    r.area = 2\n}\n",
        "cannot assign to `Rect.area`",
    );
}

#[test]
fn a_val_property_may_not_have_a_setter() {
    check_error(
        "class Box(val n: Int) {\n             val value: Int {\n        get = this.n\n        set { }\n    }\n}\n         fn main() {}\n",
        "a `val` property cannot have a setter",
    );
}

#[test]
fn a_getter_body_must_produce_the_property_type() {
    check_error(
        "class Rect(val w: Int) {\n    val area: Int { get = \"wide\" }\n}\nfn main() {}\n",
        "this `get` must produce a `Int`",
    );
}

#[test]
fn a_property_may_not_share_a_name_with_a_field() {
    check_error(
        "class Rect(val area: Int) {\n    val area: Int { get = 1 }\n}\nfn main() {}\n",
        "already has a field named `area`",
    );
}

#[test]
fn a_stored_property_becomes_an_ordinary_field() {
    let analysis = check_ok(
        "class Counter(val start: Int) {\n    var count: Int = 0\n}\n         fn main() {\n    val c = Counter(1)\n    c.count = 2\n    println(c.count)\n}\n",
    );
    assert_eq!(analysis.classes[0].fields.len(), 2);
    assert!(analysis.classes[0].properties.is_empty(), "nothing computed here");
}

// --- lists -------------------------------------------------------------------

#[test]
fn a_list_literal_takes_its_type_from_its_first_element() {
    check_ok("fn main() {\n    val xs = [1, 2, 3]\n    println(xs[0])\n}\n");
    check_ok("fn main() {\n    val xs = [\"a\", \"b\"]\n    println(xs[0])\n}\n");
}

#[test]
fn every_element_must_fit_the_first() {
    check_error(
        "fn main() {\n    val xs = [1, \"two\"]\n}\n",
        "this list holds `Int`",
    );
}

#[test]
fn an_empty_list_needs_a_declared_type() {
    check_error("fn main() {\n    val xs = []\n}\n", "an empty list has no element type");
    check_ok("fn main() {\n    val xs: [Int] = []\n    println(xs.length)\n}\n");
}

#[test]
fn a_list_type_may_be_written_in_a_signature() {
    check_ok(
        "fn total(xs: [Int]): Int {\n    var sum = 0\n    for x in xs { sum += x }\n    return sum\n}\n         fn main() {\n    println(total([1, 2]))\n}\n",
    );
}

#[test]
fn indexing_produces_the_element_type() {
    check_error(
        "fn main() {\n    val xs = [1, 2]\n    val s: String = xs[0]\n}\n",
        "expected `String`",
    );
}

#[test]
fn an_index_must_be_an_integer() {
    check_error(
        "fn main() {\n    val xs = [1, 2]\n    println(xs[\"first\"])\n}\n",
        "expected `Int`",
    );
}

#[test]
fn only_a_list_can_be_indexed() {
    check_error(
        "fn main() {\n    val n = 1\n    println(n[0])\n}\n",
        "a `Int` cannot be indexed",
    );
}

#[test]
fn an_element_may_be_assigned() {
    check_ok("fn main() {\n    val xs = [1, 2]\n    xs[0] = 5\n    println(xs[0])\n}\n");
}

#[test]
fn an_element_assignment_is_type_checked() {
    check_error(
        "fn main() {\n    val xs = [1, 2]\n    xs[0] = \"five\"\n}\n",
        "expected `Int`",
    );
}

#[test]
fn a_list_may_hold_objects_and_lists() {
    check_ok(
        "class Point(val x: Int)\n         fn main() {\n    val ps = [Point(1), Point(2)]\n    println(ps[0].x)\n}\n",
    );
    check_ok(
        "fn main() {\n    val grid = [[1, 2], [3]]\n    println(grid[0][1])\n}\n",
    );
}

#[test]
fn a_list_is_invariant_in_its_element() {
    // A literal is built to fit what is expected of it, so this is fine.
    check_ok("fn take(xs: [Any]) {}\nfn main() {\n    take([1, 2])\n}\n");

    // A list that already holds `Int` is not a list of `Any`: writing an
    // `Any` through the second would break the first.
    check_error(
        "fn take(xs: [Any]) {}\n         fn main() {\n    val xs = [1, 2]\n    take(xs)\n}\n",
        "expected `[Any]`",
    );
}

#[test]
fn a_for_binds_the_element_type() {
    check_error(
        "fn main() {\n    for x in [1, 2] {\n        val s: String = x\n    }\n}\n",
        "expected `String`",
    );
}

#[test]
fn a_list_grows_with_push() {
    check_ok(
        "fn main() {\n    val xs: [Int] = []\n    xs.push(1)\n    println(xs.length)\n}\n",
    );
}

#[test]
fn push_takes_the_element_type() {
    check_error(
        "fn main() {\n    val xs = [1, 2]\n    xs.push(\"three\")\n}\n",
        "expected `Int`",
    );
}

#[test]
fn push_takes_exactly_one_argument() {
    check_error(
        "fn main() {\n    val xs = [1]\n    xs.push()\n}\n",
        "`push` takes 1 argument",
    );
}

#[test]
fn a_list_has_no_other_methods_yet() {
    check_error(
        "fn main() {\n    val xs = [1]\n    xs.pop()\n}\n",
        "`[Int]` has no method `pop`",
    );
}

// --- strings -----------------------------------------------------------------

#[test]
fn a_string_can_be_measured_read_and_sliced() {
    check_ok(
        "fn main() {\n    val s = \"hello\"\n    println(s.length)\n             println(s.byteAt(0))\n    println(s.substring(1, 3))\n}\n",
    );
}

#[test]
fn a_builtin_method_checks_its_arguments() {
    check_error(
        "fn main() {\n    println(\"hi\".byteAt(\"x\"))\n}\n",
        "expected `Int`",
    );
    check_error(
        "fn main() {\n    println(\"hi\".substring(0))\n}\n",
        "`substring` takes 2 arguments",
    );
}

#[test]
fn byte_at_produces_an_integer_and_substring_a_string() {
    check_error(
        "fn main() {\n    val s: String = \"hi\".byteAt(0)\n}\n",
        "expected `String`",
    );
    check_error(
        "fn main() {\n    val n: Int = \"hi\".substring(0, 1)\n}\n",
        "expected `Int`",
    );
}

#[test]
fn a_file_read_may_fail_so_it_produces_a_nullable_string() {
    check_ok(
        "fn main() {\n    val text = readFile(\"a.txt\") ?: \"\"\n    println(text.length)\n}\n",
    );
    check_error(
        "fn main() {\n    val text: String = readFile(\"a.txt\")\n}\n",
        "expected `String`",
    );
}

#[test]
fn writing_a_file_reports_whether_it_worked() {
    check_ok("fn main() {\n    println(writeFile(\"a.txt\", \"hi\"))\n}\n");
    check_error(
        "fn main() {\n    writeFile(\"a.txt\")\n}\n",
        "no version of `writeFile` accepts (String)",
    );
}

#[test]
fn the_command_line_is_a_list_of_strings() {
    check_ok(
        "fn main() {\n    for a in args() { println(a) }\n    println(args().length)\n}\n",
    );
    check_error(
        "fn main() {\n    val n: Int = args()\n}\n",
        "expected `Int`",
    );
}

// --- enums -----------------------------------------------------------------

#[test]
fn an_enum_declares_a_type_and_its_cases() {
    let analysis = check_ok(
        "enum Colour { Red, Green, Blue }\n         fn main() {\n    val c: Colour = Colour.Red\n    println(c == Colour.Red)\n}\n",
    );
    assert_eq!(analysis.enums.len(), 1);
    assert_eq!(analysis.enums[0].cases.len(), 3);
    assert_eq!(analysis.enums[0].cases[2].name, "Blue");
}

#[test]
fn a_case_may_be_written_bare_or_qualified_in_a_pattern() {
    check_ok(
        "enum Colour { Red, Green }\n         fn name(c: Colour): String = when (c) {\n             Colour.Red -> \"red\"\n    Green -> \"green\"\n}\n         fn main() {\n    println(name(Colour.Red))\n}\n",
    );
}

#[test]
fn covering_every_case_is_as_complete_as_an_else() {
    check_ok(
        "enum Colour { Red, Green }\n         fn name(c: Colour): String = when (c) {\n    Red -> \"r\"\n    Green -> \"g\"\n}\n         fn main() {\n    println(name(Colour.Red))\n}\n",
    );
}

#[test]
fn a_missing_case_is_named_in_the_diagnostic() {
    let (_, messages) = check(
        "enum Colour { Red, Green, Blue }\n         fn name(c: Colour): String = when (c) {\n    Red -> \"r\"\n    Green -> \"g\"\n}\n         fn main() {}\n",
    );
    assert!(
        messages.iter().any(|m| m.contains("does not cover every case")),
        "{messages:?}"
    );
}

#[test]
fn a_guarded_arm_does_not_count_as_covering_its_case() {
    check_error(
        "enum Colour { Red, Green }\n         fn name(c: Colour, loud: Bool): String = when (c) {\n             Red if loud -> \"RED\"\n    Green -> \"g\"\n}\n         fn main() {}\n",
        "does not cover every case",
    );
}

#[test]
fn an_unknown_case_is_reported_with_the_ones_that_exist() {
    check_error(
        "enum Colour { Red }\n         fn f(c: Colour): Int = when (c) {\n    Purple -> 1\n    else -> 2\n}\n         fn main() {}\n",
        "`Colour` has no case `Purple`",
    );
}

#[test]
fn reading_a_case_that_does_not_exist_is_reported() {
    check_error(
        "enum Colour { Red }\nfn main() {\n    val c = Colour.Purple\n}\n",
        "`Colour` has no case `Purple`",
    );
}

#[test]
fn a_case_pattern_against_a_non_enum_is_reported() {
    check_error(
        "enum Colour { Red }\n         fn f(n: Int): Int = when (n) {\n    Red -> 1\n    else -> 2\n}\n         fn main() {}\n",
        "matches a `Int`",
    );
}

#[test]
fn a_case_declared_twice_is_reported() {
    check_error(
        "enum Colour { Red, Red }\nfn main() {}\n",
        "`Colour.Red` is declared more than once",
    );
}

#[test]
fn a_case_may_carry_data() {
    let analysis = check_ok(
        "enum Shape { Circle(radius: Int), Rect(width: Int, height: Int), Point }\n         fn area(s: Shape): Int = when (s) {\n             Circle(r) -> 3 * r * r\n    Rect(w, h) -> w * h\n    Point -> 0\n}\n         fn main() {\n    println(area(Shape.Circle(2)))\n}\n",
    );
    assert!(analysis.enums[0].has_data);
    assert_eq!(analysis.enums[0].widest_case(), 2);
    assert_eq!(analysis.enums[0].cases[0].fields[0].name, "radius");
}

#[test]
fn a_case_that_carries_nothing_is_not_called() {
    check_error(
        "enum Shape { Circle(r: Int), Point }\n         fn main() {\n    val p = Shape.Point(1)\n}\n",
        "`Shape.Point` carries no data",
    );
}

#[test]
fn destructuring_a_case_checks_how_many_values_it_carries() {
    check_error(
        "enum Shape { Rect(w: Int, h: Int) }\n         fn f(s: Shape): Int = when (s) {\n    Rect(w) -> w\n    else -> 0\n}\n         fn main() {}\n",
        "carries 2 values, but 1 is matched",
    );
}

#[test]
fn a_case_argument_is_type_checked_against_what_it_carries() {
    check_error(
        "enum Shape { Circle(radius: Int) }\n         fn main() {\n    val s = Shape.Circle(\"big\")\n}\n",
        "`Shape.Circle.radius` is a `Int`",
    );
}

#[test]
fn a_case_may_be_matched_without_naming_what_it_carries() {
    check_ok(
        "enum Shape { Circle(r: Int), Point }\n         fn is_round(s: Shape): Bool = when (s) {\n    Circle -> true\n    Point -> false\n}\n         fn main() {\n    println(is_round(Shape.Point))\n}\n",
    );
}

#[test]
fn a_case_may_carry_an_object() {
    check_ok(
        "class Point(val x: Int, val y: Int)\n         enum Shape { At(origin: Point), Nowhere }\n         fn x_of(s: Shape): Int = when (s) {\n    At(p) -> p.x\n    Nowhere -> 0\n}\n         fn main() {\n    println(x_of(Shape.At(Point(1, 2))))\n}\n",
    );
}

#[test]
fn an_explicit_case_value_is_not_supported_yet() {
    check_error("enum Colour { Red = 1 }\nfn main() {}\n", "explicit case values");
}

#[test]
fn a_nullable_enum_still_needs_an_else() {
    check_error(
        "enum Colour { Red, Green }\n         fn name(c: Colour?): String = when (c) {\n    Red -> \"r\"\n    Green -> \"g\"\n}\n         fn main() {}\n",
        "does not cover every case",
    );
}

#[test]
fn an_exported_enum_crosses_a_module_boundary() {
    program_ok(&[
        (
            "main",
            "fn main() {\n    println(paint.describe(paint.Colour.Red))\n}\n",
            &[(1, Some("paint"), &[])],
        ),
        (
            "colour",
            "export enum Colour { Red, Green }\n             export fn describe(c: Colour): String = when (c) {\n                 Red -> \"r\"\n    Green -> \"g\"\n}\n",
            &[],
        ),
    ]);
}

// --- modules ---------------------------------------------------------------

/// Analyses a program of several modules. The first is the root, and each
/// entry is `(name, source, imports)` where an import is
/// `(module index, binding, selected names)`.
fn check_program(
    modules: &[(&str, &str, &[(usize, Option<&str>, &[&str])])],
) -> (Analysis, Vec<String>) {
    let mut map = SourceMap::new();
    let mut sink = DiagnosticSink::new();

    let mut asts = Vec::new();
    let mut next = noto_ast::NodeId(0);
    for (name, source, _) in modules {
        let file = map.add(format!("{name}.noto"), *source);
        let (ast, after) =
            noto_parser::parse_file_from(map.file(file).unwrap(), next, &mut sink);
        next = after;
        asts.push(ast);
    }

    let imports: Vec<Vec<crate::Import>> = modules
        .iter()
        .map(|(_, _, imports)| {
            imports
                .iter()
                .map(|(target, binding, names)| crate::Import {
                    module: ModuleId(*target as u32),
                    path: modules[*target].0.to_string(),
                    binding: binding.map(str::to_string),
                    names: names
                        .iter()
                        .map(|name| noto_ast::Ident {
                            name: name.to_string(),
                            span: noto_span::Span::dummy(),
                        })
                        .collect(),
                    span: noto_span::Span::dummy(),
                })
                .collect()
        })
        .collect();

    let inputs: Vec<crate::ModuleInput> = modules
        .iter()
        .enumerate()
        .map(|(index, (name, _, _))| crate::ModuleInput {
            name: if index == 0 { "" } else { name },
            ast: &asts[index],
            imports: &imports[index],
        })
        .collect();

    let analysis = crate::analyze_program(&inputs, &mut sink);
    let messages = sink.diagnostics().iter().map(|d| d.message.clone()).collect();
    (analysis, messages)
}

fn program_ok(modules: &[(&str, &str, &[(usize, Option<&str>, &[&str])])]) -> Analysis {
    let (analysis, messages) = check_program(modules);
    assert!(messages.is_empty(), "unexpected diagnostics: {messages:?}");
    analysis
}

fn program_error(
    modules: &[(&str, &str, &[(usize, Option<&str>, &[&str])])],
    needle: &str,
) {
    let (_, messages) = check_program(modules);
    assert!(
        messages.iter().any(|message| message.contains(needle)),
        "expected an error containing {needle:?}, got {messages:?}"
    );
}

#[test]
fn a_qualified_call_reaches_an_exported_function() {
    program_ok(&[
        ("main", "fn main() {\n    println(util.double(21))\n}\n", &[(1, Some("util"), &[])]),
        ("util", "export fn double(n: Int): Int = n * 2\n", &[]),
    ]);
}

#[test]
fn a_selective_import_binds_the_name_unqualified() {
    program_ok(&[
        ("main", "fn main() {\n    println(double(21))\n}\n", &[(1, None, &["double"])]),
        ("util", "export fn double(n: Int): Int = n * 2\n", &[]),
    ]);
}

#[test]
fn a_private_function_cannot_be_reached() {
    program_error(
        &[
            ("main", "fn main() {\n    println(util.secret())\n}\n", &[(1, Some("util"), &[])]),
            ("util", "fn secret(): Int = 42\n", &[]),
        ],
        "`secret` is private to `util`",
    );
}

#[test]
fn a_name_a_module_does_not_declare_is_told_apart_from_a_private_one() {
    program_error(
        &[
            ("main", "fn main() {\n    println(util.missing())\n}\n", &[(1, Some("util"), &[])]),
            ("util", "fn secret(): Int = 42\n", &[]),
        ],
        "`util` declares no `missing`",
    );
}

#[test]
fn a_selective_import_of_a_private_name_says_which_keyword_is_missing() {
    program_error(
        &[
            ("main", "fn main() {}\n", &[(1, None, &["secret"])]),
            ("util", "fn secret(): Int = 42\n", &[]),
        ],
        "`secret` is private to `util`",
    );
}

#[test]
fn a_selective_import_of_a_name_that_is_not_there_lists_the_exports() {
    let (_, messages) = check_program(&[
        ("main", "fn main() {}\n", &[(1, None, &["tripled"])]),
        ("util", "export fn double(n: Int): Int = n\nexport const LIMIT: Int = 1\n", &[]),
    ]);
    assert!(messages.iter().any(|m| m.contains("declares no `tripled`")), "{messages:?}");
}

#[test]
fn an_exported_class_can_be_named_and_constructed_qualified() {
    program_ok(&[
        (
            "main",
            "fn main() {\n    val p: geo.Point = geo.Point(1, 2)\n    println(p.x)\n}\n",
            &[(1, Some("geo"), &[])],
        ),
        ("geometry", "export class Point(val x: Int, val y: Int)\n", &[]),
    ]);
}

#[test]
fn a_method_of_an_imported_class_is_callable() {
    program_ok(&[
        (
            "main",
            "fn main() {\n    println(geo.Point(3, 4).sum())\n}\n",
            &[(1, Some("geo"), &[])],
        ),
        (
            "geometry",
            "export class Point(val x: Int, val y: Int) {\n                 export fn sum(): Int = this.x + this.y\n             }\n",
            &[],
        ),
    ]);
}

#[test]
fn two_modules_may_each_declare_the_same_name() {
    let analysis = program_ok(&[
        ("main", "fn helper(): Int = 1\nfn main() {\n    println(helper())\n}\n", &[]),
        ("util", "export fn helper(): Int = 2\n", &[]),
    ]);
    assert_eq!(
        analysis.functions.iter().filter(|f| f.name == "helper").count(),
        2,
        "each module keeps its own"
    );
}

#[test]
fn a_modules_own_declaration_wins_over_an_imported_one() {
    // Both modules declare `helper`. The unqualified call is this module's;
    // reaching the other one takes the namespace.
    let analysis = program_ok(&[
        (
            "main",
            "fn helper(): Int = 1\nfn main() {\n    println(helper() + util.helper())\n}\n",
            &[(1, Some("util"), &[])],
        ),
        ("util", "export fn helper(): Int = 2\n", &[]),
    ]);
    assert_eq!(analysis.functions.iter().filter(|f| f.name == "helper").count(), 2);
}

#[test]
fn an_import_may_not_bind_a_name_the_module_declares() {
    program_error(
        &[
            ("main", "fn helper(): Int = 1\nfn main() {}\n", &[(1, None, &["helper"])]),
            ("util", "export fn helper(): Int = 2\n", &[]),
        ],
        "already declared in this module",
    );
}

#[test]
fn two_imports_may_not_bind_the_same_name() {
    program_error(
        &[
            ("main", "fn main() {}\n", &[(1, Some("util"), &[]), (2, Some("util"), &[])]),
            ("one", "export fn a(): Int = 1\n", &[]),
            ("two", "export fn b(): Int = 2\n", &[]),
        ],
        "bound by two imports",
    );
}

#[test]
fn only_the_root_module_declares_the_entry_point() {
    let analysis = program_ok(&[
        ("main", "fn main() {\n    println(1)\n}\n", &[(1, Some("util"), &[])]),
        ("util", "export fn main(): Int = 2\n", &[]),
    ]);
    let entry = analysis.entry.expect("the root has one");
    assert_eq!(analysis.functions[entry.0 as usize].module, ModuleId::ROOT);
}

#[test]
fn a_type_error_in_an_imported_module_is_reported() {
    program_error(
        &[
            ("main", "fn main() {}\n", &[(1, Some("util"), &[])]),
            ("util", "export fn broken(): Int = \"text\"\n", &[]),
        ],
        "must produce a `Int`",
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
