# Noto — Handoff

State of the project at **0.14**. Written for whoever picks the work up next,
human or agent. Read this before touching anything.

**Where the project stands:** the compiler is real and works end to end. A
`.noto` file becomes a static native ELF executable with no LLVM, no libc and
no external toolchain. 546 tests pass, 0 fail, no warnings. The whole tool
set — `run`, `build`, `check`, `test`, `lint`, `fmt` — is implemented, and so
is enough of the language to write real programs in it: `examples/wc.noto` is
a `wc` that prints the same numbers as the system one.

```
$ cargo run -q -p noto-cli -- run examples/hello.noto
Hello, Noto!

$ cargo run -q -p noto-cli -- build examples/wc.noto -o /tmp/wc
$ /tmp/wc examples/wc.noto
106     429     2822    examples/wc.noto
```

Three modules of the standard library are written **in Noto**, on top of a
handful of compiler builtins: `std/math.noto`, `std/string.noto` and
`std/list.noto`. That is the shape the rest of the library should take —
add a builtin only when the language cannot express the thing at all.

### The design decisions worth reading before changing anything

Each of these is written down where the reasoning lives, and each was chosen
over a real alternative:

| Decision | Where |
|---|---|
| Generics are erased, not monomorphised — because every value is one word | `docs/design/generics.md` |
| A lambda captures by value, so it cannot assign to what it captured | `docs/spec.md`, Lambdas |
| An object is a reference, which is why `struct` is refused | `docs/spec.md`, Objects |
| A list is a header plus a buffer, so `push` never invalidates a pointer | `docs/spec.md`, Lists |
| Strings are measured in bytes, not characters | `docs/spec.md`, Strings |
| One file is one module and its path is its name | `docs/design/modules.md` |
| The formatter never moves code between lines | `docs/design/formatter.md` |
| An enum with no data *is* its tag | `docs/spec.md`, Enums |

---

## 1. Environment — read this first

**This machine has no C toolchain.** No `gcc`, no `cc`, no `crt1.o`, no libc
development files. Rust was installed with rustup into `~/.cargo`, and
`~/.cargo/config.toml` was written to work around it:

```toml
[build]
target = "x86_64-unknown-linux-musl"

[target.x86_64-unknown-linux-musl]
linker = "/home/epicnerdbr/.rustup/toolchains/stable-x86_64-unknown-linux-gnu/lib/rustlib/x86_64-unknown-linux-gnu/bin/rust-lld"
rustflags = ["-Clinker-flavor=ld.lld", "-Clink-self-contained=yes"]
```

That file is **not part of the project** and must not be committed into it. It
only makes `cargo` able to link the *Rust* compiler binary on this box. Noto's
own output needs none of it — the backend writes the ELF file itself.

Add `export PATH="$HOME/.cargo/bin:$PATH"` before any cargo command.

## 2. Layout

```
noto/
├── compiler/
│   ├── span/         noto-span         source positions, source map          9 tests
│   ├── diagnostics/  noto-diagnostics  diagnostics + terminal renderer        8 tests
│   ├── lexer/        noto-lexer        tokens, keywords, literals            50 tests
│   ├── ast/          noto-ast          syntax tree + visitor                  3 tests
│   ├── parser/       noto-parser       recursive descent + precedence        60 tests
│   ├── types/        noto-types        types, interning, unification         19 tests
│   ├── semantic/     noto-semantic     name resolution + type checking      200 tests
│   ├── ir/           noto-ir           Noto IR + textual form                11 tests
│   ├── lower/        noto-lower        AST -> Noto IR                        62 tests
│   ├── optimizer/    noto-optimizer    IR passes                              4 tests
│   ├── codegen/      noto-codegen      x86-64 backend + ELF writer           25 tests
│   └── driver/       noto-driver       pipeline, module graph                20 tests
├── runtime/          noto-runtime      runtime contract (no machine code)     3 tests
├── cli/              noto-cli          the `noto` command
├── formatter/        noto-formatter    `noto fmt`, token-stream based        35 tests
├── linter/           noto-linter       `noto lint`, NOTO0600/0601/0603/0604/0605  26 tests
├── test-runner/      noto-test-runner  `noto test`, one process per test     11 tests
├── lsp/              noto-lsp          STUB
├── debugger/         noto-debugger     STUB
├── std/              math.noto, string.noto, list.noto — written in Noto
├── docs/             architecture, spec, design notes, RFCs
├── examples/         hello, tests, point, enums, lists, generics, modules,
│                     config, wc — plus copies of the std modules they import
└── tests/            EMPTY
```

Every `.noto` file under `examples/` and `std/` carries its own tests, run
with `noto test <file>`. They are not part of `cargo test`, so run both:
a change to lowering can pass every Rust test and still break every program.

