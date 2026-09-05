# The Noto language specification — 0.14

The language as implemented, section by section. Everything marked **not
implemented** parses (the parser covers the full grammar) but is rejected
during semantic analysis or lowering with `NOTO0500 … not implemented in
Noto 0.14`. Nothing is silently accepted and miscompiled.

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

`this` names the receiver, and a bare name means the receiver's member when
nothing nearer holds it: `width * height` inside a method reads two fields.
A local, a parameter and a declaration all win over a member, so nothing a
method writes can be shadowed by a field added later.

A method is an ordinary function with the receiver as its first parameter — that is what it is compiled to, under the
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

A null check narrows what it proves. Inside the branch it guards, and after
a guard clause that leaves the block, a `T?` is a `T`:

```noto
fn size(text: String?): Int {
    if text == null { return 0 }
    return text.length          // a `String` here, not a `String?`
}
```

It reads one shape — `x == null` and `x != null`, either way round — and says
nothing about any other condition. A branch that can fall through proves
nothing after it, and what a branch proved ends with the branch.

Reading a member of something nullable takes `?.`, which produces a nullable
result: `p?.x` is an `Int?` when `p` is a `Point?`, and null when `p` is. On
a receiver that cannot be null it is a warning, because the check can never
fail.

Not implemented for classes: default values for constructor parameters,
inheritance, interfaces, generics, method values, safe method calls
(`p?.f()`), `data class` equality and printing. An object has no `toString`, so
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

## Talking to the world

Three builtins reach outside the process:

```noto
val text = readFile("config.txt") ?: ""    // String?, null when it fails
val ok = writeFile("out.txt", text)        // Bool, true when every byte landed
for argument in args() { println(argument) }
```

`readFile` produces a `String?` because a file that cannot be read is an
ordinary outcome, and the language already has a way to say so. `writeFile`
reports true only when every byte reached the file: a short write left
unretried is a truncated file, and calling that success would be worse than
calling it failure. `args()` gives the command line with the program's own
name first, the way a C `main` receives it.

There is no networking yet, and no standard input.

## Strings

A `String` holds UTF-8, and **everything about it is measured in bytes**:
`"joão".length` is 5, not 4. That is what a protocol, a file format and a
parser want; characters need a decoder, and there is not one yet.

Three operations are built in:

```noto
"hello".length            // 5
"hello".byteAt(1)         // 101, the byte value at that offset
"hello".substring(1, 3)   // "el", from one offset up to another
```

Both `byteAt` and `substring` check their bounds, and a `substring` may end
at the length. `+` joins two strings and `==` compares their contents rather
than their addresses.

Everything else — `indexOf`, `contains`, `startsWith`, `split`, `trim`,
`join`, `repeat` — is written in Noto in `std/string.noto`, on top of those
three.

## Generics

```noto
fn first<T>(xs: [T]): T = xs[0]

fn mapped<T, U>(xs: [T], f: fn(T): U): [U] {
    val out: [U] = []
    for x in xs { out.push(f(x)) }
    return out
}
```

A generic function is **compiled once**, not once per set of type arguments,
because every value in Noto is one machine word. The reasoning, and what
would overturn it, is in [design/generics.md](design/generics.md).

Type arguments are inferred from what the call is passed — `[T]` against
`[Int]` says `T` is `Int`, and a lambda passed after a list takes its
parameter types from what that list bound. A parameter appearing in no
argument cannot be inferred and is an error naming it.

A `T` has no members, no operators and no literals: you may bind it, pass it,
return it and store it, and nothing else. That is not a rule of its own — `T`
is not `Int`, so `+` does not apply, and it declares no fields, so `.x` does
not resolve. Bounds would lift it and need interfaces, which do not exist.

A class is generic the same way, and its parameters are in scope for its
fields and methods:

```noto
class Pair<A, B>(val first: A, val second: B) {
    fn swapped(): Pair<B, A> = Pair(second, first)
}
```

What is expected of a call fills in what its arguments do not say, which is
how `val s: Stack<Int> = emptyStack()` knows what it holds.

Not implemented: generic enums, bounds, explicit type arguments.

## Lambdas

```noto
val double = { n: Int -> n * 2 }
println(double(21))

fn apply(f: fn(Int): Int, n: Int): Int = f(n)
println(apply({ it + 1 }, 41))
```

A lambda is a value of function type, written `fn(A): B`. Its parameter types
come from what is expected of it where it is written, which is what lets
`{ it + 1 }` be passed without a type anywhere; with nothing to infer from,
they must be written. A lambda with no parameter list takes one argument,
bound to `it`.

**A lambda captures by value.** What it reads from an enclosing function is
copied when the lambda is written, which is why a lambda returned from a
function still works after that function's frame is gone:

```noto
fn adder(by: Int): fn(Int): Int = { n: Int -> n + by }
val addTen = adder(10)
println(addTen(32))     // 42
```

The other side of that choice: a lambda cannot assign to what it captured.
The write would change its own copy and leave the original alone, which is
never what anyone means, so it is an error rather than a surprise.

A lambda compiles to a function taking its environment first, and a closure
is that environment: its code address followed by what it captured.

Lists take lambdas:

```noto
xs.map({ it * 2 })              // [T] -> [U], for whatever the lambda gives
xs.filter({ it % 2 == 0 })      // [T] -> [T]
xs.each({ n: Int -> println(n) })
```

Not implemented: lambdas that outlive a captured `var` and see its changes,
`fold`, `sorted`, and a function value from a declared function's name.

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

A `when` arm may hold a block: after `->`, a brace opens statements rather
than a lambda. Braces holding a parameter list are still a lambda, so
`else -> { n -> n + 1 }` is an arm producing a function.

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
