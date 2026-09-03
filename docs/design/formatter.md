# The formatter

`noto fmt` is deterministic and opinionated: one input has one formatting, and
there are no options. What follows is the whole rule set. It is implemented in
`noto-formatter` and every rule below has a test.

## What the formatter is allowed to change

Whitespace, and nothing else.

> **The invariant: formatting never changes the token stream.**
> Lexing the formatted text produces exactly the sequence of tokens that
> lexing the original produced, including `newline_before` on every token.
> There is a test that asserts this over the whole corpus.

That is a stronger promise than "the program still means the same thing", and
it is deliberate. A formatter is run without reading its output — on save, in
a pre-commit hook, over a whole tree — so it has to be impossible for it to
change a program. Comparing token streams is a check the formatter can make
about itself; "means the same thing" is not.

It also settles what `noto fmt` will not do. It will not delete a `;` that
ends a line, even though a line break already terminates the statement, and it
will not add or remove parentheses. Both change the token stream.

## Line breaks belong to the author

The formatter never moves code between lines. It does not wrap a long line and
it does not join two short ones.

This is not laziness, it is the language. Noto has no statement terminator: a
line break ends a statement when the statement is complete, an operator at the
end of a line continues the expression, and a line beginning with `.` or `?.`
continues the previous one. A formatter that re-flowed lines would be deciding
where statements end — which is to say, it would be rewriting the program and
then hoping the parser agreed. Re-flowing needs its own design and its own
RFC; until then the author decides where the lines are and the formatter
decides everything else about them.

The rules below are therefore all *within* a line, or about the empty space
between lines.

## Indentation

Four spaces per level. Never tabs.

A line's level is the bracket depth it starts at — `(`, `[` and `{` each open
one level and their partners close it. A line that *begins* with a closing
bracket is printed at the level of the line that opened it:

```noto
fn main() {
    println(
        "one argument on its own line",
    )
}
```

A continuation line gets one extra level. A line is a continuation when the
line before it ends with an operator, or when it begins with `.` or `?.`:

```noto
val total = first +
    second

val name = user
    .profile
    .displayName
```

The extra level is added once, not once per continuation line, so a chain of
five calls stays at one indent rather than marching to the right.

## Spacing inside a line

One space between tokens, except where a rule below says otherwise. Never two.
Never a space at the end of a line.

**Never a space before:** `)` `]` `,` `;` `:` `.` `?.` `::` `..` `..=` `?`

**Never a space after:** `(` `[` `.` `?.` `::` `..` `..=` `@` `!` `~` and a
unary `-` or `+`

`(` and `[` additionally take no space *before* them when they follow a name,
a `)` or a `]` — that is a call or an index. After anything else they are a
grouped expression or a list literal and take their space:

```noto
println(items[0])
val grouped = (a + b) * c
when (age) { else -> 0 }
```

An empty pair of brackets is written tight — `{}`, `()`, `[]` — on the one
line. A `{` that opens a block still takes a space before it and, when
something follows on the same line, after it.

A `-` is unary when nothing precedes it on the line or the token before it is
an operator, an opening bracket, a comma, a colon, an arrow or a keyword.
Otherwise it is binary and takes spaces on both sides.

Ranges bind tightly and are written without spaces — `0..12`, `1..=10` — which
is how they read in a `when` arm. Every other binary operator, `?:` included,
takes one space on each side.

Alignment is removed. Two `when` arms whose `->` were lined up with extra
spaces come back with one space each; the column an arm lands in is not
information, and keeping it makes every edit a re-alignment.

## Blank lines

At most one in a row. None at the start of the file, none directly after a
`{`, none directly before a `}`, and none at the end — the file ends with
exactly one newline after the last line of code.

Everywhere else a blank line is the author's paragraph break and is kept.

## Comments

Comments are preserved exactly as written, including their internal spacing
and any ASCII art inside them. The formatter only decides where a comment sits:

- a comment that was alone on its line stays alone on its line, indented to
  that line's level;
- a comment that followed code on the same line stays there, separated from
  the code by exactly one space.

The lexer discards `//` and `/* */` as trivia — only `///` doc comments reach
the parser — so the formatter recovers comments from the source text between
consecutive token spans. That is also why the formatter works on the token
stream rather than on the AST: an AST printer would silently delete every
ordinary comment in the file.

## What the formatter refuses

A file whose lexing reports an error is left alone and the error is reported.
The formatter needs a trustworthy token stream to make its promise about the
token stream, and an unterminated string means it does not have one.

A file that lexes but does not parse *is* formatted. Indentation and spacing
are lexical, and a syntax error is exactly when an editor is most likely to
ask for formatting.
