//! aarch64 timer path: GICv2 distributor + CPU-interface bring-up and the
//! EL1 physical generic timer, either driving the shared `TimeSource`. The
//! GIC registers live inside the first 1 GiB of physical memory, which the
//! boot-time window at `hhdm` already maps.

#[cfg(target_arch = "aarch64")]
use core::sync::atomic::{AtomicU64, Ordering};

use crate::volatile::Volatile;
#[cfg(target_arch = "aarch64")]
use ferric_api::TimeSource;

// QEMU virt platform GICv2 register bases (QEMU `hw/arm/virt.c`, VIRT_GIC_*
// for a gic-version=2 machine).
pub const GICD_BASE: usize = 0x0800_0000;
pub const GICC_BASE: usize = 0x0801_0000;

// Distributor offsets (ARM GIC-400 TRM, "Register map").
const GICD_CTLR: usize = 0x000;
const GICD_ISENABLER0: usize = 0x100;
const GICD_CTLR_ENABLE_GRP0: u32 = 1 << 0;
const GICD_CTLR_ENABLE_GRP1: u32 = 1 << 1;

// CPU-interface offsets (ARM GIC-400 TRM, "Register map").
const GICC_CTLR: usize = 0x000;
const GICC_PMR: usize = 0x004;
const GICC_IAR: usize = 0x00C;
const GICC_EOIR: usize = 0x010;
const GICC_CTLR_ENABLE_GRP0: u32 = 1 << 0;
const GICC_CTLR_ENABLE_GRP1: u32 = 1 << 1;

/// Highest priority filter (0xFF): no interrupt is too low to signal.
const GICC_PMR_ALL: u32 = 0xFF;

/// IAR spurious-interrupt id (ARM GIC v2/v3 spec, "Interrupt Acknowledge
/// Register"): no active interrupt; must not write EOIR.
const GICC_SPURIOUS: u32 = 0x3FF;

/// CNTPNSIRQ, the EL1 physical timer PPI on CPU0 (ARM DDI 0487, "Generic
/// timer"; GIC bank PPI 14).
const IRQ_CNTPNSIRQ: u32 = 30;

/// The timer re-arms with frequency/1000, i.e. a ~1 ms tick.
#[cfg(target_arch = "aarch64")]
const PERIOD_DIVISOR: u64 = 1000;

/// Physical GIC CPU-interface base (virtual offset into the `hhdm` window).
#[cfg(target_arch = "aarch64")]
static CPU_BASE: AtomicU64 = AtomicU64::new(0);

/// `CNTFRQ_EL0` captured at init; 0 before `init` (never used on the host).
#[cfg(target_arch = "aarch64")]
static FREQ_HZ: AtomicU64 = AtomicU64::new(0);

/// The aarch64 time source: nanoseconds derived from the physical counter.
pub struct GenericTimer;

/// Global [`GenericTimer`] handed out by [`crate::time::time_source`].
pub static GENERIC_TIMER: GenericTimer = GenericTimer;

#[cfg(target_arch = "aarch64")]
impl TimeSource for GenericTimer {
    fn uptime_ns(&self) -> u64 {
        crate::time::ticks_to_ns(read_cntpct(), FREQ_HZ.load(Ordering::Relaxed))
    }
}

/// Register handles over the GIC MMIO window; register semantics host-tested
/// against fake storage.
struct GicRegs {
    dist_ctlr: Volatile<u32>,
    isenabler0: Volatile<u32>,
    cpu_ctlr: Volatile<u32>,
    pmr: Volatile<u32>,
}

impl GicRegs {
    const fn at(dist_base: usize, cpu_base: usize) -> Self {
        Self {
            dist_ctlr: Volatile::new((dist_base + GICD_CTLR) as *mut u32),
            isenabler0: Volatile::new((dist_base + GICD_ISENABLER0) as *mut u32),
            cpu_ctlr: Volatile::new((cpu_base + GICC_CTLR) as *mut u32),
            pmr: Volatile::new((cpu_base + GICC_PMR) as *mut u32),
        }
    }

    /// Brings up both controller halves: forward groups 0+1, unmask the
    /// timer PPI, enable the CPU interface at full priority.
    fn program(&mut self) {
        self.dist_ctlr
            .write(GICD_CTLR_ENABLE_GRP0 | GICD_CTLR_ENABLE_GRP1);
        self.isenabler0.write(1 << IRQ_CNTPNSIRQ);
        self.cpu_ctlr
            .write(GICC_CTLR_ENABLE_GRP0 | GICC_CTLR_ENABLE_GRP1);
        self.pmr.write(GICC_PMR_ALL);
    }
}

/// Programs the GIC + generic timer for the given `hhdm_offset`; returns
/// false if the registers cannot be addressed (no such machine here).
#[cfg(target_arch = "aarch64")]
pub fn init(hhdm_offset: u64) -> bool {
    let dist_base = hhdm_offset + GICD_BASE as u64;
    let cpu_base = hhdm_offset + GICC_BASE as u64;
    CPU_BASE.store(cpu_base, Ordering::Relaxed);

    let mut gic = GicRegs::at(dist_base as usize, cpu_base as usize);
    gic.program();

    let freq = read_cntfrq().max(1);
    FREQ_HZ.store(freq, Ordering::Release);
    arm_timer(freq / PERIOD_DIVISOR);
    true
}

