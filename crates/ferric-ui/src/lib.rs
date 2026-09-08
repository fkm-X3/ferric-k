#![no_std]
// deny (not forbid) so Slint's macro-generated code — which carries its own
// `allow(unsafe_code)` overrides deep inside `include_modules!` — may compile.
// forbid is sticky and forbids those overrides; deny preserves the guarantee
// that our own hand-written code in this crate stays free of unsafe.
#![deny(unsafe_code)]

extern crate alloc;

slint::include_modules!();
