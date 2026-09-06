//! Local time-of-day for the kernel: derives a broken-down clock from the
//! monotonic uptime, treating boot as `00:00:00`. Ferric-K has no real-time
//! clock yet, so this is the accessible "local time" other components (the
//! clock app) read; it ticks forward from boot and wraps at 24 h.

use ferric_api::{Clock, TimeOfDay};

pub const SECONDS_PER_MINUTE: u64 = 60;
pub const SECONDS_PER_HOUR: u64 = 3600;
pub const SECONDS_PER_DAY: u64 = 86_400;

/// The kernel-wide clock: uptime-derived time-of-day.
pub struct UptimeClock;

pub static CLOCK: UptimeClock = UptimeClock;

impl Clock for UptimeClock {
    fn local_time(&self) -> TimeOfDay {
        from_uptime_ns(crate::time::time_source().uptime_ns())
    }
}

/// Converts monotonic nanoseconds since boot into a 24 h time-of-day.
fn from_uptime_ns(ns: u64) -> TimeOfDay {
    let secs = (ns / crate::time::NANOS_PER_SEC) % SECONDS_PER_DAY;
    TimeOfDay {
        hours: (secs / SECONDS_PER_HOUR) as u8,
        minutes: ((secs % SECONDS_PER_HOUR) / SECONDS_PER_MINUTE) as u8,
        seconds: (secs % SECONDS_PER_MINUTE) as u8,
    }
}

/// The current kernel local time, for any component that wants to read it.
pub fn time_of_day() -> TimeOfDay {
    CLOCK.local_time()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rounds_suptime_into_seconds_dropping_fraction() {
        // 1.5 s -> 1 s past boot.
        let c = TestClock(1_500_000_000);
        let t = c.local_time();
        assert_eq!(t, TimeOfDay::new(0, 0, 1));
    }

    #[test]
    fn rolls_minutes_and_hours() {
        let c = TestClock(63 * SECONDS_PER_MINUTE * crate::time::NANOS_PER_SEC);
        assert_eq!(c.local_time(), TimeOfDay::new(1, 3, 0));
    }

    #[test]
    fn wraps_after_24_hours() {
        let c = TestClock(25 * SECONDS_PER_HOUR * crate::time::NANOS_PER_SEC);
        assert_eq!(c.local_time(), TimeOfDay::new(1, 0, 0));
    }

    struct TestClock(u64);
    impl Clock for TestClock {
        fn local_time(&self) -> TimeOfDay {
            from_uptime_ns(self.0)
        }
    }
}