`compiler/lower` was added beyond the originally proposed layout: lowering
needs both the AST and the type checker's results, and putting it in `noto-ir`
would force the IR to depend on the whole front end. `compiler/span` and
`compiler/diagnostics` were split out for the same reason — every phase needs
them and nothing else should be pulled in with them.

## 3. What actually works

Compile and run today:

```noto
import list { mapped, kept }

class Point(val x: Int, var y: Int) {
    fn distance(): Int = abs(x) + abs(y)
}

enum Shape { Circle(radius: Int), Rect(width: Int, height: Int), Empty }

fn area(s: Shape): Int = when (s) {
    Circle(r) -> 3 * r * r
    Rect(w, h) -> w * h
    Empty -> 0
}

class Box<T>(val value: T)

fn firstOr<T>(xs: [T], fallback: T): T {
    if xs.length == 0 { return fallback }
    return xs[0]
}

fn main() {
    val shapes = [Shape.Circle(2), Shape.Rect(3, 4), Shape.Empty]
    val areas = mapped(shapes, { s: Shape -> area(s) })
    println(firstOr(kept(areas, { it > 10 }), 0))

    val text = readFile("config.txt")
    if text == null { return }
    println(text.length)      // a String here, not a String?
}
```

- `val`/`var` with inference, shadowing, scoping
- `Int Int8 Int16 Int32 Int64 UInt UInt8..64 Bool Char Byte String Unit Nothing Any`
- functions: parameters, results, expression bodies (`= expr`), forward references
- `if`/`when` as expressions, ranges (`..`, `..=`), guards, multiple patterns per arm
- `while`, `loop`, `for i in a..b`, `break`, `continue`
- null safety: `T?`, `?:`, nullable rejected where a plain value is needed
- string interpolation, concatenation, `.length`, `.toString()`
- `const` with compile-time folding
- `println`/`print` overloaded on String/Int/Bool, `assert`
- `readFile`, `writeFile`, `args()` — raw syscalls, no libc
- `test "name" { ... }` declarations are collected, type checked and run by
  `noto test`
- `import`/`export`: a program is many files, one module each, resolved from
  the root file's directory
- a null check narrows the local it tests, in the branch it guards and after
  a guard clause that leaves the block. One shape only: `x == null` and
  `x != null`
- generic functions and classes (`fn first<T>`, `class Pair<A, B>`), erased
  rather than
  monomorphised because every value is one machine word — see
  `docs/design/generics.md`, which also says what floats would overturn.
  `std/list.noto` is written on them
- inside a method a bare name is the receiver's member, and `p?.x` reads a
  field or property through a nullable receiver, producing a nullable result
- lambdas: a value of type `fn(A): B`, capturing by value into a closure of
  code address plus captures. `xs.map/filter/each` walk a list in the runtime
  and call back into the closure
- `readFile`, `writeFile` and `args()`: raw Linux syscalls, no libc. A path
  is copied into a NUL-terminated buffer by a private runtime helper, and
  `_start` saves the entry stack pointer so `args` can find `argc`/`argv`
- strings: `.length`, `.byteAt`, `.substring` are compiler builtins measured
  in **bytes**; `std/string.noto` builds `indexOf`, `split`, `trim`, `join`
  and the rest on top of them, in Noto
- `[T]` lists: literals, `.length`, `.push`, indexing, element assignment and
  `for`, with every index bounds-checked at runtime. A list is a header of
  length, capacity and a data pointer, so growing one leaves every pointer to
  it valid
- `enum Direction { North, East }` and `enum Shape { Circle(r: Int) }`:
  cases with or without data, matched bare or qualified, destructured in a
  pattern, with coverage counting as exhaustive
- `class Point(val x: Int, var y: Int) { fn ... }`: a constructor, field
  reads, field writes, methods and `this`; fields declared in the body with
  their own initialisers, run by a synthesised `Class.<init>`; properties
  with `get`/`set` bodies, compiled to functions taking the receiver. An object is a reference; every
  field takes a machine word and lives at `index * 8`; a method is a function
  named `Class.method` taking the receiver first

## 4. What does NOT work — and how it fails

Everything below **parses** (the parser covers the full language) but is
rejected during semantic analysis or lowering with `NOTO0500 … not implemented
in Noto 0.14`. Nothing is silently accepted and miscompiled.

