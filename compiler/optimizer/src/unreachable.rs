//! Removing blocks nothing can reach.

use noto_ir::{BlockId, Function, Terminator};
use std::collections::HashSet;

/// Deletes every block that cannot be reached from the entry block.
///
/// Lowering creates a block for each arm of a `when` and each side of an `if`
/// whether or not control can get there; an arm after `else`, or the tail of a
/// block that always returns, leaves one behind. Removing them is a pure win:
/// they can never run, so nothing observes their absence.
pub fn remove_unreachable_blocks(function: &mut Function) {
    if function.blocks.is_empty() {
        return;
    }

    let reachable = reachable_blocks(function);
    if reachable.len() == function.blocks.len() {
        return;
    }

    // Blocks are renumbered as they are kept, and every jump is rewritten to
    // the new numbering.
    let mut new_index = vec![None; function.blocks.len()];
    let mut next = 0u32;
    for (index, _) in function.blocks.iter().enumerate() {
        if reachable.contains(&BlockId(index as u32)) {
            new_index[index] = Some(BlockId(next));
            next += 1;
        }
    }

    let mut blocks = Vec::with_capacity(next as usize);
    for (index, mut block) in std::mem::take(&mut function.blocks).into_iter().enumerate() {
        let Some(id) = new_index[index] else { continue };
        block.id = id;
        block.terminator = remap(block.terminator, &new_index);
        blocks.push(block);
    }

    function.blocks = blocks;
}

/// Every block reachable from the entry block.
fn reachable_blocks(function: &Function) -> HashSet<BlockId> {
    let mut reachable = HashSet::new();
    let mut worklist = vec![function.entry_block()];

    while let Some(id) = worklist.pop() {
        if !reachable.insert(id) {
            continue;
        }
        for successor in function.block(id).terminator.successors() {
            worklist.push(successor);
        }
    }

    reachable
}

/// Rewrites a terminator's targets to the new block numbering.
fn remap(terminator: Terminator, new_index: &[Option<BlockId>]) -> Terminator {
    let lookup = |id: BlockId| {
        new_index[id.0 as usize].expect("a reachable block only jumps to reachable blocks")
    };
    match terminator {
        Terminator::Jump(target) => Terminator::Jump(lookup(target)),
        Terminator::Branch { condition, then_block, else_block } => Terminator::Branch {
            condition,
            then_block: lookup(then_block),
            else_block: lookup(else_block),
        },
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use noto_ir::{Block, FuncId, IrType, Operand, Const};
    use noto_span::Span;

    fn block(id: u32, terminator: Terminator) -> Block {
        Block {
            id: BlockId(id),
            label: format!("block{id}"),
            instructions: Vec::new(),
            terminator,
        }
    }

    fn function(blocks: Vec<Block>) -> Function {
        Function {
            id: FuncId(0),
            name: "f".to_string(),
            parameters: Vec::new(),
            slots: Vec::new(),
            result: IrType::Unit,
            blocks,
            value_types: Vec::new(),
            span: Span::dummy(),
        }
    }

    #[test]
    fn drops_a_block_nothing_jumps_to() {
        let mut f = function(vec![
            block(0, Terminator::Jump(BlockId(2))),
            block(1, Terminator::Return(None)),
            block(2, Terminator::Return(None)),
        ]);
        remove_unreachable_blocks(&mut f);

        assert_eq!(f.blocks.len(), 2);
        assert_eq!(f.blocks[0].label, "block0");
        assert_eq!(f.blocks[1].label, "block2");
        // The jump follows the block to its new number.
        assert_eq!(f.blocks[0].terminator, Terminator::Jump(BlockId(1)));
    }

    #[test]
    fn keeps_everything_reachable_through_a_branch() {
        let mut f = function(vec![
            block(
                0,
                Terminator::Branch {
                    condition: Operand::Const(Const::Bool(true)),
                    then_block: BlockId(1),
                    else_block: BlockId(2),
                },
            ),
            block(1, Terminator::Jump(BlockId(2))),
            block(2, Terminator::Return(None)),
        ]);
        remove_unreachable_blocks(&mut f);
        assert_eq!(f.blocks.len(), 3);
    }

    #[test]
    fn a_loop_is_reachable_through_its_back_edge() {
        let mut f = function(vec![
            block(0, Terminator::Jump(BlockId(1))),
            block(1, Terminator::Jump(BlockId(1))),
        ]);
        remove_unreachable_blocks(&mut f);
        assert_eq!(f.blocks.len(), 2);
        assert_eq!(f.blocks[1].terminator, Terminator::Jump(BlockId(1)));
    }

    #[test]
    fn an_empty_function_is_left_alone() {
        let mut f = function(Vec::new());
        remove_unreachable_blocks(&mut f);
        assert!(f.blocks.is_empty());
    }
}
