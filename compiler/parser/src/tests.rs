//! Parser tests.

use super::*;
use noto_ast::{
    ClassKind, Expr, ExprKind, ItemKind, LetKind, Literal, PatternKind, StmtKind, StringSegment,
};
use noto_diagnostics::RenderStyle;
use noto_span::SourceMap;

/// Parses `source`, asserting that it produced no diagnostics.
fn parse(source: &str) -> Module {
    let mut map = SourceMap::new();
    let file = map.add("test.noto", source);
    let mut sink = DiagnosticSink::new();
    let module = parse_file(map.file(file).unwrap(), &mut sink);
    assert!(
        !sink.has_errors(),
        "unexpected diagnostics for:\n{source}\n---\n{}",
        sink.render_all(&map, RenderStyle::Plain)
    );
    module
}

/// Parses `source` and returns the error messages it produced.
fn parse_errors(source: &str) -> Vec<String> {
    let mut map = SourceMap::new();
    let file = map.add("test.noto", source);
    let mut sink = DiagnosticSink::new();
    parse_file(map.file(file).unwrap(), &mut sink);
    sink.diagnostics().iter().map(|d| d.message.clone()).collect()
}

/// Parses a single expression by wrapping it in a function.
fn parse_expr_source(source: &str) -> Expr {
    let module = parse(&format!("fn f() {{\n    {source}\n}}\n"));
    let function = module.function("f").expect("function `f`");
    let body = function.body.as_ref().expect("a body");
    match &body.statements.first().expect("one statement").kind {
        StmtKind::Expr(expr) => expr.clone(),
        other => panic!("expected an expression statement, got {other:?}"),
    }
}

/// Renders an expression as a fully parenthesised S-expression, which makes
/// precedence and associativity easy to assert on.
fn sexp(expr: &Expr) -> String {
    match &expr.kind {
        ExprKind::Literal(Literal::Int { value, .. }) => value.to_string(),
        ExprKind::Literal(Literal::Float { value, .. }) => format!("{value}"),
        ExprKind::Literal(Literal::Bool(value)) => value.to_string(),
        ExprKind::Literal(Literal::Char(value)) => format!("'{value}'"),
        ExprKind::Literal(Literal::Null) => "null".to_string(),
        ExprKind::Literal(Literal::Str(segments)) => {
            let parts: Vec<String> = segments
                .iter()
                .map(|segment| match segment {
                    StringSegment::Text(text) => format!("{text:?}"),
                    StringSegment::Interpolation(expr) => format!("${{{}}}", sexp(expr)),
                })
                .collect();
            format!("(str {})", parts.join(" "))
        }
        ExprKind::Path(path) => path.to_dotted(),
        ExprKind::This => "this".to_string(),
        ExprKind::Super => "super".to_string(),
        ExprKind::Unary { op, operand } => format!("({} {})", op.as_str(), sexp(operand)),
        ExprKind::Binary { op, left, right, .. } => {
            format!("({} {} {})", op.as_str(), sexp(left), sexp(right))
        }
        ExprKind::Member { receiver, name, safe } => {
            format!("({} {} {name})", if *safe { "?." } else { "." }, sexp(receiver))
        }
        ExprKind::Index { target, index } => format!("(index {} {})", sexp(target), sexp(index)),
        ExprKind::Call(call) => {
            let mut parts = vec![sexp(&call.callee)];
            for argument in &call.arguments {
                parts.push(match &argument.name {
                    Some(name) => format!("{name}={}", sexp(&argument.value)),
                    None => sexp(&argument.value),
                });
            }
            format!("(call {})", parts.join(" "))
        }
        ExprKind::Assign { target, value, op, .. } => {
            let op = op.map(|op| op.as_str()).unwrap_or("");
            format!("({op}= {} {})", sexp(target), sexp(value))
        }
        ExprKind::If { condition, then_branch, else_branch } => {
            let then = then_branch.tail_expr().map(sexp).unwrap_or_else(|| "{}".to_string());
            match else_branch {
                Some(alt) => format!("(if {} {then} {})", sexp(condition), sexp(alt)),
                None => format!("(if {} {then})", sexp(condition)),
            }
        }
        ExprKind::When { scrutinee, arms } => {
            let subject = scrutinee.as_ref().map(|s| sexp(s)).unwrap_or_default();
            let arms: Vec<String> = arms
                .iter()
                .map(|arm| {
                    let patterns = if arm.is_else {
                        "else".to_string()
                    } else {
                        arm.patterns.iter().map(pat).collect::<Vec<_>>().join("|")
                    };
                    format!("[{patterns} {}]", sexp(&arm.body))
                })
                .collect();
            format!("(when {subject} {})", arms.join(" "))
        }
        ExprKind::Block(block) => {
            block.tail_expr().map(sexp).map(|e| format!("(block {e})")).unwrap_or_else(|| "(block)".to_string())
        }
        ExprKind::Lambda(lambda) => {
            let params: Vec<String> =
                lambda.parameters.iter().map(|p| p.name.name.clone()).collect();
            let body = lambda.body.tail_expr().map(sexp).unwrap_or_default();
            format!("(lambda [{}] {body})", params.join(" "))
        }
        ExprKind::Range { start, end, inclusive } => format!(
            "({} {} {})",
            if *inclusive { "..=" } else { ".." },
            start.as_ref().map(|e| sexp(e)).unwrap_or_else(|| "_".into()),
            end.as_ref().map(|e| sexp(e)).unwrap_or_else(|| "_".into())
        ),
        ExprKind::Is { value, ty, negated } => {
            format!("({} {} {})", if *negated { "!is" } else { "is" }, sexp(value), ty.render())
        }
        ExprKind::As { value, ty, safe } => {
            format!("({} {} {})", if *safe { "as?" } else { "as" }, sexp(value), ty.render())
        }
        ExprKind::Try(inner) => format!("(try {})", sexp(inner)),
        ExprKind::Await(inner) => format!("(await {})", sexp(inner)),
        ExprKind::Unsafe(_) => "(unsafe)".to_string(),
        ExprKind::Return(value) => match value {
            Some(value) => format!("(return {})", sexp(value)),
            None => "(return)".to_string(),
        },
        ExprKind::Break => "(break)".to_string(),
        ExprKind::Continue => "(continue)".to_string(),
        ExprKind::Tuple(items) => {
            format!("(tuple {})", items.iter().map(sexp).collect::<Vec<_>>().join(" "))
        }
        ExprKind::ListLiteral(items) => {
            format!("(list {})", items.iter().map(sexp).collect::<Vec<_>>().join(" "))
        }
        ExprKind::Error => "<error>".to_string(),
    }
}

