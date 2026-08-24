//! Arch-neutral traits forming the boundary between safe logic and hardware
//! implementations (`TextSink`, `TimeSource`, ...); implementors live in
//! `ferric-unsafe-core`.
#![no_std]
#![forbid(unsafe_code)]
