//! x86_64 timer path: remaps the dual 8259A PICs to vectors 0x20/0x28 and
//! programs PIT channel 0 for a ~1 kHz tick, then exposes the shared
//! `TimeSource` trait. Interrupt gate for IRQ0 is vector [`IRQ0_VECTOR`].

use crate::port::Port;
use ferric_api::TimeSource;

/// PIT oscillator frequency (osdev wiki "Programmable Interval Timer").
const PIT_OSCILLATOR_HZ: u64 = 1_193_182;

/// Channel-0 reload value: oscillator / 1193 ≈ 1000.15 Hz.
const TIMER_DIVISOR: u16 = 1193;

/// Command word: channel 0, lobyte/highbyte access, mode 3 (square wave),
/// 16-bit binary (osdev wiki "Programmable Interval Timer").
const CMD_MODE_3: u8 = 0x36;

/// Master 8259A command/data ports (Intel SDM Vol. 3A §10.9).
const PIC1_CMD: u16 = 0x20;
const PIC1_DATA: u16 = 0x21;
/// Slave 8259A command/data ports.
const PIC2_CMD: u16 = 0xA0;
const PIC2_DATA: u16 = 0xA1;

/// ICW1: ICW4 needed, cascade mode, edge triggered.
const ICW1: u8 = 0x11;
/// ICW2 offsets: master vector base 0x20, slave 0x28.
const ICW2_MASTER: u8 = 0x20;
const ICW2_SLAVE: u8 = 0x28;
/// ICW3 cascade wiring: master line 2, slave cascade id 2.
const ICW3_MASTER: u8 = 0x04;
const ICW3_SLAVE: u8 = 0x02;
/// ICW4: 8086 mode.
const ICW4: u8 = 0x01;
/// Master mask: timer (IRQ0) + cascade (IRQ2) unmasked, everything else off.
const MASTER_MASK: u8 = !((1 << 0) | (1 << 2));
/// Slave mask: all IRQs masked (timer is the only source so far).
const SLAVE_MASK: u8 = 0xFF;

/// End-of-interrupt command for non-specific EOI on the master.
const PIC_EOI: u8 = 0x20;

/// Interrupt vector IRQ0 lands on after the PIC remap.
pub const IRQ0_VECTOR: u8 = 0x20;

/// # `ticks` to nanoseconds: each period is exactly `divisor/oscillator` s.
const fn pit_ticks_to_ns(ticks: u64) -> u64 {
    (ticks as u128 * crate::time::NANOS_PER_SEC as u128 * TIMER_DIVISOR as u128
        / PIT_OSCILLATOR_HZ as u128) as u64
}

/// The x86_64 time source: each tick equals one PIT channel-0 period.
pub struct Pit;

/// Global [`Pit`] handed out by [`crate::time::time_source`].
pub static PIT: Pit = Pit;

impl TimeSource for Pit {
    fn uptime_ns(&self) -> u64 {
        pit_ticks_to_ns(crate::time::ticks())
    }
}

/// Legacy I/O recovery delay after each 8259A programming byte; the POST
/// port 0x80 always decodes as an unassigned slot on q35 (osdev wiki
/// "I/O delay").
fn io_wait() {
    // SAFETY: one byte read from an unassigned 8-bit port costs bus time.
    unsafe {
        core::arch::asm!("in al, 0x80", out("al") _, options(nostack));
    }
}

/// Remaps the PICs so hardware IRQs land at vectors 0x20..0x2F instead of
/// aliasing CPU exceptions 0–7 (Intel SDM Vol. 3A §10.9).
fn remap_pic() {
    let mut pic1_cmd = Port::<u8>::new(PIC1_CMD);
    let mut pic1_data = Port::<u8>::new(PIC1_DATA);
    let mut pic2_cmd = Port::<u8>::new(PIC2_CMD);
    let mut pic2_data = Port::<u8>::new(PIC2_DATA);

    pic1_cmd.write(ICW1);
    io_wait();
    pic2_cmd.write(ICW1);
    io_wait();
    pic1_data.write(ICW2_MASTER);
    io_wait();
    pic2_data.write(ICW2_SLAVE);
    io_wait();
    pic1_data.write(ICW3_MASTER);
    io_wait();
    pic2_data.write(ICW3_SLAVE);
    io_wait();
    pic1_data.write(ICW4);
    io_wait();
    pic2_data.write(ICW4);
    io_wait();
    pic1_data.write(MASTER_MASK);
    io_wait();
    pic2_data.write(SLAVE_MASK);
}

/// Programs PIT channel 0 for a ~1 kHz square wave and unmasks IRQ0 on the
/// master PIC. Must run before interrupts are enabled.
pub fn init() {
    remap_pic();

    let mut cmd = Port::<u8>::new(0x43);
    let mut ch0 = Port::<u8>::new(0x40);
    cmd.write(CMD_MODE_3);
    ch0.write(TIMER_DIVISOR as u8);
    ch0.write((TIMER_DIVISOR >> 8) as u8);
}

/// IRQ0 handler: records the tick and sends the master EOI so IRQ1+ can
/// deliver. Never prints (the console spinlock may be held by the
/// interrupted thread).
pub fn handle_timer_irq() {
    crate::time::bump_tick();
    let mut pic1_cmd = Port::<u8>::new(PIC1_CMD);
    pic1_cmd.write(PIC_EOI);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn constants_match_the_chip_docs() {
        assert_eq!(PIT_OSCILLATOR_HZ, 1_193_182);
        assert_eq!(TIMER_DIVISOR, 1193);
        assert_eq!(CMD_MODE_3, 0x36);
        assert_eq!(IRQ0_VECTOR, 0x20);
        assert_eq!(PIC1_CMD, 0x20);
        assert_eq!(PIC1_DATA, 0x21);
        assert_eq!(PIC2_CMD, 0xA0);
        assert_eq!(PIC2_DATA, 0xA1);
    }

    #[test]
    fn timer_frequency_is_approximately_1khz() {
        // 1193182 / 1193 ≈ 1000.15 Hz: within 0.02% of 1000.
        let actual_hz_x1000 = PIT_OSCILLATOR_HZ * 1000 / TIMER_DIVISOR as u64;
        assert!((999_800..=1_000_200).contains(&actual_hz_x1000));
    }

    #[test]
    fn conversion_uses_the_exact_oscillator_not_nominal_1khz() {
        // 1,000 ticks = 1,000 × 1193/1193182 s ≈ 0.999847 s, i.e. 0.153 ms
        // under a naive 1,000,000,000 ns.
        assert_eq!(pit_ticks_to_ns(1_000), 999_847_466);
        assert_eq!(pit_ticks_to_ns(0), 0);
    }
}
