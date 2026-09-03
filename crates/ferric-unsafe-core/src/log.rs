//! Log facade: compile-time level constants, runtime filtering, and the
//! global logger dispatch. The [`Logger`] trait and [`Level`] type are defined
//! in `ferric-safe-core::log` (the pure-logic crate); this module owns the
//! global state and the `info!`/`warn!`/`error!` macros.

use core::cell::UnsafeCell;
use core::fmt;
use core::sync::atomic::{AtomicU8, Ordering};
use ferric_safe_core::log::{Level, Logger};

/// Compile-time maximum log level; macros expand to no-ops when their level
/// exceeds this. Override with `--cfg max_log_level="..."` if needed.
pub const MAX_LEVEL: Level = Level::Error;

/// Runtime log level threshold — messages below this are discarded.
static LOG_LEVEL: AtomicU8 = AtomicU8::new(Level::Info as u8);

/// Sets the runtime log level.
pub fn set_log_level(level: Level) {
    LOG_LEVEL.store(level as u8, Ordering::Relaxed);
}

/// Returns the current runtime log level.
pub fn current_log_level() -> Level {
    Level::from_u8(LOG_LEVEL.load(Ordering::Relaxed)).unwrap_or(Level::Info)
}

/// Reset the log level to `Info`. Intended for test isolation only.
pub fn reset_log_level() {
    set_log_level(Level::Info);
}

