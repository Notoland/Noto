# Noto — Handoff

State of the project at 0.3. Written for whoever picks the work up next,
human or agent. Read this before touching anything.

**Where the project stands:** the compiler is real and works end to end. A
`.noto` file becomes a static native ELF executable with no LLVM, no libc, and
no external toolchain. 494 tests pass, 0 fail, no warnings. The whole tool
set — `run`, `build`, `check`, `test`, `lint`, `fmt` — is implemented, and
`class` gives the language its first object type.

```
$ cargo run -q -p noto-driver --example emit -- examples/hello.noto /tmp/hello
$ /tmp/hello
Hello, Noto!
```

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
│   ├── lexer/        noto-lexer        tokens, keywords, literals            49 tests
│   ├── ast/          noto-ast          syntax tree + visitor                  3 tests
│   ├── parser/       noto-parser       recursive descent + precedence        58 tests
│   ├── types/        noto-types        type representation, interning        19 tests
│   ├── semantic/     noto-semantic     name resolution + type checking       70 tests
│   ├── ir/           noto-ir           Noto IR + textual form                 9 tests
│   ├── lower/        noto-lower        AST -> Noto IR                        24 tests
│   ├── optimizer/    noto-optimizer    IR passes                              7 tests
│   ├── codegen/      noto-codegen      x86-64 backend + ELF writer           16 tests
│   └── driver/       noto-driver       pipeline orchestration                 6 tests
├── runtime/          noto-runtime      runtime contract (no machine code)     3 tests
├── cli/              noto-cli          the `noto` command
├── formatter/        noto-formatter    `noto fmt`, token-stream based     31 tests
├── linter/           noto-linter       `noto lint`, NOTO0600/0601/0604/0605  18 tests
├── test-runner/      noto-test-runner  `noto test`, one process per test  11 tests
├── lsp/              noto-lsp          STUB
├── debugger/         noto-debugger     STUB
├── std/              math.noto, string.noto
├── docs/             architecture, spec, design notes, RFCs
├── examples/         hello.noto, tests.noto, point.noto
└── tests/            EMPTY
```

`compiler/lower` was added beyond the originally proposed layout: lowering
needs both the AST and the type checker's results, and putting it in `noto-ir`
would force the IR to depend on the whole front end. `compiler/span` and
`compiler/diagnostics` were split out for the same reason — every phase needs
them and nothing else should be pulled in with them.

## 3. What actually works

Compile and run today:

```noto
fn add(a: Int, b: Int): Int { return a + b }

fn classify(age: Int): String = when (age) {
    0..12  -> "Criança"
    13..17 -> "Adolescente"
    else   -> "Adulto"
}

