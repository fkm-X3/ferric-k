//! Architectural boundary contract for Ferric-K.
//!
//! This crate defines arch-neutral traits (`TextSink`, `TimeSource`,
//! `MachineInfo`, ...) that higher-level crates program against. Hardware-facing
//! implementations live exclusively in `ferric-unsafe-core`, one per
//! architecture. See `ARCHITECTURE.md` for the layering rules.
#![no_std]
#![forbid(unsafe_code)]
