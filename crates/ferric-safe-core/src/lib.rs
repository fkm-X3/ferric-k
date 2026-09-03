//! Architecture-independent kernel logic — console model, font/text-grid
//! rendering, logging — as pure logic over byte buffers and `ferric-api`
//! traits, fully host-testable.
#![cfg_attr(not(test), no_std)]
#![forbid(unsafe_code)]

pub mod font;
pub mod grid;
pub mod log;

pub use ferric_api::Rgb;
pub use font::Font;
pub use grid::{Cell, GlyphStyle, Surface, TextGrid};
