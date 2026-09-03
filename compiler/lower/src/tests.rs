//! Lowering tests.
//!
//! The tests assert on the textual IR, which keeps them readable and makes a
//! change in generated code visible in the diff.

use super::*;
use noto_diagnostics::RenderStyle;
use noto_ir::{Intrinsic, Terminator};
use noto_span::SourceMap;

/// Compiles `source` as far as Noto IR, asserting it produced no diagnostics.
fn lower_source(source: &str) -> Program {
    let mut map = SourceMap::new();
    let file = map.add("test.noto", source);
    let mut sink = DiagnosticSink::new();
    let module = noto_parser::parse_file(map.file(file).unwrap(), &mut sink);
    let analysis = noto_semantic::analyze(&module, &mut sink);
    assert!(
        !sink.has_errors(),
        "unexpected diagnostics for:\n{source}\n---\n{}",
        sink.render_all(&map, RenderStyle::Plain)
    );
    let program = lower(&module, &analysis, &mut sink);
    assert!(
        !sink.has_errors(),
        "lowering failed for:\n{source}\n---\n{}",
        sink.render_all(&map, RenderStyle::Plain)
    );
    program
}

/// The textual IR of one function.
fn function_ir(source: &str, name: &str) -> String {
    let program = lower_source(source);
    program
        .function_named(name)
        .unwrap_or_else(|| panic!("no function `{name}` in:\n{program}"))
        .to_string()
}

/// Every intrinsic a function calls, in order.
fn intrinsics(source: &str, name: &str) -> Vec<Intrinsic> {
    let program = lower_source(source);
    let function = program.function_named(name).expect("the function");
    function
        .blocks
        .iter()
        .flat_map(|block| &block.instructions)
        .filter_map(|inst| match &inst.kind {
            noto_ir::InstKind::Intrinsic { which, .. } => Some(*which),
            _ => None,
        })
        .collect()
}

#[test]
fn lowers_hello_world() {
    let ir = function_ir("fn main() {\n    println(\"Hello, Noto!\")\n}\n", "main");
    assert_eq!(
        ir,
        "\
fn main(): unit {
  entry0:
    intrinsic println_string str @0
    return
}
"
    );
}

#[test]
fn interns_each_distinct_string_once() {
    let program = lower_source(
        "fn main() {\n    println(\"a\")\n    println(\"a\")\n    println(\"b\")\n}\n",
    );
    assert_eq!(program.strings, vec!["a", "b"]);
}

#[test]
fn lowers_locals_to_slots() {
    let ir = function_ir(
        "fn main() {\n    val a = 1\n    var b = a + 2\n    println(b)\n}\n",
        "main",
    );
    assert_eq!(
        ir,
        "\
fn main(): unit {
  local $0 a: i64
  local $1 b: i64
  entry0:
    store $0 1:i64
    %0 = load $0
    %1 = add %0 2:i64
    store $1 %1
    %2 = load $1
    intrinsic println_int %2
    return
}
"
    );
}

#[test]
fn lowers_a_function_call_with_a_result() {
    let ir = function_ir(
        "fn add(a: Int, b: Int): Int = a + b\nfn main() {\n    println(add(2, 3))\n}\n",
        "add",
    );
    assert_eq!(
        ir,
        "\
fn add($0, $1): i64 {
  param $0 a: i64
  param $1 b: i64
  entry0:
    %0 = load $0
    %1 = load $1
    %2 = add %0 %1
    return %2
}
"
    );
}

#[test]
fn signedness_comes_from_the_operand_type() {
    let signed = function_ir("fn f(a: Int, b: Int): Bool = a < b\n", "f");
    assert!(signed.contains("slt"), "{signed}");
    let unsigned = function_ir("fn f(a: UInt, b: UInt): Bool = a < b\n", "f");
    assert!(unsigned.contains("ult"), "{unsigned}");
    let division = function_ir("fn f(a: UInt, b: UInt): UInt = a / b\n", "f");
    assert!(division.contains("udiv"), "{division}");
}

#[test]
fn lowers_if_into_blocks() {
    let ir = function_ir(
        "fn f(c: Bool): Int {\n    if c {\n        return 1\n    }\n    return 2\n}\n",
        "f",
    );
    assert!(ir.contains("branch %0 if_then"), "{ir}");
    assert!(ir.contains("return 1:i64"), "{ir}");
    assert!(ir.contains("return 2:i64"), "{ir}");
}