| Construct | Rejected in | Notes |
|---|---|---|
| `struct` / `data class` / `data struct` | `compiler/semantic/src/collect.rs` `declare_class` | value semantics need RFC 0001; `class` works |
| class inheritance, interfaces, defaults on constructor parameters | `collect.rs` `declare_class` | fields, methods and properties work |
| `interface` | same | |
| explicit enum case values (`Red = 1`), methods on an enum | `collect.rs` `declare_enum` | enums otherwise work, data included |
| generic enums, bounds, explicit type arguments | `collect.rs` `declare_enum`, `check_call` | generic functions and classes work |
| extension functions | `collect.rs` `collect_fn` | receiver resolution missing |
| floats | `compiler/lower/src/expr.rs` `lower_literal` | needs SSE registers in the backend |
| `defer` | `compiler/lower/src/stmt.rs` `lower_stmt` | needs scope-exit tracking |
| safe calls `p?.f()`, `is`/`as`, `?` propagation, `await`, `unsafe`, tuples | `check.rs` / `expr.rs` fallthrough arms | |
| named arguments | `check.rs` `check_call` | |
| local `fn` inside a body | `check.rs` `check_stmt` | |
| more than 6 parameters | `codegen/src/lib.rs` `CodegenError::TooManyParameters` | System V register limit; stack arguments not implemented |

Runtime limitations, documented and deliberate:

- **the allocator never frees.** Bump pointer over `mmap` regions. See
  `compiler/codegen/src/x86_64/runtime.rs`. This is a placeholder for the real
  memory model.
- no threads, no async runtime, no FFI, no networking, no standard input.

## 5. What remains, in the order it should be done

Everything the original handoff listed under this heading — the CLI, the
documentation, the tooling, the object model, modules, enums, generics — is
done. What follows is what is left, ordered by what unblocks the most.

### 5.1 Interfaces and bounds — the biggest single unlock

Nobody can write a `Map<K, V>` today, because comparing two `K` values needs
`==` on a type parameter and a bare `T` permits only moving the value. Bounds
(`fn largest<T: Comparable>(..)`) are what lift that, and they need
interfaces, which parse and are rejected in `collect.rs`.

This also unblocks `data class` (structural equality is an interface a class
implements), sorting, and hashing.

**Decide first, in an RFC:** whether a bound is checked structurally or
nominally, and whether an interface can be implemented for a type declared
elsewhere. Both change what erasure can keep doing — a bounded `T` may need
to carry a witness, and that is a pointer per call, which is the first thing
in the language that would not be free.

### 5.2 Floating point

`Float32` and `Float64` parse, have `Primitive` variants and `IrType` variants,
and are rejected at `compiler/lower/src/expr.rs` `lower_literal`. What is
missing is a second register class in the backend: SSE registers, `movsd` and
friends in the encoder, and the System V rule that floats are passed in `xmm0`
through `xmm7` rather than the integer registers.

**Read `docs/design/generics.md` before starting.** Erasure holds because
every value is one machine word; a float in a different register class is the
first thing that breaks that. Landing floats means choosing between
monomorphising generic functions, boxing floats, or splitting the register
classes at the call boundary. That choice is yours and the reasoning it has to
overturn is written down there.

### 5.3 Sockets, and what they do and do not unlock

`readFile`, `writeFile` and `args()` show the shape: a routine in
`compiler/codegen/src/x86_64/runtime.rs` making raw syscalls, exposed as a
builtin. `socket`, `connect`, `send` and `recv` are the same shape, and a
plaintext TCP client is a day of work.

Be honest about what that gets: **it is not enough for an HTTPS client.**
Discord, and every other modern API, needs TLS 1.2 or 1.3 — elliptic curve
key exchange, AES-GCM, SHA-256 and X.509 parsing, written from scratch. That
is months, not days, and it is the real distance between here and a bot. DNS
is a smaller version of the same problem: no resolver exists either.

### 5.4 Smaller language work, roughly in dependency order

1. **`is` and `as`**, with narrowing the way a null check already narrows.
   `check_pattern` and `check_expr` reject both today.
2. **`defer`** — needs scope-exit tracking in lowering, including the `return`
   and `break` paths. **Answer [RFC 0002](docs/rfcs/0002-defer-semantics.md)
   first:** it has four open questions and none of them should be decided by
   whoever happens to be typing.
3. **Generic enums** — `enum Maybe<T>`. The machinery is all there; it is the
   same work generic classes took.
4. **More than six parameters** — stack arguments in the System V calling
   convention. `CodegenError::TooManyParameters` is where it fails.
5. **Named arguments**, **local `fn` inside a body**, **extension functions**,
   **tuples** — each self-contained, each rejected in one place.
6. **Safe calls `p?.f()`** — `p?.x` works; the call form reuses the same
   branch and was left out only for scope.

### 5.5 The memory model — the one nobody should decide alone

The allocator never frees. Every list, string, object, enum case and closure
is bump-allocated over `mmap` and stays until the process exits. That is fine
for a compiler run and wrong for anything long-lived, and it is the single
largest open question in the project.

