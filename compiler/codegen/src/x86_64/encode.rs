//! An x86-64 instruction encoder.
//!
//! The backend emits machine code directly rather than going through an
//! assembler or an external toolchain, which is what lets `noto build` produce
//! an executable with nothing installed but the compiler itself.
//!
//! Only the instructions Noto currently generates are encoded here. Each is
//! covered by a test comparing against the bytes a reference assembler
//! produces, because a wrong encoding fails at run time rather than at compile
//! time and is very hard to debug from the symptom.

use std::collections::HashMap;

/// A general purpose 64-bit register.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[allow(missing_docs)]
pub enum Reg {
    Rax = 0,
    Rcx = 1,
    Rdx = 2,
    Rbx = 3,
    Rsp = 4,
    Rbp = 5,
    Rsi = 6,
    Rdi = 7,
    R8 = 8,
    R9 = 9,
    R10 = 10,
    R11 = 11,
    R12 = 12,
    R13 = 13,
    R14 = 14,
    R15 = 15,
}

impl Reg {
    /// The low three bits of the register number, used in ModRM and SIB.
    fn low(self) -> u8 {
        self as u8 & 0b111
    }

    /// The fourth bit, which goes into a REX prefix.
    fn high(self) -> u8 {
        (self as u8 >> 3) & 1
    }
}

/// A condition a `setcc` or `jcc` tests.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Cond {
    /// Equal, `ZF = 1`.
    Eq,
    /// Not equal.
    Ne,
    /// Signed less than.
    Lt,
    /// Signed less than or equal.
    Le,
    /// Signed greater than.
    Gt,
    /// Signed greater than or equal.
    Ge,
    /// Unsigned below.
    Below,
    /// Unsigned below or equal.
    BelowEq,
    /// Unsigned above.
    Above,
    /// Unsigned above or equal.
    AboveEq,
}

impl Cond {
    /// The `tttn` field that selects the condition in `0F 8x` and `0F 9x`.
    fn code(self) -> u8 {
        match self {
            Cond::Eq => 0x4,
            Cond::Ne => 0x5,
            Cond::Below => 0x2,
            Cond::AboveEq => 0x3,
            Cond::BelowEq => 0x6,
            Cond::Above => 0x7,
            Cond::Lt => 0xC,
            Cond::Ge => 0xD,
            Cond::Le => 0xE,
            Cond::Gt => 0xF,
        }
    }
}

/// A jump target that is bound after the jump is emitted.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Label(u32);

/// Something outside the code stream that an instruction refers to.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Reference {
    /// A byte offset into the read-only data section.
    RoData(u32),
    /// A byte offset into the writable data section.
    Data(u32),
}

/// A place in the code that needs patching once addresses are known.
#[derive(Clone, Copy, Debug)]
pub struct Relocation {
    /// Offset of the 32-bit field to patch, relative to the start of the code.
    pub at: u32,
    /// Offset of the end of the instruction, which RIP-relative addressing is
    /// measured from.
    pub next_instruction: u32,
    /// What the instruction refers to.
    pub reference: Reference,
}

/// Emits x86-64 machine code.
#[derive(Default)]
pub struct Assembler {
    code: Vec<u8>,
    labels: Vec<Option<u32>>,
    /// Jumps whose target was not yet bound when they were emitted.
    pending: Vec<(u32, Label)>,
    /// References to data sections, patched by the caller once laid out.
    relocations: Vec<Relocation>,
    /// The offset each named routine or function starts at.
    symbols: HashMap<String, u32>,
}

impl Assembler {
    /// Creates an empty assembler.
    pub fn new() -> Self {
        Assembler::default()
    }

    /// The number of bytes emitted so far.
    pub fn position(&self) -> u32 {
        self.code.len() as u32
    }

    /// Records that a symbol starts at the current position.
    pub fn define_symbol(&mut self, name: impl Into<String>) {
        let position = self.position();
        self.symbols.insert(name.into(), position);
    }

    /// The offset a symbol was defined at.
    pub fn symbol(&self, name: &str) -> Option<u32> {
        self.symbols.get(name).copied()
    }

    /// Allocates a label that can be bound later.
    pub fn label(&mut self) -> Label {
        self.labels.push(None);
        Label(self.labels.len() as u32 - 1)
    }