fn pat(pattern: &noto_ast::Pattern) -> String {
    match &pattern.kind {
        PatternKind::Wildcard => "_".to_string(),
        PatternKind::Binding { name, subpattern } => match subpattern {
            Some(inner) => format!("{name}@{}", pat(inner)),
            None => name.name.clone(),
        },
        PatternKind::Value(expr) => sexp(expr),
        PatternKind::Range { start, end, inclusive } => format!(
            "{}{}{}",
            start.as_ref().map(|e| sexp(e)).unwrap_or_default(),
            if *inclusive { "..=" } else { ".." },
            end.as_ref().map(|e| sexp(e)).unwrap_or_default()
        ),
        PatternKind::Type(ty) => format!("is {}", ty.render()),
        PatternKind::EnumCase { path, fields } => match fields {
            Some(fields) => format!(
                "{}({})",
                path.to_dotted(),
                fields.iter().map(pat).collect::<Vec<_>>().join(",")
            ),
            None => path.to_dotted(),
        },
        PatternKind::Tuple(items) => {
            format!("({})", items.iter().map(pat).collect::<Vec<_>>().join(","))
        }
        PatternKind::Destructure { path, fields } => format!(
            "{}{{{}}}",
            path.to_dotted(),
            fields.iter().map(pat).collect::<Vec<_>>().join(",")
        ),
        PatternKind::Null => "null".to_string(),
        PatternKind::Error => "<error>".to_string(),
    }
}

fn expr_sexp(source: &str) -> String {
    sexp(&parse_expr_source(source))
}

// --- the first program ---------------------------------------------------

