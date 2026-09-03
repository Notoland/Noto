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
