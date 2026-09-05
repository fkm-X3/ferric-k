//! Arch-neutral traits (`TextSink`, `TimeSource`, ...) and shared data types
//! forming the boundary between safe logic and hardware implementations;
//! implementors live in `ferric-unsafe-core`.
#![no_std]
#![forbid(unsafe_code)]

/// A destination for kernel text output, implemented per architecture.
pub trait TextSink {
    /// Writes all of `s`, blocking until the device accepts every byte. Line
    /// endings arrive as bare `\n`; implementations translate for the device.
    fn write_str(&mut self, s: &str);
}

/// A monotonic clock measuring time since the kernel's uptime counter
/// started, implemented per architecture.
pub trait TimeSource {
    /// Elapsed time since the counter started, in whole nanoseconds.
    fn uptime_ns(&self) -> u64;
}

/// An 8-bit-per-channel color, independent of a surface's pixel layout.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Rgb {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

impl Rgb {
    pub const fn new(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b }
    }
}
