# The Noto language specification — 0.3

The language as implemented, section by section. Everything marked **not
implemented** parses (the parser covers the full grammar) but is rejected
during semantic analysis or lowering with `NOTO0500 … not implemented in
Noto 0.3`. Nothing is silently accepted and miscompiled.

This document describes behaviour; syntax details that deserve their own
rationale live in [design/](design/).

## Programs and files

A program is one `.noto` file with a `fn main()` — multi-file compilation
(**`import`/`export`**) is not implemented. Execution starts at `main` and the
program's exit status is 0 unless `main` is changed to return otherwise (also
not implemented: `main` returning `Int`).

## Declarations

```noto
fn add(a: Int, b: Int): Int { return a + b }
fn classify(age: Int): String = when (age) { ... }

const LIMIT = 100

test "addition works" {
    assert(add(2, 3) == 5)
}
```

- Functions have parameters with types and an optional result type. The body
  is a block, or `= expr` for an expression body.
- Forward references work: a function may call one declared later.
- At most **six parameters** (System V register limit); more is not
  implemented. A method's receiver is one of the six.
- `const` declares a compile-time constant; constant expressions are folded.
- `test "name" { … }` declares a test. Tests are collected, type checked and
  lowered as functions named `test$<name>`; `noto test` compiles one
  executable per test and runs each in its own process.
- `class` declares an object type — see [Objects](#objects).
- Not implemented: `struct`, `data class`, `data struct`, `interface`,
  `enum`, generics, extension functions, local `fn` inside a body, named
  arguments.

## Objects

```noto
class Point(val x: Int, var y: Int)

fn main() {
    val p = Point(3, 4)
    p.y = 10
    println(p.x + p.y)
}
```

A `class` declares a type and, with it, one constructor: its parameter list
is its field list, in order. Fields need declared types — there is no
initialiser to infer one from — and a field is read with `.` and written with
`.` when it was declared `var`. Assigning to a `val` field is `NOTO0304`, the
same diagnostic a `val` binding gets.

A class name may be used before it is declared, as a type or as a
constructor, so declaration order in a file never matters.

Methods are declared in the class body and called through a receiver:

```noto
class Counter(var count: Int) {
    fn bump() {
        this.count += 1
    }

    fn plus(n: Int): Int = this.count + n
}
```

`this` names the receiver. A method is an ordinary function with the
receiver as its first parameter — that is what it is compiled to, under the
name `Class.method` — so **the receiver spends one of the six argument
registers**: a method takes at most five parameters of its own.

A method and a field cannot share a name. Reading a method without calling it
(`counter.bump`) is an error: there are no method values yet.

**An object is a reference.** `val b = a` makes `b` name the same object as
`a`, and a change through one is visible through the other. Every object is
allocated on the heap, which today never frees — see
[RFC 0001](rfcs/0001-memory-model.md).

That is why `struct` and the `data` flavours are still rejected: they promise
value semantics, copied on assignment, and giving them reference semantics
under a keyword that says otherwise would be worse than refusing them.

Not implemented for classes: properties with `get`/`set`, fields declared in
the class body, default values for fields, inheritance, interfaces, generics,
method values, `data class` equality and printing. An object has no
`toString`, so `println(p)` does not compile — print its fields, or give the
class a method that builds a `String`.

## Statements and termination

**There is no statement terminator.** A line break ends a statement once it
is complete. An operator left dangling at the end of a line continues the
expression onto the next line, and a line beginning with `.` or `?.`
continues the previous expression — which is what makes call chains read
naturally. `;` still separates statements that share a line.

`val` declares an immutable binding, `var` a mutable one; both infer their
type from the initialiser. Shadowing is allowed and scoping is lexical.
Control flow: `if`/`else`, `when`, `while`, `loop`, `for i in expr`,
`break`, `continue`, `return`. `defer` is not implemented.

## Types

```
Int Int8 Int16 Int32 Int64 UInt UInt8 UInt16 UInt32 UInt64
Bool Char Byte String Unit Nothing Any
T?   — nullable
```

- **`Int` is 64-bit on every target.** A program correct on one target is
  correct on all of them.
- **No implicit numeric conversions, at all.** An `Int32` does not quietly
  become an `Int64`. When a widening would be safe the diagnostic suggests
  `.toInt64()` (code `NOTO0409`).
- `Unit` has exactly one value; `Nothing` has none and is assignable to
  everything — which is what lets
  `val x: Int = if c { 1 } else { return }` typecheck.
- `null` has type `Nothing?` and is assignable to every nullable type.
- Floating-point types are not implemented.

## Expressions

- Integer arithmetic `+ - * / %`, bitwise `& | ^ << >>`, comparisons
  `== != < <= > >=`, boolean `&& || !` (short-circuiting).
- **Bitwise binds tighter than comparison:** `flags & MASK == 0` parses as
  `(flags & MASK) == 0`. Rationale in
  [design/operator-precedence.md](design/operator-precedence.md).
- Ranges `a..b` (exclusive) and `a..=b` (inclusive), usable in `for` and as
  `when` patterns.
- `if` and `when` are expressions.
- `when` arms accept multiple patterns separated by `,`, ranges, guards
  (`pattern if condition`), and `else`. Uppercase names in patterns are enum
  cases and lowercase names are new bindings — a pattern can be read without
  knowing the scrutinee's type. (Pattern *matching on enums* waits for enums;
  the rule is settled.)
- Null safety: `T?` types, the elvis operator `?:`, and a nullable value is
  rejected where a plain `T` is required (`NOTO0406`). Not implemented:
  safe calls `?.f()`, `is`/`as`, `?` propagation.
- Strings: `$name` and `${expr}` interpolation, `+` concatenation,
  `.length`, `.toString()` on the scalar types.
- Not implemented: lambdas as values, tuples, lists, `await`, `unsafe`, and
  safe field access (`p?.x`), which needs a null check lowering does not emit
  yet.

## Built-ins

`println` and `print` (overloaded on `String`/`Int`/`Bool`) and `assert`.
`assert` failing exits with status 101, distinct from an ordinary non-zero
exit, so a test runner can tell assertion failures apart.

## Runtime contract

Static native executable; no VM, no interpreter, no libc, no system linker.
The compiler writes the ELF file itself. No threads, no async runtime, no
FFI. The allocator is a non-freeing bump pointer over `mmap` — a placeholder
pending [RFC 0001](rfcs/0001-memory-model.md).

## Reserved words

The reserved vocabulary is fixed and small; words Noto may want later are
already reserved so that 1.0 code never breaks: `actor`, `impl`, `macro`,
`match`, `module`, `mut`, `operator`, `package`, `static`, `trait`, `type`,
`typealias`, `use`, `where`, `yield`.