#[test]
fn parses_hello_world() {
    let module = parse("fn main() {\n    println(\"Hello, Noto!\")\n}\n");
    assert_eq!(module.items.len(), 1);
    let main = module.function("main").expect("a `main` function");
    assert_eq!(main.params.len(), 0);
    assert!(main.result.is_none());
    let body = main.body.as_ref().expect("a body");
    assert_eq!(body.statements.len(), 1);
    assert_eq!(
        sexp(body.tail_expr().unwrap()),
        r#"(call println (str "Hello, Noto!"))"#
    );
}

// --- precedence ----------------------------------------------------------

#[test]
fn arithmetic_binds_by_precedence() {
    assert_eq!(expr_sexp("1 + 2 * 3"), "(+ 1 (* 2 3))");
    assert_eq!(expr_sexp("1 * 2 + 3"), "(+ (* 1 2) 3)");
    assert_eq!(expr_sexp("(1 + 2) * 3"), "(* (+ 1 2) 3)");
    assert_eq!(expr_sexp("10 - 4 - 3"), "(- (- 10 4) 3)");
    assert_eq!(expr_sexp("2 * 3 % 4"), "(% (* 2 3) 4)");
}

#[test]
fn unary_binds_tighter_than_binary() {
    assert_eq!(expr_sexp("-a + b"), "(+ (- a) b)");
    assert_eq!(expr_sexp("!a && b"), "(&& (! a) b)");
    assert_eq!(expr_sexp("-a.b"), "(- (. a b))");
}

#[test]
fn bitwise_binds_tighter_than_comparison() {
    // The C precedence for `&` is a well-known source of bugs; Noto binds it
    // tighter so this reads the way it looks.
    assert_eq!(expr_sexp("flags & MASK == 0"), "(== (& flags MASK) 0)");
    assert_eq!(expr_sexp("a | b ^ c & d"), "(| a (^ b (& c d)))");
    assert_eq!(expr_sexp("a << 2 + 1"), "(<< a (+ 2 1))");
}

#[test]
fn logical_operators_bind_looser_than_comparison() {
    assert_eq!(expr_sexp("a < b && c > d"), "(&& (< a b) (> c d))");
    assert_eq!(expr_sexp("a || b && c"), "(|| a (&& b c))");
    assert_eq!(expr_sexp("a == b || c != d"), "(|| (== a b) (!= c d))");
}

#[test]
fn elvis_binds_looser_than_logical_operators() {
    assert_eq!(expr_sexp("a ?: b || c"), "(?: a (|| b c))");
}

#[test]
fn assignment_is_right_associative_and_lowest() {
    assert_eq!(expr_sexp("a = b = c"), "(= a (= b c))");
    assert_eq!(expr_sexp("a = b + c"), "(= a (+ b c))");
    assert_eq!(expr_sexp("total += price * 2"), "(+= total (* price 2))");
}

#[test]
fn ranges_bind_looser_than_arithmetic() {
    assert_eq!(expr_sexp("0..n + 1"), "(.. 0 (+ n 1))");
    assert_eq!(expr_sexp("0..=9"), "(..= 0 9)");
    assert_eq!(expr_sexp("x in 0..10"), "(in x (.. 0 10))");
}

#[test]
fn casts_bind_tighter_than_arithmetic() {
    assert_eq!(expr_sexp("a as Int + b"), "(+ (as a Int) b)");
    assert_eq!(expr_sexp("value as? String"), "(as? value String)");
}

#[test]
fn type_tests_bind_looser_than_arithmetic() {
    assert_eq!(expr_sexp("a + b is Int"), "(is (+ a b) Int)");
    assert_eq!(expr_sexp("x !is String"), "(!is x String)");
}

// --- postfix -------------------------------------------------------------

#[test]
fn chains_calls_and_member_access() {
    assert_eq!(expr_sexp("a.b.c()"), "(call (. (. a b) c))");
    assert_eq!(expr_sexp("f()()"), "(call (call f))");
    assert_eq!(expr_sexp("items[0].name"), "(. (index items 0) name)");
}

