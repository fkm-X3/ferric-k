//! Per-architecture entry points and early bring-up; shared flow in [`crate::boot`].

#[cfg(all(target_arch = "x86_64", not(test)))]
pub mod x86_64;

#[cfg(all(target_arch = "aarch64", not(test)))]
pub mod aarch64;
