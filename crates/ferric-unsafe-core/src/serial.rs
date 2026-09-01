//! 16550 UART driver for legacy COM ports; register set and init sequence
//! per osdev wiki "Serial Ports". Polled operation, fixed 115200 baud 8N1.

use crate::port::Port;
use crate::sync::{OnceLock, Spinlock};
use crate::text::expand_lf_to_crlf;
use ferric_api::TextSink;

/// COM1 on PCs (osdev wiki "Serial Ports").
pub const COM1_BASE: u16 = 0x3f8;

// Register offsets from the port base (osdev wiki "Serial Ports").
const REG_DATA: u16 = 0x0; // THR/RBR; divisor low byte while DLAB set
const REG_IER: u16 = 0x1; // divisor high byte while DLAB set
const REG_FCR: u16 = 0x2;
const REG_LCR: u16 = 0x3;
const REG_MCR: u16 = 0x4;
const REG_LSR: u16 = 0x5;

const LSR_DATA_READY: u8 = 1 << 0;
const LSR_TRANSMIT_EMPTY: u8 = 1 << 5;

const LCR_DLAB: u8 = 0x80;
/// 8 data bits, no parity, 1 stop bit.
const LCR_8N1: u8 = 0x03;

const FCR_ENABLE: u8 = 1 << 0;
const FCR_CLEAR_RX: u8 = 1 << 1;
const FCR_CLEAR_TX: u8 = 1 << 2;
const FCR_TRIGGER_14: u8 = 0b11 << 6;

const MCR_DTR: u8 = 1 << 0;
const MCR_RTS: u8 = 1 << 1;
const MCR_OUT1: u8 = 1 << 2;
const MCR_OUT2: u8 = 1 << 3;
const MCR_LOOPBACK: u8 = 1 << 4;

const LOOPBACK_PROBE: u8 = 0xae;

/// A brought-up COM-port UART usable as a [`TextSink`].
pub struct Serial {
    data: Port<u8>,
    ier: Port<u8>,
    fcr: Port<u8>,
    lcr: Port<u8>,
    mcr: Port<u8>,
    lsr: Port<u8>,
}

impl Serial {
    const fn at(base: u16) -> Self {
        Self {
            data: Port::new(base + REG_DATA),
            ier: Port::new(base + REG_IER),
            fcr: Port::new(base + REG_FCR),
            lcr: Port::new(base + REG_LCR),
            mcr: Port::new(base + REG_MCR),
            lsr: Port::new(base + REG_LSR),
        }
    }

    /// Programs `base` for polled output (115200-8N1, FIFOs flushed) and
    /// verifies TX/RX with a loopback echo; `None` when absent or dead.
    pub fn new(base: u16) -> Option<Self> {
        let mut uart = Self::at(base);

        uart.ier.write(0);
        uart.lcr.write(LCR_DLAB);
        uart.data.write(0x01); // divisor latch low: speed = 115200 / 1
        uart.ier.write(0x00); // divisor latch high
        uart.lcr.write(LCR_8N1);
        uart.fcr
            .write(FCR_ENABLE | FCR_CLEAR_RX | FCR_CLEAR_TX | FCR_TRIGGER_14);
        uart.mcr.write(MCR_DTR | MCR_RTS | MCR_OUT1 | MCR_OUT2);

        uart.mcr.write(MCR_LOOPBACK | MCR_RTS | MCR_OUT1 | MCR_OUT2);
        uart.data.write(LOOPBACK_PROBE);
        while (uart.lsr.read() & LSR_DATA_READY) == 0 {
            core::hint::spin_loop();
        }
        let echoed = uart.data.read();
        uart.mcr.write(MCR_DTR | MCR_RTS | MCR_OUT1 | MCR_OUT2);
        if echoed != LOOPBACK_PROBE {
            return None;
        }
        Some(uart)
    }

    fn send(&mut self, byte: u8) {
        while (self.lsr.read() & LSR_TRANSMIT_EMPTY) == 0 {
            core::hint::spin_loop();
        }
        self.data.write(byte);
    }
}

impl TextSink for Serial {
    fn write_str(&mut self, s: &str) {
        expand_lf_to_crlf(s, |byte| self.send(byte));
    }
}

static SERIAL: OnceLock<Spinlock<Serial>> = OnceLock::new();

/// Probes `base` and captures the UART into a global; `false` on failure.
pub fn init_global(base: u16) -> bool {
    let Some(serial) = Serial::new(base) else {
        return false;
    };
    SERIAL.set(Spinlock::new(serial)).is_ok()
}

/// Runs `f` with exclusive access to the global serial; `None` before init.
pub fn with_serial<R>(f: impl FnOnce(&mut Serial) -> R) -> Option<R> {
    let mut guard = SERIAL.get()?.lock();
    Some(f(&mut guard))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn constants_match_osdev_canonical_init_values() {
        assert_eq!(COM1_BASE, 0x3f8);
        assert_eq!(LCR_DLAB, 0x80);
        assert_eq!(LCR_8N1, 0x03);
        assert_eq!(
            FCR_ENABLE | FCR_CLEAR_RX | FCR_CLEAR_TX | FCR_TRIGGER_14,
            0xC7
        );
        assert_eq!(MCR_DTR | MCR_RTS | MCR_OUT1 | MCR_OUT2, 0x0F);
        assert_eq!(MCR_LOOPBACK | MCR_RTS | MCR_OUT1 | MCR_OUT2, 0x1E);
        assert_eq!(LSR_DATA_READY, 0x01);
        assert_eq!(LSR_TRANSMIT_EMPTY, 0x20);
    }

    #[test]
    fn register_offsets_are_distinct_within_the_eight_byte_window() {
        let offsets = [REG_DATA, REG_IER, REG_FCR, REG_LCR, REG_MCR, REG_LSR];
        for i in 0..offsets.len() {
            for j in (i + 1)..offsets.len() {
                assert_ne!(offsets[i], offsets[j]);
            }
        }
        assert!(offsets.iter().all(|&off| off < 8));
    }
}
