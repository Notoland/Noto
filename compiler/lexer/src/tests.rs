//! Lexer tests.

use super::*;
use noto_span::SourceMap;

/// Lexes `source`, asserting that no diagnostic was produced.
fn lex(source: &str) -> Vec<Token> {
    let mut map = SourceMap::new();
    let file = map.add("test.noto", source);
    let mut sink = DiagnosticSink::new();
    let tokens = tokenize(map.file(file).unwrap(), &mut sink);
    assert!(
        !sink.has_errors(),
        "unexpected diagnostics for {source:?}:\n{}",
        sink.render_all(&map, noto_diagnostics::RenderStyle::Plain)
    );
    tokens
}

/// Lexes `source` and returns the token kinds plus every diagnostic message.
fn lex_with_errors(source: &str) -> (Vec<TokenKind>, Vec<String>) {
    let mut map = SourceMap::new();
    let file = map.add("test.noto", source);
    let mut sink = DiagnosticSink::new();
    let tokens = tokenize(map.file(file).unwrap(), &mut sink);
    let messages = sink.diagnostics().iter().map(|d| d.message.clone()).collect();
    (tokens.into_iter().map(|t| t.kind).collect(), messages)
}

fn kinds(source: &str) -> Vec<TokenKind> {
    lex(source).into_iter().map(|t| t.kind).collect()
}

fn ident(name: &str) -> TokenKind {
    TokenKind::Ident(name.to_string())
}

fn int(value: u128) -> TokenKind {
    TokenKind::Int(IntLiteral { value, base: Base::Decimal, suffix: None })
}

fn text(value: &str) -> TokenKind {
    TokenKind::Str(StringLiteral {
        parts: vec![StringPart::Text(value.to_string())],
        multiline: false,
    })
}

// --- basics -------------------------------------------------------------

#[test]
fn empty_input_is_just_eof() {
    assert_eq!(kinds(""), vec![TokenKind::Eof]);
}

#[test]
fn whitespace_only_input_is_just_eof() {
    assert_eq!(kinds("   \n\t  \r\n "), vec![TokenKind::Eof]);
}

#[test]
fn lexes_the_hello_world_program() {
    let source = "fn main() {\n    println(\"Hello, Noto!\")\n}\n";
    assert_eq!(
        kinds(source),
        vec![
            TokenKind::Keyword(Keyword::Fn),
            ident("main"),
            TokenKind::LParen,
            TokenKind::RParen,
            TokenKind::LBrace,
            ident("println"),
            TokenKind::LParen,
            text("Hello, Noto!"),
            TokenKind::RParen,
            TokenKind::RBrace,
            TokenKind::Eof,
        ]
    );
}

#[test]
fn spans_cover_exactly_the_token_text() {
    let source = "val answer = 42";
    let tokens = lex(source);
    let slices: Vec<&str> = tokens
        .iter()
        .filter(|t| !t.is_eof())
        .map(|t| &source[t.span.start as usize..t.span.end as usize])
        .collect();
    assert_eq!(slices, vec!["val", "answer", "=", "42"]);
}

// --- identifiers and keywords -------------------------------------------

#[test]
fn distinguishes_keywords_from_identifiers() {
    assert_eq!(
        kinds("val value var variable fn fnName"),
        vec![
            TokenKind::Keyword(Keyword::Val),
            ident("value"),
            TokenKind::Keyword(Keyword::Var),
            ident("variable"),
            TokenKind::Keyword(Keyword::Fn),
            ident("fnName"),
            TokenKind::Eof,
        ]
    );
}

#[test]
fn identifiers_may_be_unicode() {
    assert_eq!(kinds("usuário 名前 _private a1"), vec![
        ident("usuário"),
        ident("名前"),
        ident("_private"),
        ident("a1"),
        TokenKind::Eof
    ]);
}

#[test]
fn a_lone_underscore_is_the_wildcard() {
    assert_eq!(kinds("_ _x"), vec![TokenKind::Underscore, ident("_x"), TokenKind::Eof]);
}

// --- numbers -------------------------------------------------------------

#[test]
fn lexes_integers_in_every_base() {
    let ks = kinds("42 0xFF 0b1010 0o755");
    assert_eq!(
        ks,
        vec![
            TokenKind::Int(IntLiteral { value: 42, base: Base::Decimal, suffix: None }),
            TokenKind::Int(IntLiteral { value: 255, base: Base::Hex, suffix: None }),
            TokenKind::Int(IntLiteral { value: 10, base: Base::Binary, suffix: None }),
            TokenKind::Int(IntLiteral { value: 493, base: Base::Octal, suffix: None }),
            TokenKind::Eof,
        ]
    );
}

