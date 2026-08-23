//! aarch64 entry point — PLACEHOLDER.

#[unsafe(no_mangle)]
pub extern "C" fn _start() -> ! {
    crate::boot()
}