fn main() {
    val name = "João"
    var total = 0
    for i in 1..=10 { total += i }
    println("Olá, $name! Soma = $total, ${classify(16)}")
    val maybe: String? = null
    println(maybe ?: "sem valor")
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
- `test "name" { ... }` declarations are collected, type checked and run by
  `noto test`
- `import`/`export`: a program is many files, one module each, resolved from
  the root file's directory
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
in Noto 0.1`. Nothing is silently accepted and miscompiled.

| Construct | Rejected in | Notes |
|---|---|---|
| `struct` / `data class` / `data struct` | `compiler/semantic/src/collect.rs` `declare_class` | value semantics need RFC 0001; `class` works |
| class inheritance, interfaces, defaults on constructor parameters | `collect.rs` `declare_class` | fields, methods and properties work |
| `interface` | same | |
| explicit enum case values (`Red = 1`), methods on an enum | `collect.rs` `declare_enum` | enums otherwise work, data included |
| generics (`fn f<T>`, `List<T>`) | `collect.rs` `collect_fn`, `resolve_type` | monomorphisation not designed yet |
| extension functions | `collect.rs` `collect_fn` | receiver resolution missing |
| floats | `compiler/lower/src/expr.rs` `lower_literal` | needs SSE registers in the backend |
| `defer` | `compiler/lower/src/stmt.rs` `lower_stmt` | needs scope-exit tracking |
| safe field access `p?.x`, safe calls `?.f()`, `is`/`as`, `?` propagation, `await`, `unsafe`, lambdas as values, tuples | `check.rs` / `expr.rs` fallthrough arms | |
| named arguments | `check.rs` `check_call` | |
| local `fn` inside a body | `check.rs` `check_stmt` | |
| more than 6 parameters | `codegen/src/lib.rs` `CodegenError::TooManyParameters` | System V register limit; stack arguments not implemented |

Runtime limitations, documented and deliberate:

- **the allocator never frees.** Bump pointer over `mmap` regions. See
  `compiler/codegen/src/x86_64/runtime.rs`. This is a placeholder for the real
  memory model.
- no threads, no async runtime, no FFI.

## 5. What remains, in the order it should be done

### 5.1 CLI — highest priority, nothing else is usable without it

`cli/src/main.rs` is `fn main() {}`. Everything it needs already exists in
`noto-driver`; this is wiring, not new compiler work.

Use `compiler/driver/examples/emit.rs` as the reference — it already does
read → compile → write with the executable bit set.

Commands for this milestone:

```
noto run <file.noto>      compile to a temporary file and execute it
noto build <file.noto>    write the executable next to the source
noto check <file.noto>    Stage::Check, diagnostics only
noto version
```

Then: `noto test`, `noto fmt`, `noto lint`, `noto clean`.

Details that matter:
- `CompileOptions { stage, target, optimize }` selects how far to go —
  `Stage::{Parse, Check, Ir, Executable}`.
- Add `--emit=ir` printing `program.to_string()`; the textual IR already exists
  and is tested.
- Use `RenderStyle::Ansi` when stdout is a terminal, `Plain` otherwise.
- Exit code 1 when `sink.has_errors()`, and print `noto_driver::summary(&sink)`.
- Executables need mode `0o755` (`noto_codegen::EXECUTABLE_MODE`).

**Acceptance:** `noto run examples/hello.noto` prints `Hello, Noto!`.

### 5.2 Documentation — required by the specification, currently absent

Nothing exists in `docs/`. All of it has to be written, and the decisions are
already made and encoded in the code and tests — write them down, do not
re-invent them.

- `README.md` — what Noto is, the philosophy statement, how to build, hello world
- `docs/architecture.md` — the pipeline, one section per crate, why Noto IR is
  its own IR and LLVM is not the architecture
- `docs/spec.md` — the language as implemented, section by section, marking
  what is not implemented yet
- `docs/design/operator-precedence.md` — **the table is in
  `compiler/parser/src/expr.rs`, enum `Precedence`.** The two must agree. The
  deliberate divergence from C (bitwise binds tighter than comparison) needs
  its rationale written down.
- `docs/design/lexer.md` — statement termination rule, reserved-for-future word
  list, interpolation
- `docs/design/noto-ir.md` — instruction set, textual form, why locals are slots
  rather than SSA
- `docs/design/diagnostics.md` — the code ranges allocated per phase, from
  `compiler/diagnostics/src/lib.rs` module `codes`
- `docs/rfcs/0000-template.md` and the RFC process
- `docs/rfcs/0001-memory-model.md` — **open question, do not decide alone.**
  The spec asks for a hybrid model that is not Rust, not GC, not ARC. Nothing
  is decided; the bump allocator is a placeholder.
- `docs/rfcs/0002-defer-semantics.md` — referenced from
  `compiler/parser/src/stmt.rs` and does not exist yet
- `LICENSE` — Apache 2.0, referenced by every `Cargo.toml`, file missing

### 5.3 Standard library

`std/` is empty. Start with what the compiler already provides as builtins and
grow outward. Needs the module system (5.5) before it can be more than a
handful of intrinsics.

### 5.4 Tooling

- ~~**formatter** (`noto fmt`)~~ — done; the rules are in
  `docs/design/formatter.md`. It works on the token stream, not the AST: the
  lexer keeps only `///` comments, so an AST printer would delete the rest.
  It does not re-flow lines — that needs its own RFC, because a line break is
  part of the grammar.
- ~~**linter** (`noto lint`)~~ — done, except `NOTO0603` (unused import),
  which needs the module system first. `NOTO0602` stayed in semantic analysis
  rather than moving to the linter: it falls out of the `Nothing` type for
  free and catches more than a syntactic lint could.
- ~~**test runner** (`noto test`)~~ — done. Compiles the file once, then emits
  one executable per test with that test as `Program::entry` and runs each in
  its own process. `CompileOptions::allow_no_main` lets a file of tests build.

### 5.5 Language work, roughly in dependency order

1. **object model** — ~~layout, construction, field access, methods,
   properties and body fields~~ done for `class`. What remains: defaults on
   constructor parameters, method values, then inheritance and interfaces. `struct` and the `data` flavours wait on RFC 0001, since
   they promise value semantics and an object is a reference today. A `data
   class` also needs structural equality and a `toString` — an object cannot
   be printed at all right now.
2. **module system** — ~~`import`/`export`, multi-file compilation~~ done.
   What remains: re-exporting an imported name, visibility between `export`
   and private, and a package manifest so a program can depend on something
   it did not copy in.
3. **enums** — ~~cases, associated data, matching, destructuring and
   exhaustiveness~~ done. What remains: explicit case values (`Red = 1`),
   methods on an enum, and `is`/`as` narrowing.
4. **generics** — decide monomorphisation vs boxing, write an RFC first.
5. **floats** — SSE registers in the encoder and a second register class.
6. **`defer`** — scope-exit tracking in lowering, including error paths.
7. async/await, FFI, LSP, debugger, package manager, registry.

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
- **Noto emits the ELF file itself.** No system linker, no LLVM. Keep it that
  way — it is why `noto build` works on a machine with nothing installed.

## 7. Working on the code

```bash
export PATH="$HOME/.cargo/bin:$PATH"
cargo test --workspace          # 494 tests, must stay at 0 failures
cargo build --workspace
cargo run -q -p noto-driver --example emit -- examples/hello.noto /tmp/hello && /tmp/hello
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