#[test]
fn underscores_separate_digits() {
    assert_eq!(kinds("1_000_000"), vec![int(1_000_000), TokenKind::Eof]);
    assert_eq!(
        kinds("0xDEAD_BEEF"),
        vec![
            TokenKind::Int(IntLiteral { value: 0xDEAD_BEEF, base: Base::Hex, suffix: None }),
            TokenKind::Eof
        ]
    );
}

#[test]
fn lexes_numeric_suffixes() {
    assert_eq!(
        kinds("10u8 7i32 1.5f32"),
        vec![
            TokenKind::Int(IntLiteral {
                value: 10,
                base: Base::Decimal,
                suffix: Some(NumericSuffix::U8)
            }),
            TokenKind::Int(IntLiteral {
                value: 7,
                base: Base::Decimal,
                suffix: Some(NumericSuffix::I32)
            }),
            TokenKind::Float(FloatLiteral { value: 1.5, suffix: Some(NumericSuffix::F32) }),
            TokenKind::Eof,
        ]
    );
}

#[test]
fn lexes_floats_with_exponents() {
    assert_eq!(
        kinds("3.14 1e9 2.5e-3"),
        vec![
            TokenKind::Float(FloatLiteral { value: 3.14, suffix: None }),
            TokenKind::Float(FloatLiteral { value: 1e9, suffix: None }),
            TokenKind::Float(FloatLiteral { value: 2.5e-3, suffix: None }),
            TokenKind::Eof,
        ]
    );
}

#[test]
fn a_range_is_not_a_float() {
    // `1..10` must lex as three tokens, never as `1.` `.10`.
    assert_eq!(kinds("1..10"), vec![int(1), TokenKind::DotDot, int(10), TokenKind::Eof]);
    assert_eq!(kinds("0..=9"), vec![int(0), TokenKind::DotDotEq, int(9), TokenKind::Eof]);
}

#[test]
fn a_method_call_on_an_integer_is_not_a_float() {
    assert_eq!(
        kinds("10.toFloat()"),
        vec![int(10), TokenKind::Dot, ident("toFloat"), TokenKind::LParen, TokenKind::RParen, TokenKind::Eof]
    );
}

#[test]
fn rejects_an_unknown_numeric_suffix() {
    let (_, errors) = lex_with_errors("10xyz");
    assert_eq!(errors.len(), 1);
    assert!(errors[0].contains("not a numeric type suffix"), "{errors:?}");
}

#[test]
fn rejects_an_integer_that_is_too_large() {
    let (_, errors) = lex_with_errors("999999999999999999999999999999999999999999");
    assert_eq!(errors, vec!["integer literal is too large to represent"]);
}

#[test]
fn rejects_a_base_prefix_without_digits() {
    let (_, errors) = lex_with_errors("0x");
    assert_eq!(errors, vec!["malformed numeric literal"]);
}

// --- strings -------------------------------------------------------------

