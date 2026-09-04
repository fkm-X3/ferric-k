//! Minimal GDT + TSS for x86_64: flat-model kernel segments and a TSS with
//! IST slots for critical exception stacks. Loaded once during early boot.

use core::arch::asm;

/// GDT selector constants (index << 3 | RPL).
pub const KERNEL_CODE_SELECTOR: u16 = 0x08;
pub const KERNEL_DATA_SELECTOR: u16 = 0x10;
pub const TSS_SELECTOR: u16 = 0x18;

const GDT_LEN: usize = 5;

/// Size of the double-fault IST stack.
const IST_STACK_BYTES: usize = 4096;

/// Single GDT entry (Intel SDM Vol. 3A §3.4.5, Fig. 3-8).
/// A single GDT descriptor (Intel SDM Vol. 3A §3.4.5, Fig. 3-8): 8 bytes.
///
/// Stored as one packed u64. Non-system descriptors (code/data) occupy exactly
/// one GDT slot (index << 3); only the TSS descriptor spans two slots.
#[repr(transparent)]
#[derive(Clone, Copy)]
struct GdtEntry(u64);

/// TSS entry occupies two GDT slots (16 bytes, Intel SDM Vol. 3A §8.2.3).
#[repr(C, packed)]
struct TssEntry {
    low: u64,
    high: u64,
}

/// Task State Segment (Intel SDM Vol. 3A §8.2.3, Fig. 8-4).
/// Only rsp0 (ring-0 stack for interrupts) and ist1 (double-fault IST) are
/// populated; the rest stays zero-initialized.
#[repr(C, packed)]
struct Tss {
    length: u16,
    rsp0_low: u16,
    rsp0_mid: u8,
    rsp0_high: u8,
    rsp0_upper: u32,
    _reserved1: u32,
    rsp1_low: u16,
    rsp1_mid: u8,
    rsp1_high: u8,
    rsp1_upper: u32,
    _reserved2: u32,
    rsp2_low: u16,
    rsp2_mid: u8,
    rsp2_high: u8,
    rsp2_upper: u32,
    _reserved3: u32,
    ist1_low: u16,
    ist1_mid: u8,
    ist1_high: u8,
    ist1_upper: u32,
    _reserved4: u32,
    ist2_low: u16,
    ist2_mid: u8,
    ist2_high: u8,
    ist2_upper: u32,
    _reserved5: u32,
    ist3: u64,
    ist4: u64,
    ist5: u64,
    ist6: u64,
    ist7: u64,
    _reserved6: u32,
    iopb_offset: u16,
}

// SAFETY: TSS is accessed only during single-threaded early boot; all fields
// are written before `ltr` and never modified after.
unsafe impl Send for Tss {}

/// Helper: build a flat-model GDT entry.
///
/// `limit` is the 20-bit segment limit (in bytes when G=0, in pages when G=1).
/// `access` encodes P, DPL, S, and type (Intel SDM Vol. 3A §3.4.5.1).
/// `flags` is the 4-bit field at descriptor byte 6 upper nibble: G, D/B, L, AVL.
///
/// The 8-byte descriptor is stored as a little-endian u64. The CPU field
/// positions (Intel SDM Vol. 3A §3.4.5, Fig. 3-8) map to bits as:
/// limit15:0 (bits 0-15), base23:0 (bits 16-39), access (bits 40-47),
/// limit19:16+flags (bits 48-55), base31:24 (bits 56-63). The upper 32 base
/// bits are always zero for flat kernel segments.
const fn gdt_entry(base: u32, limit: u32, access: u8, flags: u8) -> GdtEntry {
    let limit_lo = (limit & 0xFFFF) as u64;
    let base_lo = (base & 0xFFFF) as u64;
    let base_mid = ((base >> 16) & 0xFF) as u64;
    let access_u64 = access as u64;
    let limit_hi = ((limit >> 16) & 0x0F) as u64;
    let flags_u64 = (flags & 0x0F) as u64;
    let base_hi = ((base >> 24) & 0xFF) as u64;

    // base is u32, so the upper 32 base bits are always zero.
    GdtEntry(
        limit_lo
            | (base_lo << 16)
            | (base_mid << 24)
            | (access_u64 << 40)
            | (limit_hi << 48)
            | (flags_u64 << 52)
            | (base_hi << 56),
    )
}

/// Kernel code segment: base=0, limit=4 GiB (G=1), L=1 (64-bit), D=0.
/// Access = 0x9A (P=1, DPL=0, S=1, type=0xA: execute/read, accessed).
const KERNEL_CODE: GdtEntry = gdt_entry(0, 0xFFFFF, 0x9A, 0xA);

/// Kernel data segment: base=0, limit=4 GiB (G=1).
/// Access = 0x93 (P=1, DPL=0, S=1, type=0x3: read/write, accessed).
const KERNEL_DATA: GdtEntry = gdt_entry(0, 0xFFFFF, 0x93, 0xC);

