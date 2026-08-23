//! Safe, architecture-independent kernel logic.
//!
//! Console model, PSF2 font parsing, text grid/rendering, logging and shell
//! logic are written here — pure logic over byte buffers and traits from
//! `ferric-api`, fully host-testable before it ever touches hardware.
#![no_std]
#![forbid(unsafe_code)]
