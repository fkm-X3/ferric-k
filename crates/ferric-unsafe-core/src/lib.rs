//! The sole home of `unsafe` in Ferric-K.
//!
//! Everything that must touch raw hardware or define ABI symbols lives here:
//! entry assembly, UART/framebuffer drivers, spinlocks, Limine ABI structs,
//! and the per-architecture modules under `arch/{x86_64,aarch64}` (cfg-gated).
//!
//! Contract (enforced by compiler + lints):
//! - The **public** API of this crate is 100% safe. Misuse is made impossible
//!   by types (bounds-checked framebuffer, initialized-once globals, lock
//!   guards), not by documentation.
//! - Every `unsafe` block carries a `// SAFETY:` justification
//!   (`clippy::undocumented_unsafe_blocks` is a hard error).
//! - `unsafe_op_in_unsafe_fn` is denied: unsafety inside an `unsafe fn` must
//!   still be explicitly scoped and justified.
//!
//! This crate intentionally does *not* carry `#![forbid(unsafe_code)]`; every
//! other workspace crate does. See ARCHITECTURE.md.
//
// `no_std` is dropped under `cfg(test)` so the Limine ABI layout can be
// unit-tested on the host (the test harness needs std; the kernel code
// itself only ever uses `core::`, which std re-exports).
// `linkage` backs the weak mem* shims in `mem.rs`; host builds never see it.
#![cfg_attr(not(test), no_std)]
#![cfg_attr(not(test), feature(linkage))]

pub mod arch;
pub mod limine;
// Kernel-runtime only: defining `memcpy` etc. on the host would collide with
// the platform CRT when linking the test harness.
#[cfg(not(test))]
pub mod mem;
pub mod qemu;

/// Common early-boot path, reached from the architecture entry points
/// immediately after bootloader handoff.
///
/// Still running on the bootloader-provided stack at this point (>= 64 KiB,
/// return address 0 pushed — Limine protocol, "Machine State at Entry").
pub fn boot() -> ! {
    let boot_info = limine::collect();

    // Proof-of-execution while no console exists: report the outcome of early
    // init to the run harness through QEMU's isa-debug-exit device
    // (scripts/run.ps1 asserts the resulting exit code).
    #[cfg(all(target_arch = "x86_64", not(test)))]
    {
        let status = if boot_info.is_some() {
            qemu::STATUS_BOOT_OK
        } else {
            qemu::STATUS_BOOT_INFO_MISSING
        };
        qemu::debug_exit(status);
    }

    // Host-test builds and architectures without an isa-debug-exit
    // equivalent park forever for now.
    #[cfg(any(not(target_arch = "x86_64"), test))]
    {
        let _ = boot_info;
        halt()
    }
}

/// Parks the CPU forever.
pub fn halt() -> ! {
    loop {
        core::hint::spin_loop();
    }
}
