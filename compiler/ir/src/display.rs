//! Printing Noto IR in its textual form.
//!
//! The textual form is what `noto build --emit=ir` writes and what the IR
//! tests assert on. It is meant to be read by a person: one instruction per
//! line, values as `%n`, slots as `$n`, and jump targets named by block label.
//!
//! Slots are printed by index rather than by name because two locals in
//! different scopes may share a name; the `param`/`local` lines at the top of
//! a function carry the names.
//!
//! ```text
//! fn main(): unit {
//!   local $0 name: str
//!   entry0:
//!     store $0 str @0
//!     %0 = load $0
//!     intrinsic println_string %0
//!     return
//! }
//! ```

use crate::{Block, Const, Function, Inst, InstKind, Operand, Program, Terminator};
use std::fmt::{self, Display, Formatter, Write};

impl Display for Program {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        for (index, text) in self.strings.iter().enumerate() {
            writeln!(f, "string @{index} = {text:?}")?;
        }
        if !self.strings.is_empty() {
            writeln!(f)?;
        }
        for (index, function) in self.functions.iter().enumerate() {
            if index > 0 {
                writeln!(f)?;
            }
            write!(f, "{function}")?;
        }
        Ok(())
    }
}

impl Display for Function {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        let parameters: Vec<String> =
            self.parameters.iter().map(|slot| format!("${}", slot.0)).collect();
        writeln!(f, "fn {}({}): {} {{", self.name, parameters.join(", "), self.result.name())?;

        for (index, slot) in self.slots.iter().enumerate() {
            let keyword = if slot.is_parameter { "param" } else { "local" };
            writeln!(f, "  {keyword} ${index} {}: {}", slot.name, slot.ty.name())?;
        }

        for block in &self.blocks {
            write_block(f, block, self)?;
        }
        writeln!(f, "}}")
    }
}

fn write_block(f: &mut Formatter<'_>, block: &Block, function: &Function) -> fmt::Result {
    writeln!(f, "  {}:", block.label)?;
    for inst in &block.instructions {
        writeln!(f, "    {}", render_inst(inst))?;
    }
    writeln!(f, "    {}", render_terminator(&block.terminator, function))
}

/// Renders one instruction.
fn render_inst(inst: &Inst) -> String {
    let mut out = String::new();
    match &inst.kind {
        InstKind::Const { dest, value } => {
            let _ = write!(out, "%{} = const {value}", dest.0);
        }
        InstKind::LoadLocal { dest, slot } => {
            let _ = write!(out, "%{} = load ${}", dest.0, slot.0);
        }
        InstKind::StoreLocal { slot, value } => {
            let _ = write!(out, "store ${} {value}", slot.0);
        }
        InstKind::Unary { dest, op, operand } => {
            let _ = write!(out, "%{} = {} {operand}", dest.0, op.mnemonic());
        }
        InstKind::Binary { dest, op, left, right } => {
            let _ = write!(out, "%{} = {} {left} {right}", dest.0, op.mnemonic());
        }
        InstKind::Cast { dest, operand, to } => {
            let _ = write!(out, "%{} = cast {operand} to {}", dest.0, to.name());
        }
        InstKind::Call { dest, callee, arguments } => {
            if let Some(dest) = dest {
                let _ = write!(out, "%{} = ", dest.0);
            }
            let _ = write!(out, "call fn{}", callee.0);
            for argument in arguments {
                let _ = write!(out, " {argument}");
            }
        }
        InstKind::Intrinsic { dest, which, arguments } => {
            if let Some(dest) = dest {
                let _ = write!(out, "%{} = ", dest.0);
            }
            let _ = write!(out, "intrinsic {}", which.name());
            for argument in arguments {
                let _ = write!(out, " {argument}");
            }
        }
    }
    out
}

/// Renders a terminator, naming its targets by label.
fn render_terminator(terminator: &Terminator, function: &Function) -> String {
    let label = |id: crate::BlockId| {
        function
            .blocks
            .get(id.0 as usize)
            .map(|block| block.label.clone())
            .unwrap_or_else(|| format!("block{}", id.0))
    };

    match terminator {
        Terminator::Jump(target) => format!("jump {}", label(*target)),
        Terminator::Branch { condition, then_block, else_block } => {
            format!("branch {condition} {} {}", label(*then_block), label(*else_block))
        }
        Terminator::Return(None) => "return".to_string(),
        Terminator::Return(Some(value)) => format!("return {value}"),
        Terminator::Unreachable => "unreachable".to_string(),
    }
}

impl Display for Inst {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.write_str(&render_inst(self))
    }
}

impl Display for Operand {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Operand::Value(id) => write!(f, "%{}", id.0),
            Operand::Const(value) => write!(f, "{value}"),
        }
    }
}

impl Display for Const {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Const::Int { value, ty } => write!(f, "{}:{}", value, ty.name()),
            Const::Bool(value) => write!(f, "{value}"),
            Const::Char(value) => write!(f, "{value:?}"),
            Const::Str(id) => write!(f, "str @{}", id.0),
            Const::Null => write!(f, "null"),
            Const::Unit => write!(f, "unit"),
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::*;
    use noto_span::Span;

    #[test]
    fn prints_a_function_in_the_textual_form() {
        let mut program = Program::new();
        let text = program.intern_string("Hello, Noto!");

        program.functions.push(Function {
            id: FuncId(0),
            name: "main".to_string(),
            parameters: Vec::new(),
            slots: Vec::new(),
            result: IrType::Unit,
            value_types: vec![IrType::Str],
            blocks: vec![Block {
                id: BlockId(0),
                label: "entry0".to_string(),
                instructions: vec![
                    Inst::new(
                        InstKind::Const { dest: ValueId(0), value: Const::Str(text) },
                        Span::dummy(),
                    ),
                    Inst::new(
                        InstKind::Intrinsic {
                            dest: None,
                            which: Intrinsic::PrintlnString,
                            arguments: vec![Operand::Value(ValueId(0))],
                        },
                        Span::dummy(),
                    ),
                ],
                terminator: Terminator::Return(None),
            }],
            span: Span::dummy(),
        });
        program.entry = Some(FuncId(0));

        let expected = "\
string @0 = \"Hello, Noto!\"

fn main(): unit {
  entry0:
    %0 = const str @0
    intrinsic println_string %0
    return
}
";
        assert_eq!(program.to_string(), expected);
    }

    #[test]
    fn jump_targets_are_named_by_label() {
        let function = Function {
            id: FuncId(0),
            name: "f".to_string(),
            parameters: Vec::new(),
            slots: vec![Slot { name: "n".to_string(), ty: IrType::I64, is_parameter: false }],
            result: IrType::Unit,
            value_types: Vec::new(),
            blocks: vec![
                Block {
                    id: BlockId(0),
                    label: "entry0".to_string(),
                    instructions: Vec::new(),
                    terminator: Terminator::Branch {
                        condition: Operand::Const(Const::Bool(true)),
                        then_block: BlockId(1),
                        else_block: BlockId(2),
                    },
                },
                Block {
                    id: BlockId(1),
                    label: "if_then1".to_string(),
                    instructions: Vec::new(),
                    terminator: Terminator::Jump(BlockId(2)),
                },
                Block {
                    id: BlockId(2),
                    label: "if_join2".to_string(),
                    instructions: Vec::new(),
                    terminator: Terminator::Return(None),
                },
            ],
            span: Span::dummy(),
        };

        let text = function.to_string();
        assert!(text.contains("local $0 n: i64"), "{text}");
        assert!(text.contains("branch true if_then1 if_join2"), "{text}");
        assert!(text.contains("jump if_join2"), "{text}");
    }
}
