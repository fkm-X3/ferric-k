//! The sole home of `unsafe` in Ferric-K: entry assembly, drivers, locks,
//! Limine ABI, and the cfg-gated `arch/{x86_64,aarch64}` modules. The public
//! API stays 100% safe — misuse is prevented by types, not documentation.
//
// no_std dropped under cfg(test): the host test harness needs std.
#![cfg_attr(not(test), no_std)]
// linkage backs the weak mem* shims in `mem.rs`.
#![cfg_attr(not(test), feature(linkage))]

pub mod arch;
pub mod limine;
// Excluded on host: defining memcpy etc. would collide with the platform CRT
// when linking the test harness.
#[cfg(not(test))]
pub mod mem;
pub mod qemu;

/// Common early-boot path after bootloader handoff; still on the bootloader stack.
pub fn boot() -> ! {
    let boot_info = limine::collect();

    // Proof-of-execution pre-console: run.ps1 asserts the isa-debug-exit code.
    #[cfg(all(target_arch = "x86_64", not(test)))]
    {
        let status = if boot_info.is_some() {
            qemu::STATUS_BOOT_OK
        } else {
            qemu::STATUS_BOOT_INFO_MISSING
        };
        qemu::debug_exit(status);
    }

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
