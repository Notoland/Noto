//! The Noto runtime, emitted as x86-64 machine code.
//!
//! These routines are the only code in a Noto executable that is not generated
//! from Noto source. They talk to Linux through raw syscalls, so a program has
//! no dependency on libc or on any other runtime. The contract they implement
//! — names, signatures, and the layout of a string — is written down in
//! `noto-runtime`.
//!
//! # The allocator
//!
//! Noto 0.10 allocates from a bump pointer over regions obtained with `mmap`
//! and never frees. That is enough to run programs that build strings, and it
//! is deliberately the simplest thing that is correct while the memory model
//! is being designed; see `docs/rfcs/0002-memory-model.md`.

use super::encode::{Assembler, Cond, Label, Reg, Reference};
use noto_runtime::{
    Routine, ASSERT_FAILURE_MESSAGE, ASSERT_FAILURE_STATUS, FALSE_TEXT, HEAP_CHUNK_SIZE,
    INDEX_FAILURE_MESSAGE, INDEX_FAILURE_STATUS, LIST_CAPACITY_OFFSET, LIST_DATA_OFFSET,
    LIST_HEADER_SIZE, LIST_INITIAL_CAPACITY, LIST_LENGTH_OFFSET, OUT_OF_MEMORY_MESSAGE, OUT_OF_MEMORY_STATUS,
    STRING_DATA_OFFSET, STRING_LENGTH_OFFSET, TRUE_TEXT,
};
use std::collections::HashMap;

/// Linux x86-64 syscall numbers.
mod syscall {
    /// `write(fd, buffer, count)`
    pub const WRITE: i64 = 1;
    /// `exit(status)`
    pub const EXIT: i64 = 60;
    /// `mmap(addr, length, prot, flags, fd, offset)`
    pub const MMAP: i64 = 9;
    /// `read(fd, buffer, count)`
    pub const READ: i64 = 0;
    /// `open(path, flags, mode)`
    pub const OPEN: i64 = 2;
    /// `close(fd)`
    pub const CLOSE: i64 = 3;
    /// `lseek(fd, offset, whence)`
    pub const LSEEK: i64 = 8;
}

/// `O_WRONLY | O_CREAT | O_TRUNC`, what writing a whole file wants.
const O_WRITE_CREATE_TRUNCATE: i64 = 0x241;

/// `rw-r--r--`, the mode a new file is created with.
const NEW_FILE_MODE: i64 = 0o644;

/// `SEEK_END`, for measuring a file by seeking to its end.
const SEEK_END: i64 = 2;

/// `SEEK_SET`, for returning to the start of it.
const SEEK_SET: i64 = 0;

/// `PROT_READ | PROT_WRITE`
const PROT_READ_WRITE: i64 = 0x3;
/// `MAP_PRIVATE | MAP_ANONYMOUS`
const MAP_PRIVATE_ANONYMOUS: i64 = 0x22;

/// Standard output.
const STDOUT: i64 = 1;
/// Standard error.
const STDERR: i64 = 2;

/// Where the runtime's own state lives in the writable segment.
pub mod globals {
    /// Offset of the bump pointer: the next free byte of the current region.
    pub const HEAP_NEXT: u32 = 0;
    /// Offset of the limit: one past the last byte of the current region.
    pub const HEAP_END: u32 = 8;
    /// Offset of the stack pointer as the process was entered with it, which
    /// is where `argc` and `argv` are.
    pub const ENTRY_STACK: u32 = 16;
    /// Total size of the runtime's writable state.
    pub const SIZE: u64 = 24;
}

/// Constants the runtime itself needs in read-only memory.
pub struct RuntimeData {
    /// Offset of the newline byte.
    pub newline: u32,
    /// Offset of the assertion failure message.
    pub assert_message: u32,
    /// Its length.
    pub assert_message_len: u32,
    /// Offset of the out-of-range index message.
    pub index_message: u32,
    /// Its length.
    pub index_message_len: u32,
    /// Offset of the out-of-memory message.
    pub oom_message: u32,
    /// Its length.
    pub oom_message_len: u32,
    /// Offset of the string object for `true`.
    pub true_string: u32,
    /// Offset of the string object for `false`.
    pub false_string: u32,
}

/// Appends the runtime's constants to the read-only data section.
pub fn append_data(rodata: &mut Vec<u8>) -> RuntimeData {
    fn align(rodata: &mut Vec<u8>) {
        while rodata.len() % 8 != 0 {
            rodata.push(0);
        }
    }

    let newline = rodata.len() as u32;
    rodata.push(b'\n');

    let assert_message = rodata.len() as u32;
    rodata.extend_from_slice(ASSERT_FAILURE_MESSAGE.as_bytes());

    let index_message = rodata.len() as u32;
    rodata.extend_from_slice(INDEX_FAILURE_MESSAGE.as_bytes());

    let oom_message = rodata.len() as u32;
    rodata.extend_from_slice(OUT_OF_MEMORY_MESSAGE.as_bytes());

    align(rodata);
    let true_string = rodata.len() as u32;
    rodata.extend_from_slice(&(TRUE_TEXT.len() as u64).to_le_bytes());
    rodata.extend_from_slice(TRUE_TEXT.as_bytes());

    align(rodata);
    let false_string = rodata.len() as u32;
    rodata.extend_from_slice(&(FALSE_TEXT.len() as u64).to_le_bytes());
    rodata.extend_from_slice(FALSE_TEXT.as_bytes());

    align(rodata);

    RuntimeData {
        newline,
        assert_message,
        assert_message_len: ASSERT_FAILURE_MESSAGE.len() as u32,
        index_message,
        index_message_len: INDEX_FAILURE_MESSAGE.len() as u32,
        oom_message,
        oom_message_len: OUT_OF_MEMORY_MESSAGE.len() as u32,
        true_string,
        false_string,
    }
}

/// The labels of every runtime routine, so generated code can call them.
pub struct RuntimeLabels {
    labels: HashMap<Routine, Label>,
    /// A helper the file routines share, which is not a [`Routine`]: nothing
    /// outside this module calls it and nothing in Noto can name it.
    c_path: Label,
    /// The other file helper, building a Noto string from a terminated one.
    from_c_string: Label,
}

impl RuntimeLabels {
    /// Allocates one label per routine.
    pub fn new(assembler: &mut Assembler) -> Self {
        let mut labels = HashMap::new();
        for routine in Routine::all() {
            labels.insert(*routine, assembler.label());
        }
        RuntimeLabels {
            labels,
            c_path: assembler.label(),
            from_c_string: assembler.label(),
        }
    }

    /// The label of a routine.
    pub fn get(&self, routine: Routine) -> Label {
        self.labels[&routine]
    }
}

/// Emits every runtime routine.
///
/// `main_label` is the entry point `_start` calls; `main_returns_status` says
/// whether it produces the process exit status.
pub fn emit(
    assembler: &mut Assembler,
    labels: &RuntimeLabels,
    data: &RuntimeData,
    main_label: Option<Label>,
    main_returns_status: bool,
) {
    emit_start(assembler, labels, main_label, main_returns_status);
    emit_write(assembler, labels);
    emit_exit(assembler, labels);
    emit_alloc(assembler, labels, data);
    emit_print_string(assembler, labels, false);
    emit_print_string(assembler, labels, true);
    emit_newline(assembler, labels, data);
    emit_print_int(assembler, labels, false);
    emit_print_int(assembler, labels, true);
    emit_print_bool(assembler, labels, false);
    emit_print_bool(assembler, labels, true);
    emit_println_empty(assembler, labels);
    emit_int_to_string(assembler, labels);
    emit_bool_to_string(assembler, labels, data);
    emit_string_concat(assembler, labels);
    emit_string_length(assembler, labels);
    emit_string_equals(assembler, labels);
    emit_from_c_string(assembler, labels);
    emit_args(assembler, labels);
    emit_c_path(assembler, labels);
    emit_read_file(assembler, labels);
    emit_write_file(assembler, labels);
    emit_string_byte_at(assembler, labels);
    emit_string_slice(assembler, labels);
    emit_list_push(assembler, labels);
    emit_list_walk(assembler, labels, Routine::ListMap);
    emit_list_walk(assembler, labels, Routine::ListFilter);
    emit_list_walk(assembler, labels, Routine::ListEach);
    emit_index_check(assembler, labels, data);
    emit_assert(assembler, labels, data);
}

/// Starts a routine: binds its label and records its symbol.
fn begin(assembler: &mut Assembler, labels: &RuntimeLabels, routine: Routine) {
    assembler.bind(labels.get(routine));
    assembler.define_symbol(routine.symbol());
}

