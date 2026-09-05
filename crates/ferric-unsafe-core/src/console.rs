//! Synchronous text console: renders a [`TextGrid`] through the global
//! framebuffer and mirrors every write to the arch serial sink. `kmain` is
//! the safe kernel entry reached from `boot()`; it lives in unsafe-core
//! because the source of truth for hardware (framebuffer + serial) is here,
//! while the pure text model stays in `ferric-safe-core`.

use crate::sync::Spinlock;
use core::fmt;
use ferric_api::Rgb;
use ferric_safe_core::{Cell, Font, TextGrid};

const FONT_DATA: &[u8] = include_bytes!("../../../fonts/zap-light16.psf");

const MAX_COLS: u32 = 256;
const MAX_ROWS: u32 = 128;
const CELL_COUNT: usize = (MAX_COLS * MAX_ROWS) as usize;

/// Crash-screen colors: white text on a dark red field.
const PANIC_FG: Rgb = Rgb::new(0xFF, 0xFF, 0xFF);
const PANIC_BG: Rgb = Rgb::new(0x80, 0x00, 0x00);

static CONSOLE: Spinlock<Console> = Spinlock::new(Console::new());

/// An adapter backing the `print!`/`println!` macros.
struct ConsoleWriter;

impl fmt::Write for ConsoleWriter {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        CONSOLE.lock().write_str(s);
        Ok(())
    }
}

macro_rules! print {
    ($($arg:tt)*) => {{
        use core::fmt::Write;
        let _ = write!(ConsoleWriter, $($arg)*);
    }};
}

macro_rules! println {
    () => { print!("\n"); };
    ($($arg:tt)*) => {{
        print!($($arg)*);
        print!("\n");
    }};
}

/// Cell buffer plus cursor state; the real grid is rebuilt on each write
/// because `TextGrid` borrows it (allocation-free design).
struct Console {
    cells: [Cell; CELL_COUNT],
    cols: u32,
    rows: u32,
    cursor_row: u32,
    cursor_col: u32,
    fg: Rgb,
    bg: Rgb,
}

impl Console {
    const fn new() -> Self {
        Self {
            cells: [Cell::blank(Rgb::new(0xC0, 0xC0, 0xC0), Rgb::new(0, 0, 0)); CELL_COUNT],
            cols: 0,
            rows: 0,
            cursor_row: 0,
            cursor_col: 0,
            fg: Rgb::new(0xC0, 0xC0, 0xC0),
            bg: Rgb::new(0, 0, 0),
        }
    }

    /// Fits the grid to the whole framebuffer in one-scaled glyphs.
    fn set_geometry(&mut self, fb_width: u32, fb_height: u32) {
        let font = Font::parse(FONT_DATA).expect("font parse failed");
        self.cols = fb_width / font.width();
        self.rows = fb_height / font.height();
    }

    fn write_str(&mut self, s: &str) {
        self.put_text(s);
        if s.ends_with('\n') {
            crate::framebuffer::with_framebuffer(|fb| self.render(fb));
        }
        mirror_to_serial(s);
    }

    fn put_text(&mut self, s: &str) {
        if self.cols == 0 || self.rows == 0 {
            return;
        }
        let mut grid = TextGrid::new(&mut self.cells[..], self.cols, self.rows)
            .expect("console geometry must fit the cell buffer");
        grid.set_cursor(self.cursor_row, self.cursor_col);
        let (fg, bg) = (self.fg, self.bg);
        for c in s.chars() {
            grid.put(c, fg, bg);
        }
        self.cursor_row = grid.cursor_row();
        self.cursor_col = grid.cursor_col();
    }

    /// Rebuilds the cell grid from scratch in crash colors: clears to `bg`,
    /// prints `lines` in `fg`, and records the cursor. No-op before geometry
    /// exists.
    fn panic_grid(&mut self, lines: &[&str], fg: Rgb, bg: Rgb) {
        if self.cols == 0 || self.rows == 0 {
            return;
        }
        self.fg = fg;
        self.bg = bg;
        self.cursor_row = 0;
        self.cursor_col = 0;
        let mut grid = TextGrid::new(&mut self.cells[..], self.cols, self.rows)
            .expect("console geometry must fit the cell buffer");
        grid.clear(fg, bg);
        for line in lines {
            for c in line.chars() {
                grid.put(c, fg, bg);
            }
            grid.put('\n', fg, bg);
        }
        self.cursor_row = grid.cursor_row();
        self.cursor_col = grid.cursor_col();
    }

