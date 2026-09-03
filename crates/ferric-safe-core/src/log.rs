//! Compile-time filtered logging facade: `info!`, `warn!`, `error!` macros
//! that write through a [`Logger`] trait object. The global logger is set once
//! during boot (from `ferric-unsafe-core`'s console module) and remains fixed
//! for the kernel's lifetime.
//!
//! Compile-time constants (`MAX_LEVEL`, `LOG_LEVEL`) eliminate dead calls
//! at monomorphization. Runtime filtering via [`set_log_level`] gates the
//! remaining calls without the overhead of a level check on every write.

use core::fmt;

/// Log severity levels ordered from least to most severe.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
#[repr(u8)]
pub enum Level {
    Info = 0,
    Warn = 1,
    Error = 2,
}

impl Level {
    pub const fn from_u8(v: u8) -> Option<Self> {
        match v {
            0 => Some(Self::Info),
            1 => Some(Self::Warn),
            2 => Some(Self::Error),
            _ => None,
        }
    }
}

impl fmt::Display for Level {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Info => f.write_str("INFO"),
            Self::Warn => f.write_str("WARN"),
            Self::Error => f.write_str("ERROR"),
        }
    }
}

/// A sink that receives formatted log output. Implementations live in
/// `ferric-unsafe-core` and route to the dual serial + framebuffer console.
pub trait Logger {
    fn log(&self, level: Level, args: fmt::Arguments<'_>);
}

/// No-op logger used before a real logger is installed.
pub struct NoOpLogger;

impl Logger for NoOpLogger {
    fn log(&self, _level: Level, _args: fmt::Arguments<'_>) {}
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn log_level_ordering() {
        assert!(Level::Info < Level::Warn);
        assert!(Level::Warn < Level::Error);
        assert!(Level::Info < Level::Error);
    }

    #[test]
    fn level_display() {
        assert_eq!(Level::Info.to_string(), "INFO");
        assert_eq!(Level::Warn.to_string(), "WARN");
        assert_eq!(Level::Error.to_string(), "ERROR");
    }

    #[test]
    fn level_from_u8_round_trips() {
        assert_eq!(Level::from_u8(0), Some(Level::Info));
        assert_eq!(Level::from_u8(1), Some(Level::Warn));
        assert_eq!(Level::from_u8(2), Some(Level::Error));
        assert_eq!(Level::from_u8(3), None);
        assert_eq!(Level::from_u8(255), None);
    }

    #[test]
    fn noop_logger_does_not_panic() {
        let logger = NoOpLogger;
        logger.log(Level::Info, format_args!("test"));
        logger.log(Level::Warn, format_args!("test {}", 42));
        logger.log(Level::Error, format_args!("test"));
    }
}
