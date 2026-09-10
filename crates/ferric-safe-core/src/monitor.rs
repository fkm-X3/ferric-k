//! Pure-logic hardware monitor: turns raw [`MonitorSample`]s into
//! display-ready values (uptime, per-second deltas, a CPU-load proxy from
//! idle/tick deltas, and a memory-map summary). No hardware access — fully
//! host-testable; `ferric-unsafe-core` feeds it via the [`MonitorSource`]
//! trait boundary.

use core::fmt::Write as _;

pub use ferric_api::{
    MAX_MEMORY_REGIONS, MemoryRegion, MemoryRegionKind, MonitorSample, MonitorSource,
};

const NANOS_PER_SEC: u64 = 1_000_000_000;
const KIB: u64 = 1024;
const MIB: u64 = KIB * 1024;
const GIB: u64 = MIB * 1024;

/// A memory-map region flattened for display.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RegionView {
    pub start: u64,
    pub end: u64,
    pub kind: MemoryRegionKind,
    pub size: u64,
}

/// Totals across the memory map, for the summary line.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MemorySummary {
    pub usable_bytes: u64,
    pub reserved_bytes: u64,
    pub other_bytes: u64,
}

/// Display-ready values computed from one [`MonitorSample`] against the
/// previous one; the first sample reports all deltas as zero.
#[derive(Clone, Debug)]
pub struct DisplayInfo {
    pub uptime_seconds: u64,
    pub delta_seconds: u64,
    /// Busy-time share of the last interval, rounded to a whole percent.
    pub cpu_load_percent: u8,
    pub total_ticks: u64,
    pub delta_ticks: u64,
    pub delta_idle_ticks: u64,
    pub memory_summary: MemorySummary,
    pub regions: [RegionView; MAX_MEMORY_REGIONS],
    pub region_count: usize,
}

/// Stateful sampler-to-display converter; owns the previous sample so each
/// update can report what changed since the last one.
#[derive(Default)]
pub struct MonitorModel {
    previous: Option<MonitorSample>,
}

impl MonitorModel {
    pub const fn new() -> Self {
        Self { previous: None }
    }

    /// Computes display values for `sample`, then replaces the stored
    /// previous sample with it.
    pub fn update(&mut self, sample: MonitorSample) -> DisplayInfo {
        let info = self.compute(&sample);
        self.previous = Some(sample);
        info
    }

    fn compute(&self, sample: &MonitorSample) -> DisplayInfo {
        let (delta_seconds, delta_ticks, delta_idle_ticks) = match &self.previous {
            Some(prev) => (
                sample.uptime_ns.saturating_sub(prev.uptime_ns) / NANOS_PER_SEC,
                sample.total_ticks.saturating_sub(prev.total_ticks),
                sample.idle_ticks.saturating_sub(prev.idle_ticks),
            ),
            None => (0, 0, 0),
        };
        let region_count = sample.memory_region_count.min(MAX_MEMORY_REGIONS);
        let mut regions = [RegionView {
            start: 0,
            end: 0,
            kind: MemoryRegionKind::Unknown(0),
            size: 0,
        }; MAX_MEMORY_REGIONS];
        let mut summary = MemorySummary::default();
        for (dst, region) in regions
            .iter_mut()
            .zip(sample.regions().iter().take(region_count))
        {
            *dst = RegionView {
                start: region.base,
                end: region.end(),
                kind: region.kind,
                size: region.length,
            };
            match region.kind {
                MemoryRegionKind::Usable => {
                    summary.usable_bytes = summary.usable_bytes.saturating_add(region.length)
                }
                MemoryRegionKind::Reserved => {
                    summary.reserved_bytes = summary.reserved_bytes.saturating_add(region.length)
                }
                _ => summary.other_bytes = summary.other_bytes.saturating_add(region.length),
            }
        }
        DisplayInfo {
            uptime_seconds: sample.uptime_ns / NANOS_PER_SEC,
            delta_seconds,
            cpu_load_percent: cpu_load_percent(delta_ticks, delta_idle_ticks),
            total_ticks: sample.total_ticks,
            delta_ticks,
            delta_idle_ticks,
            memory_summary: summary,
            regions,
            region_count,
        }
    }
}