/// The standard prologue: save the frame pointer and reserve `locals` slots.
fn prologue(assembler: &mut Assembler, locals: i32) {
    assembler.push(Reg::Rbp);
    assembler.mov_reg_reg(Reg::Rbp, Reg::Rsp);
    if locals > 0 {
        // The stack stays 16-byte aligned, as the ABI requires at every call.
        let bytes = (locals * 8 + 15) & !15;
        assembler.sub_imm(Reg::Rsp, bytes);
    }
}

fn epilogue(assembler: &mut Assembler) {
    assembler.mov_reg_reg(Reg::Rsp, Reg::Rbp);
    assembler.pop(Reg::Rbp);
    assembler.ret();
}

/// `_start`: reserve the first heap region, run `main`, exit.
///
/// The process entry point is not a function: there is no return address on
/// the stack and nothing to return to, so it ends in `exit` rather than `ret`.
fn emit_start(
    assembler: &mut Assembler,
    labels: &RuntimeLabels,
    main_label: Option<Label>,
    main_returns_status: bool,
) {
    begin(assembler, labels, Routine::Start);

    // `argc` sits at the stack pointer the kernel entered with, and `argv`
    // right after it. Nothing has pushed yet, so this is the only moment that
    // address is known; `args` reads it back later.
    assembler.lea_rip(Reg::Rax, Reference::Data(globals::ENTRY_STACK));
    assembler.mov_mem_reg(Reg::Rax, 0, Reg::Rsp);

    // The stack is 16-byte aligned at entry but the ABI assumes a call has
    // pushed a return address, so one word is dropped to match.
    assembler.xor(Reg::Rbp, Reg::Rbp);
    assembler.sub_imm(Reg::Rsp, 8);

    match main_label {
        Some(main) => {
            assembler.call(main);
            if !main_returns_status {
                assembler.mov_reg_imm64(Reg::Rax, 0);
            }
            assembler.mov_reg_reg(Reg::Rdi, Reg::Rax);
        }
        None => {
            assembler.mov_reg_imm64(Reg::Rdi, 0);
        }
    }

    assembler.call(labels.get(Routine::Exit));
    assembler.ud2();
}

/// `write(fd, pointer, length)`.
fn emit_write(assembler: &mut Assembler, labels: &RuntimeLabels) {
    begin(assembler, labels, Routine::Write);
    // The syscall takes its arguments in the same registers the ABI passed
    // them in, so only the syscall number has to be set.
    assembler.mov_reg_imm64(Reg::Rax, syscall::WRITE);
    assembler.syscall();
    assembler.ret();
}

/// `exit(status)`.
fn emit_exit(assembler: &mut Assembler, labels: &RuntimeLabels) {
    begin(assembler, labels, Routine::Exit);
    assembler.mov_reg_imm64(Reg::Rax, syscall::EXIT);
    assembler.syscall();
    // exit never returns; if it somehow did, faulting is better than running
    // whatever follows.
    assembler.ud2();
}

/// `alloc(size) -> pointer`.
///
/// Rounds the request up to the heap alignment, takes it from the current
/// region when it fits, and asks the kernel for a new region when it does not.
fn emit_alloc(assembler: &mut Assembler, labels: &RuntimeLabels, data: &RuntimeData) {
    begin(assembler, labels, Routine::Alloc);
    prologue(assembler, 2);

    let size = -8; // the rounded request
    let fits = assembler.label();
    let out_of_memory = assembler.label();

    // rounded = (size + 7) & ~7
    assembler.add_imm(Reg::Rdi, 7);
    assembler.and_imm(Reg::Rdi, -8);
    assembler.mov_mem_reg(Reg::Rbp, size, Reg::Rdi);

    // rax = heap_next; rcx = rax + rounded
    assembler.lea_rip(Reg::Rdx, Reference::Data(globals::HEAP_NEXT));
    assembler.mov_reg_mem(Reg::Rax, Reg::Rdx, 0);
    assembler.mov_reg_reg(Reg::Rcx, Reg::Rax);
    assembler.add(Reg::Rcx, Reg::Rdi);

    // if rcx <= heap_end, the current region has room.
    assembler.lea_rip(Reg::Rdx, Reference::Data(globals::HEAP_END));
    assembler.mov_reg_mem(Reg::Rdx, Reg::Rdx, 0);
    assembler.cmp(Reg::Rcx, Reg::Rdx);
    assembler.jcc(Cond::BelowEq, fits);

    // Otherwise map a fresh region, at least a chunk and never smaller than
    // the request, so a large string is still served.
    assembler.mov_reg_mem(Reg::Rsi, Reg::Rbp, size);
    assembler.mov_reg_imm64(Reg::Rax, HEAP_CHUNK_SIZE as i64);
    assembler.cmp(Reg::Rsi, Reg::Rax);
    let large_enough = assembler.label();
    assembler.jcc(Cond::AboveEq, large_enough);
    assembler.mov_reg_reg(Reg::Rsi, Reg::Rax);
    assembler.bind(large_enough);

    // mmap(NULL, length, PROT_READ|PROT_WRITE, MAP_PRIVATE|MAP_ANONYMOUS, -1, 0)
    assembler.mov_reg_reg(Reg::Rcx, Reg::Rsi); // keep the length
    assembler.mov_reg_imm64(Reg::Rdi, 0);
    assembler.mov_reg_imm64(Reg::Rdx, PROT_READ_WRITE);
    assembler.mov_reg_imm64(Reg::R10, MAP_PRIVATE_ANONYMOUS);
    assembler.mov_reg_imm64(Reg::R8, -1);
    assembler.mov_reg_imm64(Reg::R9, 0);
    assembler.mov_reg_imm64(Reg::Rax, syscall::MMAP);
    assembler.push(Reg::Rcx);
    assembler.syscall();
    assembler.pop(Reg::Rcx);

    // mmap reports failure as a small negative value.
    assembler.cmp_imm(Reg::Rax, 0);
    assembler.jcc(Cond::Lt, out_of_memory);

    // heap_next = base + rounded; heap_end = base + length
    assembler.mov_reg_mem(Reg::Rdi, Reg::Rbp, size);
    assembler.mov_reg_reg(Reg::Rdx, Reg::Rax);
    assembler.add(Reg::Rdx, Reg::Rcx);
    assembler.lea_rip(Reg::Rsi, Reference::Data(globals::HEAP_END));
    assembler.mov_mem_reg(Reg::Rsi, 0, Reg::Rdx);
    assembler.mov_reg_reg(Reg::Rcx, Reg::Rax);
    assembler.add(Reg::Rcx, Reg::Rdi);

    assembler.bind(fits);
    // rax holds the allocation, rcx the new bump pointer.
    assembler.lea_rip(Reg::Rdx, Reference::Data(globals::HEAP_NEXT));
    assembler.mov_mem_reg(Reg::Rdx, 0, Reg::Rcx);
    epilogue(assembler);

    assembler.bind(out_of_memory);
    assembler.mov_reg_imm64(Reg::Rdi, STDERR);
    assembler.lea_rip(Reg::Rsi, Reference::RoData(data.oom_message));
    assembler.mov_reg_imm64(Reg::Rdx, data.oom_message_len as i64);
    assembler.call(labels.get(Routine::Write));
    assembler.mov_reg_imm64(Reg::Rdi, OUT_OF_MEMORY_STATUS as i64);
    assembler.call(labels.get(Routine::Exit));
    assembler.ud2();
}

/// `print_string(string)` and `println_string(string)`.
fn emit_print_string(assembler: &mut Assembler, labels: &RuntimeLabels, newline: bool) {
    let routine = if newline { Routine::PrintlnString } else { Routine::PrintString };
    begin(assembler, labels, routine);
    prologue(assembler, 0);

    let done = assembler.label();
    // A null string prints nothing, which is what a `String?` holding null
    // means; the line break is still written by `println`.
    assembler.cmp_imm(Reg::Rdi, 0);
    assembler.jcc(Cond::Eq, done);

    assembler.mov_reg_mem(Reg::Rdx, Reg::Rdi, STRING_LENGTH_OFFSET);
    assembler.mov_reg_reg(Reg::Rsi, Reg::Rdi);
    assembler.add_imm(Reg::Rsi, STRING_DATA_OFFSET);
    assembler.mov_reg_imm64(Reg::Rdi, STDOUT);
    assembler.call(labels.get(Routine::Write));

    assembler.bind(done);
    if newline {
        assembler.call(labels.get(Routine::Newline));
    }
    epilogue(assembler);
}

/// `newline()`: writes one line break.
fn emit_newline(assembler: &mut Assembler, labels: &RuntimeLabels, data: &RuntimeData) {
    begin(assembler, labels, Routine::Newline);
    prologue(assembler, 0);
    assembler.mov_reg_imm64(Reg::Rdi, STDOUT);
    assembler.lea_rip(Reg::Rsi, Reference::RoData(data.newline));
    assembler.mov_reg_imm64(Reg::Rdx, 1);
    assembler.call(labels.get(Routine::Write));
    epilogue(assembler);
}

