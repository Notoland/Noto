# Noto IR design

Noto IR is the instruction set between the typed AST and machine code. It
lives in `noto-ir`; `noto-lower` produces it, `noto-optimizer` transforms
it, `noto-codegen` consumes it. Lowering tests assert on its textual form,
which doubles as the `noto build --emit=ir` output.

The hello-world program in textual form:

```text
string @0 = "Hello, Noto!"

fn main(): unit {
  entry0:
    intrinsic println_string str @0
    return
}
```

## Shape

- A `Program` is a set of `Function`s plus a pool of string constants
  referenced by index.
- A `Function` has a signature, its locals, and labelled basic blocks.
  Every block ends in a terminator (`return`, branch, jump, unreachable) —
  a block that falls off the end is malformed and tests reject it.
- Control flow is branches to labelled blocks; there is no phi
  instruction.

## Why locals are slots rather than SSA

**Locals are mutable slots, addressed by `SlotId`, read with `LoadLocal`
and written with `StoreLocal`.** This is a deliberate divergence from the
SSA convention (LLVM, Cranelift's original form, most modern IRs):

- Noto has `var` and `while`. A mutable local *is* a slot in the source
  language; representing it as one makes lowering a direct transliteration
  of the AST, which is why the lowering crate is small enough to test
  exhaustively against the textual IR.
- SSA's payoff is optimisation freedom, and that payoff is collected where
  register allocation happens anyway. The optimizer's passes (constant
  folding, dead code) operate fine on slots; if a pass ever needs SSA it
  can build it locally and destroy it — the IR does not have to *be* SSA
  for that.
- Phi construction is where lowering bugs live in most compilers. Not
  having it removes the class.

The bet: at Noto's optimisation tier, transliteration clarity beats
textbook IR purity. Revisit with an RFC if the optimizer outgrows it.

## Objects are addresses, not aggregates

The IR has no aggregate type and no field access. An object is a pointer,
and three instructions do the work:

```
%0 = alloc 16          reserve 16 bytes, produce their address
store [%0+8] 4:i64     write a word at address + 8
%1 = load [%0+8]       read a word from address + 8
```

Field names and layout are the front end's business. Lowering knows a class's
field order, turns each field into a byte offset, and from there the IR — and
the backend — deal only in addresses. That keeps the object model out of
three crates that do not need to know about it, and means a future feature
that lays memory out differently (packed fields, inline objects, tagged
enums) changes lowering alone.

The offsets it produces today are simple: **every field takes a full machine
word**, so field *n* lives at *n × 8*. That wastes space on a `Bool` and
keeps every access aligned, and it is what makes the alternative easy to see
in a diff when the memory model settles.

An enum case's values sit behind the same three instructions: `alloc` a word
for the tag plus one per value the widest case carries, `store` the tag at
offset zero, and `load` the payload once a match has proved which case is
live. An enum whose cases carry nothing never reaches the IR as an object at
all — it is an `i64` holding the tag.

`alloc` calls the runtime's bump allocator, which never frees. The memory it
returns is uninitialised — the code that allocates an object writes every
field before the pointer escapes.

Note that `load` prints two ways: `load $0` reads a local slot, `load [%1+0]`
reads memory. The brackets are the difference, and they are the reason the
textual form does not use bare `+`.

## Intrinsics and the runtime contract

Operations the machine cannot express as pure arithmetic — `println`,
allocation, and friends — appear as `intrinsic` instructions naming a
symbol from the runtime contract (`noto-runtime` defines the contract; the
backend emits calls the ELF writer resolves). This keeps the IR
target-neutral: an intrinsic is a name and an argument list, nothing more.

## Textual form

Every instruction has a printing form, round-trippable enough that tests
can diff lowered output against expected text. The form is a debugging
surface (`noto build --emit=ir`), not an input format — nothing parses it
back.
