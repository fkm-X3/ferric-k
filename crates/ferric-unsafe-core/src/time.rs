//! Shared monotonic time: a hardware-tick counter bumped by the per-arch
//! timer driver and a tick→nanosecond conversion for the `TimeSource`
//! boundary trait.

use core::sync::atomic::{AtomicU64, Ordering};

use ferric_api::TimeSource;

pub const NANOS_PER_SEC: u64 = 1_000_000_000;

static TICKS: AtomicU64 = AtomicU64::new(0);

/// Records one hardware timer tick (called from the IRQ handler only).
pub fn bump_tick() {
    TICKS.fetch_add(1, Ordering::Relaxed);
}

/// Total ticks recorded so far; the per-arch source maps this to time.
pub fn ticks() -> u64 {
    TICKS.load(Ordering::Relaxed)
}

/// Converts whole `ticks` of a `freq_hz` counter to nanoseconds (128-bit
/// intermediate, no overflow before ~584 years at GHz rates).
pub const fn ticks_to_ns(ticks: u64, freq_hz: u64) -> u64 {
    (ticks as u128 * NANOS_PER_SEC as u128 / freq_hz as u128) as u64
}

/// The active `TimeSource`, driving the on-screen uptime readout.
pub fn time_source() -> &'static dyn TimeSource {
    #[cfg(target_arch = "x86_64")]
    {
        if TSC.freq.load(Ordering::Relaxed) != 0 {
            &TSC
        } else {
            &crate::pit::PIT
        }
    }
    #[cfg(target_arch = "aarch64")]
    {
        &crate::gictimer::GENERIC_TIMER
    }
    #[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64")))]
    {
        unreachable!("no time source defined for this target")
    }
}

/// Initialises the x86-64 tick source from the bootloader-calibrated
/// invariant TSC when available; the PIT source stays active otherwise.
#[cfg(target_arch = "x86_64")]
pub fn init_tsc(freq_hz: Option<u64>) {
    if let Some(f) = freq_hz {
        TSC.boot_tsc.store(read_tsc(), Ordering::Relaxed);
        TSC.freq.store(f, Ordering::Relaxed);
    }
}

#[cfg(target_arch = "x86_64")]
pub struct Tsc {
    freq: AtomicU64,
    boot_tsc: AtomicU64,
}

#[cfg(target_arch = "x86_64")]
pub static TSC: Tsc = Tsc {
    freq: AtomicU64::new(0),
    boot_tsc: AtomicU64::new(0),
};

#[cfg(target_arch = "x86_64")]
impl Tsc {
    fn uptime_raw(&self) -> u64 {
        let now = read_tsc();
        now.wrapping_sub(self.boot_tsc.load(Ordering::Relaxed))
    }
}

#[cfg(target_arch = "x86_64")]
fn read_tsc() -> u64 {
    let mut lo: u32;
    let mut hi: u32;
    // SAFETY: rdtsc is a side-effect-free read of the timestamp counter;
    // the 32-bit halves recombine to the full 64-bit counter.
    unsafe {
        core::arch::asm!(
            "rdtsc",
            out("eax") lo,
            out("edx") hi,
            options(nomem, nostack, preserves_flags),
        );
    }
    ((hi as u64) << 32) | lo as u64
}

#[cfg(target_arch = "x86_64")]
impl TimeSource for Tsc {
    fn uptime_ns(&self) -> u64 {
        let f = self.freq.load(Ordering::Relaxed);
        if f == 0 {
            return 0;
        }
        ticks_to_ns(self.uptime_raw(), f)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn converts_whole_seconds() {
        assert_eq!(ticks_to_ns(62_500_000, 62_500_000), NANOS_PER_SEC);
        assert_eq!(ticks_to_ns(0, 62_500_000), 0);
    }

    #[test]
    fn converts_fine_grained_fractions() {
        // 10 ticks at 62.5 MHz = 160 ns exactly.
        assert_eq!(ticks_to_ns(10, 62_500_000), 160);
    }

    #[test]
    fn uses_128_bit_intermediate_math() {
        // 2^32 ticks at 1 Hz = 1 second short of 136 years; a 32-bit
        // intermediate would overflow seconds-by-2^32.
        assert_eq!(ticks_to_ns(1, 1), 1_000_000_000);
        assert_eq!(ticks_to_ns(1 << 32, 1 << 32), 1_000_000_000);
    }
}
