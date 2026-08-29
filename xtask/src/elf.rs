use crate::steps;
use std::path::Path;

const HIGHER_HALF_BASE: u64 = 0xFFFF_FFFF_8000_0000;
const ENTRY_CEILING: u64 = 0xFFFF_FFFF_C000_0000;
const PAGE_SIZE: u64 = 0x1000;

fn r16(b: &[u8], off: usize) -> u16 {
    u16::from_le_bytes([b[off], b[off + 1]])
}
fn r32(b: &[u8], off: usize) -> u32 {
    u32::from_le_bytes([b[off], b[off + 1], b[off + 2], b[off + 3]])
}
fn r64(b: &[u8], off: usize) -> u64 {
    u64::from_le_bytes([
        b[off],
        b[off + 1],
        b[off + 2],
        b[off + 3],
        b[off + 4],
        b[off + 5],
        b[off + 6],
        b[off + 7],
    ])
}

fn find_bytes(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|w| w == needle)
}

/// Validate the kernel ELF: entry in the higher-half window, PT_LOAD layout,
/// and the structural .limine_requests gate (Limine PROTOCOL.md).
pub fn limine_elf_gate(path: &Path, elf: &[u8]) -> Result<(), String> {
    let p = path.display();
    if elf.len() < 64 || &elf[0..4] != b"\x7fELF" {
        return Err(format!("{p} is not an ELF file"));
    }

    let entry = r64(elf, 0x18);
    if !(HIGHER_HALF_BASE..ENTRY_CEILING).contains(&entry) {
        return Err(format!(
            "{p}: e_entry 0x{entry:X} outside higher-half range [0x{HIGHER_HALF_BASE:X}, 0x{ENTRY_CEILING:X})"
        ));
    }
    steps::ok(&format!("entry 0x{entry:X} in higher-half window"));

    // Program headers.
    let phoff = r64(elf, 0x20) as usize;
    let phentsize = r16(elf, 0x36) as usize;
    let phnum = r16(elf, 0x38) as usize;
    if phnum == 0 {
        return Err(format!("{p}: no program headers"));
    }
    let mut seen_load = 0usize;
    for i in 0..phnum {
        let base = phoff + i * phentsize;
        let ty = r32(elf, base);
        if ty != 1 {
            continue; // PT_LOAD
        }
        seen_load += 1;
        let vaddr = r64(elf, base + 0x10);
        let align = r64(elf, base + 0x30);
        if align != PAGE_SIZE {
            return Err(format!(
                "{p}: PT_LOAD #{i} p_align 0x{align:X}, expected 0x{PAGE_SIZE:X} (loader requires <=4 KiB pages)"
            ));
        }
        if seen_load == 1 && vaddr != HIGHER_HALF_BASE {
            return Err(format!(
                "{p}: first PT_LOAD vaddr 0x{vaddr:X}, expected 0x{HIGHER_HALF_BASE:X} (higher-half base)"
            ));
        }
    }
    if seen_load == 0 {
        return Err(format!("{p}: no PT_LOAD segments"));
    }
    steps::ok(&format!(
        "{seen_load} PT_LOAD segments, first at base, p_align=4KiB"
    ));

    // Section headers: needed to locate .limine_requests.
    let shoff = r64(elf, 0x28) as usize;
    let shentsize = r16(elf, 0x3A) as usize;
    let shnum = r16(elf, 0x3C) as usize;
    let shstrndx = r16(elf, 0x3E) as usize;
    if shnum == 0 || shstrndx == 0 {
        return Err(format!(
            "{p}: stripped section headers cannot prove Limine requests"
        ));
    }

    let strtab_hdr = shoff + shstrndx * shentsize;
    let strtab_off = r64(elf, strtab_hdr + 0x18) as usize;
    let strtab_len = r64(elf, strtab_hdr + 0x20) as usize;
    if strtab_off + strtab_len > elf.len() {
        return Err(format!("{p}: section header string table out of bounds"));
    }
    let strtab = &elf[strtab_off..strtab_off + strtab_len];
    let cstr = |off: usize| -> &[u8] {
        let end = strtab[off..]
            .iter()
            .position(|&b| b == 0)
            .unwrap_or(strtab.len() - off);
        &strtab[off..off + end]
    };

    for i in 0..shnum {
        let hdr = shoff + i * shentsize;
        if cstr(r32(elf, hdr) as usize) != b".limine_requests" {
            continue;
        }
        let addr = r64(elf, hdr + 0x10);
        let off = r64(elf, hdr + 0x18) as usize;
        let len = r64(elf, hdr + 0x20) as usize;
        if !addr.is_multiple_of(8) {
            return Err(format!(
                "{p}: .limine_requests vaddr 0x{addr:X} not 8-byte aligned"
            ));
        }
        if len < 216 {
            return Err(format!(
                "{p}: .limine_requests size {len} < 216 (markers + base rev + 3 requests)"
            ));
        }
        if off + len > elf.len() {
            return Err(format!("{p}: .limine_requests section out of bounds"));
        }
        let content = &elf[off..off + len];

        // Start marker then end marker.
        let start_marker: [u64; 4] = [
            0xF6B8F4B39DE7D1AE,
            0xFAB91A6940FCB9CF,
            0x785C6ED015D3E316,
            0x181E920A7852B9D9,
        ];
        let end_marker: [u64; 2] = [0xADC0E0531BB10D03, 0x9572709F31764C62];
        for (w, &want) in start_marker.iter().enumerate() {
            let got = r64(content, 8 * w);
            if got != want {
                return Err(format!(
                    "{p}: start marker word {w}: found 0x{got:X}, expected 0x{want:X}"
                ));
            }
        }
        for (w, &want) in end_marker.iter().enumerate() {
            let got = r64(content, len - 16 + 8 * w);
            if got != want {
                return Err(format!(
                    "{p}: end marker word {w}: found 0x{got:X}, expected 0x{want:X}"
                ));
            }
        }

        // Required magic words as raw LE byte sequences.
        let words: [(&str, &str); 4] = [
            ("base-revision-magic", "F9562B2D5C95A6C8"),
            ("hhdm-request-id", "48DCF1CB8AD2B852"),
            ("framebuffer-id", "9D5827DCD881DD75"),
            ("memmap-request-id", "67CF3D9D378A806F"),
        ];
        for (label, hex) in words {
            // Little-endian: walk pairs back-to-front.
            let mut needle = [0u8; 8];
            for (k, slot) in needle.iter_mut().enumerate() {
                // hex "48DCF1CB8AD2B852": slot0=0x52, slot1=0xB8 ...
                let pair = &hex[14 - k * 2..16 - k * 2];
                *slot = u8::from_str_radix(pair, 16).unwrap();
            }
            if find_bytes(content, &needle).is_none() {
                return Err(format!("{p}: .limine_requests missing {label} (0x{hex})"));
            }
        }

        steps::ok(&format!(
            ".limine_requests: markers + base-rev + hhdm/framebuffer/memmap IDs, addr 0x{addr:X} len {len}"
        ));
        return Ok(());
    }
    Err(format!("{p}: .limine_requests section not found"))
}
