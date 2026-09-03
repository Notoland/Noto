# Noto RFCs

Changes to the **language** go through an RFC. Decisions do not land in the
compiler undocumented.

An RFC is needed for anything that changes what a Noto program means:
syntax, semantics, type rules, the memory model, the module system, the
standard library's public surface. It is not needed for internal refactors,
new diagnostics, or performance work — though a perf trade-off that changes
observable behaviour is.

## Process

1. Copy `0000-template.md` to `NNNN-short-name.md` (next free number).
2. Write it. "Not implemented yet" sections are acceptable; hidden
   alternatives are not.
3. Open a PR with the RFC. Discussion happens there.
4. Consensus → status `Accepted`; disagreement → `Rejected` or rewritten.
   The RFC stays in the tree either way — rejected RFCs are the record of
   why not.
5. Implementing an accepted RFC updates its status to `Implemented` and,
   where relevant, [docs/spec.md](../spec.md) in the same PR.

## Statuses

- **Draft** — being written, not ready for a decision.
- **Accepted** — decided, not yet in the compiler.
- **Implemented** — in the compiler, spec updated.
- **Rejected** — decided against; kept as the record of why.
