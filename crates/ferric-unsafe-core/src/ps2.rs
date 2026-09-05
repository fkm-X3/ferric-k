//! x86_64 PS/2 keyboard: polls the 8042 controller output buffer and feeds a
//! scancode-set-1 decoder from `ferric-safe-core`. No command/responder
//! dance — the controller is already in translated set 1 under QEMU; a make
//! byte is exactly the raw value on the data port. Hardware constants cite
//! the osdev wiki "8042 PS/2 Controller" page.

use crate::port::Port;
use crate::sync::{OnceLock, Spinlock};
use ferric_api::{InputDevice, KeyEvent};
use ferric_safe_core::ScancodeDecoder;

/// 8042 data port: read delivers the next output byte.
const DATA_PORT: u16 = 0x60;
/// 8042 status port: bit 0 signals the output buffer is full.
const STATUS_PORT: u16 = 0x64;
/// Status bit 0: controller output buffer full.
const STATUS_OUTPUT_FULL: u8 = 1;

/// The 8042 controller: two narrow ports, no state worth framing in a type.
pub struct Ps2 {
    data: Port<u8>,
    status: Port<u8>,
}

impl Ps2 {
    /// Port handles over the controller's two I/O addresses.
    pub const fn new() -> Self {
        Self {
            data: Port::new(DATA_PORT),
            status: Port::new(STATUS_PORT),
        }
    }
    /// True when the keyboard is holding a scancode for the CPU.
    pub fn output_full(&self) -> bool {
        self.status.read() & STATUS_OUTPUT_FULL != 0
    }

    /// One scancode byte, or `None` when the output buffer is empty.
    pub fn poll_byte(&mut self) -> Option<u8> {
        if !self.output_full() {
            return None;
        }
        Some(self.data.read())
    }

    /// Discards anything the firmware left in the output buffer so the
    /// decoder starts from a clean slate.
    pub fn flush(&mut self) {
        while self.output_full() {
            self.data.read();
        }
    }
}

impl Default for Ps2 {
    fn default() -> Self {
        Self::new()
    }
}

/// Polling keyboard: an 8042 controller + streaming set-1 decoder.
pub struct Ps2Keyboard {
    controller: Ps2,
    decoder: ScancodeDecoder,
}

impl Ps2Keyboard {
    /// Idle keyboard over the live controller, draining anything queued by
    /// firmware before the kernel took over.
    pub fn new() -> Self {
        let mut controller = Ps2::new();
        controller.flush();
        Self {
            controller,
            decoder: ScancodeDecoder::new(),
        }
    }

    /// Drains scancode bytes until the decoder yields an event or the output
    /// buffer runs dry.
    pub fn next_key(&mut self) -> Option<KeyEvent> {
        loop {
            let byte = self.controller.poll_byte()?;
            if let Some(event) = self.decoder.push(byte) {
                return Some(event);
            }
        }
    }
}

impl InputDevice for Ps2Keyboard {
    fn poll(&mut self) -> Option<KeyEvent> {
        self.next_key()
    }
}

impl Default for Ps2Keyboard {
    fn default() -> Self {
        Self::new()
    }
}

static KEYBOARD: OnceLock<Spinlock<Ps2Keyboard>> = OnceLock::new();

/// Installs the global keyboard; false when already installed. Runs after
/// the loader has left the keyboard idle, so the flush is cheap.
pub fn init() -> bool {
    KEYBOARD.set(Spinlock::new(Ps2Keyboard::new())).is_ok()
}

/// One event from the global keyboard; `None` when idle or uninitialized.
pub fn poll_next() -> Option<KeyEvent> {
    KEYBOARD.get()?.lock().next_key()
}