#[test]
fn an_if_expression_stores_both_branches_into_one_slot() {
    let ir = function_ir("fn f(c: Bool): Int = if c { 1 } else { 2 }\n", "f");
    // Both arms write the same slot, and the join block reads it back.
    assert_eq!(ir.matches("store $1").count(), 2, "{ir}");
    assert!(ir.contains("if_join"), "{ir}");
}

#[test]
fn short_circuit_operators_become_branches() {
    let ir = function_ir("fn f(a: Bool, b: Bool): Bool = a && b\n", "f");
    assert!(ir.contains("sc_rhs"), "the right operand gets its own block:\n{ir}");
    assert!(ir.contains("sc_join"), "{ir}");
    // `&&` skips the right side when the left is false.
    assert!(ir.contains("branch %0 sc_rhs1 sc_join2"), "{ir}");

    let or = function_ir("fn f(a: Bool, b: Bool): Bool = a || b\n", "f");
    assert!(or.contains("branch %0 sc_join2 sc_rhs1"), "`||` skips on true:\n{or}");
}

#[test]
fn lowers_a_while_loop() {
    let ir = function_ir(
        "fn f() {\n    var i = 0\n    while i < 3 {\n        i += 1\n    }\n}\n",
        "f",
    );
    assert!(ir.contains("while_test"), "{ir}");
    assert!(ir.contains("while_body"), "{ir}");
    assert!(ir.contains("while_exit"), "{ir}");
    assert!(ir.contains("jump while_test"), "the body loops back:\n{ir}");
}

#[test]
fn lowers_a_for_over_a_range() {
    let ir = function_ir(
        "fn f() {\n    for i in 0..3 {\n        println(i)\n    }\n}\n",
        "f",
    );
    // The counter starts at the lower bound, is tested against the upper one
    // and is incremented in its own block.
    assert!(ir.contains("store $0 0:i64"), "{ir}");
    assert!(ir.contains("slt"), "an exclusive range tests with `<`:\n{ir}");
    assert!(ir.contains("add") && ir.contains("1:i64"), "{ir}");
    assert!(ir.contains("for_step"), "{ir}");
}

#[test]
fn an_inclusive_range_tests_with_less_or_equal() {
    let ir = function_ir("fn f() {\n    for i in 0..=3 {\n        println(i)\n    }\n}\n", "f");
    assert!(ir.contains("sle"), "{ir}");
    assert!(!ir.contains(" slt "), "{ir}");
}

#[test]
fn continue_jumps_to_the_step_block_so_the_counter_advances() {
    let ir = function_ir(
        "fn f() {\n    for i in 0..3 {\n        if i == 1 { continue }\n        println(i)\n    }\n}\n",
        "f",
    );
    let step = ir
        .lines()
        .find(|line| line.trim_start().starts_with("for_step"))
        .expect("a step block");
    let label = step.trim().trim_end_matches(':');
    assert!(ir.contains(&format!("jump {label}")), "`continue` must reach {label}:\n{ir}");
}

#[test]
fn break_leaves_the_loop() {
    let ir = function_ir(
        "fn f() {\n    loop {\n        break\n    }\n}\n",
        "f",
    );
    assert!(ir.contains("loop_exit"), "{ir}");
    assert!(ir.contains("jump loop_exit"), "{ir}");
}

#[test]
fn lowers_when_into_a_chain_of_tests() {
    let ir = function_ir(
        "fn f(n: Int) {\n    when (n) {\n        0 -> println(\"zero\")\n        1..9 -> println(\"small\")\n        else -> println(\"big\")\n    }\n}\n",
        "f",
    );
    assert!(ir.contains("when_arm"), "{ir}");
    assert!(ir.contains("when_next"), "{ir}");
    assert!(ir.contains("eq"), "the first arm compares for equality:\n{ir}");
    assert!(ir.contains("sge") && ir.contains("slt"), "the range arm tests both bounds:\n{ir}");
}

#[test]
fn a_when_subject_is_evaluated_once() {
    let ir = function_ir(
        "fn side(): Int = 1\nfn f() {\n    when (side()) {\n        0 -> println(\"a\")\n        1 -> println(\"b\")\n        else -> println(\"c\")\n    }\n}\n",
        "f",
    );
    assert_eq!(ir.matches("call fn0").count(), 1, "the subject is called once:\n{ir}");
}

