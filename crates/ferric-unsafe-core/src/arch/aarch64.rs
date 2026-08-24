//! aarch64 entry point. Limine hands off at EL1 (base revision < 6) with the
//! MMU and caches on, interrupts masked, SP at the top of a >=64 KiB
//! bootloader stack, all GPRs zeroed, and `VBAR_EL1` undefined ("Machine
//! State at Entry", Limine protocol).

/// Naked ELF entry point (`ENTRY(_start)` in `kernels/aarch64.ld`); never returns.
#[unsafe(naked)]
#[unsafe(no_mangle)]
pub extern "C" fn _start() -> ! {
    core::arch::naked_asm!(
        // The target spec keeps NEON enabled, so LLVM may emit FP/SIMD from
        // the first Rust statements on; base revisions < 6 leave CPACR_EL1
        // unspecified, so open FPEN first (ARM DDI 0487 CPACR_EL1.FPEN).
        "mov x9, {fpen}",
        "msr cpacr_el1, x9",
        // SCTLR_EL1.{SA, SA0} are live: rounding down can only move SP
        // deeper into the provided stack. Logical ops take no SP operand,
        // so the mask runs through a GPR.
        "mov x9, sp",
        "mov x10, #16",
        "neg x10, x10",
        "and x9, x9, x10",
        "mov sp, x9",
        "bl {boot}",
        "brk #0",
        fpen = const 0b11 << 20,
        boot = sym crate::boot,
    );
}
