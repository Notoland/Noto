# Noto

Noto is a native, modern, safe and productive programming language, designed to
let you write anything from a small program to a complex system, combining high
performance with a development experience that stays simple.

```noto
fn main() {
    println("Hello, Noto!")
}
```

Noto compiles to a **static native executable**. There is no virtual machine, no
interpreter, and no runtime to install — not even libc. The compiler writes the
ELF file itself.

```
$ noto run hello.noto
Hello, Noto!
```

---

## Status

**Noto is in early development.** Version 0.13 is a working compiler for a real
subset of the language, not a finished product. The pipeline runs end to end:
source becomes a native Linux x86-64 executable that you can run.

What that means in practice:

- **540 tests pass, zero failures, zero warnings.**
- Constructs that are not implemented yet are **rejected with a clear error**,
  never silently accepted and miscompiled.
- The parser covers the whole language; the back end does not yet.

See [HANDOFF.md](HANDOFF.md) for the precise state of every part, and
[section 4](HANDOFF.md#4-what-does-not-work--and-how-it-fails) for exactly what
is not implemented and where it is rejected.

## What works today

```noto
fn add(a: Int, b: Int): Int {
    return a + b
}

fn classify(age: Int): String = when (age) {
    0..12  -> "Criança"
    13..17 -> "Adolescente"
    else   -> "Adulto"
}

fn main() {
    val name = "João"
    var total = 0

    for i in 1..=10 {
        total += i
    }

    println("Olá, $name! Soma = $total")
    println(classify(16))
    println(add(2, 3) == 5)

    val maybe: String? = null
    println(maybe ?: "sem valor")
}
```

Objects:

```noto
class Rect(val width: Int, var height: Int) {
    fn area(): Int = this.width * this.height

    fn scale(factor: Int) {
        this.height = this.height * factor
    }
}

fn main() {
    val r = Rect(3, 4)
    r.scale(2)
    println(r.area())
}
```

An object is a **reference**: `val b = a` makes both names see one object.
That is why `struct` and `data class` are still rejected — they promise value
semantics, and giving them reference semantics under a keyword that says
otherwise would be worse than refusing them until the memory model is
settled. Fields are read with `.` and written with `.` when declared `var`. A method
is compiled to a function taking the receiver first, so it spends one of the
six argument registers and takes at most five parameters of its own.
Inheritance, interfaces and generics are not implemented yet.

Enums:

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

Covering every case is as complete as an `else`, so adding a case later turns
every `when` over the enum into an error naming what is missing. An enum
whose cases carry nothing *is* its tag — an `Int`, no allocation.

Modules:

```noto
// examples/modules.noto
import geometry.vector
import math { clamp }

fn main() {
    val delta = vector.between(vector.Vector(0, 0), vector.Vector(3, -4))
    println("${delta.describe()} is ${delta.manhattan()} steps away")
    println("clamped: ${clamp(delta.manhattan(), 0, 5)}")
}
```

One file is one module and its path is its name: `import geometry.vector`
reads `geometry/vector.noto` next to the root file. No manifest, no search
path. Everything is private until `export`; a plain import binds the module
as a namespace, a selective one binds the names it lists. There is no
wildcard import and no import cycles. See
[docs/design/modules.md](docs/design/modules.md).

```
Olá, João! Soma = 55
Adolescente
true
sem valor
```

- the `noto` CLI: `run`, `build`, `check`, `test`, `lint`, `fmt`, `version`
- `val` and `var` with type inference
- `Int`, `Int8`…`Int64`, `UInt`…`UInt64`, `Bool`, `Char`, `Byte`, `String`,
  `Unit`, `Nothing`, `Any`
- functions with parameters, results, expression bodies, forward references
- `if` and `when` as expressions, with ranges, guards and multiple patterns
- `while`, `loop`, `for … in`, `break`, `continue`
- null safety: non-nullable by default, `T?`, `?:`
- string interpolation and concatenation
- `const` folded at compile time
- `test "…" { … }` declarations, run by `noto test`
- `class` with fields and methods: a constructor, field reads, field writes,
  `this`
- `import` and `export`: a program made of many files
- `enum` with cases that may carry data, and a `when` that checks it covered
  every case
- `[T]` lists: literals, indexing, element assignment, `for`, with every
  index checked

## Design

Noto borrows good ideas from many languages and copies none of them. A few
decisions that are already settled:

**No statement terminator.** A line break ends a statement once it is complete.
An operator left at the end of a line continues the expression, and a line
starting with `.` continues the previous one:

```noto
val adults = users
    .filter { it.age >= 18 }
    .map { it.name }
```

**Bitwise operators bind tighter than comparison.** `flags & MASK == 0` parses
as `(flags & MASK) == 0`, which is what the code looks like it means. C's
precedence here is a long-standing source of bugs and Noto does not repeat it.

**No implicit numeric conversions, at all.** An `Int32` does not quietly become
an `Int64`. When a widening would be safe, the compiler says so:

```
error[NOTO0409]: `+` cannot mix `Int64` and `Int32`
 --> main.noto:4:17
  |
4 |     val c = a + b
  |                 ^ this is a `Int32`
  |
  = note: Noto never converts between number types on its own
  = help: convert it with `.toInt64()`
```

**`Int` is 64 bits on every target.** A program that is correct on one platform
is correct on all of them.

**Diagnostics are part of the language surface.** Every message has a stable
`NOTOnnnn` code, a span that points at the right bytes, and a `help:` line when
there is a concrete fix.

## Architecture

Noto has its own intermediate representation. LLVM is not part of the
architecture; the backend emits machine code and writes the executable itself.

```
Noto source
    ↓  lexer
  tokens
    ↓  parser
   AST
    ↓  semantic analysis + type checker
 typed AST
    ↓  lowering
  Noto IR
    ↓  optimizer
  Noto IR
    ↓  native backend
machine code
    ↓  ELF writer
 executable
```

Each phase is a separate crate that knows nothing about the file system, which
is what lets the same code serve the compiler, the language server and the test
runner. See [docs/architecture.md](docs/architecture.md).

## Building

Noto is written in Rust and has **no external dependencies** — the whole
compiler builds from `std` alone.

```bash
git clone https://github.com/Notoland/Noto.git
cd Noto
cargo build --release
cargo test --workspace
```

Compiling and running a Noto program:

```bash
cargo run -q -p noto-cli -- run examples/hello.noto
```

The implemented commands:

```
noto run <file.noto>        compile to a temporary executable and run it
noto build <file.noto>      write the executable next to the source
noto check <file.noto>      parse and analyse, report diagnostics only
noto test <file.noto>       compile and run every `test` declaration
noto lint <file.noto>       report what is legal but probably not meant
noto fmt <file.noto>        format the file in place
noto version                print the version
```

`noto build` also accepts `-o/--output <path>` and `--emit=ir`, which prints
the textual Noto IR instead of writing an executable. `noto test` accepts
`--filter <text>` to run only the tests whose name contains it, and
`noto lint` accepts `-D/--deny-warnings` to exit non-zero when any lint fires,
and `noto fmt` accepts `--check` and `--stdout`. Still planned: `noto new`,
`noto clean`.

Every lint is a warning with a stable code — `NOTO0600` a binding nothing
reads, `NOTO0601` a `var` nothing reassigns, `NOTO0604` a function nothing
calls, `NOTO0605` a constant nothing reads. A leading underscore
(`val _scratch = ...`) says the declaration is unused on purpose.

Tests need no `main` and no test framework — a `test` declaration is part of
the language:

```noto
fn add(a: Int, b: Int): Int = a + b

test "add sums its arguments" {
    assert(add(2, 3) == 5)
}
```

```bash
cargo run -q -p noto-cli -- test examples/tests.noto
```

Each test is compiled as its own executable with that test as the entry point
and run in its own process, so a failing `assert` — which ends the process,
because Noto has no unwinding — cannot hide the tests after it.

`noto fmt` is deterministic and has no options: one input has one formatting.
It changes whitespace and nothing else, and it never moves code between lines
— a line break is part of Noto's grammar, so re-flowing lines would mean
deciding where statements end. The rules are written down in
[docs/design/formatter.md](docs/design/formatter.md), and the formatter holds
itself to a promise there is a test for: **lexing the formatted text produces
exactly the token stream that lexing the original produced.**

## Project layout

```
compiler/
  span/          source positions and source maps
  diagnostics/   diagnostics and the terminal renderer
  lexer/         tokens, keywords, literals, interpolation
  ast/           syntax tree and visitor
  parser/        recursive descent with precedence climbing
  types/         type representation and interning
  semantic/      name resolution and type checking
  ir/            Noto IR and its textual form
  lower/         AST to Noto IR
  optimizer/     passes over Noto IR
  codegen/       x86-64 backend and ELF writer
  driver/        pipeline orchestration
runtime/         the runtime contract
cli/             the `noto` command
std/             the standard library
docs/            specification, architecture, RFCs
```

## Targets

Linux x86-64 is the first target and the only one implemented. The backend is
behind a `Target` abstraction so that Linux ARM64, Windows, macOS ARM64 and
RISC-V can be added without touching the phases above.

## Contributing

Noto is early enough that the most valuable contributions are the unfinished
pieces listed in [HANDOFF.md § 5](HANDOFF.md#5-what-remains-in-the-order-it-should-be-done).

Two rules matter more than the rest:

1. **Changes to the language go through an RFC** in `docs/rfcs/`. Decisions do
   not land in the compiler undocumented.
2. **Tests are written with the code, not after it.** Parser tests assert on
   S-expressions, lowering tests on the textual IR, and the instruction encoder
   on exact bytes.

The decisions that are already settled are listed in
[HANDOFF.md § 6](HANDOFF.md#6-decisions-already-made--do-not-silently-change-these).
Each one is held in place by a test; changing one means changing its test and
writing an RFC.

## License

Apache License 2.0. See [LICENSE](LICENSE).