#[test]
fn a_when_guard_is_combined_with_its_pattern() {
    let ir = function_ir(
        "fn f(n: Int) {\n    when (n) {\n        0..9 if n != 5 -> println(\"a\")\n        else -> println(\"b\")\n    }\n}\n",
        "f",
    );
    assert!(ir.contains("and"), "the guard is combined with the range test:\n{ir}");
}

#[test]
fn lowers_string_interpolation_into_conversions_and_joins() {
    assert_eq!(
        intrinsics(
            "fn main() {\n    val name = \"João\"\n    val age = 16\n    println(\"$name tem $age anos\")\n}\n",
            "main"
        ),
        vec![
            Intrinsic::StringConcat, // "" is skipped; name is already a String
            Intrinsic::IntToString,
            Intrinsic::StringConcat,
            Intrinsic::StringConcat,
            Intrinsic::PrintlnString,
        ]
    );
}

#[test]
fn a_plain_literal_needs_no_runtime_work() {
    assert_eq!(
        intrinsics("fn main() {\n    println(\"plain\")\n}\n", "main"),
        vec![Intrinsic::PrintlnString]
    );
}

#[test]
fn string_concatenation_calls_the_runtime() {
    assert_eq!(
        intrinsics("fn main() {\n    println(\"a\" + \"b\")\n}\n", "main"),
        vec![Intrinsic::StringConcat, Intrinsic::PrintlnString]
    );
}

#[test]
fn to_string_lowers_to_its_intrinsic() {
    assert_eq!(
        intrinsics("fn main() {\n    println(42.toString())\n}\n", "main"),
        vec![Intrinsic::IntToString, Intrinsic::PrintlnString]
    );
}

#[test]
fn a_constant_is_folded_into_the_instruction() {
    let ir = function_ir("const MAX = 4 * 25\nfn main() {\n    println(MAX)\n}\n", "main");
    assert!(ir.contains("println_int 100:i64"), "{ir}");
}

#[test]
fn every_block_ends_in_a_terminator() {
    let program = lower_source(
        "fn f(c: Bool): Int {\n    if c {\n        return 1\n    } else {\n        return 2\n    }\n}\n\nfn main() {\n    var i = 0\n    while i < 3 {\n        i += 1\n        if i == 2 { continue }\n    }\n    println(i)\n}\n",
    );
    for function in &program.functions {
        for block in &function.blocks {
            // `Unreachable` is legitimate for a block nothing jumps to, but a
            // block with instructions must actually end somewhere.
            if !block.instructions.is_empty() {
                assert!(
                    !matches!(block.terminator, Terminator::Unreachable),
                    "{}:{} has instructions but no terminator\n{function}",
                    function.name,
                    block.label
                );
            }
        }
    }
}

#[test]
fn every_value_is_defined_before_it_is_used() {
    let program = lower_source(
        "fn add(a: Int, b: Int): Int = a + b\nfn main() {\n    val n = add(1, 2)\n    if n > 0 {\n        println(\"positive: $n\")\n    }\n}\n",
    );
    for function in &program.functions {
        let mut defined = std::collections::HashSet::new();
        for block in &function.blocks {
            for inst in &block.instructions {
                for operand in operands(&inst.kind) {
                    if let Some(value) = operand.as_value() {
                        assert!(
                            defined.contains(&value)
                                || crosses_blocks(function, value),
                            "%{} used before it is defined in {}",
                            value.0,
                            function.name
                        );
                    }
                }
                if let Some(dest) = inst.dest() {
                    defined.insert(dest);
                }
            }
        }
        assert_eq!(
            defined.len(),
            function.value_types.len(),
            "every declared value should be defined in {}",
            function.name
        );
    }
}

/// A value defined in an earlier block is still in scope; lowering never
/// forwards a value across a branch, so this is only a safety valve for the
/// linear scan above.
fn crosses_blocks(function: &noto_ir::Function, value: noto_ir::ValueId) -> bool {
    function
        .blocks
        .iter()
        .flat_map(|block| &block.instructions)
        .any(|inst| inst.dest() == Some(value))
}

fn operands(kind: &noto_ir::InstKind) -> Vec<noto_ir::Operand> {
    use noto_ir::InstKind::*;
    match kind {
        Const { .. } | LoadLocal { .. } | Alloc { .. } => Vec::new(),
        StoreLocal { value, .. } => vec![value.clone()],
        Unary { operand, .. } | Cast { operand, .. } => vec![operand.clone()],
        Load { address, .. } => vec![address.clone()],
        Binary { left, right, .. } => vec![left.clone(), right.clone()],
        Store { address, value, .. } => vec![address.clone(), value.clone()],
        Call { arguments, .. } | Intrinsic { arguments, .. } => arguments.clone(),
    }
}

