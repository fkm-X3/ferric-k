//! Architecture-independent kernel logic — console model, font/text-grid
//! rendering, logging — as pure logic over byte buffers and `ferric-api`
//! traits, fully host-testable.
#![no_std]
#![forbid(unsafe_code)]
