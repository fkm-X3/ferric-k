//! Thin kernel bin: links `ferric-unsafe-core` so the final image retains the
//! entry point and boot path. All ABI symbols, hardware access, and the
//! panic handler live in unsafe-core, keeping this crate 100% safe under
//! `#![forbid(unsafe_code)]`.
#![no_std]
#![no_main]
#![forbid(unsafe_code)]

// The bin's only job is to force the linker to keep the unsafe-core object
// (whose `_start` is the entry); cargo would otherwise drop a library it
// never sees referenced from here.
#[used]
static KEEP_UNSAFE_CORE_LINKED: [fn() -> !; 1] = [ferric_unsafe_core::halt];