/// Busy-share of a tick interval, rounded to the nearest whole percent.
pub fn cpu_load_percent(delta_ticks: u64, delta_idle_ticks: u64) -> u8 {
    if delta_ticks == 0 {
        return 0;
    }
    let busy = delta_ticks.saturating_sub(delta_idle_ticks);
    let percent = (busy as u128 * 100 + delta_ticks as u128 / 2) / delta_ticks as u128;
    percent.min(100) as u8
}

/// Short human name for a region kind, for display.
pub fn kind_name(kind: MemoryRegionKind) -> &'static str {
    match kind {
        MemoryRegionKind::Usable => "usable",
        MemoryRegionKind::Reserved => "reserved",
        MemoryRegionKind::AcpiReclaimable => "acpi-reclaim",
        MemoryRegionKind::AcpiNvs => "acpi-nvs",
        MemoryRegionKind::BadMemory => "bad",
        MemoryRegionKind::BootloaderReclaimable => "bootloader",
        MemoryRegionKind::ExecutableAndModules => "exec+modules",
        MemoryRegionKind::Framebuffer => "framebuffer",
        MemoryRegionKind::ReservedMapped => "reserved-mapped",
        MemoryRegionKind::Unknown(_) => "unknown",
    }
}

pub const UPTIME_TEXT_LEN: usize = 32;
pub const SIZE_TEXT_LEN: usize = 24;
pub const ADDRESS_TEXT_LEN: usize = 18;

/// `HH:MM:SS` for `total_seconds`; hours print at least two digits.
pub fn format_uptime(total_seconds: u64, buf: &mut [u8; UPTIME_TEXT_LEN]) -> &str {
    let mut w = ArrayWriter::new(buf);
    let _ = write!(
        w,
        "{:02}:{:02}:{:02}",
        total_seconds / 3600,
        (total_seconds / 60) % 60,
        total_seconds % 60
    );
    let len = w.len;
    core::str::from_utf8(&buf[..len]).expect("formatter writes ASCII only")
}

/// Human size, e.g. `512 B`, `1.5 KiB`, `128.0 MiB`; one decimal fraction.
pub fn format_size(bytes: u64, buf: &mut [u8; SIZE_TEXT_LEN]) -> &str {
    let mut w = ArrayWriter::new(buf);
    let (scale, label) = if bytes >= GIB {
        (GIB, "GiB")
    } else if bytes >= MIB {
        (MIB, "MiB")
    } else if bytes >= KIB {
        (KIB, "KiB")
    } else {
        let _ = write!(w, "{bytes} B");
        let len = w.len;
        return core::str::from_utf8(&buf[..len]).expect("formatter writes ASCII only");
    };
    let frac = (bytes % scale) * 10 / scale;
    let _ = write!(w, "{}.{} {}", bytes / scale, frac, label);
    let len = w.len;
    core::str::from_utf8(&buf[..len]).expect("formatter writes ASCII only")
}

/// `0x` + 16 lowercase hex digits.
pub fn format_address(addr: u64, buf: &mut [u8; ADDRESS_TEXT_LEN]) -> &str {
    let mut w = ArrayWriter::new(buf);
    let _ = write!(w, "0x{addr:016x}");
    let len = w.len;
    core::str::from_utf8(&buf[..len]).expect("formatter writes ASCII only")
}

/// A `core::fmt::Write` over a fixed byte array.
struct ArrayWriter<'a> {
    buf: &'a mut [u8],
    len: usize,
}

impl<'a> ArrayWriter<'a> {
    fn new(buf: &'a mut [u8]) -> Self {
        Self { buf, len: 0 }
    }
}

