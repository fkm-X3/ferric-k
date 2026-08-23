//! Per-architecture entry points and early machine bring-up.
//!
//! Each module is cfg-gated to its target architecture and hosts the ELF
//! entry symbol plus any arch-specific pre-`boot` glue. Shared boot flow
//! lives in [`crate::boot`].

#[cfg(all(target_arch = "x86_64", not(test)))]
pub mod x86_64;

// Placeholder. Exists only so the aarch64 target keeps linking
// with a valid entry symbol and the quality gate stays green.
#[cfg(all(target_arch = "aarch64", not(test)))]
pub mod aarch64;