/// `print_int(value)` and `println_int(value)`.
fn emit_print_int(assembler: &mut Assembler, labels: &RuntimeLabels, newline: bool) {
    let routine = if newline { Routine::PrintlnInt } else { Routine::PrintInt };
    begin(assembler, labels, routine);
    prologue(assembler, 0);
    assembler.call(labels.get(Routine::IntToString));
    assembler.mov_reg_reg(Reg::Rdi, Reg::Rax);
    let target = if newline { Routine::PrintlnString } else { Routine::PrintString };
    assembler.call(labels.get(target));
    epilogue(assembler);
}

/// `print_bool(value)` and `println_bool(value)`.
fn emit_print_bool(assembler: &mut Assembler, labels: &RuntimeLabels, newline: bool) {
    let routine = if newline { Routine::PrintlnBool } else { Routine::PrintBool };
    begin(assembler, labels, routine);
    prologue(assembler, 0);
    assembler.call(labels.get(Routine::BoolToString));
    assembler.mov_reg_reg(Reg::Rdi, Reg::Rax);
    let target = if newline { Routine::PrintlnString } else { Routine::PrintString };
    assembler.call(labels.get(target));
    epilogue(assembler);
}

/// `println_empty()`.
fn emit_println_empty(assembler: &mut Assembler, labels: &RuntimeLabels) {
    begin(assembler, labels, Routine::PrintlnEmpty);
    prologue(assembler, 0);
    assembler.call(labels.get(Routine::Newline));
    epilogue(assembler);
}

/// `int_to_string(value) -> string`.
///
/// Digits are produced least significant first into a buffer written from its
/// end backwards, which needs no reversal pass afterwards.
fn emit_int_to_string(assembler: &mut Assembler, labels: &RuntimeLabels) {
    begin(assembler, labels, Routine::IntToString);
    prologue(assembler, 6);

    let value = -8; // the remaining magnitude
    let object = -16; // the allocated string object
    let cursor = -24; // where the next digit goes
    let negative = -32; // 1 when a minus sign is needed

    assembler.mov_mem_reg(Reg::Rbp, value, Reg::Rdi);

    // A 64-bit integer needs at most 20 characters: 19 digits and a sign.
    assembler.mov_reg_imm64(Reg::Rdi, 8 + 20);
    assembler.call(labels.get(Routine::Alloc));
    assembler.mov_mem_reg(Reg::Rbp, object, Reg::Rax);

    // The cursor starts one past the last byte and moves backwards.
    assembler.mov_reg_reg(Reg::Rcx, Reg::Rax);
    assembler.add_imm(Reg::Rcx, 8 + 20);
    assembler.mov_mem_reg(Reg::Rbp, cursor, Reg::Rcx);

    // Record the sign and work with the magnitude.
    assembler.mov_reg_imm64(Reg::Rdx, 0);
    assembler.mov_mem_reg(Reg::Rbp, negative, Reg::Rdx);
    assembler.mov_reg_mem(Reg::Rax, Reg::Rbp, value);
    assembler.cmp_imm(Reg::Rax, 0);
    let non_negative = assembler.label();
    assembler.jcc(Cond::Ge, non_negative);
    assembler.mov_reg_imm64(Reg::Rdx, 1);
    assembler.mov_mem_reg(Reg::Rbp, negative, Reg::Rdx);
    // Negating i64::MIN overflows back to itself, so the magnitude is built by
    // negating each digit instead of the whole value.
    assembler.bind(non_negative);

    let loop_start = assembler.label();
    let loop_end = assembler.label();

    assembler.bind(loop_start);
    // rax:rdx / 10, with the remainder in rdx.
    assembler.mov_reg_mem(Reg::Rax, Reg::Rbp, value);
    assembler.cqo();
    assembler.mov_reg_imm64(Reg::Rcx, 10);
    assembler.idiv(Reg::Rcx);
    assembler.mov_mem_reg(Reg::Rbp, value, Reg::Rax);

    // The remainder has the sign of the dividend, so a negative value yields
    // negative digits; flipping them here handles i64::MIN without overflow.
    assembler.mov_reg_mem(Reg::Rcx, Reg::Rbp, negative);
    assembler.cmp_imm(Reg::Rcx, 0);
    let digit_ready = assembler.label();
    assembler.jcc(Cond::Eq, digit_ready);
    assembler.neg(Reg::Rdx);
    assembler.bind(digit_ready);

    assembler.add_imm(Reg::Rdx, b'0' as i32);
    assembler.mov_reg_mem(Reg::Rcx, Reg::Rbp, cursor);
    assembler.sub_imm(Reg::Rcx, 1);
    assembler.mov_mem_reg(Reg::Rbp, cursor, Reg::Rcx);
    assembler.mov_mem_reg8(Reg::Rcx, 0, Reg::Rdx);

    assembler.mov_reg_mem(Reg::Rax, Reg::Rbp, value);
    assembler.cmp_imm(Reg::Rax, 0);
    assembler.jcc(Cond::Ne, loop_start);
    assembler.bind(loop_end);

    // Prepend the sign.
    assembler.mov_reg_mem(Reg::Rcx, Reg::Rbp, negative);
    assembler.cmp_imm(Reg::Rcx, 0);
    let no_sign = assembler.label();
    assembler.jcc(Cond::Eq, no_sign);
    assembler.mov_reg_mem(Reg::Rcx, Reg::Rbp, cursor);
    assembler.sub_imm(Reg::Rcx, 1);
    assembler.mov_mem_reg(Reg::Rbp, cursor, Reg::Rcx);
    assembler.mov_reg_imm64(Reg::Rdx, b'-' as i64);
    assembler.mov_mem_reg8(Reg::Rcx, 0, Reg::Rdx);
    assembler.bind(no_sign);

    // The text sits at the end of the buffer, so the object is moved to start
    // right before it: length field, then the digits.
    assembler.mov_reg_mem(Reg::Rax, Reg::Rbp, object);
    assembler.add_imm(Reg::Rax, 8 + 20);
    assembler.mov_reg_mem(Reg::Rcx, Reg::Rbp, cursor);
    assembler.sub(Reg::Rax, Reg::Rcx); // rax = length
    assembler.mov_reg_reg(Reg::Rdx, Reg::Rcx);
    assembler.sub_imm(Reg::Rdx, 8); // rdx = object start
    assembler.mov_mem_reg(Reg::Rdx, 0, Reg::Rax);
    assembler.mov_reg_reg(Reg::Rax, Reg::Rdx);

    epilogue(assembler);
}

/// `bool_to_string(value) -> string`, returning a shared constant.
fn emit_bool_to_string(assembler: &mut Assembler, labels: &RuntimeLabels, data: &RuntimeData) {
    begin(assembler, labels, Routine::BoolToString);
    let is_false = assembler.label();
    assembler.cmp_imm(Reg::Rdi, 0);
    assembler.jcc(Cond::Eq, is_false);
    assembler.lea_rip(Reg::Rax, Reference::RoData(data.true_string));
    assembler.ret();
    assembler.bind(is_false);
    assembler.lea_rip(Reg::Rax, Reference::RoData(data.false_string));
    assembler.ret();
}

