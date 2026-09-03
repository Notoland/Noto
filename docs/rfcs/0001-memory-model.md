# RFC 0001: The memory model

- **Status:** Draft — **open question, not decided**
- **Discussion:** (open a PR to discuss)

## Summary

Decide how Noto manages memory: when values are heap-allocated, who frees
them, and what the compiler can assume. Today the allocator is a
**placeholder** — a bump pointer over `mmap` regions that never frees
(`compiler/codegen/src/x86_64/runtime.rs`). It exists so the pipeline runs
end to end; it is not a memory model.

## Motivation

The project's stated goal is a model that is **not Rust, not GC, not ARC** —
a hybrid that keeps safety without borrow-checker friction or a garbage
collector's pause profile. Nothing beyond that sentence is decided, and
every item in language work 5.5 (object model above all) is blocked on this
decision: field layout, `class` vs `struct` identity, what `data class`
copies, and what a destructor (see [RFC 0002](0002-defer-semantics.md))
would even free.

## Constraints already settled

- No runtime to install, no VM — whatever the model is, it ships inside the
  executable as part of the runtime contract (`noto-runtime`).
- Diagnostics over runtime errors where feasible: if the compiler can prove
  misuse, misuse is a compile error.
- `noto build` must keep working on a machine with nothing installed. No
  dependency on a system allocator's extended features.

## Proposal

*To be written. The options below are the candidate space, listed to be
argued about, none endorsed.*

### Option A — ownership with moves, no borrows

Values have owners; assignment moves; the compiler inserts frees at scope
exit. No borrow checker: aliasing is prevented by construction (a moved
value is unusable) rather than by lifetime analysis.

- For: deterministic, no collector, simpler than Rust.
- Against: "cannot use moved value" is still friction; shared structures
  need an explicit story (`Shared<T>`?).

### Option B — arena/region-based

Allocation goes into regions tied to scopes or tasks; a region frees in one
operation. Escape analysis keeps values that don't outlive their region in
registers or on the stack.

- For: frees are batched and cheap; maps well onto `defer` and tasks.
- Against: long-lived data needs an explicit region; cross-region pointers
  need rules of their own.

### Option C — hybrid: ownership for the common path, tracing for the escape hatch

Moves and scope-free by default; explicitly-shared graphs (`Shared<T>`)
collected by a compact, incremental collector that only ever sees what was
opted in.

- For: the ergonomic case is deterministic; shared graphs just work.
- Against: two models to teach, two to implement; the collector is a
  runtime cost paid by everyone who shares anything.

## Unresolved questions

- What is the ABI of a destructor call, and who inserts it — lowering or
  the backend?
- Does `String` (currently what — inline, borrowed, owned?) stay as-is?
- Interaction with the future module system and `std`: what does
  `std.mem` look like from user code?
- What can the optimizer assume? (No aliasing between distinct locals?
  Between a local and anything reachable from a `Shared`?)

## What this RFC must not do

Be decided by one person on one afternoon. The placeholder allocator keeps
the compiler honest (it leaks, and that is *visible*) until this document
has a Proposal section with consensus behind it.
