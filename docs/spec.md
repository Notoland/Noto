# The Noto language specification — 0.6

The language as implemented, section by section. Everything marked **not
implemented** parses (the parser covers the full grammar) but is rejected
during semantic analysis or lowering with `NOTO0500 … not implemented in
Noto 0.6`. Nothing is silently accepted and miscompiled.

This document describes behaviour; syntax details that deserve their own
rationale live in [design/](design/).

## Programs and files

A program is a `.noto` file with a `fn main()`, plus every module it imports.
The file handed to the compiler is the **root**, and its directory is where
imports are resolved from: `import geometry.point` reads
`geometry/point.noto`. Execution starts at the root's `main`, and the
program's exit status is 0 unless `main` is changed to return otherwise (not
implemented: `main` returning `Int`).

Every declaration is private to its module; `export` makes one visible to a
module that imports it.

```noto
import geometry.point              // binds `point`
import geometry.point as geo       // binds `geo`
import util { double }             // binds `double`

export fn area(): Int = ...        // visible to importers
fn helper(): Int = ...             // private to this module
```

A plain import binds the module's last segment as a namespace and its exports
are reached through it — `point.distance(1, 2)`, `point.Point`. A selective
import binds the names it lists directly. There is no wildcard import, and
imports may not form a cycle. The whole design is in
[design/modules.md](design/modules.md).

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
- `import` and `export` carry names across modules — see
  [Programs and files](#programs-and-files).
- `enum` declares a closed set of cases — see [Enums](#enums).
- Not implemented: `struct`, `data class`, `data struct`, `interface`,
  generics, extension functions, local `fn` inside a body, named arguments,
  re-exporting an imported name.

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

A field may instead be declared in the class body, where it carries its own
initialiser rather than receiving an argument:

```noto
class Person(val first: String, val last: String) {
    val full: String = first + " " + last
}
```

Such an initialiser runs when the object is built, in declaration order, and
may read the constructor's parameters — but not `this`, which does not exist
until the object does. A field declared in the body without an initialiser is
an error: there would be nothing to put in it.

A **property** is read like a field but computed by its accessors:

```noto
class Rect(val width: Int, val height: Int) {
    val area: Int { get = this.width * this.height }
}
```

A property with only an initialiser and no accessor body is simply a stored
field — the default accessors are what a field read and write already are. An
accessor with a body makes the property computed: reading calls its `get`,
assigning calls its `set`, and nothing is stored. The value being assigned
arrives in the setter under the name `value`. A `val` property may not have a
setter, and a property with custom accessors may not also have an initialiser:
that value could only be reached through storage the accessors cannot name.

Not implemented for classes: default values for constructor parameters,
inheritance, interfaces, generics, method values, safe property access
(`p?.x`), `data class` equality and printing. An object has no `toString`, so
`println(p)` does not compile — print its fields, or give the class a method
that builds a `String`.

## Enums

```noto
enum Direction { North, East, South, West }

fn label(d: Direction): String = when (d) {
    North -> "norte"
    East -> "leste"
    South -> "sul"
    West -> "oeste"
}
```

An `enum` declares a closed set of cases. A case is written `Direction.North`
as a value; in a `when` arm it may be written bare (`North`) because the
scrutinee's type says which enum it belongs to — this is the rule that
uppercase names in a `when` arm are cases and lowercase ones are new
bindings.

**Covering every case is as complete as an `else`.** A `when` over an enum
that names each case needs no `else` arm, and that is the point of an enum:
adding a case later turns every such `when` into an error that names what is
missing. An arm behind a guard does not count as covering its case — the
guard may not hold. A nullable enum is never exhaustive by cases alone, since
no case pattern matches `null`.

A case may carry data, and a pattern names what it carries:

```noto
enum Shape {
    Circle(radius: Int),
    Rect(width: Int, height: Int),
    Empty
}

fn area(s: Shape): Int = when (s) {
    Circle(r) -> 3 * r * r
    Rect(w, h) -> w * h
    Empty -> 0
}
```

Matching a case without naming what it carries (`Circle -> ...`) is fine when
the arm only cares which case it is.

**Representation follows the declaration.** An enum whose cases carry nothing
*is* its tag: a value of it is an `Int` holding the case's position, with no
allocation and no indirection, and a match is an integer comparison. An enum
where any case carries data is a pointer to that tag followed by the live
case's values — every case of it, including one carrying nothing.

Not implemented: explicit case values (`Red = 1`), methods on an enum,
generic enums, interfaces on an enum.

## Lists

```noto
val readings = [12, 7, 30]
println(readings.length)
println(readings[0])
readings[0] = 15

for x in readings { println(x) }
```

`[T]` is a sequence of `T`. A list is a header — its length, its capacity,
and a pointer to its elements — with the elements in a block of their own.
The indirection is what lets `push` grow one: replacing the block leaves
every pointer to the list itself valid, so a list passed to a function and
appended to there is the same list the caller holds.

```noto
val out: [Int] = []
out.push(1)
out.push(2)
```

A full buffer doubles, so a run of pushes costs a constant amount each on
average. A literal takes its element type from its first element, and every
later element must fit that type. An empty
literal has nothing to infer from, so it takes its type from where it is used
and is an error where nothing expects one — `val xs: [Int] = []`.

**Every index is checked.** An index outside `0..length` ends the process
with status 102 rather than reading whatever the allocator last left there.
One unsigned comparison covers both ends, so a negative index is caught by
the same check.

A list is invariant in its element: a `[Int]` is not a `[Any]`, because
writing an `Any` through the second would break the first. A literal is a
different matter — it is built to fit what is expected of it, so passing
`[1, 2]` where `[Any]` is wanted is fine.

A list has two members so far: `length` and `push`. Not implemented: removing
an element, slicing, concatenation, and searching.

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
