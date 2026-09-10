//! The GUI super-loop: creates the top-level Slint component, then drives the
//! platform's own event loop — poll input, update timers/animations, redraw
//! the software renderer into the framebuffer, wait for the next interrupt.
//! Returns to the caller when Escape is pressed; the caller repaints the
//! console over the framebuffer.

use crate::slint_platform::{self, FbPixel};
use crate::sync::Spinlock;
use ferric_api::{Key, KeyEvent};

/// Serial proof line emitted once the first GUI frame has been rendered and
/// blitted to the framebuffer (the boot-path mirror of `FRAMEBUFFER_OK_MARKER`).
pub const SLINT_OK_MARKER: &str = "SLINT OK\n";

/// Serial proof line emitted when the GUI exits and the console is about to
/// be repainted (gates the smoke test's re-entry into the shell).
pub const GUI_EXIT_MARKER: &str = "GUI EXIT OK\n";

/// Reusable render buffer, grown to the framebuffer size on first frame.
static RENDER_BUFFER: Spinlock<alloc::vec::Vec<FbPixel>> = Spinlock::new(alloc::vec::Vec::new());

/// Runs the full-screen Slint GUI, returning when Escape is pressed. The
/// caller must repaint the console over the framebuffer afterwards.
pub fn run_gui() {
    let _ui = ferric_ui::main_window();
    let window = slint_platform::window();

    let (w, h) = {
        let size = window.size();
        (size.width, size.height)
    };

    let mut ok_emitted = false;
    loop {
        slint::platform::update_timers_and_animations();
        while let Some(event) = crate::input::next_key() {
            if event == KeyEvent::Press(Key::Escape) {
                slint_platform::write_serial(GUI_EXIT_MARKER);
                return;
            }
            if let Some(win_event) = slint_platform::map_key_event(event) {
                window.dispatch_event(win_event);
            }
        }
        let rendered = window.draw_if_needed(|renderer| {
            let w = w as usize;
            let h = h as usize;
            let mut buffer = RENDER_BUFFER.lock();
            if buffer.len() < w * h {
                buffer.resize(w * h, FbPixel::default());
            }
            let region = renderer.render(&mut buffer[..w * h], w);
            let _ = region;
            slint_platform::blit_to_framebuffer(&buffer[..w * h], w as u32, h as u32);
        });
        if rendered && !ok_emitted {
            slint_platform::write_serial(SLINT_OK_MARKER);
            ok_emitted = true;
        }
        crate::wait_for_interrupt();
    }
}
