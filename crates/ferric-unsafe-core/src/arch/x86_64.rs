//! x86_64 entry point. Limine hands off with paging on (image mapped in the
//! higher half), interrupts off, and a >=64 KiB bootloader-provided stack
//! ("Machine State at Entry", Limine protocol).

/// Naked ELF entry point (`ENTRY(_start)` in `kernels/x86_64.ld`); never returns.
#[unsafe(naked)]
#[unsafe(no_mangle)]
pub extern "C" fn _start() -> ! {
    core::arch::naked_asm!(
        "xor ebp, ebp",
        // CR access takes only the 64-bit register form in long mode.
        "mov rax, cr4",
        // SSE2 is the target-spec baseline, so LLVM may emit SSE spills from
        // the first Rust statements on; Limine hands off with CR4.OSFXSR
        // clear, which makes such instructions #UD (Intel SDM Vol. 3A §2.5).
        "or rax, {osfxsr}",
        "mov cr4, rax",
        // SysV: rsp % 16 == 8 expected inside the callee prologue.
        "and rsp, -16",
        "call {boot}",
        "ud2",
        osfxsr = const 1 << 9,
        boot = sym crate::boot,
    );
}
