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
    /// Start of the line the shell is currently editing, plus how many cells
    /// it has drawn since; powering repaints of in-progress lines.
    edit_row: u32,
    edit_col: u32,
    edited_cells: u32,
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
            edit_row: 0,
            edit_col: 0,
            edited_cells: 0,
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

    /// Pushes the current grid to the framebuffer immediately (used after
    /// writes that do not end in a newline, like the shell prompt).
    fn render_now(&self) {
        if self.cols == 0 || self.rows == 0 {
            return;
        }
        crate::framebuffer::with_framebuffer(|fb| self.render(fb));
    }

    /// Records the current cursor as the start of an in-progress line, so
    /// later [`Self::set_edit_line`] repaints can blank only what the editor
    /// owns.
    fn begin_edit(&mut self) {
        self.edit_row = self.cursor_row;
        self.edit_col = self.cursor_col;
        self.edited_cells = 0;
    }

    /// Repaints the shell's in-progress line, blanking the cells it drew
    /// before. Chars beyond the grid are drawn off-screen; the editor buffer
    /// still carries them for the command parser. Only the edited range is
    /// re-blitted unless a scroll shifted the whole frame.
    fn set_edit_line(&mut self, chars: &[char]) {
        if self.cols == 0 || self.rows == 0 {
            return;
        }
        let origin = (self.edit_row * self.cols + self.edit_col) as usize;
        let old_cells = self.edited_cells as usize;
        if chars.is_empty() && old_cells == 0 {
            return;
        }
        let total = (self.rows * self.cols) as usize;
        let (end, scrolled, cursor_row, cursor_col) = {
            let mut grid = TextGrid::new(&mut self.cells[..], self.cols, self.rows)
                .expect("console geometry must fit the cell buffer");
            if old_cells > 0 {
                let blank = Cell::blank(self.fg, self.bg);
                for off in 0..old_cells {
                    let index = origin + off;
                    if index >= total {
                        break;
                    }
                    grid.set_cell(
                        (index / self.cols as usize) as u32,
                        (index % self.cols as usize) as u32,
                        blank,
                    );
                }
            }
            grid.set_cursor(self.edit_row, self.edit_col);
            let (fg, bg) = (self.fg, self.bg);
            for &c in chars {
                grid.put(c, fg, bg);
            }
            (
                (grid.cursor_row() * self.cols + grid.cursor_col()) as usize,
                grid.scroll_count(),
                grid.cursor_row(),
                grid.cursor_col(),
            )
        };
        self.edited_cells = end.saturating_sub(origin) as u32;
        self.cursor_row = cursor_row;
        self.cursor_col = cursor_col;
        crate::framebuffer::with_framebuffer(|fb| {
            if scrolled > 0 {
                self.edit_row = self.edit_row.saturating_sub(scrolled);
                self.render(fb);
            } else {
                let old_end = origin + old_cells;
                let new_end = origin + self.edited_cells as usize;
                self.render_range(fb, origin, old_end.max(new_end).saturating_sub(1));
            }
        });
    }

    /// Closes the current edit and returns to ordinary write mode.
    fn end_edit(&mut self) {
        self.edited_cells = 0;
    }

    /// Blanks the whole grid, homes the cursor, and renders.
    fn clear_screen(&mut self) {
        if self.cols == 0 || self.rows == 0 {
            return;
        }
        let mut grid = TextGrid::new(&mut self.cells[..], self.cols, self.rows)
            .expect("console geometry must fit the cell buffer");
        grid.clear(self.fg, self.bg);
        self.cursor_row = 0;
        self.cursor_col = 0;
        self.edited_cells = 0;
        crate::framebuffer::with_framebuffer(|fb| self.render(fb));
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
        if self.cols == 0 || self.rows == 0 {
            return;
        }
        let font = Font::parse(FONT_DATA).expect("font parse failed");
        for row in 0..self.rows {
            for col in 0..self.cols {
                self.blit_cell(fb, &font, row, col);
            }
        }
    }

    /// Re-blits just the cells in the row-major index range `[from, to]`
    /// (inclusive, clamped to the grid), so the shell repaints only its edited
    /// line on each keystroke instead of the whole framebuffer.
    fn render_range(
        &self,
        fb: &mut crate::framebuffer::FrameBuffer,
        mut from: usize,
        mut to: usize,
    ) {
        if self.cols == 0 || self.rows == 0 {
            return;
        }
        let total = (self.rows * self.cols) as usize;
        if from >= total || to >= total {
            from = from.min(total - 1);
            to = to.min(total - 1);
        }
        if from > to {
            return;
        }
        let font = Font::parse(FONT_DATA).expect("font parse failed");
        let cols = self.cols as usize;
        for index in from..=to {
            self.blit_cell(fb, &font, (index / cols) as u32, (index % cols) as u32);
        }
    }

    fn blit_cell(&self, fb: &mut crate::framebuffer::FrameBuffer, font: &Font, row: u32, col: u32) {
        let (cw, ch) = (font.width(), font.height());
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

/// Restores the console behind a moved window: fills `(x, y, w, h)` with the
/// console background (so the strip below the last grid row is clean too) and
/// re-blits every cell intersecting it. Used by the clock app.
pub(crate) fn repaint_rect(x: u32, y: u32, w: u32, h: u32) {
    let console = CONSOLE.lock();
    if console.cols == 0 || console.rows == 0 || w == 0 || h == 0 {
        return;
    }
    let bg = console.bg;
    let font = Font::parse(FONT_DATA).expect("font parse failed");
    let (cw, ch) = (font.width(), font.height());
    let row0 = (u64::from(y) / u64::from(ch)).min(u64::from(console.rows - 1));
    let col0 = (u64::from(x) / u64::from(cw)).min(u64::from(console.cols - 1));
    let row1 = (u64::from(y).saturating_add(u64::from(h)).saturating_sub(1) / u64::from(ch))
        .min(u64::from(console.rows - 1));
    let col1 = (u64::from(x).saturating_add(u64::from(w)).saturating_sub(1) / u64::from(cw))
        .min(u64::from(console.cols - 1));
    crate::framebuffer::with_framebuffer(|fb| {
        let _ = fb.fill_rect(x, y, w, h, bg);
        for row in row0..=row1 {
            for col in col0..=col1 {
                console.blit_cell(fb, &font, row as u32, col as u32);
            }
        }
    });
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

    crate::slint_platform::init_platform();

    println!("Hello from Ferric-K!");

    #[cfg(feature = "gui-on-boot")]
    {
        crate::gui::run_gui();
        CONSOLE.lock().render_now();
    }
    shell_loop();
}

const PROMPT: &str = "ferric-k$ ";

/// Prints the prompt, renders it, and marks the cursor as the start of the
/// line the editor will own.
fn new_prompt(console: &mut Console) {
    console.write_str(PROMPT);
    console.render_now();
    console.begin_edit();
}

/// Drains every input device feeding keystrokes into the line editor and
/// dispatching finished lines; never returns.
fn shell_loop() -> ! {
    let mut editor = ferric_safe_core::LineEditor::new();
    new_prompt(&mut CONSOLE.lock());
    loop {
        if let Some(event) = crate::input::next_key() {
            match editor.handle(event) {
                ferric_safe_core::LineAction::Inserted => {
                    if let Some(&c) = editor.line().last() {
                        let mut buf = [0u8; 4];
                        mirror_to_serial(c.encode_utf8(&mut buf));
                    }
                    CONSOLE.lock().set_edit_line(editor.line());
                }
                ferric_safe_core::LineAction::Erased => {
                    mirror_to_serial("\x08 \x08");
                    CONSOLE.lock().set_edit_line(editor.line());
                }
                ferric_safe_core::LineAction::Submitted => {
                    CONSOLE.lock().end_edit();
                    println!();
                    run_command(editor.line());
                    editor.clear();
                    new_prompt(&mut CONSOLE.lock());
                }
                ferric_safe_core::LineAction::Ignored => {}
            }
        } else {
            core::hint::spin_loop();
        }
    }
}

fn run_command(line: &[char]) {
    use ferric_safe_core::Command;
    match ferric_safe_core::parse_command(line) {
        Command::Empty => {}
        Command::Help => {
            println!("Ferric-K commands:");
            println!("  help     show this list");
            println!("  clear    clear the screen");
            println!("  echo     print the arguments");
            println!("  uptime   show time since boot");
            println!("  arch     show the CPU architecture");
            println!("  panic    panic the kernel (test hook)");
            println!("  clock    open the graphical clock window");
            println!("  gui      launch the Slint GUI");
            println!("  monitor  live hardware monitor");
            println!("  halt     power off the machine");
        }
        Command::Clear => CONSOLE.lock().clear_screen(),
        Command::Echo(rest) => {
            put_chars(rest);
            println!();
        }
        Command::Uptime => {
            let ns = crate::time::time_source().uptime_ns();
            let ds = ns / crate::time::NANOS_PER_SEC;
            let dms = (ns % crate::time::NANOS_PER_SEC) / 1_000_000;
            println!("Uptime: {ds}.{dms:03}s");
        }
        Command::Arch => {
            #[cfg(target_arch = "x86_64")]
            println!("x86_64");
            #[cfg(target_arch = "aarch64")]
            println!("aarch64");
        }
        Command::Panic => panic!("panic at user request (shell command)"),
        Command::Clock => {
            crate::app::run_clock();
            // Repaint the console over whatever the window left behind; the
            // prompt is redrawn by the shell loop's `new_prompt`.
            CONSOLE.lock().render_now();
        }
        Command::Gui => {
            crate::gui::run_gui();
            CONSOLE.lock().render_now();
        }
        Command::Monitor => {
            crate::monitor::run_monitor();
            CONSOLE.lock().render_now();
        }
        Command::Halt => {
            println!("HALT");
            exit_qemu(crate::qemu::STATUS_SHELL_HALT);
        }
        Command::Unknown(name) => {
            print!("unknown command '");
            put_chars(name);
            println!("'");
        }
    }
}

/// Writes `chars` to the console without a trailing newline.
fn put_chars(chars: &[char]) {
    let mut buf = [0u8; ferric_safe_core::MAX_LINE_CHARS * 4];
    let mut end = 0;
    for &c in chars {
        let mut enc = [0u8; 4];
        let s = c.encode_utf8(&mut enc);
        if end + s.len() > buf.len() {
            break;
        }
        buf[end..end + s.len()].copy_from_slice(s.as_bytes());
        end += s.len();
    }
    let s = core::str::from_utf8(&buf[..end]).expect("chars encode as valid UTF-8");
    if end > 0 {
        print!("{s}");
    }
}

/// Terminates the emulator with `status` through the arch's exit channel.
fn exit_qemu(status: u8) -> ! {
    #[cfg(target_arch = "x86_64")]
    crate::qemu::debug_exit(status);
    #[cfg(target_arch = "aarch64")]
    crate::qemu::semihosting_exit(status);
}
