//! Ferric-K kernel binary.
//!
//! Deliberately thin and 100% safe: every ABI symbol and all
//! hardware access belong to `ferric-unsafe-core`. This crate only wires
//! unsafe-core initialization into the safe `kmain`.
//!
//! The ELF entry `_start` is exported by `ferric-unsafe-core` so that this
//! crate can keep `#![forbid(unsafe_code)]` — `#[no_mangle]` is an
//! ABI-affecting unsafe attribute and cannot appear under a forbid.
#![no_std]
#![no_main]
#![forbid(unsafe_code)]

#[cfg(not(test))]
use core::panic::PanicInfo;

// Linker glue: nothing in this crate references `ferric-unsafe-core` yet, so
// the rlib object holding its exported `_start` would never be pulled into the
// final image and LLD would fail with "cannot find entry symbol". Taking the
// address of a public unsafe-core function in a `#[used]` static forces the
// resolution while keeping this crate 100% free of unsafe syntax.
#[used]
static KEEP_UNSAFE_CORE_LINKED: [fn() -> !; 1] = [ferric_unsafe_core::halt];

/// Temporary panic handler.

#[cfg(not(test))]
#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    loop {
        core::hint::spin_loop();
    }
}
