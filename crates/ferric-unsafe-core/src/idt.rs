//! Interrupt Descriptor Table for x86_64: gates for CPU exceptions
//! (divide-by-zero, GPF, page fault). Loaded once during early boot.

use core::arch::asm;

use crate::gdt::KERNEL_CODE_SELECTOR;

/// Each IDT gate is 16 bytes (Intel SDM Vol. 3A §6.10.1, Fig. 6-10).
const IDT_ENTRIES: usize = 256;

#[repr(C, packed)]
#[derive(Clone, Copy)]
struct IdtEntry {
    offset_low16: u16,
    selector: u16,
    ist: u8,
    type_attr: u8,
    offset_mid16: u16,
    offset_high32: u32,
    reserved: u32,
}

/// 64-bit interrupt gate: P=1, DPL=0, type=0xE (Intel SDM §6.10.1).
const GATE_TYPE_INTERRUPT: u8 = 0x8E;

/// IDT register format (Intel SDM Vol. 3A §2.4.1).
#[repr(C, packed)]
struct IdtPtr {
    limit: u16,
    base: u64,
}

/// The IDT: 256 entries, zero-initialized in BSS. Only the vectors we handle
/// are populated by `init()`.
static mut IDT: [IdtEntry; IDT_ENTRIES] = [IdtEntry {
    offset_low16: 0,
    selector: 0,
    ist: 0,
    type_attr: 0,
    offset_mid16: 0,
    offset_high32: 0,
    reserved: 0,
}; IDT_ENTRIES];

/// Sets one IDT entry to point at `handler` with the given IST slot (0 = none).
///
/// SAFETY: must be called only from `init()` during single-threaded early boot.
unsafe fn set_entry(vector: usize, handler: u64, selector: u16, ist: u8) {
    // SAFETY: caller guarantees single-threaded early boot; IDT is only
    // accessed from `init()` before any concurrency or re-entrancy.
    unsafe {
        let entry = &mut IDT[vector];
        entry.offset_low16 = handler as u16;
        entry.selector = selector;
        entry.ist = ist & 0x07;
        entry.type_attr = GATE_TYPE_INTERRUPT;
        entry.offset_mid16 = (handler >> 16) as u16;
        entry.offset_high32 = (handler >> 32) as u32;
        entry.reserved = 0;
    }
}

// External assembly symbols: one per exception vector (defined in interrupt.rs).
unsafe extern "sysv64" {
    fn exc_vector_0();
    fn exc_vector_1();
    fn exc_vector_2();
    fn exc_vector_3();
    fn exc_vector_4();
    fn exc_vector_5();
    fn exc_vector_6();
    fn exc_vector_7();
    fn exc_vector_9();
    fn exc_vector_13();
    fn exc_vector_14();
    fn exc_vector_16();
    fn exc_vector_18();
    fn exc_vector_20();
    fn exc_vector_30();
}

/// Populates the IDT with CPU exception gates and loads it via `lidt`.
///
/// Must be called after GDT init (selectors reference the kernel code segment)
/// and before any code that might fault. Interrupts remain disabled (IF=0).
pub fn init() {
    unsafe {
        // SAFETY: single-threaded early boot; functions are `'static`-lived
        // naked stubs whose addresses fit in 64 bits.
        // Vectors without error code (dummy 0 pushed by stub).
        set_entry(0, handler_addr(exc_vector_0), KERNEL_CODE_SELECTOR, 0);
        set_entry(1, handler_addr(exc_vector_1), KERNEL_CODE_SELECTOR, 0);
        set_entry(2, handler_addr(exc_vector_2), KERNEL_CODE_SELECTOR, 0);
        set_entry(3, handler_addr(exc_vector_3), KERNEL_CODE_SELECTOR, 0);
        set_entry(4, handler_addr(exc_vector_4), KERNEL_CODE_SELECTOR, 0);
        set_entry(5, handler_addr(exc_vector_5), KERNEL_CODE_SELECTOR, 0);
        set_entry(6, handler_addr(exc_vector_6), KERNEL_CODE_SELECTOR, 0);
        set_entry(7, handler_addr(exc_vector_7), KERNEL_CODE_SELECTOR, 0);
        set_entry(9, handler_addr(exc_vector_9), KERNEL_CODE_SELECTOR, 0);
        set_entry(16, handler_addr(exc_vector_16), KERNEL_CODE_SELECTOR, 0);
        set_entry(18, handler_addr(exc_vector_18), KERNEL_CODE_SELECTOR, 0);
        set_entry(20, handler_addr(exc_vector_20), KERNEL_CODE_SELECTOR, 0);
        set_entry(30, handler_addr(exc_vector_30), KERNEL_CODE_SELECTOR, 0);

        // Vectors with error code (no dummy push in stub).
        // IST=1 for vector 8 (double fault) → TSS.ist1 (dedicated stack).
        set_entry(13, handler_addr(exc_vector_13), KERNEL_CODE_SELECTOR, 0);
        set_entry(14, handler_addr(exc_vector_14), KERNEL_CODE_SELECTOR, 0);

        let idt_ptr = IdtPtr {
            limit: (core::mem::size_of::<IdtEntry>() * IDT_ENTRIES - 1) as u16,
            base: &raw const IDT as u64,
        };

        // SAFETY: lidt loads the IDT base and limit into the CPU's IDTR
        // register. The table lives in BSS (.bss), which is mapped by the
        // bootloader's higher-half page tables. Single-threaded early boot.
        asm!(
            "lidt [{idt_ptr}]",
            idt_ptr = in(reg) &idt_ptr,
            options(nostack),
        );
    }
}

/// Type-erase a naked vector-stub address to a 64-bit integer.
///
/// SAFETY: caller must pass an `exc_vector_*` symbol defined in `interrupt.rs`.
unsafe fn handler_addr(f: unsafe extern "sysv64" fn()) -> u64 {
    f as *const () as u64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn idt_entry_size() {
        assert_eq!(core::mem::size_of::<IdtEntry>(), 16);
    }

    #[test]
    fn idt_ptr_size() {
        assert_eq!(core::mem::size_of::<IdtPtr>(), 10);
    }

    #[test]
    fn gate_type_is_64bit_interrupt() {
        // 64-bit interrupt gate = 0xE in the type nibble; P=1, DPL=0 in the
        // upper nibble → type_attr = 0x8E.
        assert_eq!(GATE_TYPE_INTERRUPT, 0x8E);
    }
}
