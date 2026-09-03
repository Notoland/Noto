//! Optimization passes over Noto IR.
//!
//! Noto 0.1 ships one pass, chosen because it is the one the lowering most
//! obviously needs: lowering emits a block per construct, and many of them end
//! up empty or with a single predecessor. Removing them shrinks the code and
//! makes the IR dumps readable without changing behaviour.
//!
//! Passes here must preserve semantics exactly. Anything that changes
//! observable behaviour — reordering effects, assuming absence of overflow —
//! needs a written rule in the specification first.

#![deny(missing_docs)]

mod unreachable;

pub use unreachable::remove_unreachable_blocks;

use noto_ir::Program;

/// Runs every optimization pass over `program`.
pub fn optimize(program: &mut Program) {
    for function in &mut program.functions {
        remove_unreachable_blocks(function);
    }
}