#[test]
fn tests_are_lowered_as_functions() {
    let program = lower_source(
        "fn add(a: Int, b: Int): Int = a + b\n\ntest \"soma\" {\n    assert(add(2, 3) == 5)\n}\n",
    );
    let test = program.function_named("test$soma").expect("the test function");
    assert!(test
        .blocks
        .iter()
        .flat_map(|block| &block.instructions)
        .any(|inst| matches!(
            &inst.kind,
            noto_ir::InstKind::Intrinsic { which: Intrinsic::Assert, .. }
        )));
}

// --- objects ---------------------------------------------------------------

#[test]
fn constructing_an_object_allocates_and_stores_every_field() {
    let ir = function_ir(
        "class Point(val x: Int, val y: Int)\nfn make(): Point = Point(3, 4)\n",
        "make",
    );
    assert!(ir.contains("alloc 16"), "{ir}");
    assert!(ir.contains("store [%0+0] 3:i64"), "{ir}");
    assert!(ir.contains("store [%0+8] 4:i64"), "{ir}");
}

#[test]
fn an_object_with_no_fields_still_gets_an_address() {
    let ir = function_ir("class Marker()\nfn make(): Marker = Marker()\n", "make");
    assert!(ir.contains("alloc 8"), "two markers must not share an address: {ir}");
}

#[test]
fn a_field_is_read_at_its_offset() {
    let ir = function_ir(
        "class Point(val x: Int, val y: Int)\nfn second(p: Point): Int = p.y\n",
        "second",
    );
    assert!(ir.contains("+8]"), "the second field sits one word in: {ir}");
}

#[test]
fn a_field_write_stores_at_the_same_offset_it_reads() {
    let ir = function_ir(
        "class Counter(var count: Int)\nfn bump(c: Counter) {\n    c.count = 1\n}\n",
        "bump",
    );
    assert!(ir.contains("store [%0+0] 1:i64"), "{ir}");
}

#[test]
fn a_compound_field_assignment_reads_and_writes_one_object() {
    let ir = function_ir(
        "class Counter(var count: Int)\nfn bump(c: Counter) {\n    c.count += 1\n}\n",
        "bump",
    );
    // One load of the receiver slot: evaluating it twice would read one
    // object and store into another.
    assert_eq!(ir.matches("load $0").count(), 1, "{ir}");
    assert!(ir.contains("load [%0+0]"), "{ir}");
    assert!(ir.contains("add"), "{ir}");
    assert!(ir.contains("store [%0+0]"), "{ir}");
}

#[test]
fn a_field_holding_an_object_is_a_pointer() {
    let ir = function_ir(
        "class Inner(val n: Int)\nclass Outer(val inner: Inner)\n\
         fn reach(o: Outer): Int = o.inner.n\n",
        "reach",
    );
    assert!(ir.contains("load [%0+0]"), "the field, then the field of the field: {ir}");
    assert!(ir.contains("load [%1+0]"), "{ir}");
}

#[test]
fn constructor_arguments_are_evaluated_before_the_allocation() {
    // Whatever the arguments call runs in the order it was written, which is
    // only true if none of it is deferred past the alloc.
    let ir = function_ir(
        "class Pair(val a: Int, val b: Int)\nfn one(): Int = 1\n\
         fn make(): Pair = Pair(one(), one())\n",
        "make",
    );
    let alloc = ir.find("alloc").expect("an allocation");
    let last_call = ir.rfind("call fn").expect("both calls");
    assert!(last_call < alloc, "the calls come first:\n{ir}");
}

#[test]
fn a_field_past_the_first_hundred_bytes_gets_its_real_offset() {
    // Offsets above 127 leave the byte-displacement encoding behind, so the
    // layout rule has to hold past it as plainly as it does at zero.
    let fields: Vec<String> = (0..20).map(|index| format!("val f{index}: Int")).collect();
    let source = format!(
        "class Wide({})\nfn last(w: Wide): Int = w.f19\n",
        fields.join(", ")
    );
    let ir = function_ir(&source, "last");
    assert!(ir.contains("+152]"), "field 19 sits at 19 * 8: {ir}");
}
