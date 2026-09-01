//! PL011 UART (ARM PrimeCell, ARM DDI 0183G) for the QEMU virt platform's
//! first serial port; polled operation programmed as 115200-8N1.

use crate::sync::{OnceLock, Spinlock};
use crate::text::expand_lf_to_crlf;
use crate::volatile::Volatile;
use ferric_api::TextSink;

/// QEMU virt: PL011 uart0 MMIO base (QEMU `hw/arm/virt.c`, VIRT_UART).
pub const UART0_BASE: usize = 0x0900_0000;

/// QEMU virt clocks the PL011 at 24 MHz (QEMU `hw/arm/virt.c`); divisor math
/// per osdev wiki "PL011 UART": BAUDDIV = clock / (16 * baud)
/// = 24 MHz / (16 * 115200) = 13 + 1/48 -> IBRD 13, FBRD round(64/48) = 1.
const IBRD_115200: u32 = 13;
const FBRD_115200: u32 = 1;

// Register offsets within the PL011 block (ARM DDI 0183G, "Programmers'
// Model").
const REG_DR: usize = 0x000;
const REG_FR: usize = 0x018;
const REG_IBRD: usize = 0x024;
const REG_FBRD: usize = 0x028;
const REG_LCR_H: usize = 0x02C;
const REG_CR: usize = 0x030;
const REG_IMSC: usize = 0x038;
const REG_ICR: usize = 0x044;

// Flag register bits (ARM DDI 0183G, FR).
const FR_BUSY: u32 = 1 << 3;
const FR_TXFF: u32 = 1 << 5;

// Line control: FIFO enable; 8-bit word length (ARM DDI 0183G, LCR_H).
const LCR_H_FEN: u32 = 1 << 4;
const LCR_H_WLEN_8: u32 = 0b11 << 5;

// Control bits (ARM DDI 0183G, CR): UARTEN [0], TXE [8], RXE [9].
const CR_UARTEN: u32 = 1 << 0;
const CR_TXE: u32 = 1 << 8;
const CR_RXE: u32 = 1 << 9;

// Interrupt clear: all eleven sources (ARM DDI 0183G, ICR).
const ICR_ALL: u32 = 0x7FF;

/// A brought-up PL011 usable as a [`TextSink`].
pub struct Pl011Uart {
    dr: Volatile<u32>,
    fr: Volatile<u32>,
    ibrd: Volatile<u32>,
    fbrd: Volatile<u32>,
    lcr_h: Volatile<u32>,
    cr: Volatile<u32>,
    imsc: Volatile<u32>,
    icr: Volatile<u32>,
}

// SAFETY: the raw-pointer fields are accessed only through the global spin
// lock, and the MMIO window stays mapped for the kernel lifetime.
unsafe impl Send for Pl011Uart {}

impl Pl011Uart {
    const fn at(base: usize) -> Self {
        Self {
            dr: Volatile::new((base + REG_DR) as *mut u32),
            fr: Volatile::new((base + REG_FR) as *mut u32),
            ibrd: Volatile::new((base + REG_IBRD) as *mut u32),
            fbrd: Volatile::new((base + REG_FBRD) as *mut u32),
            lcr_h: Volatile::new((base + REG_LCR_H) as *mut u32),
            cr: Volatile::new((base + REG_CR) as *mut u32),
            imsc: Volatile::new((base + REG_IMSC) as *mut u32),
            icr: Volatile::new((base + REG_ICR) as *mut u32),
        }
    }

    /// Programs `base` for polled output per the TRM bring-up sequence:
    /// quiesce and disable, mask/clear interrupts, divisors + line control,
    /// then enable.
    pub fn new(base: usize) -> Self {
        let mut uart = Self::at(base);

        uart.cr.write(0);
        while (uart.fr.read() & FR_BUSY) != 0 {
            core::hint::spin_loop();
        }
        uart.icr.write(ICR_ALL);
        uart.imsc.write(0);
        uart.ibrd.write(IBRD_115200);
        uart.fbrd.write(FBRD_115200);
        uart.lcr_h.write(LCR_H_FEN | LCR_H_WLEN_8);
        uart.cr.write(CR_UARTEN | CR_TXE | CR_RXE);
        uart
    }

