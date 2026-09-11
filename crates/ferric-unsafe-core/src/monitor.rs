//! Live hardware monitor: samples arch counters and the Limine memory map via
//! [`MonitorSource`], feeds the pure-logic [`MonitorModel`], and
//! binds each second's display values to the Slint `MonitorWindow`. Runs until
//! Escape, exactly like `gui::run_gui`.

use alloc::format;

use ferric_api::{Key, KeyEvent, MAX_MEMORY_REGIONS, MemoryRegion, MonitorSample, MonitorSource};
use ferric_safe_core::monitor::{
    ADDRESS_TEXT_LEN, DisplayInfo, MonitorModel, RegionView, SIZE_TEXT_LEN, UPTIME_TEXT_LEN,
    format_address, format_size, format_uptime, kind_name,
};

use crate::slint_platform;

/// Serial proof line emitted once the first monitor frame with live values is
/// blitted (gates the monitor step of the smoke test).
pub const MONITOR_OK_MARKER: &str = "MONITOR OK\n";

/// Serial proof line emitted when the monitor exits and the console is about
/// to be repainted (separate from `GUI_EXIT_MARKER` so the smoke test cannot
/// advance on the GUI's exit line).
pub const MONITOR_EXIT_MARKER: &str = "MONITOR EXIT OK\n";

/// How many memory-map rows the monitor scene shows at once.
const VISIBLE_REGIONS: usize = 16;

/// Reads identical fields on both arches: uptime + tick counters straight from
/// `time` and the bootloader's memory map. One instance for the whole kernel.
pub struct ArchMonitor;

impl MonitorSource for ArchMonitor {
    fn sample(&self) -> MonitorSample {
        let mut regions =
            [MemoryRegion::new(0, 0, ferric_api::MemoryRegionKind::Unknown(0)); MAX_MEMORY_REGIONS];
        let memory_region_count = match crate::limine::memmap_entries() {
            Some(entries) => {
                let count = entries.len().min(MAX_MEMORY_REGIONS);
                for (dst, entry) in regions.iter_mut().zip(entries.iter().take(count)) {
                    *dst = entry.to_region();
                }
                count
            }
            None => 0,
        };
        MonitorSample {
            uptime_ns: crate::time::time_source().uptime_ns(),
            total_ticks: crate::time::ticks(),
            idle_ticks: crate::time::idle_ticks(),
            memory_regions: regions,
            memory_region_count,
        }
    }
}

/// Runs the full-screen hardware monitor, returning when Escape is pressed.
/// The caller must repaint the console over the framebuffer afterwards.
pub fn run_monitor() {
    let ui = ferric_ui::monitor_window();
    let window = slint_platform::window();

    let (w, h) = {
        let size = window.size();
        (size.width, size.height)
    };

    let source = ArchMonitor;
    let mut model = MonitorModel::new();
    // First sample binds immediately; subsequent ones only when the whole
    // second advances, so mid-second restarts never look frozen.
    let mut last_second = u64::MAX;

    let mut ok_emitted = false;
    loop {
        slint::platform::update_timers_and_animations();
        while let Some(event) = crate::input::next_key() {
            if event == KeyEvent::Press(Key::Escape) {
                slint_platform::write_serial(MONITOR_EXIT_MARKER);
                return;
            }
            if let Some(win_event) = slint_platform::map_key_event(event) {
                window.dispatch_event(win_event);
            }
        }

        let info = model.update(source.sample());
        if info.uptime_seconds != last_second {
            last_second = info.uptime_seconds;
            refresh(&ui, &info);
        }

        if crate::gui::render_and_blit(&window, w, h) && !ok_emitted {
            slint_platform::write_serial(MONITOR_OK_MARKER);
            ok_emitted = true;
        }
        core::hint::spin_loop();
    }
}

/// Pushes one [`DisplayInfo`] into the Slint window's properties. Called at
/// most once per whole second, so the heap churn is ~55 strings/sec at worst.
fn refresh(ui: &ferric_ui::MonitorWindow, info: &DisplayInfo) {
    let mut uptime = [0u8; UPTIME_TEXT_LEN];
    let mut size = [0u8; SIZE_TEXT_LEN];
    ui.set_uptime_text(shared(format_uptime(info.uptime_seconds, &mut uptime)));
    ui.set_cpu_text(alloc::format!("{}%", info.cpu_load_percent).into());
    ui.set_arch_text(arch_name().into());
    ui.set_region_count_text(alloc::format!("{} regions", info.region_count).into());
    ui.set_total_usable_text(shared(format_size(
        info.memory_summary.usable_bytes,
        &mut size,
    )));
    ui.set_total_reserved_text(shared(format_size(
        info.memory_summary.reserved_bytes,
        &mut size,
    )));
    ui.set_total_other_text(shared(format_size(
        info.memory_summary.other_bytes,
        &mut size,
    )));

    let kinds = slint::VecModel::<slint::SharedString>::default();
    let ranges = slint::VecModel::<slint::SharedString>::default();
    let sizes = slint::VecModel::<slint::SharedString>::default();
    for index in 0..VISIBLE_REGIONS {
        match info.regions.get(index) {
            Some(region) if index < info.region_count => {
                let (kind, range, size) = region_texts(region);
                kinds.push(kind);
                ranges.push(range);
                sizes.push(size);
            }
            _ => {
                kinds.push(slint::SharedString::default());
                ranges.push(slint::SharedString::default());
                sizes.push("-".into());
            }
        }
    }
    ui.set_region_kinds(slint::ModelRc::new(kinds));
    ui.set_region_ranges(slint::ModelRc::new(ranges));
    ui.set_region_sizes(slint::ModelRc::new(sizes));
}

/// Formats a single region row: kind name, `start..end` range, human size.
fn region_texts(
    region: &RegionView,
) -> (
    slint::SharedString,
    slint::SharedString,
    slint::SharedString,
) {
    let mut start_buf = [0u8; ADDRESS_TEXT_LEN];
    let mut end_buf = [0u8; ADDRESS_TEXT_LEN];
    let mut size_buf = [0u8; SIZE_TEXT_LEN];
    let range = format!(
        "{}..{}",
        format_address(region.start, &mut start_buf),
        format_address(region.end, &mut end_buf)
    );
    (
        slint::SharedString::from(kind_name(region.kind)),
        slint::SharedString::from(range),
        slint::SharedString::from(format_size(region.size, &mut size_buf)),
    )
}

fn shared(s: &str) -> slint::SharedString {
    slint::SharedString::from(s)
}

/// Short arch label for the header.
fn arch_name() -> &'static str {
    if cfg!(target_arch = "x86_64") {
        "x86_64"
    } else {
        "aarch64"
    }
}