/// Construct a TSS descriptor from a `Tss` address (Intel SDM Vol. 3A §8.2.3).
///
/// The 16-byte descriptor occupies GDT entries [index, index+1]; the selector
/// must point to the first slot (index << 3). Field layout matches
/// [`gdt_entry`]: access at `low` bits 40-47, base high in `high` bits 0-31.
const fn tss_descriptor(addr: u64) -> TssEntry {
    let base_lo = addr & 0xFFFF;
    let base_mid = (addr >> 16) & 0xFF;
    let base_high = (addr >> 24) & 0xFF;
    let base_upper = (addr >> 32) & 0xFFFFFFFF;
    // Limit: last valid byte offset of TSS (size - 1).
    let limit = (core::mem::size_of::<Tss>() as u64 - 1) & 0xFFFF;
    // Access: P=1, DPL=0, type=0x9 (available 64-bit TSS, SDM §3.4.5.1).
    let access = 0x89u64;
    let flags = 0x0u64; // G=0, D/B=0, L=0, AVL=0

    let low = limit
        | (base_lo << 16)
        | (base_mid << 24)
        | (access << 40)
        | (flags << 52)
        | (base_high << 56);
    let high = base_upper;

    TssEntry { low, high }
}

// 4 KiB IST stack for double-fault handler.
#[repr(C, align(4096))]
struct AlignedStack([u8; 4096]);

static mut IST_DF_STACK: AlignedStack = AlignedStack([0u8; 4096]);

// SAFETY: TSS and static mut are accessed only from `init()` during single-
// threaded early boot, before any concurrent access is possible.
static mut TSS: Tss = Tss {
    length: core::mem::size_of::<Tss>() as u16,
    rsp0_low: 0,
    rsp0_mid: 0,
    rsp0_high: 0,
    rsp0_upper: 0,
    _reserved1: 0,
    rsp1_low: 0,
    rsp1_mid: 0,
    rsp1_high: 0,
    rsp1_upper: 0,
    _reserved2: 0,
    rsp2_low: 0,
    rsp2_mid: 0,
    rsp2_high: 0,
    rsp2_upper: 0,
    _reserved3: 0,
    ist1_low: 0,
    ist1_mid: 0,
    ist1_high: 0,
    ist1_upper: 0,
    _reserved4: 0,
    ist2_low: 0,
    ist2_mid: 0,
    ist2_high: 0,
    ist2_upper: 0,
    _reserved5: 0,
    ist3: 0,
    ist4: 0,
    ist5: 0,
    ist6: 0,
    ist7: 0,
    _reserved6: 0,
    iopb_offset: core::mem::size_of::<Tss>() as u16,
};

/// The GDT: null + kernel code + kernel data + TSS (2 slots).
/// Patched in place with the real TSS descriptor during `init()`.
static mut GDT: [GdtEntry; GDT_LEN] = [
    gdt_entry(0, 0, 0, 0), // null
    KERNEL_CODE,
    KERNEL_DATA,
    // TSS is constructed at init; placeholder entries overwritten by `init`.
    GdtEntry(0),
    GdtEntry(0),
];

/// Loads the GDT + TSS and reloads all segment registers.
///
/// Must be called once during early boot, before any segment-dependent code
/// runs. The current bootloader segments are replaced with flat kernel
/// segments. Interrupts remain disabled (IF=0) through the transition.
pub fn init() {
    // SAFETY: single-threaded early boot; TSS fields are written before `ltr`
    // and never modified after.
    unsafe {
        let ist1_addr = (&raw const IST_DF_STACK) as u64 + IST_STACK_BYTES as u64;
        write_tss_ist1(&raw mut TSS, ist1_addr);

        // Build the real TSS descriptor (the const placeholder GDT has zeroed
        // entries 3..4; we patch them here).
        let tss_desc = tss_descriptor(&raw const TSS as u64);
        GDT[3] = GdtEntry(tss_desc.low);
        GDT[4] = GdtEntry(tss_desc.high);

        let gdt_ptr = GdtPtr {
            limit: (core::mem::size_of::<GdtEntry>() * GDT_LEN - 1) as u16,
            base: (&raw const GDT) as *const GdtEntry as u64,
        };

        asm!(
            "lgdt [{gdt_ptr}]",
            // Reload CS via `iretq` (Intel SDM Vol. 3A §6.14.2). QEMU's long-mode
            // `iretq` always treats the return as a privilege change (see its
            // helper_ret_protected same-privilege condition) and pops a full
            // RIP:CS:RFLAGS:RSP:SS frame, so push all five slots: the new stack
            // pointer is the pre-frame RSP and the new SS is our data selector.
            "mov {tmp}, rsp",
            "push {ss}",
            "push {tmp}",
            "pushfq",
            "push {cs}",
            "lea {tmp}, [rip + 2f]",
            "push {tmp}",
            "iretq",
            "2:",
            gdt_ptr = in(reg) &gdt_ptr,
            cs = in(reg) KERNEL_CODE_SELECTOR as u64,
            ss = in(reg) KERNEL_DATA_SELECTOR as u64,
            tmp = out(reg) _,
            options(preserves_flags),
        );

        // Load the TSS (16-bit selector) to arm the IST for double faults.
        let sel: u32 = TSS_SELECTOR.into();
        core::arch::asm!("ltr {sel:x}", sel = in(reg) sel);
    }
}