/// `string_concat(left, right) -> string`.
fn emit_string_concat(assembler: &mut Assembler, labels: &RuntimeLabels) {
    begin(assembler, labels, Routine::StringConcat);
    prologue(assembler, 6);

    let left = -8;
    let right = -16;
    let result = -24;
    let left_len = -32;
    let right_len = -40;

    assembler.mov_mem_reg(Reg::Rbp, left, Reg::Rdi);
    assembler.mov_mem_reg(Reg::Rbp, right, Reg::Rsi);

    // A null operand contributes nothing, so `null + "x"` is `"x"` rather than
    // a fault.
    let left_null = assembler.label();
    let have_left_len = assembler.label();
    assembler.cmp_imm(Reg::Rdi, 0);
    assembler.jcc(Cond::Eq, left_null);
    assembler.mov_reg_mem(Reg::Rax, Reg::Rdi, STRING_LENGTH_OFFSET);
    assembler.jmp(have_left_len);
    assembler.bind(left_null);
    assembler.mov_reg_imm64(Reg::Rax, 0);
    assembler.bind(have_left_len);
    assembler.mov_mem_reg(Reg::Rbp, left_len, Reg::Rax);

    let right_null = assembler.label();
    let have_right_len = assembler.label();
    assembler.cmp_imm(Reg::Rsi, 0);
    assembler.jcc(Cond::Eq, right_null);
    assembler.mov_reg_mem(Reg::Rcx, Reg::Rsi, STRING_LENGTH_OFFSET);
    assembler.jmp(have_right_len);
    assembler.bind(right_null);
    assembler.mov_reg_imm64(Reg::Rcx, 0);
    assembler.bind(have_right_len);
    assembler.mov_mem_reg(Reg::Rbp, right_len, Reg::Rcx);

    // alloc(8 + left + right)
    assembler.mov_reg_reg(Reg::Rdi, Reg::Rax);
    assembler.add(Reg::Rdi, Reg::Rcx);
    assembler.add_imm(Reg::Rdi, STRING_DATA_OFFSET);
    assembler.call(labels.get(Routine::Alloc));
    assembler.mov_mem_reg(Reg::Rbp, result, Reg::Rax);

    // Write the combined length.
    assembler.mov_reg_mem(Reg::Rcx, Reg::Rbp, left_len);
    assembler.mov_reg_mem(Reg::Rdx, Reg::Rbp, right_len);
    assembler.add(Reg::Rcx, Reg::Rdx);
    assembler.mov_mem_reg(Reg::Rax, STRING_LENGTH_OFFSET, Reg::Rcx);

    // Copy both halves one byte at a time. A word-at-a-time copy is a later
    // optimization; correctness first.
    copy_bytes(assembler, left, left_len, STRING_DATA_OFFSET, result);
    copy_bytes_after(assembler, right, right_len, left_len, result);

    assembler.mov_reg_mem(Reg::Rax, Reg::Rbp, result);
    epilogue(assembler);
}

/// Copies a string's bytes to a fixed offset in the result.
fn copy_bytes(
    assembler: &mut Assembler,
    source_slot: i32,
    length_slot: i32,
    destination_offset: i32,
    result_slot: i32,
) {
    let start = assembler.label();
    let done = assembler.label();

    assembler.mov_reg_mem(Reg::Rsi, Reg::Rbp, source_slot);
    assembler.cmp_imm(Reg::Rsi, 0);
    assembler.jcc(Cond::Eq, done);
    assembler.add_imm(Reg::Rsi, STRING_DATA_OFFSET);
    assembler.mov_reg_mem(Reg::Rdi, Reg::Rbp, result_slot);
    assembler.add_imm(Reg::Rdi, destination_offset);
    assembler.mov_reg_mem(Reg::Rcx, Reg::Rbp, length_slot);

    assembler.bind(start);
    assembler.cmp_imm(Reg::Rcx, 0);
    assembler.jcc(Cond::Eq, done);
    assembler.movzx_reg_mem8(Reg::Rax, Reg::Rsi, 0);
    assembler.mov_mem_reg8(Reg::Rdi, 0, Reg::Rax);
    assembler.add_imm(Reg::Rsi, 1);
    assembler.add_imm(Reg::Rdi, 1);
    assembler.sub_imm(Reg::Rcx, 1);
    assembler.jmp(start);
    assembler.bind(done);
}

/// Copies a string's bytes after another string's, whose length is in a slot.
fn copy_bytes_after(
    assembler: &mut Assembler,
    source_slot: i32,
    length_slot: i32,
    offset_slot: i32,
    result_slot: i32,
) {
    let start = assembler.label();
    let done = assembler.label();

    assembler.mov_reg_mem(Reg::Rsi, Reg::Rbp, source_slot);
    assembler.cmp_imm(Reg::Rsi, 0);
    assembler.jcc(Cond::Eq, done);
    assembler.add_imm(Reg::Rsi, STRING_DATA_OFFSET);
    assembler.mov_reg_mem(Reg::Rdi, Reg::Rbp, result_slot);
    assembler.add_imm(Reg::Rdi, STRING_DATA_OFFSET);
    assembler.mov_reg_mem(Reg::Rdx, Reg::Rbp, offset_slot);
    assembler.add(Reg::Rdi, Reg::Rdx);
    assembler.mov_reg_mem(Reg::Rcx, Reg::Rbp, length_slot);

    assembler.bind(start);
    assembler.cmp_imm(Reg::Rcx, 0);
    assembler.jcc(Cond::Eq, done);
    assembler.movzx_reg_mem8(Reg::Rax, Reg::Rsi, 0);
    assembler.mov_mem_reg8(Reg::Rdi, 0, Reg::Rax);
    assembler.add_imm(Reg::Rsi, 1);
    assembler.add_imm(Reg::Rdi, 1);
    assembler.sub_imm(Reg::Rcx, 1);
    assembler.jmp(start);
    assembler.bind(done);
}

/// `string_length(string) -> length`.
fn emit_string_length(assembler: &mut Assembler, labels: &RuntimeLabels) {
    begin(assembler, labels, Routine::StringLength);
    let null = assembler.label();
    assembler.cmp_imm(Reg::Rdi, 0);
    assembler.jcc(Cond::Eq, null);
    assembler.mov_reg_mem(Reg::Rax, Reg::Rdi, STRING_LENGTH_OFFSET);
    assembler.ret();
    assembler.bind(null);
    assembler.mov_reg_imm64(Reg::Rax, 0);
    assembler.ret();
}

/// `string_equals(left, right) -> bool`.
///
/// Compares length and then bytes. Comparing addresses would make two
/// strings built different ways from the same characters unequal, which is
/// not a distinction a Noto program can see or reason about.
fn emit_string_equals(assembler: &mut Assembler, labels: &RuntimeLabels) {
    begin(assembler, labels, Routine::StringEquals);

    let equal = assembler.label();
    let different = assembler.label();
    let start = assembler.label();

    // The same object, null included, is equal to itself without a read.
    assembler.mov_reg_reg(Reg::Rax, Reg::Rdi);
    assembler.sub(Reg::Rax, Reg::Rsi);
    assembler.cmp_imm(Reg::Rax, 0);
    assembler.jcc(Cond::Eq, equal);

    // One null and one not cannot be equal, and must not be dereferenced.
    assembler.cmp_imm(Reg::Rdi, 0);
    assembler.jcc(Cond::Eq, different);
    assembler.cmp_imm(Reg::Rsi, 0);
    assembler.jcc(Cond::Eq, different);

    // Different lengths end it before any byte is read.
    assembler.mov_reg_mem(Reg::Rcx, Reg::Rdi, STRING_LENGTH_OFFSET);
    assembler.mov_reg_mem(Reg::Rdx, Reg::Rsi, STRING_LENGTH_OFFSET);
    assembler.mov_reg_reg(Reg::Rax, Reg::Rcx);
    assembler.sub(Reg::Rax, Reg::Rdx);
    assembler.cmp_imm(Reg::Rax, 0);
    assembler.jcc(Cond::Ne, different);

    assembler.add_imm(Reg::Rdi, STRING_DATA_OFFSET);
    assembler.add_imm(Reg::Rsi, STRING_DATA_OFFSET);

    assembler.bind(start);
    assembler.cmp_imm(Reg::Rcx, 0);
    assembler.jcc(Cond::Eq, equal);
    assembler.movzx_reg_mem8(Reg::Rax, Reg::Rdi, 0);
    assembler.movzx_reg_mem8(Reg::Rdx, Reg::Rsi, 0);
    assembler.sub(Reg::Rax, Reg::Rdx);
    assembler.cmp_imm(Reg::Rax, 0);
    assembler.jcc(Cond::Ne, different);
    assembler.add_imm(Reg::Rdi, 1);
    assembler.add_imm(Reg::Rsi, 1);
    assembler.sub_imm(Reg::Rcx, 1);
    assembler.jmp(start);

    assembler.bind(equal);
    assembler.mov_reg_imm64(Reg::Rax, 1);
    assembler.ret();

    assembler.bind(different);
    assembler.mov_reg_imm64(Reg::Rax, 0);
    assembler.ret();
}

/// Multiplies a register by the size of one element.
///
/// Three doublings rather than a shift: the encoder has `add` and this needs
/// no new instruction to be encoded and tested for one use.
fn times_eight(assembler: &mut Assembler, reg: Reg) {
    assembler.add(reg, reg);
    assembler.add(reg, reg);
    assembler.add(reg, reg);
}