/// Timer IRQ handler: acknowledges the PPI, re-arms the compare value
/// (level-triggered — re-arm before EOI or it re-fires immediately), then
/// signals EOI. Never prints.
#[cfg(target_arch = "aarch64")]
pub fn handle_timer_irq() {
    let cpu_base = CPU_BASE.load(Ordering::Relaxed) as usize;
    let iar = Volatile::<u32>::new((cpu_base + GICC_IAR) as *mut u32);
    let intid = iar.read();
    if intid == GICC_SPURIOUS {
        return;
    }
    if intid & 0x3FF == IRQ_CNTPNSIRQ {
        let freq = FREQ_HZ.load(Ordering::Relaxed);
        if freq > 0 {
            arm_timer(freq / PERIOD_DIVISOR);
        }
    }
    let mut eoir = Volatile::<u32>::new((cpu_base + GICC_EOIR) as *mut u32);
    eoir.write(intid);
}

/// Reads `CNTFRQ_EL0`: ticks/second of the system counter.
#[cfg(target_arch = "aarch64")]
fn read_cntfrq() -> u64 {
    let freq: u64;
    // SAFETY: reads the EL1-readable counter frequency (ARM DDI 0487).
    unsafe {
        core::arch::asm!(
            "mrs {freq}, cntfrq_el0",
            freq = out(reg) freq,
            options(nomem, nostack, preserves_flags),
        );
    }
    freq
}

/// Reads `CNTPCT_EL0`: the running physical counter value.
#[cfg(target_arch = "aarch64")]
fn read_cntpct() -> u64 {
    let pct: u64;
    // SAFETY: reads the EL1-readable physical counter (ARM DDI 0487).
    unsafe {
        core::arch::asm!(
            "mrs {pct}, cntpct_el0",
            pct = out(reg) pct,
            options(nomem, nostack, preserves_flags),
        );
    }
    pct
}

/// Reloads `CNTP_TVAL_EL0` with `delta` and enables the EL1 physical timer
/// with its interrupt unmasked.
#[cfg(target_arch = "aarch64")]
fn arm_timer(delta: u64) {
    // SAFETY: bare msr/mrs to the timer control registers; the scratch GPR
    // is caller-saved and declared (ARM DDI 0487, CNTP_TVAL/CNTP_CTL).
    unsafe {
        core::arch::asm!(
            "msr cntp_tval_el0, {val}",
            "mrs {ctl}, cntp_ctl_el0",
            "orr {ctl}, {ctl}, #1",
            "bic {ctl}, {ctl}, #2",
            "msr cntp_ctl_el0, {ctl}",
            val = in(reg) delta,
            ctl = out(reg) _,
            options(nostack),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[repr(C, align(4))]
    struct FakeGic {
        regs: [u32; (GICC_PMR / 4) + 1],
    }

    #[test]
    fn bases_and_offsets_match_the_virt_and_timer_layouts() {
        assert_eq!(GICD_BASE, 0x0800_0000);
        assert_eq!(GICC_BASE, 0x0801_0000);
        let dist = [GICD_CTLR, GICD_ISENABLER0];
        let cpu = [GICC_CTLR, GICC_PMR, GICC_IAR, GICC_EOIR];
        for (i, list) in [dist.as_slice(), cpu.as_slice()].into_iter().enumerate() {
            for a in 0..list.len() {
                for b in (a + 1)..list.len() {
                    assert_ne!(list[a], list[b]);
                }
                assert_eq!(
                    (if i == 0 { GICD_BASE } else { GICC_BASE } + list[a]) % 4,
                    0
                );
            }
        }
        assert_eq!(IRQ_CNTPNSIRQ, 30);
        assert_eq!(GICC_SPURIOUS, 0x3FF);
    }

    #[test]
    fn program_brings_up_both_halves() {
        let block = FakeGic {
            regs: [0; (GICC_PMR / 4) + 1],
        };
        // GICD_ISENABLER0 sits at offset 0x100, beyond the 0x14-wide fake;
        // extend the window on the host behind the handles.
        let mut backing = [0u32; (GICD_ISENABLER0 / 4) + 1];
        let dist = backing.as_mut_ptr() as usize;
        let cpu = block.regs.as_ptr() as usize;

        // SAFETY: `backing` and `block` outlive the handles; u32 storage.
        let mut gic = GicRegs::at(dist, cpu);
        gic.program();

        assert_eq!(backing[GICD_CTLR / 4], 0b11);
        assert_eq!(backing[GICD_ISENABLER0 / 4], 1 << IRQ_CNTPNSIRQ);
        assert_eq!(block.regs[GICC_CTLR / 4], 0b11);
        assert_eq!(block.regs[GICC_PMR / 4], GICC_PMR_ALL);
    }
}