/// Write the IST1 field of a TSS.
///
/// The IST1 base address is split across bytes 36-39 (low16+mid8) and bytes
/// 40-43 (high8 + upper32) of the packed TSS struct.
///
/// SAFETY: `tss` must point to a valid, unique TSS; called once during boot.
unsafe fn write_tss_ist1(tss: *mut Tss, addr: u64) {
    // SAFETY: caller guarantees valid TSS pointer; single-threaded early boot.
    unsafe {
        let base = tss as *mut u8;
        // IST1 at byte offset 36: low16, mid8, high8, upper32
        core::ptr::write_volatile(base.add(36), addr as u16 as u8);
        core::ptr::write_volatile(base.add(37), (addr >> 8) as u8);
        core::ptr::write_volatile(base.add(38), (addr >> 16) as u8);
        core::ptr::write_volatile(base.add(39), (addr >> 24) as u8);
        core::ptr::write_volatile(base.add(40), (addr >> 32) as u8);
        core::ptr::write_volatile(base.add(41), (addr >> 40) as u8);
        core::ptr::write_volatile(base.add(42), (addr >> 48) as u8);
        core::ptr::write_volatile(base.add(43), (addr >> 56) as u8);
    }
}

/// Descriptor table register format (Intel SDM Vol. 3A §2.4.1).
#[repr(C, packed)]
struct GdtPtr {
    limit: u16,
    base: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gdt_entry_size() {
        // A non-system descriptor is a single 8-byte GDT slot.
        assert_eq!(core::mem::size_of::<GdtEntry>(), 8);
    }

    #[test]
    fn tss_entry_size() {
        assert_eq!(core::mem::size_of::<TssEntry>(), 16);
    }

    #[test]
    fn tss_size() {
        // Minimum TSS size for 64-bit mode (Intel SDM §8.2.3, Fig. 8-4).
        assert_eq!(core::mem::size_of::<Tss>(), 104);
    }

    #[test]
    fn kernel_code_access_byte() {
        // P=1, DPL=0, S=1, type=0xA → 1001_1010 = 0x9A at descriptor byte 5
        // (bits 40-47).
        let entry = gdt_entry(0, 0xFFFFF, 0x9A, 0xA);
        let access = (entry.0 >> 40) & 0xFF;
        assert_eq!(access, 0x9A);
    }

    #[test]
    fn kernel_data_access_byte() {
        // P=1, DPL=0, S=1, type=0x2 → 1001_0010 = 0x92 at descriptor byte 5.
        let entry = gdt_entry(0, 0xFFFFF, 0x92, 0xC);
        let access = (entry.0 >> 40) & 0xFF;
        assert_eq!(access, 0x92);
    }

    #[test]
    fn kernel_code_flags_l_bit_set() {
        // flags nibble at descriptor byte 6 (bits 48-55); L=1 → bit 1 set
        // (0xA = G=1, L=1).
        let entry = gdt_entry(0, 0xFFFFF, 0x9A, 0xA);
        let flags = (entry.0 >> 52) & 0x0F;
        assert_eq!(flags, 0xA);
    }

    #[test]
    fn tss_access_byte() {
        let addr = 0x1000u64;
        let desc = tss_descriptor(addr);
        let access = (desc.low >> 40) & 0xFF;
        // P=1, DPL=0, type=0x9 (available 64-bit TSS) → 0x89
        assert_eq!(access, 0x89);
    }

    #[test]
    fn tss_descriptor_base_field() {
        let addr = 0x0000_0001_2345_6789u64;
        let desc = tss_descriptor(addr);
        let base_lo = (desc.low >> 16) & 0xFFFF;
        let base_mid = (desc.low >> 24) & 0xFF;
        let base_high = (desc.low >> 56) & 0xFF;
        let base_upper = desc.high & 0xFFFFFFFF;
        assert_eq!(base_lo, 0x6789);
        assert_eq!(base_mid, 0x23);
        assert_eq!(base_high, 0x01);
        assert_eq!(base_upper, 0x0000_0001);
    }

    #[test]
    fn selector_constants_match_gdt_layout() {
        // Selector = index << 3 | RPL.
        assert_eq!(KERNEL_CODE_SELECTOR, 1 << 3); // index 1, RPL 0
        assert_eq!(KERNEL_DATA_SELECTOR, 2 << 3); // index 2, RPL 0
        assert_eq!(TSS_SELECTOR, 3 << 3); // index 3, RPL 0
    }
}
