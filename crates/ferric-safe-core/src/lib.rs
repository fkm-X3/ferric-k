//! Architecture-independent kernel logic — console model, font/text-grid
//! rendering, logging — as pure logic over byte buffers and `ferric-api`
//! traits, fully host-testable.
#![cfg_attr(not(test), no_std)]
#![forbid(unsafe_code)]

pub mod clock_app;
pub mod font;
pub mod grid;
pub mod line_editor;
pub mod log;
pub mod monitor;
pub mod scancodes;
pub mod shell;
pub mod terminal;

pub use ferric_api::{Key, KeyEvent, Rgb};
pub use font::Font;
pub use grid::{Cell, GlyphStyle, Surface, TextGrid};
pub use line_editor::{LineAction, LineEditor, MAX_LINE_CHARS};
pub use monitor::{DisplayInfo, MemorySummary, MonitorModel, RegionView};
pub use scancodes::ScancodeDecoder;
pub use shell::{Command, parse_command};
pub use terminal::terminal_byte_to_key;

pub use ferric_api::{
    MAX_MEMORY_REGIONS, MemoryRegion, MemoryRegionKind, MonitorSample, MonitorSource,
};
