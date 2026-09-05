//! aarch64 entry point. Limine hands off with the MMU and caches on,
//! interrupts masked, SP at the top of a >=64 KiB bootloader stack, and all
//! GPRs zeroed ("Machine State at Entry", Limine protocol). Base revision 6:
//! `CPACR_EL1` is 0 at entry, and entry is at EL1 or — when the bootloader
//! runs at EL2 — EL2 with VHE, where `*_EL1` accesses hit the EL2 bank via
//! VHE register redirection.

use crate::mmu::{self, ENTRIES};
use crate::pl011;
use crate::volatile::Volatile;

/// Naked ELF entry point (`ENTRY(_start)` in `kernels/aarch64.ld`); never returns.
#[unsafe(naked)]
#[unsafe(no_mangle)]
pub extern "C" fn _start() -> ! {
    core::arch::naked_asm!(
        // The target spec keeps NEON enabled, so LLVM may emit FP/SIMD from
        // the first Rust statements on; base revision 6 guarantees
        // CPACR_EL1 == 0 (CPTR_EL2 == 0 on the EL2 path), so open FPEN before
        // any Rust runs. The write lands in the redirected register bank at
        // EL2+VHE, covering both entry states (ARM DDI 0487 CPACR_EL1.FPEN).
        "mov x9, {fpen}",
        "msr cpacr_el1, x9",
        // SCTLR_EL1.{SA, SA0} are live: rounding down can only move SP
        // deeper into the provided stack. Logical ops take no SP operand,
        // so the mask runs through a GPR.
        "mov x9, sp",
        "mov x10, #16",
        "neg x10, x10",
        "and x9, x9, x10",
        // Fix the SP bank: exceptions at EL1 use SP_EL1, so from here on the
        // kernel runs with SPSel=1 and the aligned stack lives in SP_EL1.
        // Handoff may have left the stack in SP_EL0 (SPSel=0).
        "msr spsel, #1",
        "mov sp, x9",
        "bl {boot}",
        "brk #0",
        fpen = const 0b11 << 20,
        boot = sym crate::boot,
    );
}

/// Maps one 2 MiB block covering the PL011 window into the bootloader's live
/// higher-half tables; false when the tables do not match the expected
/// shape. Device regions are absent from Limine's direct map (it maps memory
/// map ranges only), so MMIO stays inaccessible until this runs.
pub fn map_uart_window(hhdm_offset: u64) -> bool {
    let va = hhdm_offset + pl011::UART0_BASE as u64;

    // SAFETY: TTBR1_EL1 holds the physical base of the bootloader-provided
    // higher-half table (Limine protocol, "Machine State at Entry"); its
    // storage is mapped through the direct map for as long as the kernel
    // runs because Ferric-K never reclaims bootloader memory.
    let l0_phys = unsafe {
        let ttbr1: u64;
        core::arch::asm!(
            "mrs {}, ttbr1_el1",
            out(reg) ttbr1,
            options(nomem, nostack, preserves_flags)
        );
        ttbr1 & mmu::OUT_ADDR_MASK
    };
    if l0_phys == 0 {
        return false;
    }
    let l0 = table_at(hhdm_offset.wrapping_add(l0_phys));
    let top_entry = l0[mmu::l0_index(va)].read();
    if top_entry & mmu::VALID == 0 || top_entry & mmu::TABLE_OR_PAGE == 0 {
        return false;
    }
    let mut l1 = table_at(hhdm_offset.wrapping_add(top_entry & mmu::OUT_ADDR_MASK));

    match mmu::insert_block_2m(&l0, &mut l1, va, pl011::UART0_BASE as u64) {
        Ok(()) => {}
        Err(mmu::InsertBlockError::Occupied) => {
            // Only acceptable when the slot already holds exactly our block.
            return l1[mmu::l1_index(va)].read()
                == mmu::block_descriptor_2m(pl011::UART0_BASE as u64);
        }
        Err(_) => return false,
    }

    // Make the new block visible: write barrier, invalidate all of stage-1,
    // then order subsequent accesses (ARM DDI 0487 barrier/TLBI requirements).
    // SAFETY: bare synchronisation instructions with no memory operands.
    unsafe {
        core::arch::asm!(
            "dsb ish",
            "tlbi vmalle1",
            "dsb ish",
            "isb",
            options(nostack)
        );
    }
    true
}

fn table_at(hhdm_va: u64) -> [Volatile<u64>; ENTRIES] {
    core::array::from_fn(|i| Volatile::new((hhdm_va as *mut u64).wrapping_add(i)))
}