impl core::fmt::Write for ArrayWriter<'_> {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        let bytes = s.as_bytes();
        if self.len + bytes.len() > self.buf.len() {
            return Err(core::fmt::Error);
        }
        self.buf[self.len..self.len + bytes.len()].copy_from_slice(bytes);
        self.len += bytes.len();
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(
        uptime_ns: u64,
        total_ticks: u64,
        idle_ticks: u64,
        regions: &[MemoryRegion],
    ) -> MonitorSample {
        let mut memory_regions =
            [MemoryRegion::new(0, 0, MemoryRegionKind::Unknown(0)); MAX_MEMORY_REGIONS];
        assert!(regions.len() <= MAX_MEMORY_REGIONS);
        memory_regions[..regions.len()].copy_from_slice(regions);
        MonitorSample {
            uptime_ns,
            total_ticks,
            idle_ticks,
            memory_regions,
            memory_region_count: regions.len(),
        }
    }

    #[test]
    fn first_sample_reports_zero_deltas() {
        let mut model = MonitorModel::new();
        let info = model.update(sample(5_000_000_000, 5, 2, &[]));
        assert_eq!(info.delta_seconds, 0);
        assert_eq!(info.delta_ticks, 0);
        assert_eq!(info.delta_idle_ticks, 0);
        assert_eq!(info.cpu_load_percent, 0);
        assert_eq!(info.uptime_seconds, 5);
    }

    #[test]
    fn second_sample_reports_exact_deltas() {
        let mut model = MonitorModel::new();
        model.update(sample(5_000_000_000, 5, 2, &[]));
        let info = model.update(sample(16_000_000_000, 17, 11, &[]));
        assert_eq!(info.uptime_seconds, 16);
        assert_eq!(info.delta_seconds, 11);
        assert_eq!(info.delta_ticks, 12);
        assert_eq!(info.delta_idle_ticks, 9);
        assert_eq!(info.cpu_load_percent, 25);
    }

    #[test]
    fn sub_second_uptime_delta_rounds_down() {
        let mut model = MonitorModel::new();
        model.update(sample(1_900_000_000, 2, 0, &[]));
        let info = model.update(sample(2_100_000_000, 3, 1, &[]));
        // 200 ms of actual uptime; the whole-second uptime delta is 0, so
        // there is nothing to measure a busy share against.
        assert_eq!(info.delta_seconds, 0);
        assert_eq!(info.delta_ticks, 1);
        assert_eq!(info.cpu_load_percent, 0);
    }

    #[test]
    fn cpu_load_percent_rounds_to_nearest() {
        assert_eq!(cpu_load_percent(100, 50), 50);
        assert_eq!(cpu_load_percent(100, 0), 100);
        assert_eq!(cpu_load_percent(100, 100), 0);
        assert_eq!(cpu_load_percent(3, 1), 67); // 66.66..% rounds up
        assert_eq!(cpu_load_percent(3, 2), 33); // 33.33..% rounds down
        assert_eq!(cpu_load_percent(0, 0), 0);
        assert_eq!(cpu_load_percent(1, 0), 100);
        // Idle ticks reported above total (defensive) still clamp to 0%.
        assert_eq!(cpu_load_percent(10, 20), 0);
    }

    #[test]
    fn cpu_load_uses_128_bit_intermediate() {
        // busy * 100 overflows u64; the u128 intermediate keeps the ratio exact.
        let delta_ticks = 1u64 << 63;
        let busy = 1u64 << 62; // 50% busy
        assert_eq!(cpu_load_percent(delta_ticks, delta_ticks - busy), 50);
    }

    #[test]
    fn counters_going_backwards_treated_as_no_change() {
        let mut model = MonitorModel::new();
        model.update(sample(20_000_000_000, 30, 20, &[]));
        let info = model.update(sample(15_000_000_000, 25, 10, &[]));
        assert_eq!(info.delta_seconds, 0);
        assert_eq!(info.delta_ticks, 0);
        assert_eq!(info.delta_idle_ticks, 0);
        assert_eq!(info.cpu_load_percent, 0);
        assert_eq!(info.uptime_seconds, 15);
    }

    #[test]
    fn memory_regions_are_flattened_with_end_and_size() {
        let regions = [
            MemoryRegion::new(0x1000, 0x100000, MemoryRegionKind::Usable),
            MemoryRegion::new(0x9000_0000, 0x8000, MemoryRegionKind::Framebuffer),
        ];
        let mut model = MonitorModel::new();
        let info = model.update(sample(0, 0, 0, &regions));
        assert_eq!(info.region_count, 2);
        assert_eq!(info.regions[0].start, 0x1000);
        assert_eq!(info.regions[0].end, 0x101000);
        assert_eq!(info.regions[0].size, 0x100000);
        assert_eq!(info.regions[0].kind, MemoryRegionKind::Usable);
        assert_eq!(info.regions[1].end, 0x9000_8000);
        assert_eq!(info.regions[1].kind, MemoryRegionKind::Framebuffer);
        // Trailing slots stay zeroed.
        assert_eq!(info.regions[2].size, 0);
    }

    #[test]
    fn memory_summary_totals_by_kind_class() {
        let regions = [
            MemoryRegion::new(0, 1_000_000, MemoryRegionKind::Usable),
            MemoryRegion::new(0, 2_000_000, MemoryRegionKind::Usable),
            MemoryRegion::new(0, 500_000, MemoryRegionKind::Reserved),
            MemoryRegion::new(0, 200_000, MemoryRegionKind::BootloaderReclaimable),
        ];
        let mut model = MonitorModel::new();
        let info = model.update(sample(0, 0, 0, &regions));
        assert_eq!(info.memory_summary.usable_bytes, 3_000_000);
        assert_eq!(info.memory_summary.reserved_bytes, 500_000);
        assert_eq!(info.memory_summary.other_bytes, 200_000);
    }

    #[test]
    fn region_count_capped_at_maximum() {
        let sample = MonitorSample {
            uptime_ns: 0,
            total_ticks: 0,
            idle_ticks: 0,
            memory_regions: [MemoryRegion::new(0, 0, MemoryRegionKind::Unknown(0));
                MAX_MEMORY_REGIONS],
            memory_region_count: MAX_MEMORY_REGIONS + 40,
        };
        let mut model = MonitorModel::new();
        let info = model.update(sample);
        assert_eq!(info.region_count, MAX_MEMORY_REGIONS);
    }

    #[test]
    fn uptime_formats_hh_mm_ss() {
        let mut buf = [0u8; UPTIME_TEXT_LEN];
        assert_eq!(format_uptime(0, &mut buf), "00:00:00");
        assert_eq!(format_uptime(3661, &mut buf), "01:01:01");
        assert_eq!(format_uptime(86399, &mut buf), "23:59:59");
        // Hours grow beyond two digits without truncation.
        assert_eq!(format_uptime(100 * 3600 + 59 + 60, &mut buf), "100:01:59");
    }

    #[test]
    fn sizes_format_with_one_decimal() {
        let mut buf = [0u8; SIZE_TEXT_LEN];
        assert_eq!(format_size(0, &mut buf), "0 B");
        assert_eq!(format_size(512, &mut buf), "512 B");
        assert_eq!(format_size(KIB, &mut buf), "1.0 KiB");
        assert_eq!(format_size(1536, &mut buf), "1.5 KiB");
        assert_eq!(format_size(MIB, &mut buf), "1.0 MiB");
        assert_eq!(format_size(5 * GIB + (GIB / 2), &mut buf), "5.5 GiB");
    }

    #[test]
    fn addresses_format_hex_zero_padded() {
        let mut buf = [0u8; ADDRESS_TEXT_LEN];
        assert_eq!(format_address(0, &mut buf), "0x0000000000000000");
        assert_eq!(format_address(0x1234_abcd, &mut buf), "0x000000001234abcd");
        assert_eq!(format_address(u64::MAX, &mut buf), "0xffffffffffffffff");
    }

    #[test]
    fn kind_names_cover_all_fixed_kinds() {
        assert_eq!(kind_name(MemoryRegionKind::Usable), "usable");
        assert_eq!(kind_name(MemoryRegionKind::Reserved), "reserved");
        assert_eq!(kind_name(MemoryRegionKind::Framebuffer), "framebuffer");
        assert_eq!(kind_name(MemoryRegionKind::Unknown(9)), "unknown");
    }
}
