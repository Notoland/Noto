# RFC 0002: `defer` semantics

- **Status:** Draft — **open question, not decided**
- **Discussion:** (open a PR to discuss)

## Summary

`defer` parses today (`compiler/parser/src/stmt.rs`) and is rejected in
lowering with `NOTO0500`. This RFC decides exactly what it means: when the
deferred expression runs, in what order multiple defers run, and what it
can capture.

## Motivation

Every language with manual or deterministic resources needs scope-exit
execution. Noto will have destructors in some form once
[RFC 0001](0001-memory-model.md) lands, and `defer` is the explicit,
user-facing form of the same mechanism. The parser accepting it while
lowering refuses it is deliberate: the grammar is settled, the semantics
are not.

## The questions that need answers

### 1. When does a defer run?

The obvious rule: at scope exit, on every path — normal fall-through,
`return`, `break`, `continue`. The non-obvious cases:

- **Panic/abort:** if `assert` fails (status 101) or the program aborts, do
  defers run? Running them requires unwinding or a landing-pad scheme;
  not running them is simpler but makes `defer` weaker than it looks.
- **Function-valued expressions:** `fn f() { defer g() }` where `g` is
  called later via another path — trivial; the scope is the function body.
  But `loop { defer … }` runs the defer **per iteration** (scope = loop
  body) or **once** (scope = function)? Per-iteration matches block
  scoping; once-per-function is what C's `goto cleanup` users expect.

### 2. Order of multiple defers

LIFO (last deferred runs first), the Go/C++-destructor convention, is the
strong default — it nests naturally:

```noto
defer a()
defer b()
// runs b(), then a()
```

This RFC should confirm LIFO unless someone argues FIFO convincingly.

### 3. What can the deferred expression capture?

By value at the `defer` site, or by reference at execution time?

```noto
var count = 0
defer println("count is $count")
count += 1
```

Value capture prints `0`; reference capture prints `1`. Value capture is
easier to lower (evaluate the expression's operands into slots at defer
time, schedule only the call); reference capture is more often what the
writer meant. This is the sharpest question in the RFC.

### 4. `defer` in the IR

Implementation sketch: lowering tracks the set of pending defers per scope
and emits them on every edge that leaves the scope — including `return`
from nested loops. Alternatively a single cleanup block per scope that all
exiting jumps target. The IR has no landing pads today; panic-time
execution (question 1) is what would force them in.

## Proposal

*To be written once the four questions above have answers people will
defend.*

## Alternatives considered

- **No `defer`, destructors only** (C++/Rust style): rejected direction —
  explicit beats implicit for resources that aren't types, and `defer`
  already parses.
- **`try`/`finally` blocks**: heavier syntax, covers the same ground.

## Unresolved questions

- Interaction with `break`/`continue` crossing multiple deferred scopes.
- Whether `defer` may be conditional (`defer if cond …`) or whether an
  `if` with a `defer` in both arms is the answer.
- Naming: does the feature stay `defer` when destructors arrive, or become
  `scope exit` sugar over them?
