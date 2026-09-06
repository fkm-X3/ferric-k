//! One poll surface over whatever input device the architecture has: the
//! PS/2 keyboard of a PC, the terminal RX on the PL011 of a virt board.
//! The console's shell loop drains this in between renders.

use ferric_api::KeyEvent;

/// Next pending key event, or `None` when no device is pollable.
pub fn next_key() -> Option<KeyEvent> {
    #[cfg(all(target_arch = "x86_64", not(test)))]
    {
        crate::ps2::poll_next()
    }
    #[cfg(all(target_arch = "aarch64", not(test)))]
    {
        crate::pl011::with_serial(|uart| uart.poll_key()).flatten()
    }
    #[cfg(test)]
    None
}
