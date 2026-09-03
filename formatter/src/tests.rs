use super::*;
use noto_diagnostics::DiagnosticSink;
use noto_span::SourceMap;

/// Formats a snippet, asserting that lexing it reported nothing.
fn fmt(source: &str) -> String {
    let mut map = SourceMap::new();
    let file = map.add("fmt.noto", source);
    let mut sink = DiagnosticSink::new();
    let formatted = format(map.file(file).unwrap(), &mut sink).expect("lexes cleanly");
    assert!(!sink.has_errors(), "{:?}", sink.diagnostics());
    formatted
}

fn lex(source: &str) -> Vec<noto_lexer::Token> {
    let mut map = SourceMap::new();
    let file = map.add("fmt.noto", source);
    let mut sink = DiagnosticSink::new();
    noto_lexer::tokenize(map.file(file).unwrap(), &mut sink)
}

/// Every sample the invariants are checked against.
fn corpus() -> Vec<&'static str> {
    vec![
        "fn main() {\n    println(\"Hello, Noto!\")\n}\n",
        "fn add(a: Int, b: Int): Int { return a + b }\n",
        "fn classify(age: Int): String = when (age) {\n    0..12 -> \"a\"\n    13..17 -> \"b\"\n    else -> \"c\"\n}\n",
        "fn main() {\n    var total = 0\n    for i in 1..=10 { total += i }\n    println(\"$total\")\n}\n",
        "// a leading comment\nfn main() {\n    val n = 1 // trailing\n    /* block */\n    println(n)\n}\n",
        "fn main() {\n    val maybe: String? = null\n    println(maybe ?: \"none\")\n}\n",
        "fn main() {\n    val n = -1\n    println(n - 1)\n}\n",
        "/// Doc comment.\nfn documented() {\n}\n",
        "const LIMIT: Int = 10\n\nfn main() {\n    println(LIMIT)\n}\n",
        "fn main() {\n    val total = 1 +\n        2\n    println(total)\n}\n",
        "test \"it works\" {\n    assert(true)\n}\n",
    ]
}

#[test]
fn formatting_never_changes_the_token_stream() {
    for source in corpus() {
        let formatted = fmt(source);
        assert!(
            tokens_match(&lex(source), &lex(&formatted)),
            "the token stream changed:\n--- before\n{source}\n--- after\n{formatted}"
        );
    }
}

#[test]
fn formatting_is_idempotent() {
    for source in corpus() {
        let once = fmt(source);
        let twice = fmt(&once);
        assert_eq!(once, twice, "formatting {source:?} twice differs");
    }
}

#[test]
fn an_already_formatted_file_is_left_alone() {
    for source in corpus() {
        let formatted = fmt(source);
        assert_eq!(fmt(&formatted), formatted);
    }
}

#[test]
fn a_block_is_indented_by_four_spaces() {
    assert_eq!(
        fmt("fn main() {\nprintln(1)\n}\n"),
        "fn main() {\n    println(1)\n}\n"
    );
}

#[test]
fn nesting_indents_once_per_level() {
    assert_eq!(
        fmt("fn main() {\nif true {\nprintln(1)\n}\n}\n"),
        "fn main() {\n    if true {\n        println(1)\n    }\n}\n"
    );
}

#[test]
fn a_closing_bracket_sits_at_the_level_that_opened_it() {
    assert_eq!(
        fmt("fn main() {\nprintln(\n\"one\",\n)\n}\n"),
        "fn main() {\n    println(\n        \"one\",\n    )\n}\n"
    );
}

#[test]
fn a_trailing_operator_indents_the_line_it_continues() {
    assert_eq!(
        fmt("fn main() {\nval n = 1 +\n2\n}\n"),
        "fn main() {\n    val n = 1 +\n        2\n}\n"
    );
}

#[test]
fn a_chain_of_continuations_indents_once_not_once_each() {
    assert_eq!(
        fmt("fn main() {\nval n = value\n.toString()\n.length\n}\n"),
        "fn main() {\n    val n = value\n        .toString()\n        .length\n}\n"
    );
}

#[test]
fn spacing_inside_a_call_and_an_index() {
    assert_eq!(fmt("fn main() { println( items [ 0 ] ) }\n"), "fn main() { println(items[0]) }\n");
}

#[test]
fn a_grouped_expression_keeps_the_space_before_its_paren() {
    assert_eq!(fmt("fn main() { val n = (1 + 2)*3 }\n"), "fn main() { val n = (1 + 2) * 3 }\n");
}

#[test]
fn a_keyword_keeps_the_space_before_its_paren() {
    assert_eq!(
        fmt("fn f(age: Int): Int = when(age) {\nelse -> 0\n}\n"),
        "fn f(age: Int): Int = when (age) {\n    else -> 0\n}\n"
    );
}

#[test]
fn a_sign_takes_no_space_but_a_subtraction_does() {
    assert_eq!(fmt("fn main() { val n = - 1 }\n"), "fn main() { val n = -1 }\n");
    assert_eq!(fmt("fn main() { val n = 3-1 }\n"), "fn main() { val n = 3 - 1 }\n");
    assert_eq!(fmt("fn main() { f(-1, 2) }\n"), "fn main() { f(-1, 2) }\n");
}

