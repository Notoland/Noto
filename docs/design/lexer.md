# Lexer design

The lexer is `noto-lexer`: source text in, tokens out, every token carrying
its span. Comments are not discarded into the void — they are preserved, which
is what a formatter will need to be lossless.

## Statement termination

Noto has **no statement terminator**. The rule, implemented in the lexer's
newline handling and consumed by the parser:

- A line break ends a statement **once it is complete.** The parser, not the
  lexer, decides completeness — the lexer just classifies newlines as
  significant or trivia depending on context.
- An operator left dangling at the **end** of a line continues the expression
  onto the next line: the newline after `+` is not a terminator.
- A line **starting with** `.` or `?.` continues the previous expression.
  This is what makes call chains work without any trailing marker:

  ```noto
  val adults = users
      .filter { it.age >= 18 }
      .map { it.name }
  ```

- `;` still separates statements that share a line, for the cases where
  someone wants two things adjacent.

The consequence worth knowing: inside parentheses, brackets and braces,
newlines are never significant — grouping is explicit there, so the
line-based rule does not apply.

## Reserved words

The keyword set is small and fixed (`fn`, `val`, `var`, `if`, `when`, `for`,
…). Beyond it, a list of words is **reserved for the future** so that later
versions can adopt them without breaking 0.1 programs:

`actor`, `impl`, `macro`, `match`, `module`, `mut`, `operator`, `package`,
`static`, `trait`, `type`, `typealias`, `use`, `where`, `yield`

Using one as an identifier is a lex error today, which is the point: code
that compiles now keeps meaning the same thing forever. The list lives in
`RESERVED_FOR_FUTURE` in `compiler/lexer/src/keyword.rs`.

## String interpolation

A double-quoted string may contain `$name` or `${expr}`. The lexer does not
try to evaluate or even parse the embedded expression — it cuts the string
into literal segments and interpolation segments, and the parser recurses on
each `${…}` as a full expression. `$` followed by neither an identifier
start nor `{` is an invalid escape; an unclosed `${` is
`NOTO0107` (unterminated interpolation).

Interpolation is why `String` concatenation and `.toString()` exist as
language-level conveniences: every interpolated piece must render, and the
compiler knows how for the scalar types.

## Numbers and literals

- Integer literals in the machine widths, with `_` separators.
- A literal that does't fit its stated type is `NOTO0106` (number out of
  range), not a wrap.
- `Char` literals are single quoted; escapes shared with strings.
- Doc comments (`///`) are ordinary comments to the lexer — preserved with
  span, interpreted by nothing yet.