/// Builds a Noto string from a NUL-terminated one.
///
/// The reverse of [`emit_c_path`]: what the kernel hands a process is
/// terminated, and what Noto passes around carries its length.
fn emit_from_c_string(assembler: &mut Assembler, labels: &RuntimeLabels) {
    assembler.bind(labels.from_c_string);
    prologue(assembler, 3);

    let source = -8;
    let length = -16;
    let result = -24;
    assembler.mov_mem_reg(Reg::Rbp, source, Reg::Rdi);

    // Measure it.
    let scan = assembler.label();
    let measured = assembler.label();
    assembler.mov_reg_imm64(Reg::Rcx, 0);
    assembler.bind(scan);
    assembler.mov_reg_mem(Reg::Rsi, Reg::Rbp, source);
    assembler.add(Reg::Rsi, Reg::Rcx);
    assembler.movzx_reg_mem8(Reg::Rax, Reg::Rsi, 0);
    assembler.cmp_imm(Reg::Rax, 0);
    assembler.jcc(Cond::Eq, measured);
    assembler.add_imm(Reg::Rcx, 1);
    assembler.jmp(scan);
    assembler.bind(measured);
    assembler.mov_mem_reg(Reg::Rbp, length, Reg::Rcx);

    assembler.mov_reg_reg(Reg::Rdi, Reg::Rcx);
    assembler.add_imm(Reg::Rdi, STRING_DATA_OFFSET);
    assembler.call(labels.get(Routine::Alloc));
    assembler.mov_mem_reg(Reg::Rbp, result, Reg::Rax);
    assembler.mov_reg_mem(Reg::Rcx, Reg::Rbp, length);
    assembler.mov_mem_reg(Reg::Rax, STRING_LENGTH_OFFSET, Reg::Rcx);

    assembler.mov_reg_mem(Reg::Rsi, Reg::Rbp, source);
    assembler.mov_reg_reg(Reg::Rdi, Reg::Rax);
    assembler.add_imm(Reg::Rdi, STRING_DATA_OFFSET);

    let copy = assembler.label();
    let done = assembler.label();
    assembler.bind(copy);
    assembler.cmp_imm(Reg::Rcx, 0);
    assembler.jcc(Cond::Eq, done);
    assembler.movzx_reg_mem8(Reg::Rax, Reg::Rsi, 0);
    assembler.mov_mem_reg8(Reg::Rdi, 0, Reg::Rax);
    assembler.add_imm(Reg::Rsi, 1);
    assembler.add_imm(Reg::Rdi, 1);
    assembler.sub_imm(Reg::Rcx, 1);
    assembler.jmp(copy);
    assembler.bind(done);

    assembler.mov_reg_mem(Reg::Rax, Reg::Rbp, result);
    epilogue(assembler);
}

/// `args() -> [String]`: the command line, the program's own name first.
fn emit_args(assembler: &mut Assembler, labels: &RuntimeLabels) {
    begin(assembler, labels, Routine::Args);
    prologue(assembler, 6);

    let count = -8;
    let vector = -16;
    let buffer = -24;
    let list = -32;
    let index = -40;

    assembler.lea_rip(Reg::Rax, Reference::Data(globals::ENTRY_STACK));
    assembler.mov_reg_mem(Reg::Rax, Reg::Rax, 0);
    assembler.mov_reg_mem(Reg::Rcx, Reg::Rax, 0);
    assembler.mov_mem_reg(Reg::Rbp, count, Reg::Rcx);
    assembler.add_imm(Reg::Rax, 8);
    assembler.mov_mem_reg(Reg::Rbp, vector, Reg::Rax);

    // The buffer is never empty, so a list of no arguments can still be
    // pushed to.
    assembler.mov_reg_reg(Reg::Rdi, Reg::Rcx);
    let sized = assembler.label();
    assembler.cmp_imm(Reg::Rdi, 0);
    assembler.jcc(Cond::Ne, sized);
    assembler.mov_reg_imm64(Reg::Rdi, 1);
    assembler.bind(sized);
    times_eight(assembler, Reg::Rdi);
    assembler.call(labels.get(Routine::Alloc));
    assembler.mov_mem_reg(Reg::Rbp, buffer, Reg::Rax);

    assembler.mov_reg_imm64(Reg::Rdi, LIST_HEADER_SIZE as i64);
    assembler.call(labels.get(Routine::Alloc));
    assembler.mov_mem_reg(Reg::Rbp, list, Reg::Rax);
    assembler.mov_reg_mem(Reg::Rcx, Reg::Rbp, count);
    assembler.mov_mem_reg(Reg::Rax, LIST_LENGTH_OFFSET, Reg::Rcx);
    let capacity = assembler.label();
    assembler.cmp_imm(Reg::Rcx, 0);
    assembler.jcc(Cond::Ne, capacity);
    assembler.mov_reg_imm64(Reg::Rcx, 1);
    assembler.bind(capacity);
    assembler.mov_mem_reg(Reg::Rax, LIST_CAPACITY_OFFSET, Reg::Rcx);
    assembler.mov_reg_mem(Reg::Rdx, Reg::Rbp, buffer);
    assembler.mov_mem_reg(Reg::Rax, LIST_DATA_OFFSET, Reg::Rdx);

    assembler.mov_reg_imm64(Reg::Rax, 0);
    assembler.mov_mem_reg(Reg::Rbp, index, Reg::Rax);

    let each = assembler.label();
    let finished = assembler.label();
    assembler.bind(each);
    assembler.mov_reg_mem(Reg::Rax, Reg::Rbp, index);
    assembler.mov_reg_mem(Reg::Rcx, Reg::Rbp, count);
    assembler.cmp(Reg::Rax, Reg::Rcx);
    assembler.jcc(Cond::Ge, finished);

    assembler.mov_reg_reg(Reg::Rdx, Reg::Rax);
    times_eight(assembler, Reg::Rdx);
    assembler.mov_reg_mem(Reg::Rsi, Reg::Rbp, vector);
    assembler.add(Reg::Rsi, Reg::Rdx);
    assembler.mov_reg_mem(Reg::Rdi, Reg::Rsi, 0);
    assembler.call(labels.from_c_string);

    assembler.mov_reg_mem(Reg::Rcx, Reg::Rbp, index);
    assembler.mov_reg_reg(Reg::Rdx, Reg::Rcx);
    times_eight(assembler, Reg::Rdx);
    assembler.mov_reg_mem(Reg::Rsi, Reg::Rbp, buffer);
    assembler.add(Reg::Rsi, Reg::Rdx);
    assembler.mov_mem_reg(Reg::Rsi, 0, Reg::Rax);

    assembler.add_imm(Reg::Rcx, 1);
    assembler.mov_mem_reg(Reg::Rbp, index, Reg::Rcx);
    assembler.jmp(each);
    assembler.bind(finished);

    assembler.mov_reg_mem(Reg::Rax, Reg::Rbp, list);
    epilogue(assembler);
}

/// Copies a string's bytes into a NUL-terminated buffer and returns it.
///
/// Linux takes a path as a NUL-terminated C string; a Noto `String` carries
/// its length instead and is not terminated. This is the one place the two
/// representations meet, so every file routine goes through it.
///
/// Not a [`Routine`]: nothing outside this module may call it, and nothing
/// in Noto can name it.
fn emit_c_path(assembler: &mut Assembler, labels: &RuntimeLabels) {
    assembler.bind(labels.c_path);
    prologue(assembler, 2);

    let path = -8;
    let buffer = -16;
    assembler.mov_mem_reg(Reg::Rbp, path, Reg::Rdi);

    assembler.mov_reg_mem(Reg::Rdi, Reg::Rdi, STRING_LENGTH_OFFSET);
    assembler.add_imm(Reg::Rdi, 1);
    assembler.call(labels.get(Routine::Alloc));
    assembler.mov_mem_reg(Reg::Rbp, buffer, Reg::Rax);

    assembler.mov_reg_mem(Reg::Rsi, Reg::Rbp, path);
    assembler.mov_reg_mem(Reg::Rcx, Reg::Rsi, STRING_LENGTH_OFFSET);
    assembler.add_imm(Reg::Rsi, STRING_DATA_OFFSET);
    assembler.mov_reg_reg(Reg::Rdi, Reg::Rax);

    let copy = assembler.label();
    let done = assembler.label();
    assembler.bind(copy);
    assembler.cmp_imm(Reg::Rcx, 0);
    assembler.jcc(Cond::Eq, done);
    assembler.movzx_reg_mem8(Reg::Rax, Reg::Rsi, 0);
    assembler.mov_mem_reg8(Reg::Rdi, 0, Reg::Rax);
    assembler.add_imm(Reg::Rsi, 1);
    assembler.add_imm(Reg::Rdi, 1);
    assembler.sub_imm(Reg::Rcx, 1);
    assembler.jmp(copy);
    assembler.bind(done);

    assembler.mov_reg_imm64(Reg::Rax, 0);
    assembler.mov_mem_reg8(Reg::Rdi, 0, Reg::Rax);

    assembler.mov_reg_mem(Reg::Rax, Reg::Rbp, buffer);
    epilogue(assembler);
}

