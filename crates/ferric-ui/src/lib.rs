#![no_std]
// deny (not forbid) so Slint's macro-generated code — which carries its own
// `allow(unsafe_code)` overrides deep inside `include_modules!` — may compile.
// forbid is sticky and forbids those overrides; deny preserves the guarantee
// that our own hand-written code in this crate stays free of unsafe.
#![deny(unsafe_code)]

extern crate alloc;

// Each `.slint` entry point compiles to its own generated module (build.rs
// compiles both); `include_modules!` would only pull the last one, so include
// each file explicitly.
include!(concat!(env!("OUT_DIR"), "/main.rs"));
include!(concat!(env!("OUT_DIR"), "/monitor.rs"));

/// Builds the top-level `MainWindow` component; panics only if Slint's
/// backend setup failed.
pub fn main_window() -> MainWindow {
    MainWindow::new().expect("Slint MainWindow creation failed")
}

/// Builds the live hardware-monitor component; panics only if Slint's
/// backend setup failed.
pub fn monitor_window() -> MonitorWindow {
    MonitorWindow::new().expect("Slint MonitorWindow creation failed")
}