    /// Binds a label to the current position.
    pub fn bind(&mut self, label: Label) {
        let position = self.position();
        self.labels[label.0 as usize] = Some(position);
    }

    /// Finishes assembly, patching every jump and returning the code.
    ///
    /// Data relocations are returned unresolved: only the caller knows where
    /// the sections will land.
    pub fn finish(mut self) -> (Vec<u8>, Vec<Relocation>, HashMap<String, u32>) {
        for (at, label) in std::mem::take(&mut self.pending) {
            let target = self.labels[label.0 as usize]
                .expect("every label must be bound before assembly finishes");
            // A rel32 is measured from the end of the instruction, which is
            // four bytes past the field being patched.
            let displacement = target as i64 - (at as i64 + 4);
            let bytes = (displacement as i32).to_le_bytes();
            self.code[at as usize..at as usize + 4].copy_from_slice(&bytes);
        }
        (self.code, self.relocations, self.symbols)
    }

    // --- raw emission -----------------------------------------------------

    fn byte(&mut self, value: u8) {
        self.code.push(value);
    }

    fn bytes(&mut self, values: &[u8]) {
        self.code.extend_from_slice(values);
    }

    fn imm32(&mut self, value: i32) {
        self.bytes(&value.to_le_bytes());
    }

    fn imm64(&mut self, value: i64) {
        self.bytes(&value.to_le_bytes());
    }

    /// Emits a REX prefix when one is needed.
    ///
    /// `w` selects 64-bit operand size; `r` and `b` are the high bits of the
    /// register fields.
    fn rex(&mut self, w: bool, reg: u8, rm: u8) {
        let value = 0x40 | (u8::from(w) << 3) | (reg << 2) | rm;
        if value != 0x40 {
            self.byte(value);
        }
    }

    /// Emits a ModRM byte for a register-to-register operation.
    fn modrm_reg(&mut self, reg: u8, rm: u8) {
        self.byte(0b1100_0000 | (reg << 3) | rm);
    }

    /// Emits ModRM (and SIB when needed) for `[base + disp32]`.
    fn modrm_mem(&mut self, reg: u8, base: Reg, displacement: i32) {
        self.byte(0b1000_0000 | (reg << 3) | base.low());
        // rsp and r12 encode "SIB follows" in the rm field, so an explicit SIB
        // byte selecting no index is required.
        if base.low() == 0b100 {
            self.byte(0x24);
        }
        self.imm32(displacement);
    }

    // --- moves ------------------------------------------------------------

    /// `mov dst, src`
    pub fn mov_reg_reg(&mut self, dst: Reg, src: Reg) {
        if dst == src {
            return;
        }
        self.rex(true, src.high(), dst.high());
        self.byte(0x89);
        self.modrm_reg(src.low(), dst.low());
    }

    /// `movabs dst, imm64`
    pub fn mov_reg_imm64(&mut self, dst: Reg, value: i64) {
        self.rex(true, 0, dst.high());
        self.byte(0xB8 + dst.low());
        self.imm64(value);
    }

    /// `mov [base + displacement], src`
    pub fn mov_mem_reg(&mut self, base: Reg, displacement: i32, src: Reg) {
        self.rex(true, src.high(), base.high());
        self.byte(0x89);
        self.modrm_mem(src.low(), base, displacement);
    }

    /// `mov dst, [base + displacement]`
    pub fn mov_reg_mem(&mut self, dst: Reg, base: Reg, displacement: i32) {
        self.rex(true, dst.high(), base.high());
        self.byte(0x8B);
        self.modrm_mem(dst.low(), base, displacement);
    }

    /// `mov byte [base + displacement], src8`
    pub fn mov_mem_reg8(&mut self, base: Reg, displacement: i32, src: Reg) {
        // The REX prefix is emitted unconditionally so that sil/dil/spl/bpl
        // are addressed rather than the legacy ah/ch/dh/bh.
        let value = 0x40 | (src.high() << 2) | base.high();
        self.byte(value);
        self.byte(0x88);
        self.modrm_mem(src.low(), base, displacement);
    }

    /// `movzx dst, byte [base + displacement]`
    pub fn movzx_reg_mem8(&mut self, dst: Reg, base: Reg, displacement: i32) {
        self.rex(true, dst.high(), base.high());
        self.bytes(&[0x0F, 0xB6]);
        self.modrm_mem(dst.low(), base, displacement);
    }