/// `read_file(path) -> string`, or null when the file cannot be read.
///
/// The size comes from seeking to the end rather than from `fstat`, which
/// would mean depending on the layout of a kernel struct. A file that cannot
/// be opened, measured or read gives null; deciding what to do about that is
/// the program's business, and `String?` is how the language says so.
fn emit_read_file(assembler: &mut Assembler, labels: &RuntimeLabels) {
    begin(assembler, labels, Routine::ReadFile);
    prologue(assembler, 4);

    let fd = -8;
    let size = -16;
    let result = -24;

    let failed = assembler.label();
    let close_and_fail = assembler.label();

    assembler.call(labels.c_path);

    // open(path, O_RDONLY, 0)
    assembler.mov_reg_reg(Reg::Rdi, Reg::Rax);
    assembler.mov_reg_imm64(Reg::Rsi, 0);
    assembler.mov_reg_imm64(Reg::Rdx, 0);
    assembler.mov_reg_imm64(Reg::Rax, syscall::OPEN);
    assembler.syscall();
    assembler.cmp_imm(Reg::Rax, 0);
    assembler.jcc(Cond::Lt, failed);
    assembler.mov_mem_reg(Reg::Rbp, fd, Reg::Rax);

    // lseek(fd, 0, SEEK_END) measures it.
    assembler.mov_reg_reg(Reg::Rdi, Reg::Rax);
    assembler.mov_reg_imm64(Reg::Rsi, 0);
    assembler.mov_reg_imm64(Reg::Rdx, SEEK_END);
    assembler.mov_reg_imm64(Reg::Rax, syscall::LSEEK);
    assembler.syscall();
    assembler.cmp_imm(Reg::Rax, 0);
    assembler.jcc(Cond::Lt, close_and_fail);
    assembler.mov_mem_reg(Reg::Rbp, size, Reg::Rax);

    assembler.mov_reg_mem(Reg::Rdi, Reg::Rbp, fd);
    assembler.mov_reg_imm64(Reg::Rsi, 0);
    assembler.mov_reg_imm64(Reg::Rdx, SEEK_SET);
    assembler.mov_reg_imm64(Reg::Rax, syscall::LSEEK);
    assembler.syscall();

    // The string object holds the length and then the bytes.
    assembler.mov_reg_mem(Reg::Rdi, Reg::Rbp, size);
    assembler.add_imm(Reg::Rdi, STRING_DATA_OFFSET);
    assembler.call(labels.get(Routine::Alloc));
    assembler.mov_mem_reg(Reg::Rbp, result, Reg::Rax);
    assembler.mov_reg_mem(Reg::Rcx, Reg::Rbp, size);
    assembler.mov_mem_reg(Reg::Rax, STRING_LENGTH_OFFSET, Reg::Rcx);

    // read(fd, data, size), repeated until the file is exhausted: one read is
    // allowed to return less than was asked for.
    let loop_start = assembler.label();
    let loop_done = assembler.label();
    let remaining = -32;
    assembler.mov_reg_mem(Reg::Rcx, Reg::Rbp, size);
    assembler.mov_mem_reg(Reg::Rbp, remaining, Reg::Rcx);
    assembler.mov_reg_mem(Reg::Rax, Reg::Rbp, result);
    assembler.add_imm(Reg::Rax, STRING_DATA_OFFSET);
    assembler.mov_reg_reg(Reg::Rsi, Reg::Rax);

    assembler.bind(loop_start);
    assembler.mov_reg_mem(Reg::Rdx, Reg::Rbp, remaining);
    assembler.cmp_imm(Reg::Rdx, 0);
    assembler.jcc(Cond::Le, loop_done);
    assembler.mov_reg_mem(Reg::Rdi, Reg::Rbp, fd);
    assembler.mov_reg_imm64(Reg::Rax, syscall::READ);
    assembler.syscall();
    assembler.cmp_imm(Reg::Rax, 0);
    assembler.jcc(Cond::Le, loop_done);
    assembler.add(Reg::Rsi, Reg::Rax);
    assembler.mov_reg_mem(Reg::Rcx, Reg::Rbp, remaining);
    assembler.sub(Reg::Rcx, Reg::Rax);
    assembler.mov_mem_reg(Reg::Rbp, remaining, Reg::Rcx);
    assembler.jmp(loop_start);
    assembler.bind(loop_done);

    assembler.mov_reg_mem(Reg::Rdi, Reg::Rbp, fd);
    assembler.mov_reg_imm64(Reg::Rax, syscall::CLOSE);
    assembler.syscall();

    assembler.mov_reg_mem(Reg::Rax, Reg::Rbp, result);
    epilogue(assembler);

    assembler.bind(close_and_fail);
    assembler.mov_reg_mem(Reg::Rdi, Reg::Rbp, fd);
    assembler.mov_reg_imm64(Reg::Rax, syscall::CLOSE);
    assembler.syscall();

    assembler.bind(failed);
    assembler.mov_reg_imm64(Reg::Rax, 0);
    epilogue(assembler);
}

/// `write_file(path, contents) -> bool`.
///
/// True only when every byte reached the file: a short write that is not
/// retried is a truncated file, and reporting success for one would be worse
/// than reporting failure.
fn emit_write_file(assembler: &mut Assembler, labels: &RuntimeLabels) {
    begin(assembler, labels, Routine::WriteFile);
    prologue(assembler, 4);

    let contents = -8;
    let fd = -16;
    let cursor = -24;
    let remaining = -32;

    let failed = assembler.label();
    let close_and_fail = assembler.label();

    assembler.mov_mem_reg(Reg::Rbp, contents, Reg::Rsi);

    assembler.call(labels.c_path);
    assembler.mov_reg_reg(Reg::Rdi, Reg::Rax);
    assembler.mov_reg_imm64(Reg::Rsi, O_WRITE_CREATE_TRUNCATE);
    assembler.mov_reg_imm64(Reg::Rdx, NEW_FILE_MODE);
    assembler.mov_reg_imm64(Reg::Rax, syscall::OPEN);
    assembler.syscall();
    assembler.cmp_imm(Reg::Rax, 0);
    assembler.jcc(Cond::Lt, failed);
    assembler.mov_mem_reg(Reg::Rbp, fd, Reg::Rax);

    assembler.mov_reg_mem(Reg::Rsi, Reg::Rbp, contents);
    assembler.mov_reg_mem(Reg::Rcx, Reg::Rsi, STRING_LENGTH_OFFSET);
    assembler.mov_mem_reg(Reg::Rbp, remaining, Reg::Rcx);
    assembler.add_imm(Reg::Rsi, STRING_DATA_OFFSET);
    assembler.mov_mem_reg(Reg::Rbp, cursor, Reg::Rsi);

    let loop_start = assembler.label();
    let loop_done = assembler.label();
    assembler.bind(loop_start);
    assembler.mov_reg_mem(Reg::Rdx, Reg::Rbp, remaining);
    assembler.cmp_imm(Reg::Rdx, 0);
    assembler.jcc(Cond::Le, loop_done);
    assembler.mov_reg_mem(Reg::Rdi, Reg::Rbp, fd);
    assembler.mov_reg_mem(Reg::Rsi, Reg::Rbp, cursor);
    assembler.mov_reg_imm64(Reg::Rax, syscall::WRITE);
    assembler.syscall();
    assembler.cmp_imm(Reg::Rax, 0);
    assembler.jcc(Cond::Le, close_and_fail);
    assembler.mov_reg_mem(Reg::Rcx, Reg::Rbp, cursor);
    assembler.add(Reg::Rcx, Reg::Rax);
    assembler.mov_mem_reg(Reg::Rbp, cursor, Reg::Rcx);
    assembler.mov_reg_mem(Reg::Rcx, Reg::Rbp, remaining);
    assembler.sub(Reg::Rcx, Reg::Rax);
    assembler.mov_mem_reg(Reg::Rbp, remaining, Reg::Rcx);
    assembler.jmp(loop_start);
    assembler.bind(loop_done);

    assembler.mov_reg_mem(Reg::Rdi, Reg::Rbp, fd);
    assembler.mov_reg_imm64(Reg::Rax, syscall::CLOSE);
    assembler.syscall();
    assembler.mov_reg_imm64(Reg::Rax, 1);
    epilogue(assembler);

    assembler.bind(close_and_fail);
    assembler.mov_reg_mem(Reg::Rdi, Reg::Rbp, fd);
    assembler.mov_reg_imm64(Reg::Rax, syscall::CLOSE);
    assembler.syscall();

    assembler.bind(failed);
    assembler.mov_reg_imm64(Reg::Rax, 0);
    epilogue(assembler);
}