#[test]
fn lexes_escape_sequences() {
    assert_eq!(kinds(r#""a\nb\tc\\d\"e\$f""#), vec![text("a\nb\tc\\d\"e$f"), TokenKind::Eof]);
}

#[test]
fn lexes_unicode_escapes() {
    assert_eq!(kinds(r#""\u{1F600}\u{4A}""#), vec![text("\u{1F600}J"), TokenKind::Eof]);
}

#[test]
fn lexes_shorthand_interpolation() {
    let ks = kinds(r#""Olá, $name!""#);
    let TokenKind::Str(literal) = &ks[0] else { panic!("expected a string, got {:?}", ks[0]) };
    assert!(literal.is_interpolated());
    assert_eq!(literal.parts.len(), 3);
    assert_eq!(literal.parts[0], StringPart::Text("Olá, ".to_string()));
    let StringPart::Interpolation { tokens, .. } = &literal.parts[1] else {
        panic!("expected an interpolation")
    };
    assert_eq!(tokens[0].kind, ident("name"));
    assert_eq!(literal.parts[2], StringPart::Text("!".to_string()));
}

#[test]
fn lexes_braced_interpolation_of_an_expression() {
    let ks = kinds(r#""total: ${a + b * 2}""#);
    let TokenKind::Str(literal) = &ks[0] else { panic!("expected a string") };
    let StringPart::Interpolation { tokens, .. } = &literal.parts[1] else {
        panic!("expected an interpolation")
    };
    let inner: Vec<TokenKind> = tokens.iter().map(|t| t.kind.clone()).collect();
    assert_eq!(
        inner,
        vec![ident("a"), TokenKind::Plus, ident("b"), TokenKind::Star, int(2), TokenKind::Eof]
    );
}

#[test]
fn interpolation_spans_point_into_the_outer_file() {
    let source = r#"val s = "x${name}""#;
    let tokens = lex(source);
    let TokenKind::Str(literal) = &tokens[3].kind else { panic!("expected a string") };
    let StringPart::Interpolation { tokens: inner, .. } = &literal.parts[1] else {
        panic!("expected an interpolation")
    };
    let span = inner[0].span;
    assert_eq!(&source[span.start as usize..span.end as usize], "name");
}

#[test]
fn a_dollar_before_a_non_identifier_is_literal_text() {
    assert_eq!(kinds(r#""price: $5, 100$""#), vec![text("price: $5, 100$"), TokenKind::Eof]);
}

#[test]
fn lexes_a_multiline_literal_and_strips_indentation() {
    let source = "val s = \"\"\"\n    line one\n    line two\n    \"\"\"\n";
    let tokens = lex(source);
    let TokenKind::Str(literal) = &tokens[3].kind else { panic!("expected a string") };
    assert!(literal.multiline);
    assert_eq!(literal.as_plain_text(), Some("line one\nline two"));
}

#[test]
fn a_multiline_literal_keeps_relative_indentation() {
    let source = "\"\"\"\n    fn main() {\n        body\n    }\n    \"\"\"";
    let tokens = lex(source);
    let TokenKind::Str(literal) = &tokens[0].kind else { panic!("expected a string") };
    assert_eq!(literal.as_plain_text(), Some("fn main() {\n    body\n}"));
}

#[test]
fn rejects_a_string_that_crosses_a_line_break() {
    let (_, errors) = lex_with_errors("\"open\nval x = 1\n");
    assert_eq!(errors, vec!["string literal is never closed"]);
}

#[test]
fn rejects_an_unterminated_string_at_end_of_file() {
    let (_, errors) = lex_with_errors("\"open");
    assert_eq!(errors, vec!["string literal is never closed"]);
}

#[test]
fn rejects_an_unknown_escape() {
    let (_, errors) = lex_with_errors(r#""a\qb""#);
    assert_eq!(errors.len(), 1);
    assert!(errors[0].contains("unknown escape sequence"), "{errors:?}");
}

#[test]
fn rejects_an_out_of_range_unicode_escape() {
    let (_, errors) = lex_with_errors(r#""\u{110000}""#);
    assert_eq!(errors.len(), 1);
    assert!(errors[0].contains("not a Unicode scalar value"), "{errors:?}");
}

#[test]
fn rejects_an_unclosed_interpolation() {
    let (_, errors) = lex_with_errors(r#""${a + b"#);
    assert!(
        errors.iter().any(|e| e.contains("interpolated expression is never closed")),
        "{errors:?}"
    );
}

// --- characters ----------------------------------------------------------

#[test]
fn lexes_character_literals() {
    assert_eq!(
        kinds(r"'a' '\n' 'ã' '\u{1F600}'"),
        vec![
            TokenKind::Char('a'),
            TokenKind::Char('\n'),
            TokenKind::Char('ã'),
            TokenKind::Char('\u{1F600}'),
            TokenKind::Eof,
        ]
    );
}

#[test]
fn rejects_a_multi_character_literal() {
    let (_, errors) = lex_with_errors("'ab'");
    assert_eq!(errors, vec!["character literal holds more than one character"]);
}

#[test]
fn rejects_an_empty_character_literal() {
    let (_, errors) = lex_with_errors("''");
    assert_eq!(errors, vec!["character literal is empty"]);
}

// --- operators -----------------------------------------------------------

#[test]
fn lexes_every_operator() {
    let source = "+ - * / % = == != < <= > >= ! && || & | ^ ~ << >> \
                  += -= *= /= %= &= |= ^= <<= >>= ? ?. ?: -> => .. ..= :: : . , ; @ _";
    assert_eq!(
        kinds(source),
        vec![
            TokenKind::Plus,
            TokenKind::Minus,
            TokenKind::Star,
            TokenKind::Slash,
            TokenKind::Percent,
            TokenKind::Eq,
            TokenKind::EqEq,
            TokenKind::BangEq,
            TokenKind::Lt,
            TokenKind::LtEq,
            TokenKind::Gt,
            TokenKind::GtEq,
            TokenKind::Bang,
            TokenKind::AmpAmp,
            TokenKind::PipePipe,
            TokenKind::Amp,
            TokenKind::Pipe,
            TokenKind::Caret,
            TokenKind::Tilde,
            TokenKind::Shl,
            TokenKind::Shr,
            TokenKind::PlusEq,
            TokenKind::MinusEq,
            TokenKind::StarEq,
            TokenKind::SlashEq,
            TokenKind::PercentEq,
            TokenKind::AmpEq,
            TokenKind::PipeEq,
            TokenKind::CaretEq,
            TokenKind::ShlEq,
            TokenKind::ShrEq,
            TokenKind::Question,
            TokenKind::QuestionDot,
            TokenKind::Elvis,
            TokenKind::Arrow,
            TokenKind::FatArrow,
            TokenKind::DotDot,
            TokenKind::DotDotEq,
            TokenKind::ColonColon,
            TokenKind::Colon,
            TokenKind::Dot,
            TokenKind::Comma,
            TokenKind::Semicolon,
            TokenKind::At,
            TokenKind::Underscore,
            TokenKind::Eof,
        ]
    );
}

#[test]
fn prefers_the_longest_operator() {
    assert_eq!(kinds("a?.b"), vec![ident("a"), TokenKind::QuestionDot, ident("b"), TokenKind::Eof]);
    assert_eq!(kinds("a ?: b"), vec![ident("a"), TokenKind::Elvis, ident("b"), TokenKind::Eof]);
    assert_eq!(kinds("x>>=1"), vec![ident("x"), TokenKind::ShrEq, int(1), TokenKind::Eof]);
}

// --- comments ------------------------------------------------------------

#[test]
fn skips_line_and_block_comments() {
    let source = "val a = 1 // trailing\n/* block\n   comment */ val b = 2";
    assert_eq!(
        kinds(source),
        vec![
            TokenKind::Keyword(Keyword::Val),
            ident("a"),
            TokenKind::Eq,
            int(1),
            TokenKind::Keyword(Keyword::Val),
            ident("b"),
            TokenKind::Eq,
            int(2),
            TokenKind::Eof,
        ]
    );
}

#[test]
fn block_comments_nest() {
    assert_eq!(kinds("/* outer /* inner */ still */ 1"), vec![int(1), TokenKind::Eof]);
}

#[test]
fn rejects_an_unterminated_block_comment() {
    let (_, errors) = lex_with_errors("/* never closed");
    assert_eq!(errors, vec!["block comment is never closed"]);
}

#[test]
fn doc_comments_survive_as_tokens() {
    let ks = kinds("/// Adds two numbers.\nfn add() {}");
    assert_eq!(ks[0], TokenKind::DocComment("Adds two numbers.".to_string()));
    assert_eq!(ks[1], TokenKind::Keyword(Keyword::Fn));
}

// --- newline tracking ----------------------------------------------------

#[test]
fn records_which_tokens_follow_a_line_break() {
    let tokens = lex("val a = 1\nval b = 2");
    let flags: Vec<bool> = tokens.iter().map(|t| t.newline_before).collect();
    //              val    a      =      1      val   b      =      2      eof
    assert_eq!(flags, vec![false, false, false, false, true, false, false, false, false]);
}

#[test]
fn a_line_break_inside_a_comment_still_counts() {
    let tokens = lex("val a = 1 /* x\ny */ val b = 2");
    let val_b = tokens.iter().find(|t| t.keyword() == Some(Keyword::Val) && t.newline_before);
    assert!(val_b.is_some(), "the second `val` should be marked as following a line break");
}

#[test]
fn statement_ending_tokens_are_classified() {
    assert!(TokenKind::RParen.can_end_statement());
    assert!(int(1).can_end_statement());
    assert!(ident("x").can_end_statement());
    assert!(TokenKind::Keyword(Keyword::Return).can_end_statement());
    assert!(!TokenKind::Plus.can_end_statement());
    assert!(!TokenKind::LBrace.can_end_statement());
    assert!(!TokenKind::Comma.can_end_statement());
}

// --- resilience ----------------------------------------------------------

#[test]
fn keeps_lexing_after_an_invalid_character() {
    let (ks, errors) = lex_with_errors("val a = 1 # val b = 2");
    assert_eq!(errors.len(), 1, "{errors:?}");
    // The stray `#` is dropped but the rest of the file still lexes, so the
    // user sees every error in one run rather than one per rebuild.
    assert!(ks.contains(&ident("b")));
    assert_eq!(ks.last(), Some(&TokenKind::Eof));
}

#[test]
fn reports_several_independent_errors_in_one_pass() {
    let (_, errors) = lex_with_errors("val a = 10zz\nval b = 'xy'\n");
    assert_eq!(errors.len(), 2, "{errors:?}");
}
