//! Writing a static ELF64 executable.
//!
//! Noto produces the executable itself rather than handing object files to a
//! system linker. A Noto program has no dynamic dependencies — not even libc —
//! so the file it needs is simple: an ELF header, two program headers, and the
//! bytes they describe. Emitting it directly means `noto build` works on a
//! machine with no toolchain installed and gives the compiler exact control
//! over the layout.
//!
//! The layout is:
//!
//! ```text
//! 0x400000  ELF header + program headers + code + read-only data   (r-x)
//! 0x600000  writable data, then zero-filled space                  (rw-)
//! ```
//!
//! The two segments are placed in separate 2 MiB regions so that one page can
//! never be both writable and executable.

/// Where the first segment is mapped.
pub const TEXT_BASE: u64 = 0x40_0000;

/// Where the writable segment is mapped.
pub const DATA_BASE: u64 = 0x60_0000;

/// The page size the segments are aligned to.
pub const PAGE_SIZE: u64 = 0x1000;

const ELF_HEADER_SIZE: u64 = 64;
const PROGRAM_HEADER_SIZE: u64 = 56;
const PROGRAM_HEADER_COUNT: u64 = 2;

/// The parts of an executable, before layout.
pub struct Image {
    /// Machine code.
    pub text: Vec<u8>,
    /// Constant data referenced by the code.
    pub rodata: Vec<u8>,
    /// Writable data with an initial value.
    pub data: Vec<u8>,
    /// Additional zero-filled writable bytes following `data`.
    pub bss_size: u64,
    /// Offset of the entry point within `text`.
    pub entry_offset: u64,
}

/// Where each section ends up once the image is laid out.
///
/// The backend needs these addresses to patch RIP-relative references, so
/// layout is computed before the file is written.
#[derive(Clone, Copy, Debug)]
pub struct Layout {
    /// Virtual address of the first byte of code.
    pub text_address: u64,
    /// Virtual address of the first byte of read-only data.
    pub rodata_address: u64,
    /// Virtual address of the first byte of writable data.
    pub data_address: u64,
}

/// Computes where each section will be mapped.
///
/// `text_size` and `rodata_size` are the sizes the image will have; the
/// backend calls this before patching relocations and again with the same
/// values when writing the file, so the two agree.
pub fn layout(text_size: u64, rodata_size: u64) -> Layout {
    let headers = ELF_HEADER_SIZE + PROGRAM_HEADER_SIZE * PROGRAM_HEADER_COUNT;
    let text_address = TEXT_BASE + headers;
    // Read-only data follows the code in the same segment, aligned so that a
    // string object's length field is naturally aligned.
    let rodata_address = align_up(text_address + text_size, 8);
    let _ = rodata_size;
    Layout { text_address, rodata_address, data_address: DATA_BASE }
}

/// Serialises an image into a runnable ELF64 executable.
pub fn write(image: &Image) -> Vec<u8> {
    let headers = ELF_HEADER_SIZE + PROGRAM_HEADER_SIZE * PROGRAM_HEADER_COUNT;
    let layout = layout(image.text.len() as u64, image.rodata.len() as u64);

    let text_offset = headers;
    let rodata_offset = text_offset + image.text.len() as u64
        + (layout.rodata_address - (layout.text_address + image.text.len() as u64));
    let text_segment_size = rodata_offset + image.rodata.len() as u64;

    // The writable segment's file offset must be congruent to its virtual
    // address modulo the page size, which is what the loader requires when it
    // maps the file.
    let data_offset = align_up(text_segment_size, PAGE_SIZE)
        + (DATA_BASE % PAGE_SIZE);

    let mut out = Vec::with_capacity((data_offset + image.data.len() as u64) as usize);

    // --- ELF header -------------------------------------------------------
    out.extend_from_slice(&[0x7F, b'E', b'L', b'F']);
    out.push(2); // ELFCLASS64
    out.push(1); // ELFDATA2LSB
    out.push(1); // EV_CURRENT
    out.push(0); // ELFOSABI_SYSV
    out.extend_from_slice(&[0; 8]); // padding

    out.extend_from_slice(&2u16.to_le_bytes()); // ET_EXEC
    out.extend_from_slice(&0x3Eu16.to_le_bytes()); // EM_X86_64
    out.extend_from_slice(&1u32.to_le_bytes()); // EV_CURRENT
    out.extend_from_slice(&(layout.text_address + image.entry_offset).to_le_bytes());
    out.extend_from_slice(&ELF_HEADER_SIZE.to_le_bytes()); // e_phoff
    out.extend_from_slice(&0u64.to_le_bytes()); // e_shoff: no section headers
    out.extend_from_slice(&0u32.to_le_bytes()); // e_flags
    out.extend_from_slice(&(ELF_HEADER_SIZE as u16).to_le_bytes());
    out.extend_from_slice(&(PROGRAM_HEADER_SIZE as u16).to_le_bytes());
    out.extend_from_slice(&(PROGRAM_HEADER_COUNT as u16).to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes()); // e_shentsize
    out.extend_from_slice(&0u16.to_le_bytes()); // e_shnum
    out.extend_from_slice(&0u16.to_le_bytes()); // e_shstrndx

    // --- program headers ---------------------------------------------------
    // The first segment starts at file offset 0 so that the headers
    // themselves are mapped, which keeps the offsets and addresses congruent.
    write_program_header(
        &mut out,
        PF_R | PF_X,
        0,
        TEXT_BASE,
        text_segment_size,
        text_segment_size,
    );
    let data_size = image.data.len() as u64;
    write_program_header(
        &mut out,
        PF_R | PF_W,
        data_offset,
        DATA_BASE,
        data_size,
        data_size + image.bss_size,
    );

    debug_assert_eq!(out.len() as u64, headers);

    out.extend_from_slice(&image.text);
    pad_to(&mut out, rodata_offset);
    out.extend_from_slice(&image.rodata);
    pad_to(&mut out, data_offset);
    out.extend_from_slice(&image.data);

    out
}

