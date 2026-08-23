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
#![no_std]

/// Temporary process entry symbol so the skeleton links as an ELF executable.
#[unsafe(no_mangle)]
pub extern "C" fn _start() -> ! {
    halt()
}

/// Halts the CPU forever.
/// Placeholder spin-halt.
pub fn halt() -> ! {
    loop {
        core::hint::spin_loop();
    }
}
