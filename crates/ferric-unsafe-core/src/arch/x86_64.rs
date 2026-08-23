//! x86_64 entry point.
//!
//! Machine state at handoff (Limine protocol, "Machine State at Entry",
//! x86-64): paging enabled with the executable mapped in the higher half,
//! interrupts off, `CS = 0x28` / data selectors `0x30`, all general-purpose
//! registers zeroed except `rsp`, which points at the top of a
//! bootloader-provided stack of at least 64 KiB (in bootloader-reclaimable
//! memory) with a return address of 0 pushed on top. We deliberately keep
//! that stack for now — switching to kernel-owned stacks happens before any
//! future bootloader-memory reclamation (ARCHITECTURE.md D-10).

/// ELF entry point (`ENTRY(_start)` in `kernels/x86_64.ld`).
///
/// Naked on purpose: no prologue may touch the stack or registers before we
/// have established our own ABI assumptions. The body is exactly:
///
/// 1. `xor ebp, ebp` — terminate frame-pointer chains at the boot boundary,
///    so stack unwinders stop here instead of wandering into bootloader
///    memory.
/// 2. `and rsp, -16` — the protocol does not promise `rsp % 16`, but the
///    System V ABI expects `rsp % 16 == 8` inside a function prologue
///    (return address pushed onto a 16-aligned stack). LLVM is allowed to
///    emit aligned SSE spills under our `-sse,+sse2` target features, so we
///    enforce the alignment ourselves rather than trusting handoff state.
///    Clobbers nothing live: every GPR except `rsp` is specified as zero at
///    entry and `boot` takes no arguments.
/// 3. `call` into the common Rust boot path; it never returns (`-> !`).
/// 4. `ud2` — belt-and-braces trap if that invariant is ever broken.
#[unsafe(naked)]
#[unsafe(no_mangle)]
pub extern "C" fn _start() -> ! {
    core::arch::naked_asm!(
        "xor ebp, ebp",
        "and rsp, -16",
        "call {boot}",
        "ud2",
        boot = sym crate::boot,
    );
}