    fn render(&self, fb: &mut crate::framebuffer::FrameBuffer) {
        let font = Font::parse(FONT_DATA).expect("font parse failed");
        let (cw, ch) = (font.width(), font.height());
        for row in 0..self.rows {
            for col in 0..self.cols {
                let cell = self.cells[(row * self.cols + col) as usize];
                let glyph = font.glyph_index_for(cell.glyph).unwrap_or(0);
                let fg_word = fb.encode(cell.fg);
                let bg_word = fb.encode(cell.bg);
                let (px, py) = (col * cw, row * ch);
                for gy in 0..ch {
                    for gx in 0..cw {
                        let set = font.glyph_pixel(glyph, gx, gy).unwrap_or(false);
                        let word = if set { fg_word } else { bg_word };
                        let _ = fb.write_word(px + gx, py + gy, word);
                    }
                }
            }
        }
    }
}

fn mirror_to_serial(s: &str) {
    #[cfg(target_arch = "x86_64")]
    crate::serial::with_serial(|serial| ferric_api::TextSink::write_str(serial, s));
    #[cfg(target_arch = "aarch64")]
    crate::pl011::with_serial(|serial| ferric_api::TextSink::write_str(serial, s));
}

/// Lock-free panic-path mirror to the arch serial sink.
fn emergency_write(s: &str) {
    #[cfg(target_arch = "x86_64")]
    crate::serial::write_emergency(s);
    #[cfg(target_arch = "aarch64")]
    crate::pl011::write_emergency(s);
}

/// Crash screen: mirrors `lines` to serial lock-free, then best-effort paints
/// a panic-red panel (skipped when either the console or framebuffer lock is
/// already held — e.g. a panic mid-`println!` — so the dump can never
/// deadlock), then parks the CPU. Called by the panic handler; never returns.
pub fn render_panic(lines: &[&str]) -> ! {
    for line in lines {
        emergency_write(line);
        emergency_write("\n");
    }
    if let Some(mut console) = CONSOLE.try_lock() {
        if (console.cols == 0 || console.rows == 0)
            && let Some((w, h)) =
                crate::framebuffer::with_framebuffer_try(|fb| (fb.width(), fb.height()))
        {
            console.set_geometry(w, h);
        }
        console.panic_grid(lines, PANIC_FG, PANIC_BG);
        drop(console);
        let _ = crate::framebuffer::with_framebuffer_try(|fb| {
            let _ = fb.fill_rect(0, 0, fb.width(), fb.height(), PANIC_BG);
            if let Some(c) = CONSOLE.try_lock() {
                c.render(fb);
            }
        });
    }
    crate::halt()
}

/// Safe kernel entry called from `boot()` after init and the colour-bar
/// self-test; never returns.
pub fn kmain() -> ! {
    let (fb_width, fb_height) =
        crate::framebuffer::with_framebuffer(|fb| (fb.width(), fb.height()))
            .expect("framebuffer not initialized");
    CONSOLE.lock().set_geometry(fb_width, fb_height);

    println!("Hello from Ferric-K!");
    soak_timer();
}

/// Timer soak: keeps the CPU parked on interrupts and re-renders the uptime
/// readout every quarter second until the 3-second mark, then proves the
/// gate and exits with the dedicated status.
const SOAK_DURATION_NS: u64 = 3 * crate::time::NANOS_PER_SEC;
const RENDER_PERIOD_NS: u64 = 250_000_000;

fn soak_timer() -> ! {
    let source = crate::time::time_source();
    let start = source.uptime_ns();
    let mut last_render = start;
    loop {
        let now = source.uptime_ns();
        if now.saturating_sub(last_render) >= RENDER_PERIOD_NS {
            last_render = now;
            let ds = (now - start) / crate::time::NANOS_PER_SEC;
            let dms = ((now - start) % crate::time::NANOS_PER_SEC) / 1_000_000;
            println!("UP {:1}.{:03}s", ds, dms);
        }
        if now.saturating_sub(start) >= SOAK_DURATION_NS {
            println!("UPTIME OK");
            #[cfg(target_arch = "x86_64")]
            crate::qemu::debug_exit(crate::qemu::STATUS_TIMER_SOAK);
            #[cfg(target_arch = "aarch64")]
            crate::qemu::semihosting_exit(crate::qemu::STATUS_TIMER_SOAK);
        }
    }
}
