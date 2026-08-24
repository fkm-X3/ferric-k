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
#[cfg(target_arch = "x86_64")]
pub mod port;
pub mod qemu;
#[cfg(target_arch = "x86_64")]
pub mod serial;

/// Common early-boot path after bootloader handoff; still on the bootloader stack.
pub fn boot() -> ! {
    #[cfg(all(target_arch = "x86_64", not(test)))]
    {
        use ferric_api::TextSink;

        const BANNER_OK: &str = "Ferric-K x86_64\nBOOT OK\n";
        const BANNER_INFO_MISSING: &str = "Ferric-K x86_64\nBOOT INFO MISSING\n";

        let Some(mut console) = serial::Serial::new(serial::COM1_BASE) else {
            qemu::debug_exit(qemu::STATUS_UART_FAULT);
        };

        let status = if limine::collect().is_some() {
            console.write_str(BANNER_OK);
            qemu::STATUS_BOOT_OK
        } else {
            console.write_str(BANNER_INFO_MISSING);
            qemu::STATUS_BOOT_INFO_MISSING
        };
        qemu::debug_exit(status);
    }

    #[cfg(any(not(target_arch = "x86_64"), test))]
    {
        // Validated but unused until the aarch64 path gains its own output.
        let _boot_info = limine::collect();
        halt()
    }
}

/// Parks the CPU forever.
pub fn halt() -> ! {
    loop {
        core::hint::spin_loop();
    }
}