#[test]
fn parses_safe_navigation_and_propagation() {
    assert_eq!(expr_sexp("user?.name"), "(?. user name)");
    assert_eq!(expr_sexp("user?.name ?: \"anon\""), r#"(?: (?. user name) (str "anon"))"#);
    assert_eq!(expr_sexp("findUser(id)?"), "(try (call findUser id))");
}

#[test]
fn parses_named_arguments() {
    assert_eq!(expr_sexp("connect(host: \"x\", port: 80)"), r#"(call connect host=(str "x") port=80)"#);
}

// --- statement termination ----------------------------------------------

#[test]
fn a_line_break_ends_a_complete_statement() {
    let module = parse("fn f() {\n    val a = 1\n    val b = 2\n}\n");
    let body = module.function("f").unwrap().body.as_ref().unwrap();
    assert_eq!(body.statements.len(), 2);
}

#[test]
fn a_trailing_operator_continues_the_expression() {
    let module = parse("fn f() {\n    val total = 1 +\n        2\n}\n");
    let body = module.function("f").unwrap().body.as_ref().unwrap();
    assert_eq!(body.statements.len(), 1);
    let binding = body.statements[0].as_simple_binding().expect("a binding");
    assert_eq!(sexp(binding.value.unwrap()), "(+ 1 2)");
}

#[test]
fn a_leading_operator_starts_a_new_statement() {
    // `first` and `-second` are two statements: an operator at the start of a
    // line does not reach back across the break.
    let module = parse("fn f() {\n    first\n    -second\n}\n");
    let body = module.function("f").unwrap().body.as_ref().unwrap();
    assert_eq!(body.statements.len(), 2);
}

#[test]
fn a_leading_dot_continues_a_chain() {
    let module = parse(
        "fn f() {\n    val adults = users\n        .filter { it.age >= 18 }\n        .map { it.name }\n}\n",
    );
    let body = module.function("f").unwrap().body.as_ref().unwrap();
    assert_eq!(body.statements.len(), 1);
    let binding = body.statements[0].as_simple_binding().expect("a binding");
    assert_eq!(
        sexp(binding.value.unwrap()),
        "(call (. (call (. users filter) (lambda [] (>= (. it age) 18))) map) (lambda [] (. it name)))"
    );
}

#[test]
fn semicolons_separate_statements_on_one_line() {
    let module = parse("fn f() {\n    val a = 1; val b = 2\n}\n");
    let body = module.function("f").unwrap().body.as_ref().unwrap();
    assert_eq!(body.statements.len(), 2);
}

#[test]
fn rejects_two_statements_on_a_line_without_a_separator() {
    let errors = parse_errors("fn f() {\n    val a = 1 val b = 2\n}\n");
    assert!(errors.iter().any(|e| e.contains("expected the statement to end")), "{errors:?}");
}

// --- declarations --------------------------------------------------------

#[test]
fn parses_a_function_with_parameters_and_a_result() {
    let module = parse("fn add(a: Int, b: Int): Int {\n    return a + b\n}\n");
    let add = module.function("add").expect("`add`");
    assert_eq!(add.params.len(), 2);
    assert_eq!(add.params[0].name.name, "a");
    assert_eq!(add.params[0].ty.as_ref().unwrap().render(), "Int");
    assert_eq!(add.result.as_ref().unwrap().render(), "Int");
}

#[test]
fn parses_an_expression_bodied_function() {
    let module = parse("fn double(n: Int): Int = n * 2\n");
    let double = module.function("double").expect("`double`");
    let body = double.body.as_ref().expect("a body");
    assert_eq!(sexp(body.tail_expr().unwrap()), "(* n 2)");
}

#[test]
fn parses_default_and_named_parameters() {
    let module = parse("fn greet(name: String, greeting: String = \"Olá\") {}\n");
    let greet = module.function("greet").unwrap();
    assert!(greet.params[0].default.is_none());
    assert!(greet.params[1].default.is_some());
}

#[test]
fn rejects_a_required_parameter_after_a_defaulted_one() {
    let errors = parse_errors("fn f(a: Int = 1, b: Int) {}\n");
    assert!(errors.iter().any(|e| e.contains("must have a default value")), "{errors:?}");
}

#[test]
fn parses_an_extension_function() {
    let module = parse("fn String.isValidEmail(): Bool {\n    return true\n}\n");
    let function = module.functions().next().expect("a function");
    assert_eq!(function.name.name, "isValidEmail");
    assert_eq!(function.receiver.as_ref().unwrap().render(), "String");
}

#[test]
fn parses_a_class_with_a_primary_constructor() {
    let module = parse("class User(val name: String, var age: Int) {}\n");
    let ItemKind::TypeDecl(decl) = &module.items[0].kind else { panic!("expected a class") };
    assert_eq!(decl.class_kind, ClassKind::Class);
    assert_eq!(decl.primary_params.len(), 2);
    assert_eq!(decl.primary_params[0].kind, LetKind::Val);
    assert_eq!(decl.primary_params[1].kind, LetKind::Var);
}

#[test]
fn parses_a_data_class() {
    let module = parse("public data class Point(val x: Int, val y: Int)\n");
    let ItemKind::TypeDecl(decl) = &module.items[0].kind else { panic!("expected a class") };
    assert_eq!(decl.class_kind, ClassKind::DataClass);
    assert!(decl.class_kind.is_data());
    assert_eq!(module.items[0].modifiers.visibility, noto_ast::Visibility::Public);
}

#[test]
fn parses_inheritance_and_interfaces() {
    let module = parse("class Dog : Animal, Loud {\n    override fn speak() {\n        println(\"Au!\")\n    }\n}\n");
    let ItemKind::TypeDecl(decl) = &module.items[0].kind else { panic!("expected a class") };
    assert_eq!(decl.base.as_ref().unwrap().render(), "Animal");
    assert_eq!(decl.interfaces.len(), 1);
    assert_eq!(decl.methods.len(), 1);
    assert!(decl.methods[0].modifiers.is_override);
}

#[test]
fn parses_a_property_with_a_getter() {
    let module = parse(
        "class User {\n    var name: String = \"\"\n\n    val displayName: String {\n        get = name.uppercase()\n    }\n}\n",
    );
    let ItemKind::TypeDecl(decl) = &module.items[0].kind else { panic!("expected a class") };
    assert_eq!(decl.fields.len(), 1);
    assert_eq!(decl.properties.len(), 1);
    assert_eq!(decl.properties[0].name.name, "displayName");
    assert!(decl.properties[0].getter.is_some());
    assert!(decl.properties[0].setter.is_none());
}

#[test]
fn parses_an_interface() {
    let module = parse("interface Shape {\n    fn area(): Float64\n    val name: String\n}\n");
    let ItemKind::Interface(interface) = &module.items[0].kind else { panic!("expected an interface") };
    assert_eq!(interface.methods.len(), 1);
    assert_eq!(interface.properties.len(), 1);
    let ItemKind::Fn(method) = &interface.methods[0].kind else { panic!("expected a method") };
    assert!(method.body.is_none(), "an interface method without a body stays abstract");
}

#[test]
fn parses_a_simple_enum() {
    let module = parse("enum Color {\n    Red\n    Green\n    Blue\n}\n");
    let ItemKind::Enum(item) = &module.items[0].kind else { panic!("expected an enum") };
    let names: Vec<&str> = item.cases.iter().map(|c| c.name.name.as_str()).collect();
    assert_eq!(names, vec!["Red", "Green", "Blue"]);
}

#[test]
fn parses_an_enum_with_associated_data() {
    let module =
        parse("enum Outcome {\n    Success(value: Int)\n    Failure(message: String)\n}\n");
    let ItemKind::Enum(item) = &module.items[0].kind else { panic!("expected an enum") };
    assert_eq!(item.cases[0].fields.len(), 1);
    assert_eq!(item.cases[0].fields[0].name.name, "value");
    assert_eq!(item.cases[1].fields[0].ty.as_ref().unwrap().render(), "String");
}

#[test]
fn parses_generics() {
    let module = parse("fn firstOf<T>(items: List<T>): T? {\n    return null\n}\n");
    let function = module.function("firstOf").unwrap();
    assert_eq!(function.type_params.len(), 1);
    assert_eq!(function.params[0].ty.as_ref().unwrap().render(), "List<T>");
    assert_eq!(function.result.as_ref().unwrap().render(), "T?");
}

#[test]
fn parses_nested_generic_arguments() {
    // `>>` lexes as one token and has to be split to close both lists.
    let module = parse("fn f(x: Map<String, List<Int>>) {}\n");
    let function = module.function("f").unwrap();
    assert_eq!(function.params[0].ty.as_ref().unwrap().render(), "Map<String, List<Int>>");
}

#[test]
fn parses_generic_constraints() {
    let module = parse("fn largest<T: Comparable>(items: List<T>): T? = null\n");
    let function = module.function("largest").unwrap();
    assert_eq!(function.type_params[0].bounds.len(), 1);
    assert_eq!(function.type_params[0].bounds[0].render(), "Comparable");
}

#[test]
fn parses_constants_imports_and_tests() {
    let module = parse(
        "import std.io\nimport std.text { trim, split }\n\nconst MAX_USERS = 100\n\ntest \"soma dois números\" {\n    assert(add(2, 3) == 5)\n}\n",
    );
    assert_eq!(module.items.len(), 4);
    let ItemKind::Import(first) = &module.items[0].kind else { panic!("expected an import") };
    assert_eq!(first.path.to_dotted(), "std.io");
    let ItemKind::Import(second) = &module.items[1].kind else { panic!("expected an import") };
    assert_eq!(second.names.len(), 2);
    let ItemKind::Const(constant) = &module.items[2].kind else { panic!("expected a const") };
    assert_eq!(constant.name.name, "MAX_USERS");
    let test = module.tests().next().expect("a test");
    assert_eq!(test.name, "soma dois números");
}

#[test]
fn parses_attributes_and_doc_comments() {
    let module = parse("/// Adds two numbers.\n/// Returns their sum.\n@inline\nfn add(a: Int, b: Int): Int = a + b\n");
    let item = &module.items[0];
    assert_eq!(item.doc.as_deref(), Some("Adds two numbers.\nReturns their sum."));
    assert!(item.has_attribute("inline"));
}

// --- control flow --------------------------------------------------------

#[test]
fn parses_if_as_an_expression() {
    assert_eq!(expr_sexp("if a { 1 } else { 2 }"), "(if a 1 (block 2))");
    assert_eq!(expr_sexp("if a { 1 } else if b { 2 } else { 3 }"), "(if a 1 (if b 2 (block 3)))");
}

#[test]
fn an_if_condition_does_not_take_a_trailing_lambda() {
    // Without the restriction, `{ .. }` would be read as an argument to
    // `isActive` and the `if` would have no body.
    let module = parse("fn f(user: User) {\n    if user.isActive { println(\"yes\") }\n}\n");
    let body = module.function("f").unwrap().body.as_ref().unwrap();
    let StmtKind::Expr(expr) = &body.statements[0].kind else { panic!("expected an expression") };
    let ExprKind::If { condition, then_branch, .. } = &expr.kind else { panic!("expected an if") };
    assert_eq!(sexp(condition), "(. user isActive)");
    assert_eq!(then_branch.statements.len(), 1);
}

#[test]
fn parses_when_with_ranges_and_else() {
    let source = "when (age) {\n        0..12 -> println(\"Criança\")\n        13..17 -> println(\"Adolescente\")\n        else -> println(\"Adulto\")\n    }";
    assert_eq!(
        expr_sexp(source),
        r#"(when age [0..12 (call println (str "Criança"))] [13..17 (call println (str "Adolescente"))] [else (call println (str "Adulto"))])"#
    );
}

#[test]
fn a_when_arm_may_hold_a_block() {
    // After `->` a brace opens statements, not a lambda: an arm that does
    // several things is the common shape.
    let expr = parse_expr_source(
        "when (n) {\n        1 -> {\n            println(1)\n            println(2)\n        }\n        else -> {}\n    }",
    );
    let ExprKind::When { arms, .. } = &expr.kind else { panic!("a when") };
    assert_eq!(arms.len(), 2);
    let ExprKind::Block(block) = &arms[0].body.kind else {
        panic!("the first arm holds a block, not {:?}", arms[0].body.kind)
    };
    assert_eq!(block.statements.len(), 2);
    let ExprKind::Block(empty) = &arms[1].body.kind else { panic!("an empty block") };
    assert!(empty.statements.is_empty());
}

#[test]
fn a_when_arm_may_still_produce_a_lambda() {
    let expr = parse_expr_source("when (n) {\n        else -> { x -> x }\n    }");
    let ExprKind::When { arms, .. } = &expr.kind else { panic!("a when") };
    assert!(
        matches!(arms[0].body.kind, ExprKind::Lambda(_)),
        "braces holding a parameter list are a lambda, not a block: {:?}",
        arms[0].body.kind
    );
}

#[test]
fn parses_when_arms_with_several_patterns_and_guards() {
    let source = "when (value) {\n        0, 1 -> \"small\"\n        n if n < 0 -> \"negative\"\n        is String -> \"text\"\n        else -> \"big\"\n    }";
    let expr = parse_expr_source(source);
    let ExprKind::When { arms, .. } = &expr.kind else { panic!("expected a when") };
    assert_eq!(arms[0].patterns.len(), 2);
    assert!(arms[1].guard.is_some());
    assert!(matches!(arms[2].patterns[0].kind, PatternKind::Type(_)));
    assert!(arms[3].is_else);
}

#[test]
fn parses_when_over_enum_cases() {
    let source = "when (result) {\n        Outcome.Success(value) -> value\n        Outcome.Failure(message) -> 0\n    }";
    let expr = parse_expr_source(source);
    let ExprKind::When { arms, .. } = &expr.kind else { panic!("expected a when") };
    assert_eq!(pat(&arms[0].patterns[0]), "Outcome.Success(value)");
    assert_eq!(arms[0].patterns[0].bindings().len(), 1);
}

#[test]
fn parses_a_subjectless_when() {
    let source = "when {\n        a > b -> 1\n        else -> 2\n    }";
    let expr = parse_expr_source(source);
    let ExprKind::When { scrutinee, arms } = &expr.kind else { panic!("expected a when") };
    assert!(scrutinee.is_none());
    assert_eq!(arms.len(), 2);
}

#[test]
fn rejects_an_else_arm_that_is_not_last() {
    let errors = parse_errors("fn f(x: Int) {\n    when (x) {\n        else -> 1\n        2 -> 3\n    }\n}\n");
    assert!(errors.iter().any(|e| e.contains("`else` must be the last arm")), "{errors:?}");
}

#[test]
fn parses_loops() {
    let module = parse(
        "fn f() {\n    for i in 0..10 {\n        println(i)\n    }\n    while a < b {\n        a += 1\n    }\n    loop {\n        break\n    }\n}\n",
    );
    let body = module.function("f").unwrap().body.as_ref().unwrap();
    assert_eq!(body.statements.len(), 3);
    assert!(matches!(body.statements[0].kind, StmtKind::For { .. }));
    assert!(matches!(body.statements[1].kind, StmtKind::While { .. }));
    assert!(matches!(body.statements[2].kind, StmtKind::Loop { .. }));
}

#[test]
fn parses_defer() {
    let module = parse("fn f(file: File) {\n    defer file.close()\n}\n");
    let body = module.function("f").unwrap().body.as_ref().unwrap();
    assert!(matches!(body.statements[0].kind, StmtKind::Defer { .. }));
}

#[test]
fn rejects_a_defer_with_no_effect() {
    let errors = parse_errors("fn f(file: File) {\n    defer file\n}\n");
    assert!(errors.iter().any(|e| e.contains("`defer` needs an expression")), "{errors:?}");
}

// --- bindings and destructuring -----------------------------------------

#[test]
fn parses_val_and_var() {
    let module = parse("fn f() {\n    val name = \"João\"\n    var age = 16\n    val explicit: Int = 3\n}\n");
    let body = module.function("f").unwrap().body.as_ref().unwrap();
    let first = body.statements[0].as_simple_binding().unwrap();
    assert_eq!(first.kind, LetKind::Val);
    assert!(first.ty.is_none(), "the type is inferred when it is obvious");
    let second = body.statements[1].as_simple_binding().unwrap();
    assert_eq!(second.kind, LetKind::Var);
    let third = body.statements[2].as_simple_binding().unwrap();
    assert_eq!(third.ty.unwrap().render(), "Int");
}

#[test]
fn parses_tuple_destructuring() {
    let module = parse("fn f(user: User) {\n    val (name, age) = user\n}\n");
    let body = module.function("f").unwrap().body.as_ref().unwrap();
    let StmtKind::Let { pattern, .. } = &body.statements[0].kind else { panic!("expected a let") };
    assert_eq!(pat(pattern), "(name,age)");
    assert_eq!(pattern.bindings().len(), 2);
}

#[test]
fn rejects_a_binding_with_neither_type_nor_value() {
    let errors = parse_errors("fn f() {\n    val x\n}\n");
    assert!(errors.iter().any(|e| e.contains("needs either a type or an initial value")), "{errors:?}");
}

// --- literals ------------------------------------------------------------

#[test]
fn parses_string_interpolation_into_segments() {
    assert_eq!(expr_sexp(r#""Olá, $name!""#), r#"(str "Olá, " ${name} "!")"#);
    assert_eq!(expr_sexp(r#""soma: ${a + b}""#), r#"(str "soma: " ${(+ a b)})"#);
}

#[test]
fn parses_tuples_and_lists() {
    assert_eq!(expr_sexp("(10, 20)"), "(tuple 10 20)");
    assert_eq!(expr_sexp("[1, 2, 3]"), "(list 1 2 3)");
    assert_eq!(expr_sexp("(10)"), "10", "parentheses only group");
}

#[test]
fn parses_lambdas() {
    assert_eq!(expr_sexp("{ it * 2 }"), "(lambda [] (* it 2))");
    assert_eq!(expr_sexp("{ x -> x * 2 }"), "(lambda [x] (* x 2))");
    assert_eq!(expr_sexp("{ a, b -> a + b }"), "(lambda [a b] (+ a b))");
    assert_eq!(expr_sexp("fn(a: Int, b: Int) { a + b }"), "(lambda [a b] (+ a b))");
}

#[test]
fn parses_await() {
    let module = parse("async fn loadUser(): User {\n    val user = await fetch()\n    return user\n}\n");
    let function = module.function("loadUser").unwrap();
    assert!(function.is_async);
    let body = function.body.as_ref().unwrap();
    let binding = body.statements[0].as_simple_binding().unwrap();
    assert_eq!(sexp(binding.value.unwrap()), "(await (call fetch))");
}

// --- error recovery ------------------------------------------------------

#[test]
fn recovers_and_reports_several_errors_in_one_run() {
    let errors = parse_errors("fn a( {}\n\nfn b() {}\n\nfn c( {}\n");
    assert!(errors.len() >= 2, "expected several diagnostics, got {errors:?}");
}

#[test]
fn a_broken_declaration_does_not_hide_the_next_one() {
    let mut map = SourceMap::new();
    let file = map.add("test.noto", "fn 123() {}\n\nfn good() {}\n");
    let mut sink = DiagnosticSink::new();
    let module = parse_file(map.file(file).unwrap(), &mut sink);
    assert!(sink.has_errors());
    assert!(module.function("good").is_some(), "the valid declaration should still be parsed");
}

#[test]
fn parsing_always_terminates_on_garbage() {
    for source in ["}}}}", "fn", "((((", "val = = =", "when { ->", "class : : {"] {
        let mut map = SourceMap::new();
        let file = map.add("test.noto", source);
        let mut sink = DiagnosticSink::new();
        parse_file(map.file(file).unwrap(), &mut sink);
        assert!(sink.has_errors(), "expected a diagnostic for {source:?}");
    }
}

#[test]
fn reports_a_reserved_word_used_as_a_name() {
    let errors = parse_errors("fn match() {}\n");
    assert!(errors.iter().any(|e| e.contains("reserved word")), "{errors:?}");
}

// --- node ids ------------------------------------------------------------

#[test]
fn every_node_gets_a_distinct_id() {
    use noto_ast::visit::{self, Visitor};

    #[derive(Default)]
    struct Collect {
        ids: Vec<noto_ast::NodeId>,
    }
    impl Visitor for Collect {
        fn visit_expr(&mut self, expr: &Expr) {
            self.ids.push(expr.id);
            visit::walk_expr(self, expr);
        }
        fn visit_stmt(&mut self, stmt: &noto_ast::Stmt) {
            self.ids.push(stmt.id);
            visit::walk_stmt(self, stmt);
        }
    }

    let module = parse(
        "fn f(a: Int): Int {\n    val b = a + 1\n    if b > 2 {\n        return b\n    }\n    return a\n}\n",
    );
    let mut collect = Collect::default();
    collect.visit_module(&module);

    let mut sorted = collect.ids.clone();
    sorted.sort();
    sorted.dedup();
    assert_eq!(sorted.len(), collect.ids.len(), "node ids must be unique");
    assert!(collect.ids.len() > 8);
}
