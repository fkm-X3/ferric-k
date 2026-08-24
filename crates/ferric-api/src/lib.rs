//! Arch-neutral traits forming the boundary between safe logic and hardware
//! implementations (`TextSink`, `TimeSource`, ...); implementors live in
//! `ferric-unsafe-core`.
#![no_std]
#![forbid(unsafe_code)]

/// A destination for kernel text output, implemented per architecture.
pub trait TextSink {
    /// Writes all of `s`, blocking until the device accepts every byte. Line
    /// endings arrive as bare `\n`; implementations translate for the device.
    fn write_str(&mut self, s: &str);
}