const PF_X: u32 = 1;
const PF_W: u32 = 2;
const PF_R: u32 = 4;

fn write_program_header(
    out: &mut Vec<u8>,
    flags: u32,
    offset: u64,
    address: u64,
    file_size: u64,
    memory_size: u64,
) {
    out.extend_from_slice(&1u32.to_le_bytes()); // PT_LOAD
    out.extend_from_slice(&flags.to_le_bytes());
    out.extend_from_slice(&offset.to_le_bytes());
    out.extend_from_slice(&address.to_le_bytes()); // p_vaddr
    out.extend_from_slice(&address.to_le_bytes()); // p_paddr
    out.extend_from_slice(&file_size.to_le_bytes());
    out.extend_from_slice(&memory_size.to_le_bytes());
    out.extend_from_slice(&PAGE_SIZE.to_le_bytes());
}

fn pad_to(out: &mut Vec<u8>, offset: u64) {
    while (out.len() as u64) < offset {
        out.push(0);
    }
}

/// Rounds `value` up to a multiple of `alignment`.
pub fn align_up(value: u64, alignment: u64) -> u64 {
    debug_assert!(alignment.is_power_of_two());
    (value + alignment - 1) & !(alignment - 1)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Image {
        Image {
            text: vec![0x0F, 0x05, 0xC3],
            rodata: b"hello".to_vec(),
            data: vec![0; 8],
            bss_size: 4096,
            entry_offset: 0,
        }
    }

    #[test]
    fn writes_a_recognisable_elf_header() {
        let bytes = write(&sample());
        assert_eq!(&bytes[..4], b"\x7FELF");
        assert_eq!(bytes[4], 2, "64-bit");
        assert_eq!(bytes[5], 1, "little endian");
        assert_eq!(u16::from_le_bytes([bytes[16], bytes[17]]), 2, "ET_EXEC");
        assert_eq!(u16::from_le_bytes([bytes[18], bytes[19]]), 0x3E, "EM_X86_64");
    }

    #[test]
    fn the_entry_point_addresses_the_first_instruction() {
        let mut image = sample();
        image.entry_offset = 2;
        let bytes = write(&image);
        let entry = u64::from_le_bytes(bytes[24..32].try_into().unwrap());
        let expected = layout(image.text.len() as u64, image.rodata.len() as u64).text_address + 2;
        assert_eq!(entry, expected);
    }

    #[test]
    fn segment_offsets_and_addresses_are_page_congruent() {
        let bytes = write(&sample());
        let header_offset = 64;
        for index in 0..2u64 {
            let base = (header_offset + index * 56) as usize;
            let offset = u64::from_le_bytes(bytes[base + 8..base + 16].try_into().unwrap());
            let address = u64::from_le_bytes(bytes[base + 16..base + 24].try_into().unwrap());
            assert_eq!(
                offset % PAGE_SIZE,
                address % PAGE_SIZE,
                "segment {index} must be mappable"
            );
        }
    }

    #[test]
    fn code_is_never_writable() {
        let bytes = write(&sample());
        let text_flags = u32::from_le_bytes(bytes[68..72].try_into().unwrap());
        let data_flags = u32::from_le_bytes(bytes[124..128].try_into().unwrap());
        assert_eq!(text_flags, PF_R | PF_X);
        assert_eq!(data_flags, PF_R | PF_W);
        assert_eq!(text_flags & PF_W, 0, "the code segment must not be writable");
        assert_eq!(data_flags & PF_X, 0, "the data segment must not be executable");
    }

    #[test]
    fn the_writable_segment_reserves_room_for_zeroed_memory() {
        let image = sample();
        let bytes = write(&image);
        let base = 64 + 56;
        let file_size = u64::from_le_bytes(bytes[base + 32..base + 40].try_into().unwrap());
        let memory_size = u64::from_le_bytes(bytes[base + 40..base + 48].try_into().unwrap());
        assert_eq!(file_size, image.data.len() as u64);
        assert_eq!(memory_size, image.data.len() as u64 + image.bss_size);
    }

    #[test]
    fn the_code_and_data_bytes_land_where_the_headers_say() {
        let image = sample();
        let bytes = write(&image);
        let text_offset = 64 + 56 * 2;
        assert_eq!(&bytes[text_offset..text_offset + image.text.len()], &image.text[..]);

        let base = 64 + 56;
        let data_offset =
            u64::from_le_bytes(bytes[base + 8..base + 16].try_into().unwrap()) as usize;
        assert_eq!(&bytes[data_offset..data_offset + image.data.len()], &image.data[..]);
    }

    #[test]
    fn alignment_rounds_up_to_the_next_multiple() {
        assert_eq!(align_up(0, 8), 0);
        assert_eq!(align_up(1, 8), 8);
        assert_eq!(align_up(8, 8), 8);
        assert_eq!(align_up(9, 8), 16);
        assert_eq!(align_up(0x1001, 0x1000), 0x2000);
    }
}
