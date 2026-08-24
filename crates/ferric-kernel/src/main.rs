//! Thin kernel bin: wires `ferric-unsafe-core` init into the safe kernel
//! entry. All ABI symbols and hardware access live in unsafe-core, keeping
//! this crate 100% safe under `#![forbid(unsafe_code)]`.
#![no_std]
#![no_main]
#![forbid(unsafe_code)]

#[cfg(not(test))]
use core::panic::PanicInfo;

// Nothing references unsafe-core yet, so LLD would drop its object and lose
// `_start`; taking a function address in a #[used] static forces the link.
#[used]
static KEEP_UNSAFE_CORE_LINKED: [fn() -> !; 1] = [ferric_unsafe_core::halt];

#[cfg(not(test))]
#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    loop {
        core::hint::spin_loop();
    }
}