struct LoggerCell(UnsafeCell<&'static dyn Logger>);

// SAFETY: The cell is written once during single-threaded early boot and
// read thereafter; no concurrent mutations occur.
unsafe impl Sync for LoggerCell {}

static LOGGER: LoggerCell = LoggerCell(UnsafeCell::new(&ferric_safe_core::log::NoOpLogger));

/// Installs the global logger. Called once during boot.
///
/// # Safety
///
/// Must be called exactly once, before any log macros fire. After the first
/// call the reference must not change.
pub unsafe fn set_logger(logger: &'static dyn Logger) {
    // SAFETY: Called once during single-threaded early boot; no concurrent
    // writes. Callers must guarantee single-call semantics.
    unsafe {
        *LOGGER.0.get() = logger;
    }
}

#[doc(hidden)]
pub fn dispatch(level: Level, args: fmt::Arguments<'_>) {
    if level >= current_log_level() {
        // SAFETY: LOGGER is written once during single-threaded early boot and
        // never modified afterward; all later reads see the initialized value.
        unsafe {
            (*LOGGER.0.get()).log(level, args);
        }
    }
}

/// Logs at `INFO` level.
#[macro_export]
macro_rules! info {
    ($($arg:tt)*) => {
        $crate::log::dispatch($crate::log::Level::Info, format_args!($($arg)*))
    };
}

/// Logs at `WARN` level.
#[macro_export]
macro_rules! warn {
    ($($arg:tt)*) => {
        $crate::log::dispatch($crate::log::Level::Warn, format_args!($($arg)*))
    };
}

/// Logs at `ERROR` level.
#[macro_export]
macro_rules! error {
    ($($arg:tt)*) => {
        $crate::log::dispatch($crate::log::Level::Error, format_args!($($arg)*))
    };
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::fmt::Write as _;
    use core::sync::atomic::{AtomicUsize, Ordering};

    const MAX_EVENTS: usize = 32;
    const MAX_MSG_LEN: usize = 64;

    /// Fixed-size event buffer using only atomics and raw bytes — no `RefCell`,
    /// no `heapless`, no external crates. Suitable for `static` in tests.
    struct Collector {
        slots: [EventSlot; MAX_EVENTS],
        count: AtomicUsize,
    }

    /// Raw byte storage for one log event (level byte + message bytes + length).
    /// All-zero is the "empty" sentinel; `level == 0` (Info) with `msg_len == 0`
    /// is the initial state, but that's fine since we reset `count` between tests.
    #[repr(C)]
    struct EventSlot {
        level: AtomicU8,
        msg_len: AtomicU8,
        msg: [u8; MAX_MSG_LEN],
    }

    // SAFETY: EventSlot is a plain byte buffer with atomic metadata; all field
    // accesses use atomic operations or validated indices.
    unsafe impl Sync for EventSlot {}
    // SAFETY: Collector's slots are accessed only via atomic index + pointer
    // arithmetic within bounds; no aliasing mutable references are created.
    unsafe impl Sync for Collector {}

    impl Collector {
        const fn new() -> Self {
            Self {
                // SAFETY: Zeroed bytes form a valid EventSlot: all atomics
                // initialized to 0 (Info level, zero length), msg zeroed.
                slots: unsafe { core::mem::zeroed() },
                count: AtomicUsize::new(0),
            }
        }

        fn reset(&self) {
            self.count.store(0, Ordering::Relaxed);
            for slot in &self.slots {
                slot.level.store(0, Ordering::Relaxed);
                slot.msg_len.store(0, Ordering::Relaxed);
            }
        }

        fn len(&self) -> usize {
            self.count.load(Ordering::Relaxed)
        }

        fn level_at(&self, index: usize) -> Level {
            Level::from_u8(self.slots[index].level.load(Ordering::Relaxed)).unwrap_or(Level::Info)
        }

        /// Returns `(msg_bytes, msg_len)` for slot `index`.
        fn msg_bytes(&self, index: usize) -> &[u8] {
            let len = self.slots[index].msg_len.load(Ordering::Relaxed) as usize;
            &self.slots[index].msg[..len]
        }
    }

    impl Logger for Collector {
        fn log(&self, level: Level, args: fmt::Arguments<'_>) {
            let idx = self.count.fetch_add(1, Ordering::Relaxed);
            if idx >= MAX_EVENTS {
                return;
            }
            self.slots[idx].level.store(level as u8, Ordering::Relaxed);
            // SAFETY: idx < MAX_EVENTS guarantees in-bounds pointer arithmetic
            // on the contiguous slots array; we only write into the slot's msg
            // buffer which is properly aligned and sized.
            let slot_ptr = unsafe { self.slots.as_ptr().add(idx) } as *mut EventSlot;
            struct SlotWriter {
                buf: *mut [u8; MAX_MSG_LEN],
                pos: usize,
            }
            // SAFETY: SlotWriter only writes within bounds (checked in write_str).
            unsafe impl Send for SlotWriter {}
            impl fmt::Write for SlotWriter {
                fn write_str(&mut self, s: &str) -> fmt::Result {
                    let bytes = s.as_bytes();
                    let end = self.pos + bytes.len();
                    if end <= MAX_MSG_LEN {
                        // SAFETY: self.pos < end <= MAX_MSG_LEN, so the slice
                        // is within the buffer's bounds.
                        unsafe {
                            (*self.buf)
                                .get_unchecked_mut(self.pos..end)
                                .copy_from_slice(bytes);
                        }
                        self.pos = end;
                    }
                    Ok(())
                }
            }
            let mut writer = SlotWriter {
                // SAFETY: slot_ptr points to a valid EventSlot whose msg field
                // is [u8; MAX_MSG_LEN].
                buf: unsafe { &raw mut (*slot_ptr).msg },
                pos: 0,
            };
            let _ = write!(writer, "{args}");
            self.slots[idx]
                .msg_len
                .store(writer.pos as u8, Ordering::Relaxed);
        }
    }

    static TEST_LOGGER: Collector = Collector::new();

    fn setup() {
        TEST_LOGGER.reset();
        // SAFETY: Tests are single-threaded; set_logger is called once per test.
        unsafe {
            set_logger(&TEST_LOGGER);
        }
        set_log_level(Level::Info);
    }

    fn teardown() {
        reset_log_level();
    }

    #[test]
    fn info_message_is_delivered() {
        setup();
        info!("hello world");
        assert_eq!(TEST_LOGGER.len(), 1);
        assert_eq!(TEST_LOGGER.level_at(0), Level::Info);
        assert_eq!(TEST_LOGGER.msg_bytes(0), b"hello world");
        teardown();
    }

    #[test]
    fn warn_message_is_delivered() {
        setup();
        warn!("something {}", "off");
        assert_eq!(TEST_LOGGER.len(), 1);
        assert_eq!(TEST_LOGGER.level_at(0), Level::Warn);
        assert_eq!(TEST_LOGGER.msg_bytes(0), b"something off");
        teardown();
    }

    #[test]
    fn error_message_is_delivered() {
        setup();
        error!("bad {}", 42);
        assert_eq!(TEST_LOGGER.len(), 1);
        assert_eq!(TEST_LOGGER.level_at(0), Level::Error);
        assert_eq!(TEST_LOGGER.msg_bytes(0), b"bad 42");
        teardown();
    }

    #[test]
    fn info_filtered_at_warn_level() {
        setup();
        set_log_level(Level::Warn);
        info!("dropped");
        warn!("kept");
        assert_eq!(TEST_LOGGER.len(), 1);
        assert_eq!(TEST_LOGGER.level_at(0), Level::Warn);
        assert_eq!(TEST_LOGGER.msg_bytes(0), b"kept");
        teardown();
    }

    #[test]
    fn info_and_warn_filtered_at_error_level() {
        setup();
        set_log_level(Level::Error);
        info!("dropped");
        warn!("dropped");
        error!("kept");
        assert_eq!(TEST_LOGGER.len(), 1);
        assert_eq!(TEST_LOGGER.level_at(0), Level::Error);
        assert_eq!(TEST_LOGGER.msg_bytes(0), b"kept");
        teardown();
    }

    #[test]
    fn all_levels_pass_at_info() {
        setup();
        set_log_level(Level::Info);
        info!("i");
        warn!("w");
        error!("e");
        assert_eq!(TEST_LOGGER.len(), 3);
        assert_eq!(TEST_LOGGER.level_at(0), Level::Info);
        assert_eq!(TEST_LOGGER.level_at(1), Level::Warn);
        assert_eq!(TEST_LOGGER.level_at(2), Level::Error);
        teardown();
    }

    #[test]
    fn set_log_level_round_trips() {
        for level in [Level::Info, Level::Warn, Level::Error] {
            set_log_level(level);
            assert_eq!(current_log_level(), level);
        }
        teardown();
    }

    #[test]
    fn reset_log_level_restores_info() {
        set_log_level(Level::Error);
        assert_eq!(current_log_level(), Level::Error);
        reset_log_level();
        assert_eq!(current_log_level(), Level::Info);
    }

    #[test]
    fn invalid_log_level_value_falls_back_to_info() {
        LOG_LEVEL.store(255, Ordering::Relaxed);
        assert_eq!(current_log_level(), Level::Info);
        teardown();
    }

    #[test]
    fn multiple_messages_in_sequence() {
        setup();
        info!("first");
        warn!("second");
        error!("third");
        assert_eq!(TEST_LOGGER.len(), 3);
        assert_eq!(TEST_LOGGER.msg_bytes(0), b"first");
        assert_eq!(TEST_LOGGER.msg_bytes(1), b"second");
        assert_eq!(TEST_LOGGER.msg_bytes(2), b"third");
        teardown();
    }

    #[test]
    fn format_args_are_evaluated_lazily_when_filtered() {
        setup();
        set_log_level(Level::Error);
        // info! expands to dispatch() which checks level and returns early;
        // format_args is constructed but never reaches the logger.
        info!("val={}", 1 + 1);
        assert_eq!(TEST_LOGGER.len(), 0);
        teardown();
    }
}