    fn send(&mut self, byte: u8) {
        while (self.fr.read() & FR_TXFF) != 0 {
            core::hint::spin_loop();
        }
        self.dr.write(byte as u32);
    }
}

impl TextSink for Pl011Uart {
    fn write_str(&mut self, s: &str) {
        expand_lf_to_crlf(s, |byte| self.send(byte));
    }
}

static PL011: OnceLock<Spinlock<Pl011Uart>> = OnceLock::new();

/// Brings up the PL011 at `base` and captures it into a global.
pub fn init_global(base: usize) -> bool {
    let uart = Pl011Uart::new(base);
    PL011.set(Spinlock::new(uart)).is_ok()
}

/// Runs `f` with exclusive access to the global PL011; `None` before init.
pub fn with_serial<R>(f: impl FnOnce(&mut Pl011Uart) -> R) -> Option<R> {
    let mut guard = PL011.get()?.lock();
    Some(f(&mut guard))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Register-counted stand-in for the MMIO window, allocated in host
    /// memory so volatile semantics are exercised end to end.
    #[repr(C, align(4))]
    struct FakeBlock {
        regs: [u32; (REG_ICR / 4) + 1],
    }

    impl FakeBlock {
        const FR: usize = REG_FR / 4;
        const IBRD: usize = REG_IBRD / 4;
        const FBRD: usize = REG_FBRD / 4;
        const LCR_H: usize = REG_LCR_H / 4;
        const CR: usize = REG_CR / 4;
        const IMSC: usize = REG_IMSC / 4;
        const ICR: usize = REG_ICR / 4;
        const DR: usize = REG_DR / 4;
    }

    #[test]
    fn base_and_offsets_match_documented_layout() {
        assert_eq!(UART0_BASE, 0x0900_0000);
        let offsets = [
            REG_DR, REG_FR, REG_IBRD, REG_FBRD, REG_LCR_H, REG_CR, REG_IMSC, REG_ICR,
        ];
        for i in 0..offsets.len() {
            for j in (i + 1)..offsets.len() {
                assert_ne!(offsets[i], offsets[j]);
            }
        }
        assert_eq!((UART0_BASE + REG_ICR) % 4, 0);
    }

    #[test]
    fn constants_match_trm_bit_definitions() {
        assert_eq!(FR_BUSY, 0x08);
        assert_eq!(FR_TXFF, 0x20);
        assert_eq!(LCR_H_FEN | LCR_H_WLEN_8, 0x70);
        assert_eq!(CR_UARTEN | CR_TXE | CR_RXE, 0x301);
        assert_eq!(ICR_ALL, 0x7FF);
        assert_eq!(IBRD_115200, 13);
        assert_eq!(FBRD_115200, 1);
    }

    #[test]
    fn init_programmes_the_full_bringup_sequence() {
        let mut block = FakeBlock {
            regs: [0; (REG_ICR / 4) + 1],
        };
        block.regs[FakeBlock::FR] = 0; // transmitter idle

        // SAFETY: `block` outlives this test; its storage is initialized,
        // 4-byte aligned u32s standing in for the MMIO window.
        Pl011Uart::new(&mut block as *mut FakeBlock as usize);

        assert_eq!(block.regs[FakeBlock::DR], 0);
        assert_eq!(block.regs[FakeBlock::IBRD], IBRD_115200);
        assert_eq!(block.regs[FakeBlock::FBRD], FBRD_115200);
        assert_eq!(block.regs[FakeBlock::LCR_H], LCR_H_FEN | LCR_H_WLEN_8);
        assert_eq!(block.regs[FakeBlock::CR], CR_UARTEN | CR_TXE | CR_RXE);
        assert_eq!(block.regs[FakeBlock::IMSC], 0);
        assert_eq!(block.regs[FakeBlock::ICR], ICR_ALL);
    }

    #[test]
    fn transmit_pushes_line_ending_expanded_bytes_into_dr() {
        let mut block = FakeBlock {
            regs: [0; (REG_ICR / 4) + 1],
        };

        // SAFETY: same stand-in storage as the init test.
        let mut uart = Pl011Uart::new(&mut block as *mut FakeBlock as usize);
        block.regs[FakeBlock::FR] = 0; // room in the TX FIFO

        uart.write_str("a\n");
        assert_eq!(block.regs[FakeBlock::DR], b'\n' as u32);
    }
}