/// `string_byte_at(string, index) -> byte`.
///
/// Bytes, not characters: a `String` holds UTF-8, so the byte at an index is
/// not always a whole character. The index is checked against the length.
fn emit_string_byte_at(assembler: &mut Assembler, labels: &RuntimeLabels) {
    begin(assembler, labels, Routine::StringByteAt);
    prologue(assembler, 2);

    let string = -8;
    let index = -16;
    assembler.mov_mem_reg(Reg::Rbp, string, Reg::Rdi);
    assembler.mov_mem_reg(Reg::Rbp, index, Reg::Rsi);

    assembler.mov_reg_mem(Reg::Rsi, Reg::Rdi, STRING_LENGTH_OFFSET);
    assembler.mov_reg_mem(Reg::Rdi, Reg::Rbp, index);
    assembler.call(labels.get(Routine::IndexCheck));

    assembler.mov_reg_mem(Reg::Rax, Reg::Rbp, string);
    assembler.add_imm(Reg::Rax, STRING_DATA_OFFSET);
    assembler.mov_reg_mem(Reg::Rcx, Reg::Rbp, index);
    assembler.add(Reg::Rax, Reg::Rcx);
    assembler.movzx_reg_mem8(Reg::Rax, Reg::Rax, 0);

    epilogue(assembler);
}

/// `string_slice(string, start, end) -> string`.
///
/// Both bounds are checked, and both may equal the length: a slice ending at
/// the end is the common case. `index_check` does the reporting, so an
/// out-of-range slice fails exactly the way an out-of-range index does.
fn emit_string_slice(assembler: &mut Assembler, labels: &RuntimeLabels) {
    begin(assembler, labels, Routine::StringSlice);
    prologue(assembler, 4);

    let string = -8;
    let start = -16;
    let end = -24;
    let result = -32;

    assembler.mov_mem_reg(Reg::Rbp, string, Reg::Rdi);
    assembler.mov_mem_reg(Reg::Rbp, start, Reg::Rsi);
    assembler.mov_mem_reg(Reg::Rbp, end, Reg::Rdx);

    // A valid end is 0..=length, so it is checked against length + 1.
    assembler.mov_reg_mem(Reg::Rsi, Reg::Rdi, STRING_LENGTH_OFFSET);
    assembler.add_imm(Reg::Rsi, 1);
    assembler.mov_reg_mem(Reg::Rdi, Reg::Rbp, end);
    assembler.call(labels.get(Routine::IndexCheck));

    // A valid start is 0..=end, which also rejects a start past the end.
    assembler.mov_reg_mem(Reg::Rsi, Reg::Rbp, end);
    assembler.add_imm(Reg::Rsi, 1);
    assembler.mov_reg_mem(Reg::Rdi, Reg::Rbp, start);
    assembler.call(labels.get(Routine::IndexCheck));

    // alloc(8 + end - start)
    assembler.mov_reg_mem(Reg::Rdi, Reg::Rbp, end);
    assembler.mov_reg_mem(Reg::Rcx, Reg::Rbp, start);
    assembler.sub(Reg::Rdi, Reg::Rcx);
    assembler.mov_reg_reg(Reg::Rdx, Reg::Rdi);
    assembler.add_imm(Reg::Rdi, STRING_DATA_OFFSET);
    assembler.call(labels.get(Routine::Alloc));
    assembler.mov_mem_reg(Reg::Rbp, result, Reg::Rax);

    assembler.mov_reg_mem(Reg::Rcx, Reg::Rbp, end);
    assembler.mov_reg_mem(Reg::Rdx, Reg::Rbp, start);
    assembler.sub(Reg::Rcx, Reg::Rdx);
    assembler.mov_mem_reg(Reg::Rax, STRING_LENGTH_OFFSET, Reg::Rcx);

    // rsi = source + 8 + start, rdi = result + 8, rcx = how many bytes
    assembler.mov_reg_mem(Reg::Rsi, Reg::Rbp, string);
    assembler.add_imm(Reg::Rsi, STRING_DATA_OFFSET);
    assembler.mov_reg_mem(Reg::Rdx, Reg::Rbp, start);
    assembler.add(Reg::Rsi, Reg::Rdx);
    assembler.mov_reg_mem(Reg::Rdi, Reg::Rbp, result);
    assembler.add_imm(Reg::Rdi, STRING_DATA_OFFSET);

    let copy = assembler.label();
    let done = assembler.label();
    assembler.bind(copy);
    assembler.cmp_imm(Reg::Rcx, 0);
    assembler.jcc(Cond::Eq, done);
    assembler.movzx_reg_mem8(Reg::Rax, Reg::Rsi, 0);
    assembler.mov_mem_reg8(Reg::Rdi, 0, Reg::Rax);
    assembler.add_imm(Reg::Rsi, 1);
    assembler.add_imm(Reg::Rdi, 1);
    assembler.sub_imm(Reg::Rcx, 1);
    assembler.jmp(copy);
    assembler.bind(done);

    assembler.mov_reg_mem(Reg::Rax, Reg::Rbp, result);
    epilogue(assembler);
}

/// `list_push(list, value)`: appends one element, growing the buffer when it
/// is full.
///
/// The elements live in a block the header points at, so replacing that block
/// leaves every pointer to the list itself valid. Growth doubles the capacity,
/// which makes a run of pushes cost a constant amount each on average.
fn emit_list_push(assembler: &mut Assembler, labels: &RuntimeLabels) {
    begin(assembler, labels, Routine::ListPush);
    prologue(assembler, 3);

    let list = -8;
    let value = -16;
    let old_data = -24;

    assembler.mov_mem_reg(Reg::Rbp, list, Reg::Rdi);
    assembler.mov_mem_reg(Reg::Rbp, value, Reg::Rsi);

    let store = assembler.label();
    let copy = assembler.label();
    let copied = assembler.label();

    // rcx = length, rdx = capacity
    assembler.mov_reg_mem(Reg::Rcx, Reg::Rdi, LIST_LENGTH_OFFSET);
    assembler.mov_reg_mem(Reg::Rdx, Reg::Rdi, LIST_CAPACITY_OFFSET);
    assembler.cmp(Reg::Rcx, Reg::Rdx);
    assembler.jcc(Cond::Below, store);

    // Full: the new capacity is double the old, or the initial one when the
    // list has never held anything.
    let has_capacity = assembler.label();
    assembler.cmp_imm(Reg::Rdx, 0);
    assembler.jcc(Cond::Ne, has_capacity);
    assembler.mov_reg_imm64(Reg::Rdx, LIST_INITIAL_CAPACITY);
    let sized = assembler.label();
    assembler.jmp(sized);
    assembler.bind(has_capacity);
    assembler.add(Reg::Rdx, Reg::Rdx);
    assembler.bind(sized);

    // Keep the old buffer and the new capacity across the call.
    assembler.mov_reg_mem(Reg::Rax, Reg::Rdi, LIST_DATA_OFFSET);
    assembler.mov_mem_reg(Reg::Rbp, old_data, Reg::Rax);
    assembler.mov_reg_mem(Reg::Rax, Reg::Rbp, list);
    assembler.mov_mem_reg(Reg::Rax, LIST_CAPACITY_OFFSET, Reg::Rdx);

    assembler.mov_reg_reg(Reg::Rdi, Reg::Rdx);
    times_eight(assembler, Reg::Rdi);
    assembler.call(labels.get(Routine::Alloc));

    // rax = new buffer, rsi = old buffer, rcx = how many words to move
    assembler.mov_reg_mem(Reg::Rsi, Reg::Rbp, old_data);
    assembler.mov_reg_mem(Reg::Rdi, Reg::Rbp, list);
    assembler.mov_mem_reg(Reg::Rdi, LIST_DATA_OFFSET, Reg::Rax);
    assembler.mov_reg_mem(Reg::Rcx, Reg::Rdi, LIST_LENGTH_OFFSET);

    assembler.bind(copy);
    assembler.cmp_imm(Reg::Rcx, 0);
    assembler.jcc(Cond::Eq, copied);
    assembler.mov_reg_mem(Reg::Rdx, Reg::Rsi, 0);
    assembler.mov_mem_reg(Reg::Rax, 0, Reg::Rdx);
    assembler.add_imm(Reg::Rsi, 8);
    assembler.add_imm(Reg::Rax, 8);
    assembler.sub_imm(Reg::Rcx, 1);
    assembler.jmp(copy);
    assembler.bind(copied);

    assembler.bind(store);
    // data[length] = value; length += 1
    assembler.mov_reg_mem(Reg::Rdi, Reg::Rbp, list);
    assembler.mov_reg_mem(Reg::Rcx, Reg::Rdi, LIST_LENGTH_OFFSET);
    assembler.mov_reg_mem(Reg::Rax, Reg::Rdi, LIST_DATA_OFFSET);
    assembler.mov_reg_reg(Reg::Rdx, Reg::Rcx);
    times_eight(assembler, Reg::Rdx);
    assembler.add(Reg::Rax, Reg::Rdx);
    assembler.mov_reg_mem(Reg::Rsi, Reg::Rbp, value);
    assembler.mov_mem_reg(Reg::Rax, 0, Reg::Rsi);
    assembler.add_imm(Reg::Rcx, 1);
    assembler.mov_mem_reg(Reg::Rdi, LIST_LENGTH_OFFSET, Reg::Rcx);

    epilogue(assembler);
}