    /// `lea dst, [rip + displacement]`, with the displacement patched later.
    pub fn lea_rip(&mut self, dst: Reg, reference: Reference) {
        self.rex(true, dst.high(), 0);
        self.byte(0x8D);
        self.byte((dst.low() << 3) | 0b101);
        let at = self.position();
        self.imm32(0);
        self.relocations.push(Relocation {
            at,
            next_instruction: self.position(),
            reference,
        });
    }

    // --- arithmetic and logic ---------------------------------------------

    fn alu_reg_reg(&mut self, opcode: u8, dst: Reg, src: Reg) {
        self.rex(true, src.high(), dst.high());
        self.byte(opcode);
        self.modrm_reg(src.low(), dst.low());
    }

    /// `add dst, src`
    pub fn add(&mut self, dst: Reg, src: Reg) {
        self.alu_reg_reg(0x01, dst, src);
    }

    /// `sub dst, src`
    pub fn sub(&mut self, dst: Reg, src: Reg) {
        self.alu_reg_reg(0x29, dst, src);
    }

    /// `and dst, src`
    pub fn and(&mut self, dst: Reg, src: Reg) {
        self.alu_reg_reg(0x21, dst, src);
    }

    /// `or dst, src`
    pub fn or(&mut self, dst: Reg, src: Reg) {
        self.alu_reg_reg(0x09, dst, src);
    }

    /// `xor dst, src`
    pub fn xor(&mut self, dst: Reg, src: Reg) {
        self.alu_reg_reg(0x31, dst, src);
    }

    /// `cmp left, right`
    pub fn cmp(&mut self, left: Reg, right: Reg) {
        self.alu_reg_reg(0x39, left, right);
    }

    /// `imul dst, src`
    pub fn imul(&mut self, dst: Reg, src: Reg) {
        self.rex(true, dst.high(), src.high());
        self.bytes(&[0x0F, 0xAF]);
        self.modrm_reg(dst.low(), src.low());
    }

    /// `add dst, imm32`
    pub fn add_imm(&mut self, dst: Reg, value: i32) {
        self.rex(true, 0, dst.high());
        self.byte(0x81);
        self.modrm_reg(0, dst.low());
        self.imm32(value);
    }

    /// `sub dst, imm32`
    pub fn sub_imm(&mut self, dst: Reg, value: i32) {
        self.rex(true, 0, dst.high());
        self.byte(0x81);
        self.modrm_reg(5, dst.low());
        self.imm32(value);
    }

    /// `cmp reg, imm32`
    pub fn cmp_imm(&mut self, reg: Reg, value: i32) {
        self.rex(true, 0, reg.high());
        self.byte(0x81);
        self.modrm_reg(7, reg.low());
        self.imm32(value);
    }

    /// `and reg, imm32`
    pub fn and_imm(&mut self, reg: Reg, value: i32) {
        self.rex(true, 0, reg.high());
        self.byte(0x81);
        self.modrm_reg(4, reg.low());
        self.imm32(value);
    }

    fn unary(&mut self, extension: u8, reg: Reg) {
        self.rex(true, 0, reg.high());
        self.byte(0xF7);
        self.modrm_reg(extension, reg.low());
    }

    /// `neg reg`
    pub fn neg(&mut self, reg: Reg) {
        self.unary(3, reg);
    }

    /// `not reg`
    pub fn not(&mut self, reg: Reg) {
        self.unary(2, reg);
    }

    /// `idiv reg`, dividing `rdx:rax`.
    pub fn idiv(&mut self, reg: Reg) {
        self.unary(7, reg);
    }

    /// `div reg`, dividing `rdx:rax`.
    pub fn div(&mut self, reg: Reg) {
        self.unary(6, reg);
    }

    /// `cqo`, sign-extending `rax` into `rdx` before a signed division.
    pub fn cqo(&mut self) {
        self.bytes(&[0x48, 0x99]);
    }

    fn shift(&mut self, extension: u8, reg: Reg) {
        self.rex(true, 0, reg.high());
        self.byte(0xD3);
        self.modrm_reg(extension, reg.low());
    }

    /// `shl reg, cl`
    pub fn shl_cl(&mut self, reg: Reg) {
        self.shift(4, reg);
    }

    /// `shr reg, cl`
    pub fn shr_cl(&mut self, reg: Reg) {
        self.shift(5, reg);
    }

