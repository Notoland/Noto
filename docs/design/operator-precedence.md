# Operator precedence

The precedence table is the `Precedence` enum in `compiler/parser/src/expr.rs`.
**This document and that enum must agree** — if one changes, the other
changes in the same commit.

From loosest to tightest:

| Level | Operators | Associativity |
|---|---|---|
| Lowest | (used internally) | — |
| Assignment | `=` `+=` `-=` `*=` `/=` `%=` … | right |
| Elvis | `?:` | right |
| Logical or | `\|\|` | left |
| Logical and | `&&` | left |
| Equality | `==` `!=` | left |
| Comparison | `<` `<=` `>` `>=` | left |
| Type test | `is` `!is` `in` `!in` | left |
| Bitwise or | `\|` | left |
| Bitwise xor | `^` | left |
| Bitwise and | `&` | left |
| Range | `..` `..=` | left |
| Shift | `<<` `>>` | left |
| Additive | `+` `-` | left |
| Multiplicative | `*` `/` `%` | left |

Unary operators (`!`, `-`, `+`) and postfixes (call, indexing, member access,
`?.`) sit above all of these.

## The deliberate divergence from C

**Bitwise operators bind tighter than comparison operators.** In C and the
languages that inherited its table, `&` sits *below* `==`, so the classic

```c
if (flags & MASK == 0)   /* means flags & (MASK == 0) — almost never intended */
```

silently computes the wrong thing and compiles without a warning. It is a
documented, decades-old bug source. Noto does not repeat it:

```noto
if (flags & MASK == 0)   // parses as (flags & MASK) == 0 — what it looks like
```

Consequently, parenthesise when mixing bitwise with shift or arithmetic if
the intent is unusual — the table optimises for the common reading of a mask
test, not for compactness.

## Range placement

`..` and `..=` bind *below* shift and arithmetic, so `1..n+1` is `1..(n+1)`
and `a..b == c..d` compares two ranges rather than a range against `b ==
c..d`. Ranges are values, not syntax restricted to `for`, and the precedence
treats them as the loose-binding operator they read as.

## Why an enum, not a table

The parser is recursive descent with precedence climbing: a level is
comparing `Precedence` values, and the compiler checks exhaustiveness when a
level is added. A data table could drift from the documentation unnoticed;
the enum makes "add a level" a type error until every `match` on it is
handled.
