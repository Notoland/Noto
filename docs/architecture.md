# Architecture

How the Noto compiler is put together, one section per crate. The state of the
project at any moment lives in [HANDOFF.md](../HANDOFF.md); this document
describes the shape, which changes far more slowly.

## The pipeline

```
Noto source
    ↓  noto-lexer
  tokens
    ↓  noto-parser
   AST
    ↓  noto-semantic (name resolution + type checking)
  typed AST (side tables keyed by NodeId)
    ↓  noto-lower
  Noto IR
    ↓  noto-optimizer
  Noto IR
    ↓  noto-codegen (x86-64 encoder + ELF writer)
 executable
```

`noto-driver` owns this sequence: it reads source, calls each phase in order,
stops at the first phase that reported errors, and turns the result into files
on disk. `noto-cli` is a thin shell over the driver.

Compilation stops at the first failing phase, but that phase reports *all* of
its diagnostics, so one run shows every problem at that level rather than one
at a time.

## Why Noto IR exists, and why LLVM is not the architecture

Noto has its own intermediate representation (`noto-ir`) and its own backend
(`noto-codegen`) that emits x86-64 machine code and writes the ELF executable
itself. This is a deliberate architectural decision:

- **No external toolchain.** `noto build` works on a machine with no LLVM, no
  system linker, no libc and no C compiler. The compiler is self-sufficient:
  the whole workspace builds from Rust's `std` with zero external
  dependencies.
- **The IR serves Noto's rules, not a generic ones.** Locals are mutable
  slots, not SSA variables, because Noto's `var` and `while` map onto slots
  directly and the phi-node construction that SSA requires would buy nothing
  at this tier (see [design/noto-ir.md](design/noto-ir.md)).
- **The diagnostics stay ours.** Every phase reports through
  `noto-diagnostics` with a stable `NOTOnnnn` code. A backend we own can
  report errors in Noto terms; a foreign one reports in its own.

The cost is real — instruction selection, register allocation and ABI details
are ours to write, target by target — and it is paid knowingly. The backend is
behind a `Target` abstraction so new targets are added without touching any
phase above `noto-lower`.

## The crates

### `noto-span` — source positions

`Span`, `Location`, `FileId` and the `SourceMap` that ties files to their
text. Split out because every other phase needs it and nothing else should be
pulled in with it.

### `noto-diagnostics` — errors and warnings

`Diagnostic` (severity, stable code, message, primary span, notes, help) and
the `DiagnosticSink` that collects them. The terminal renderer supports plain
and ANSI styles. Code ranges are allocated per phase — see
[design/diagnostics.md](design/diagnostics.md).

The rule: **phases never print.** They push diagnostics into the sink and the
driver decides what to do. This is what will let the LSP reuse them verbatim.

### `noto-lexer` — tokens

Turns source text into tokens: keywords, identifiers, integer literals,
strings with `$`-interpolation, operators. Every token keeps its span, and
comments survive as trivia attached to the stream — which is what a formatter
will need. See [design/lexer.md](design/lexer.md).

### `noto-ast` — the syntax tree

The tree the parser produces, plus a `Visitor` trait for walking it. The tree
covers the whole language, including constructs the back end does not accept
yet — the front end is ahead of the rest on purpose, so that diagnostics for
unimplemented constructs are precise rather than parse errors.

### `noto-parser` — recursive descent with precedence climbing

One function per nonterminal, one `Precedence` enum that drives expression
parsing (see [design/operator-precedence.md](design/operator-precedence.md)).
Recovery rule: on error, emit a diagnostic, build a best-effort node, and
always consume at least one token — there is a test that parsing terminates
on arbitrary garbage input.

### `noto-types` — type representation

The type graph and the interner that makes type equality pointer equality.
`Int` is 64 bits on every target by definition.

### `noto-semantic` — name resolution and type checking

Two passes over the AST: collection (what names exist) and checking (what the
code means). Results live in an `Analysis` struct of side tables keyed by
`NodeId` — **the AST is never mutated.** A side effect of that decision is
that tooling (formatter, LSP, linter) can walk the same tree the compiler
checked.

The error type absorbs everything, so one mistake produces one diagnostic
instead of a cascade. There is a test for this.

### `noto-ir` — Noto IR

The instruction set lowering targets and the optimizer transforms: locals as
slots, branches to labelled blocks, intrinsics for the runtime contract
(`println` and friends). Has a textual form with tests asserting on it. See
[design/noto-ir.md](design/noto-ir.md).

### `noto-lower` — AST to IR

Lives in its own crate (added beyond the original layout) because lowering
needs both the AST and the type checker's results; putting it in `noto-ir`
would force the IR to depend on the whole front end.

### `noto-optimizer` — IR passes

Passes over Noto IR. Currently small (constant folding and dead-code
elimination at the IR level); grows as the IR does.

### `noto-codegen` — the backend

Instruction selection and the x86-64 encoder, register allocation, the System
V calling convention (up to six integer-register parameters), and the ELF
writer that produces a static executable with mode `0o755`. Tests assert on
exact instruction bytes.

### `noto-driver` — orchestration

`read_source`, `compile`, `CompileOptions { stage, target, optimize }` and
`Stage::{Parse, Check, Ir, Executable}` — how far a compilation should go.
`noto check` and `noto build` share exactly the same front end; they differ
only in the stage they ask for.

### `noto-runtime` — the runtime contract

The intrinsics the backend may emit and what they must do. No machine code
lives here; the backend emits calls to symbols the ELF writer provides. The
allocator behind allocation intrinsics is currently a bump pointer over
`mmap` regions that never frees — a placeholder until
[RFC 0001](rfcs/0001-memory-model.md) decides the memory model.

### The tooling crates

`noto-cli` (the `noto` command), `noto-formatter`, `noto-linter`,
`noto-test-runner`, `noto-lsp`, `noto-debugger`. The CLI, the linter and the
test runner are implemented; the others are placeholders. They are crates already so that
their public APIs can grow without dependency churn later.

`noto-test-runner` reuses the pipeline rather than adding one. Semantic
analysis already records each `test` declaration and lowering already emits
its body as a function named `test$<name>`, so the runner compiles the file
once to Noto IR, then asks the backend for one executable per test with that
test set as `Program::entry`. Nothing about the backend is test-aware.

`noto-linter` reads what analysis already learned rather than recomputing it:
which name refers to which binding is the type checker's answer, and asking
the question twice is how two answers drift apart. One walk collects every
mention of every name, and the lints are then decided from side tables —
which is also why the linter reports nothing about unreachable code. The type
checker already emits `NOTO0602` from the `Nothing` type, and that catches an
`if` whose every branch returns, which a syntactic lint would miss.

One process per test is a deliberate choice, not an accident of the design. A
failing `assert` exits with `ASSERT_FAILURE_STATUS` (101) and there is no
unwinding to catch it, so a single binary calling every test in turn would
stop at the first failure. Separate processes also keep a test that corrupts
memory from taking the rest of the run with it, and make the exit status the
whole reporting protocol: `0` passed, `101` failed an assertion, anything else
is reported with its status.

## House rules the code follows

- **Every phase has tests, written with the code, not after.** Parser tests
  assert on S-expressions; lowering tests on the textual IR; the encoder on
  exact bytes.
- **No `unwrap` on anything a user can cause.** Malformed input produces a
  diagnostic and a best-effort node.
- **Zero external dependencies.** The workspace builds from `std` alone.
- **Comments explain why, not what.**