    /// `sar reg, cl`
    pub fn sar_cl(&mut self, reg: Reg) {
        self.shift(7, reg);
    }

    /// `test reg, reg` on the low byte.
    pub fn test8(&mut self, reg: Reg) {
        let rex = 0x40 | (reg.high() << 2) | reg.high();
        self.byte(rex);
        self.byte(0x84);
        self.modrm_reg(reg.low(), reg.low());
    }

    /// `setcc reg8`
    pub fn setcc(&mut self, cond: Cond, reg: Reg) {
        let rex = 0x40 | reg.high();
        self.byte(rex);
        self.bytes(&[0x0F, 0x90 | cond.code()]);
        self.modrm_reg(0, reg.low());
    }

    /// `movzx dst, src8`
    pub fn movzx8(&mut self, dst: Reg, src: Reg) {
        self.rex(true, dst.high(), src.high());
        self.bytes(&[0x0F, 0xB6]);
        self.modrm_reg(dst.low(), src.low());
    }

    /// `movsx dst, src8`
    pub fn movsx8(&mut self, dst: Reg, src: Reg) {
        self.rex(true, dst.high(), src.high());
        self.bytes(&[0x0F, 0xBE]);
        self.modrm_reg(dst.low(), src.low());
    }

    /// `movsx dst, src16`
    pub fn movsx16(&mut self, dst: Reg, src: Reg) {
        self.rex(true, dst.high(), src.high());
        self.bytes(&[0x0F, 0xBF]);
        self.modrm_reg(dst.low(), src.low());
    }

    /// `movzx dst, src16`
    pub fn movzx16(&mut self, dst: Reg, src: Reg) {
        self.rex(true, dst.high(), src.high());
        self.bytes(&[0x0F, 0xB7]);
        self.modrm_reg(dst.low(), src.low());
    }

    /// `movsxd dst, src32`
    pub fn movsx32(&mut self, dst: Reg, src: Reg) {
        self.rex(true, dst.high(), src.high());
        self.byte(0x63);
        self.modrm_reg(dst.low(), src.low());
    }

    /// `mov dst32, src32`, which zero-extends into the full 64-bit register.
    pub fn mov32(&mut self, dst: Reg, src: Reg) {
        self.rex(false, src.high(), dst.high());
        self.byte(0x89);
        self.modrm_reg(src.low(), dst.low());
    }

    // --- stack ------------------------------------------------------------

    /// `push reg`
    pub fn push(&mut self, reg: Reg) {
        if reg.high() == 1 {
            self.byte(0x41);
        }
        self.byte(0x50 + reg.low());
    }

    /// `pop reg`
    pub fn pop(&mut self, reg: Reg) {
        if reg.high() == 1 {
            self.byte(0x41);
        }
        self.byte(0x58 + reg.low());
    }

    // --- control flow ------------------------------------------------------

    /// `jmp label`
    pub fn jmp(&mut self, label: Label) {
        self.byte(0xE9);
        self.record_jump(label);
    }

    /// `jcc label`
    pub fn jcc(&mut self, cond: Cond, label: Label) {
        self.bytes(&[0x0F, 0x80 | cond.code()]);
        self.record_jump(label);
    }

    /// `call label`
    pub fn call(&mut self, label: Label) {
        self.byte(0xE8);
        self.record_jump(label);
    }

    fn record_jump(&mut self, label: Label) {
        let at = self.position();
        self.imm32(0);
        self.pending.push((at, label));
    }

    /// `ret`
    pub fn ret(&mut self) {
        self.byte(0xC3);
    }

    /// `syscall`
    pub fn syscall(&mut self) {
        self.bytes(&[0x0F, 0x05]);
    }