#[test]
fn a_range_is_written_without_spaces() {
    assert_eq!(
        fmt("fn main() { for i in 1 ..= 10 { println(i) } }\n"),
        "fn main() { for i in 1..=10 { println(i) } }\n"
    );
}

#[test]
fn a_colon_takes_a_space_after_it_and_none_before() {
    assert_eq!(fmt("fn f(a : Int) : Int = a\n"), "fn f(a: Int): Int = a\n");
}

#[test]
fn a_nullable_marker_takes_no_space_before_it() {
    assert_eq!(
        fmt("fn main() { val v : String ? = null }\n"),
        "fn main() { val v: String? = null }\n"
    );
}

#[test]
fn alignment_is_removed() {
    assert_eq!(
        fmt("fn f(n: Int): Int = when (n) {\n    0..12    -> 1\n    else     -> 2\n}\n"),
        "fn f(n: Int): Int = when (n) {\n    0..12 -> 1\n    else -> 2\n}\n"
    );
}

#[test]
fn several_blank_lines_collapse_to_one() {
    assert_eq!(
        fmt("fn a() {}\n\n\n\nfn b() {}\n"),
        "fn a() {}\n\nfn b() {}\n"
    );
}

#[test]
fn a_blank_line_against_a_brace_is_removed() {
    assert_eq!(
        fmt("fn main() {\n\n    println(1)\n\n}\n"),
        "fn main() {\n    println(1)\n}\n"
    );
}

#[test]
fn leading_and_trailing_blank_lines_are_removed() {
    assert_eq!(fmt("\n\nfn main() {}\n\n\n"), "fn main() {}\n");
}

#[test]
fn a_file_ends_with_exactly_one_newline() {
    assert_eq!(fmt("fn main() {}"), "fn main() {}\n");
    assert_eq!(fmt("fn main() {}\n\n\n"), "fn main() {}\n");
}

#[test]
fn trailing_whitespace_is_removed() {
    assert_eq!(fmt("fn main() {   \n    println(1)   \n}\n"), "fn main() {\n    println(1)\n}\n");
}

#[test]
fn a_comment_on_its_own_line_is_indented_with_the_code() {
    assert_eq!(
        fmt("fn main() {\n// why\nprintln(1)\n}\n"),
        "fn main() {\n    // why\n    println(1)\n}\n"
    );
}

#[test]
fn a_trailing_comment_keeps_one_space_before_it() {
    assert_eq!(
        fmt("fn main() {\n    println(1)      // why\n}\n"),
        "fn main() {\n    println(1) // why\n}\n"
    );
}

#[test]
fn a_comment_is_never_rewritten() {
    let source = "fn main() {\n    //   ascii   art   here\n    println(1)\n}\n";
    assert!(fmt(source).contains("//   ascii   art   here"));
}

#[test]
fn a_block_comment_survives() {
    assert_eq!(
        fmt("fn main() {\n/* two\n   lines */\nprintln(1)\n}\n"),
        "fn main() {\n    /* two\n   lines */\n    println(1)\n}\n"
    );
}

#[test]
fn the_last_comment_in_a_file_is_kept() {
    assert_eq!(fmt("fn main() {}\n// the end\n"), "fn main() {}\n// the end\n");
}

#[test]
fn a_doc_comment_is_kept_above_what_it_documents() {
    assert_eq!(
        fmt("/// Adds.\nfn add(a: Int, b: Int): Int = a + b\n"),
        "/// Adds.\nfn add(a: Int, b: Int): Int = a + b\n"
    );
}

#[test]
fn a_file_that_does_not_lex_is_left_alone() {
    let mut map = SourceMap::new();
    let file = map.add("broken.noto", "fn main() { val s = \"unterminated\n}\n");
    let mut sink = DiagnosticSink::new();
    assert!(format(map.file(file).unwrap(), &mut sink).is_none());
    assert!(sink.has_errors());
}

#[test]
fn a_file_that_lexes_but_does_not_parse_is_still_formatted() {
    // `val = 1` is a syntax error, but indentation and spacing are lexical
    // and an editor asks for formatting exactly when the file is mid-edit.
    assert_eq!(fmt("fn main() {\nval = 1\n}\n"), "fn main() {\n    val = 1\n}\n");
}

#[test]
fn an_empty_block_is_written_tight() {
    assert_eq!(fmt("fn main() { }\n"), "fn main() {}\n");
    assert_eq!(fmt("fn main() {\n}\n"), "fn main() {\n}\n");
}

#[test]
fn is_formatted_answers_the_check_command() {
    let mut map = SourceMap::new();
    let tidy = map.add("tidy.noto", "fn main() {\n    println(1)\n}\n");
    let messy = map.add("messy.noto", "fn main() {\nprintln(1)\n}\n");
    let mut sink = DiagnosticSink::new();
    assert_eq!(is_formatted(map.file(tidy).unwrap(), &mut sink), Some(true));
    assert_eq!(is_formatted(map.file(messy).unwrap(), &mut sink), Some(false));
}
