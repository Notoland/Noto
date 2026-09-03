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

A program of many modules is analysed at once, in three passes whose order
the language forces rather than taste: every module's class names, then every
module's signatures and fields, then every module's bodies. A signature
anywhere may name a class anywhere, and a body may call anything.

Imports are never copied into a scope. A name an import brings in is resolved
by following the import to the module that declares it and asking that
module's export table — which is what lets signatures be collected before
imports have been checked, and what makes a module's own declaration win over
an imported one without any precedence rule to state.

### `noto-ir` — Noto IR

The instruction set lowering targets and the optimizer transforms: locals as
slots, branches to labelled blocks, intrinsics for the runtime contract
(`println` and friends). Has a textual form with tests asserting on it. See
[design/noto-ir.md](design/noto-ir.md).

### `noto-lower` — AST to IR

Lives in its own crate (added beyond the original layout) because lowering
needs both the AST and the type checker's results; putting it in `noto-ir`
would force the IR to depend on the whole front end.

A method is lowered as a function whose first parameter is the receiver, and
a method call as an ordinary call with the receiver passed first. Nothing
below this crate knows what a method is: the IR sees a function named
`Class.method` taking one more argument than the author wrote.

Object layout lives here, and only here. A class's fields are an ordered
list in the analysis; lowering turns field *n* into byte offset *n × 8* and
emits `alloc`, `load` and `store`. Neither the IR nor the backend knows what
a field is, so a future layout — packed fields, inline objects, tagged
enums — changes this crate and nothing downstream of it.

### `noto-optimizer` — IR passes

Passes over Noto IR. Currently small (constant folding and dead-code
elimination at the IR level); grows as the IR does.

### `noto-codegen` — the backend

Instruction selection and the x86-64 encoder, register allocation, the System
V calling convention (up to six integer-register parameters), and the ELF
writer that produces a static executable with mode `0o755`. Tests assert on
exact instruction bytes.

### `noto-driver` — orchestration

`read_source`, `compile_path`, `CompileOptions { stage, target, optimize,
allow_no_main }` and
`Stage::{Parse, Check, Ir, Executable}` — how far a compilation should go.
`noto check` and `noto build` share exactly the same front end; they differ
only in the stage they ask for.

The driver also owns the module graph. `modules.rs` reads the root, resolves
each `import` to a file relative to the root's directory, and repeats
breadth-first until nothing is left, reporting a missing module or a cycle
before any checking starts. Node ids run across the whole program — every
side table analysis fills is keyed by `NodeId` — so each file is parsed with
`parse_file_from`, continuing where the last one stopped.

### `noto-runtime` — the runtime contract

The intrinsics the backend may emit and what they must do. No machine code
lives here; the backend emits calls to symbols the ELF writer provides. The
allocator behind allocation intrinsics is currently a bump pointer over
`mmap` regions that never frees — a placeholder until
[RFC 0001](rfcs/0001-memory-model.md) decides the memory model.

### The tooling crates

`noto-cli` (the `noto` command), `noto-formatter`, `noto-linter`,
`noto-test-runner`, `noto-lsp`, `noto-debugger`. The CLI, the formatter, the
linter and the test runner are implemented; the LSP and the debugger are
placeholders. They are crates already so that
their public APIs can grow without dependency churn later.

`noto-test-runner` reuses the pipeline rather than adding one. Semantic
analysis already records each `test` declaration and lowering already emits
its body as a function named `test$<name>`, so the runner compiles the file
once to Noto IR, then asks the backend for one executable per test with that
test set as `Program::entry`. Nothing about the backend is test-aware.

`noto-formatter` is the one tool that does not use the AST. Two facts push it
onto the token stream instead. The lexer drops `//` and `/* */` as trivia, so
an AST printer would delete every ordinary comment in the file — the
formatter recovers them from the source text between consecutive token spans,
which is by definition what the lexer skipped. And every token is printed by
copying its source slice, so no literal or identifier can be changed by being
reprinted. It never moves code between lines, because a line break is part of
Noto's grammar and re-flowing would mean deciding where statements end. The
promise that falls out, and that its tests assert over a corpus, is that
lexing the formatted text yields exactly the token stream lexing the original
did. See `docs/design/formatter.md` for the rules themselves.

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