    /// `ud2`, which raises an illegal instruction fault.
    ///
    /// Emitted where control must not reach, so a compiler bug crashes
    /// immediately instead of running whatever bytes follow.
    pub fn ud2(&mut self) {
        self.bytes(&[0x0F, 0x0B]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Assembles one instruction and returns its bytes.
    fn encode(build: impl FnOnce(&mut Assembler)) -> Vec<u8> {
        let mut assembler = Assembler::new();
        build(&mut assembler);
        assembler.finish().0
    }

    #[test]
    fn encodes_register_moves() {
        assert_eq!(encode(|a| a.mov_reg_reg(Reg::Rax, Reg::Rcx)), vec![0x48, 0x89, 0xC8]);
        assert_eq!(encode(|a| a.mov_reg_reg(Reg::Rdi, Reg::Rsi)), vec![0x48, 0x89, 0xF7]);
        assert_eq!(encode(|a| a.mov_reg_reg(Reg::R8, Reg::Rax)), vec![0x49, 0x89, 0xC0]);
        assert_eq!(encode(|a| a.mov_reg_reg(Reg::Rax, Reg::R15)), vec![0x4C, 0x89, 0xF8]);
        assert!(encode(|a| a.mov_reg_reg(Reg::Rax, Reg::Rax)).is_empty(), "a self-move is dropped");
    }

    #[test]
    fn encodes_immediate_moves() {
        assert_eq!(
            encode(|a| a.mov_reg_imm64(Reg::Rax, 1)),
            vec![0x48, 0xB8, 1, 0, 0, 0, 0, 0, 0, 0]
        );
        assert_eq!(
            encode(|a| a.mov_reg_imm64(Reg::Rdi, -1)),
            vec![0x48, 0xBF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF]
        );
    }

    #[test]
    fn encodes_memory_operands() {
        // mov [rbp-8], rax
        assert_eq!(
            encode(|a| a.mov_mem_reg(Reg::Rbp, -8, Reg::Rax)),
            vec![0x48, 0x89, 0x85, 0xF8, 0xFF, 0xFF, 0xFF]
        );
        // mov rax, [rbp-16]
        assert_eq!(
            encode(|a| a.mov_reg_mem(Reg::Rax, Reg::Rbp, -16)),
            vec![0x48, 0x8B, 0x85, 0xF0, 0xFF, 0xFF, 0xFF]
        );
        // rsp needs a SIB byte because its encoding is taken by "SIB follows".
        assert_eq!(
            encode(|a| a.mov_reg_mem(Reg::Rax, Reg::Rsp, 0)),
            vec![0x48, 0x8B, 0x84, 0x24, 0, 0, 0, 0]
        );
    }

    #[test]
    fn encodes_arithmetic() {
        assert_eq!(encode(|a| a.add(Reg::Rax, Reg::Rcx)), vec![0x48, 0x01, 0xC8]);
        assert_eq!(encode(|a| a.sub(Reg::Rax, Reg::Rcx)), vec![0x48, 0x29, 0xC8]);
        assert_eq!(encode(|a| a.imul(Reg::Rax, Reg::Rcx)), vec![0x48, 0x0F, 0xAF, 0xC1]);
        assert_eq!(encode(|a| a.xor(Reg::Rdx, Reg::Rdx)), vec![0x48, 0x31, 0xD2]);
        assert_eq!(encode(|a| a.cqo()), vec![0x48, 0x99]);
        assert_eq!(encode(|a| a.idiv(Reg::Rcx)), vec![0x48, 0xF7, 0xF9]);
        assert_eq!(encode(|a| a.div(Reg::Rcx)), vec![0x48, 0xF7, 0xF1]);
        assert_eq!(encode(|a| a.neg(Reg::Rax)), vec![0x48, 0xF7, 0xD8]);
        assert_eq!(encode(|a| a.not(Reg::Rax)), vec![0x48, 0xF7, 0xD0]);
    }

    #[test]
    fn encodes_immediate_arithmetic() {
        assert_eq!(
            encode(|a| a.sub_imm(Reg::Rsp, 32)),
            vec![0x48, 0x81, 0xEC, 32, 0, 0, 0]
        );
        assert_eq!(
            encode(|a| a.add_imm(Reg::Rsp, 32)),
            vec![0x48, 0x81, 0xC4, 32, 0, 0, 0]
        );
        assert_eq!(encode(|a| a.cmp_imm(Reg::Rax, 0)), vec![0x48, 0x81, 0xF8, 0, 0, 0, 0]);
    }

    #[test]
    fn encodes_shifts_and_comparisons() {
        assert_eq!(encode(|a| a.shl_cl(Reg::Rax)), vec![0x48, 0xD3, 0xE0]);
        assert_eq!(encode(|a| a.shr_cl(Reg::Rax)), vec![0x48, 0xD3, 0xE8]);
        assert_eq!(encode(|a| a.sar_cl(Reg::Rax)), vec![0x48, 0xD3, 0xF8]);
        assert_eq!(encode(|a| a.cmp(Reg::Rax, Reg::Rcx)), vec![0x48, 0x39, 0xC8]);
        assert_eq!(encode(|a| a.setcc(Cond::Eq, Reg::Rax)), vec![0x40, 0x0F, 0x94, 0xC0]);
        assert_eq!(encode(|a| a.setcc(Cond::Lt, Reg::Rax)), vec![0x40, 0x0F, 0x9C, 0xC0]);
        assert_eq!(encode(|a| a.movzx8(Reg::Rax, Reg::Rax)), vec![0x48, 0x0F, 0xB6, 0xC0]);
    }

    #[test]
    fn encodes_extensions() {
        assert_eq!(encode(|a| a.movsx8(Reg::Rax, Reg::Rax)), vec![0x48, 0x0F, 0xBE, 0xC0]);
        assert_eq!(encode(|a| a.movsx16(Reg::Rax, Reg::Rax)), vec![0x48, 0x0F, 0xBF, 0xC0]);
        assert_eq!(encode(|a| a.movsx32(Reg::Rax, Reg::Rax)), vec![0x48, 0x63, 0xC0]);
        assert_eq!(encode(|a| a.movzx16(Reg::Rax, Reg::Rax)), vec![0x48, 0x0F, 0xB7, 0xC0]);
        // A 32-bit move zeroes the upper half, which is the cheapest zero
        // extension.
        assert_eq!(encode(|a| a.mov32(Reg::Rax, Reg::Rax)), vec![0x89, 0xC0]);
    }

    #[test]
    fn encodes_the_stack_and_syscalls() {
        assert_eq!(encode(|a| a.push(Reg::Rbp)), vec![0x55]);
        assert_eq!(encode(|a| a.pop(Reg::Rbp)), vec![0x5D]);
        assert_eq!(encode(|a| a.push(Reg::R12)), vec![0x41, 0x54]);
        assert_eq!(encode(|a| a.ret()), vec![0xC3]);
        assert_eq!(encode(|a| a.syscall()), vec![0x0F, 0x05]);
        assert_eq!(encode(|a| a.ud2()), vec![0x0F, 0x0B]);
    }

    #[test]
    fn patches_a_forward_jump() {
        let mut assembler = Assembler::new();
        let target = assembler.label();
        assembler.jmp(target);
        assembler.ret();
        assembler.bind(target);
        assembler.ret();

        let (code, _, _) = assembler.finish();
        // jmp rel32 is 5 bytes; the target is 1 byte past its end.
        assert_eq!(code, vec![0xE9, 0x01, 0x00, 0x00, 0x00, 0xC3, 0xC3]);
    }

    #[test]
    fn patches_a_backward_jump() {
        let mut assembler = Assembler::new();
        let target = assembler.label();
        assembler.bind(target);
        assembler.ret();
        assembler.jmp(target);

        let (code, _, _) = assembler.finish();
        // The jump ends at offset 6 and must reach offset 0, so rel32 is -6.
        assert_eq!(code, vec![0xC3, 0xE9, 0xFA, 0xFF, 0xFF, 0xFF]);
    }

    #[test]
    fn conditional_jumps_use_the_right_condition_code() {
        let mut assembler = Assembler::new();
        let target = assembler.label();
        assembler.jcc(Cond::Ne, target);
        assembler.bind(target);
        let (code, _, _) = assembler.finish();
        assert_eq!(code, vec![0x0F, 0x85, 0x00, 0x00, 0x00, 0x00]);
    }

    #[test]
    fn records_data_references_for_later_patching() {
        let mut assembler = Assembler::new();
        assembler.lea_rip(Reg::Rax, Reference::RoData(64));
        let (code, relocations, _) = assembler.finish();

        assert_eq!(code[..3], [0x48, 0x8D, 0x05]);
        assert_eq!(relocations.len(), 1);
        assert_eq!(relocations[0].at, 3);
        assert_eq!(relocations[0].next_instruction, 7);
        assert_eq!(relocations[0].reference, Reference::RoData(64));
    }

    #[test]
    fn records_symbol_offsets() {
        let mut assembler = Assembler::new();
        assembler.define_symbol("first");
        assembler.ret();
        assembler.define_symbol("second");
        assembler.ret();

        let (_, _, symbols) = assembler.finish();
        assert_eq!(symbols.get("first"), Some(&0));
        assert_eq!(symbols.get("second"), Some(&1));
    }
}