[RFC 0001](docs/rfcs/0001-memory-model.md) is where it belongs, and it is
still empty of a decision. The specification asks for a hybrid that is not
Rust, not a GC and not ARC. **Do not answer it quietly in a commit.**

## 6. Decisions already made — do not silently change these

They are encoded in tests. Changing one means changing its test and writing an
RFC.

- **No statement terminator.** A line break ends a complete statement. An
  operator at the end of a line continues the expression. A line starting with
  `.` or `?.` continues the previous expression, which is what makes call
  chains work. `;` still separates statements on one line.
- **Bitwise binds tighter than comparison.** `flags & MASK == 0` parses as
  `(flags & MASK) == 0`. C's precedence here is a known bug source and Noto
  does not repeat it.
- **No implicit numeric conversions at all.** `Int32` does not become `Int64`.
  Diagnostics suggest `.toInt64()` when the widening would be safe.
- **`Int` is 64-bit on every target.** A program correct on one target is
  correct on all of them.
- **`Nothing` vs `Unit`.** `Unit` has one value; `Nothing` has none and is
  assignable to everything, which is what lets
  `val x: Int = if c { 1 } else { return }` typecheck.
- **`null` has type `Nothing?`**, assignable to every nullable type.
- **The AST is never mutated.** Analysis results live in side tables keyed by
  `NodeId`. Keeps the tree usable by tooling.
- **Errors are collected, never printed by a phase.** Phases push into a
  `DiagnosticSink`; the driver decides what to do. This is what lets the LSP
  reuse them.
- **The error type absorbs everything**, so one mistake produces one diagnostic
  instead of a cascade. There is a test for this.
- **Uppercase names in `when` arms are enum cases, lowercase ones are new
  bindings.** Lets a pattern be read without knowing the scrutinee's type.
- **An object is a reference, and `struct` is refused because of it.** `val b
  = a` makes both names see one object. Accepting `struct` would mean giving
  a keyword that promises value semantics the opposite behaviour.
- **Object layout lives in `noto-lower` and nowhere else.** The IR has
  `alloc`, `load [ptr+n]` and `store [ptr+n]` and no notion of a field, so a
  different layout changes one crate.
- **Generics are erased, not monomorphised.** One compiled copy serves every
  type argument, because every value is one machine word. `Float64` is what
  would break it; `docs/design/generics.md` says so and says what to do.
- **A lambda captures by value**, so it cannot assign to what it captured —
  the write would change its own copy. It is an error with a message.
- **A list is a header plus a separate buffer.** `push` replaces the buffer,
  and the indirection is what keeps every pointer to the list valid.
- **Strings are bytes.** `"joão".length` is 5. Characters need a decoder that
  does not exist.
- **A null check narrows what it proves**, in the branch it guards and after a
  guard clause that leaves the block. It reads one shape and claims nothing
  about any other condition.
- **The formatter never moves code between lines**, and never changes the
  token stream. Both are tested over a corpus.
- **Noto emits the ELF file itself.** No system linker, no LLVM. Keep it that
  way — it is why `noto build` works on a machine with nothing installed.

## 7. Working on the code

```bash
export PATH="$HOME/.cargo/bin:$PATH"
cargo test --workspace          # 546 tests, must stay at 0 failures
cargo build --workspace         # must stay at 0 warnings

# The Rust tests are half the suite. Every .noto file carries its own, and a
# change to lowering can pass every Rust test while breaking every program.
for f in examples/*.noto std/*.noto; do cargo run -q -p noto-cli -- test "$f"; done
cargo run -q -p noto-cli -- fmt --check examples/*.noto std/*.noto
cargo run -q -p noto-cli -- lint examples/wc.noto
```

House rules the existing code follows:

- **Every phase has tests, written with the code, not after.** Parser tests
  assert on S-expressions; lowering tests assert on the textual IR; the encoder
  tests assert on exact instruction bytes.
- **No `unwrap` on anything a user can cause.** Malformed input produces a
  diagnostic and a best-effort node.
- **Recovery must always consume a token.** Every parser loop has a
  `if self.position == before { self.advance(); }` guard. There is a test that
  parsing terminates on garbage.
- **Comments explain why, not what.** Density matches the surrounding code.
- **Diagnostics are part of the language surface.** Stable `NOTOnnnn` code,
  primary span, and a `help:` when there is a concrete fix.
- Keep it warning-clean: `cargo build --workspace` currently emits none.
- **A language decision gets written down before it gets written.**
  `docs/design/` holds one file per decision that had a real alternative, and
  each says what would overturn it. A commit that quietly picks one of these
  is the thing this file exists to prevent.
- **The standard library is written in Noto.** Add a compiler builtin only
  when the language cannot express the thing at all — `std/string.noto` needs
  three builtins and gets `split`, `trim`, `indexOf` and the rest from them.