/// The three list walks, which differ only in what they do with what the
/// closure returns.
///
/// `map` keeps it, `filter` keeps the element when it is true, and `each`
/// keeps nothing. Sharing the walk keeps the three in step: a fix to how a
/// closure is called is a fix to all of them.
fn emit_list_walk(assembler: &mut Assembler, labels: &RuntimeLabels, routine: Routine) {
    begin(assembler, labels, routine);
    prologue(assembler, 8);

    let list = -8;
    let closure = -16;
    let length = -24;
    let index = -32;
    let output = -40;
    let buffer = -48;
    let kept = -56;

    let keeps_result = routine == Routine::ListMap;
    let builds = routine != Routine::ListEach;

    assembler.mov_mem_reg(Reg::Rbp, list, Reg::Rdi);
    assembler.mov_mem_reg(Reg::Rbp, closure, Reg::Rsi);
    assembler.mov_reg_mem(Reg::Rcx, Reg::Rdi, LIST_LENGTH_OFFSET);
    assembler.mov_mem_reg(Reg::Rbp, length, Reg::Rcx);
    assembler.mov_reg_imm64(Reg::Rax, 0);
    assembler.mov_mem_reg(Reg::Rbp, index, Reg::Rax);
    assembler.mov_mem_reg(Reg::Rbp, kept, Reg::Rax);

    if builds {
        // The result never has more elements than the input, so one buffer of
        // that size is enough however many are kept.
        assembler.mov_reg_mem(Reg::Rdi, Reg::Rbp, length);
        let sized = assembler.label();
        assembler.cmp_imm(Reg::Rdi, 0);
        assembler.jcc(Cond::Ne, sized);
        assembler.mov_reg_imm64(Reg::Rdi, 1);
        assembler.bind(sized);
        times_eight(assembler, Reg::Rdi);
        assembler.call(labels.get(Routine::Alloc));
        assembler.mov_mem_reg(Reg::Rbp, buffer, Reg::Rax);

        assembler.mov_reg_imm64(Reg::Rdi, LIST_HEADER_SIZE as i64);
        assembler.call(labels.get(Routine::Alloc));
        assembler.mov_mem_reg(Reg::Rbp, output, Reg::Rax);
        assembler.mov_reg_mem(Reg::Rcx, Reg::Rbp, length);
        let capacity = assembler.label();
        assembler.cmp_imm(Reg::Rcx, 0);
        assembler.jcc(Cond::Ne, capacity);
        assembler.mov_reg_imm64(Reg::Rcx, 1);
        assembler.bind(capacity);
        assembler.mov_mem_reg(Reg::Rax, LIST_CAPACITY_OFFSET, Reg::Rcx);
        assembler.mov_reg_mem(Reg::Rdx, Reg::Rbp, buffer);
        assembler.mov_mem_reg(Reg::Rax, LIST_DATA_OFFSET, Reg::Rdx);
    }

    let each = assembler.label();
    let finished = assembler.label();
    let skip = assembler.label();

    assembler.bind(each);
    assembler.mov_reg_mem(Reg::Rax, Reg::Rbp, index);
    assembler.mov_reg_mem(Reg::Rcx, Reg::Rbp, length);
    assembler.cmp(Reg::Rax, Reg::Rcx);
    assembler.jcc(Cond::Ge, finished);

    // element = list.data[index]
    assembler.mov_reg_mem(Reg::Rdx, Reg::Rbp, list);
    assembler.mov_reg_mem(Reg::Rdx, Reg::Rdx, LIST_DATA_OFFSET);
    assembler.mov_reg_reg(Reg::Rsi, Reg::Rax);
    times_eight(assembler, Reg::Rsi);
    assembler.add(Reg::Rdx, Reg::Rsi);
    assembler.mov_reg_mem(Reg::Rsi, Reg::Rdx, 0);

    // The closure is its own environment, and its first word is the code.
    assembler.mov_reg_mem(Reg::Rdi, Reg::Rbp, closure);
    assembler.mov_reg_mem(Reg::R11, Reg::Rdi, 0);
    assembler.call_reg(Reg::R11);

    if builds {
        if !keeps_result {
            // `filter` keeps the element, and only when the answer was true.
            assembler.cmp_imm(Reg::Rax, 0);
            assembler.jcc(Cond::Eq, skip);
            assembler.mov_reg_mem(Reg::Rdx, Reg::Rbp, list);
            assembler.mov_reg_mem(Reg::Rdx, Reg::Rdx, LIST_DATA_OFFSET);
            assembler.mov_reg_mem(Reg::Rsi, Reg::Rbp, index);
            times_eight(assembler, Reg::Rsi);
            assembler.add(Reg::Rdx, Reg::Rsi);
            assembler.mov_reg_mem(Reg::Rax, Reg::Rdx, 0);
        }
        assembler.mov_reg_mem(Reg::Rdx, Reg::Rbp, buffer);
        assembler.mov_reg_mem(Reg::Rsi, Reg::Rbp, kept);
        times_eight(assembler, Reg::Rsi);
        assembler.add(Reg::Rdx, Reg::Rsi);
        assembler.mov_mem_reg(Reg::Rdx, 0, Reg::Rax);
        assembler.mov_reg_mem(Reg::Rcx, Reg::Rbp, kept);
        assembler.add_imm(Reg::Rcx, 1);
        assembler.mov_mem_reg(Reg::Rbp, kept, Reg::Rcx);
    }

    assembler.bind(skip);
    assembler.mov_reg_mem(Reg::Rcx, Reg::Rbp, index);
    assembler.add_imm(Reg::Rcx, 1);
    assembler.mov_mem_reg(Reg::Rbp, index, Reg::Rcx);
    assembler.jmp(each);
    assembler.bind(finished);

    if builds {
        assembler.mov_reg_mem(Reg::Rax, Reg::Rbp, output);
        assembler.mov_reg_mem(Reg::Rcx, Reg::Rbp, kept);
        assembler.mov_mem_reg(Reg::Rax, LIST_LENGTH_OFFSET, Reg::Rcx);
    } else {
        assembler.mov_reg_imm64(Reg::Rax, 0);
    }
    epilogue(assembler);
}

/// `index_check(index, length)`: ends the process when the index is outside
/// `0..length`.
///
/// One unsigned comparison covers both ends: a negative index read as
/// unsigned is enormous, so `index >= length` catches it too.
fn emit_index_check(assembler: &mut Assembler, labels: &RuntimeLabels, data: &RuntimeData) {
    begin(assembler, labels, Routine::IndexCheck);
    prologue(assembler, 0);

    let failed = assembler.label();
    assembler.cmp(Reg::Rdi, Reg::Rsi);
    assembler.jcc(Cond::AboveEq, failed);
    epilogue(assembler);

    assembler.bind(failed);
    assembler.mov_reg_imm64(Reg::Rdi, STDERR);
    assembler.lea_rip(Reg::Rsi, Reference::RoData(data.index_message));
    assembler.mov_reg_imm64(Reg::Rdx, data.index_message_len as i64);
    assembler.call(labels.get(Routine::Write));
    assembler.mov_reg_imm64(Reg::Rdi, INDEX_FAILURE_STATUS as i64);
    assembler.call(labels.get(Routine::Exit));
    assembler.ud2();
}

/// `assert(condition)`: ends the process when the condition is false.
fn emit_assert(assembler: &mut Assembler, labels: &RuntimeLabels, data: &RuntimeData) {
    begin(assembler, labels, Routine::Assert);
    prologue(assembler, 0);

    let failed = assembler.label();
    assembler.cmp_imm(Reg::Rdi, 0);
    assembler.jcc(Cond::Eq, failed);
    epilogue(assembler);

    assembler.bind(failed);
    assembler.mov_reg_imm64(Reg::Rdi, STDERR);
    assembler.lea_rip(Reg::Rsi, Reference::RoData(data.assert_message));
    assembler.mov_reg_imm64(Reg::Rdx, data.assert_message_len as i64);
    assembler.call(labels.get(Routine::Write));
    assembler.mov_reg_imm64(Reg::Rdi, ASSERT_FAILURE_STATUS as i64);
    assembler.call(labels.get(Routine::Exit));
    assembler.ud2();
}
