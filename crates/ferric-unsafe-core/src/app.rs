//! Graphical apps launched from the shell, drawn over the real framebuffer
//! through an embedded-graphics `DrawTarget` adapter.

use embedded_graphics::{
    geometry::{OriginDimensions, Size},
    pixelcolor::{Rgb888, RgbColor},
    prelude::{DrawTarget, Pixel},
};
use ferric_api::{Key, KeyEvent};
use ferric_safe_core::clock_app::{ClockWindow, HEIGHT, WIDTH};

const MOVE_STEP: i32 = 16;
/// Hold-to-move repeats come from this timer (PS/2 typematic is slow and
/// unreliable under QEMU), so the window glides while a direction is held.
const REPEAT_PERIOD_NS: u64 = 70_000_000;

#[derive(Clone, Copy)]
enum Dir {
    Up,
    Down,
    Left,
    Right,
}

impl Dir {
    const fn step(self) -> (i32, i32) {
        match self {
            Dir::Up => (0, -MOVE_STEP),
            Dir::Down => (0, MOVE_STEP),
            Dir::Left => (-MOVE_STEP, 0),
            Dir::Right => (MOVE_STEP, 0),
        }
    }
}

fn dir_of(key: Key) -> Option<Dir> {
    match key {
        Key::Up => Some(Dir::Up),
        Key::Down => Some(Dir::Down),
        Key::Left => Some(Dir::Left),
        Key::Right => Some(Dir::Right),
        _ => None,
    }
}

impl OriginDimensions for crate::framebuffer::FrameBuffer {
    fn size(&self) -> Size {
        Size::new(self.width(), self.height())
    }
}

impl DrawTarget for crate::framebuffer::FrameBuffer {
    type Color = Rgb888;
    type Error = crate::framebuffer::FramebufferError;

    fn draw_iter<I>(&mut self, pixels: I) -> Result<(), Self::Error>
    where
        I: IntoIterator<Item = Pixel<Self::Color>>,
    {
        for Pixel(point, color) in pixels {
            // DrawTarget may drop pixels outside the surface; `Point` is
            // signed, so negatives must never reach the u32 `write_pixel`.
            if point.x < 0 || point.y < 0 {
                continue;
            }
            self.write_pixel(
                point.x as u32,
                point.y as u32,
                ferric_api::Rgb::new(color.r(), color.g(), color.b()),
            )?;
        }
        Ok(())
    }
}

/// Runs the clock window until Escape: the console shell is suspended for the
/// window's lifetime and repaints over it afterwards.
pub fn run_clock() {
    let (screen_w, screen_h) = crate::framebuffer::with_framebuffer(|fb| (fb.width(), fb.height()))
        .expect("clock app needs the framebuffer");
    let mut window = ClockWindow::new();
    window.place_centered(screen_w, screen_h);
    redraw(&window);

    let mut held: Option<Dir> = None;
    let mut last_move_ns = 0;
    let mut shown_second = crate::clock::time_of_day().seconds;
    loop {
        while let Some(event) = crate::input::next_key() {
            match event {
                KeyEvent::Press(Key::Escape) => return,
                KeyEvent::Press(key) => {
                    if let Some(dir) = dir_of(key) {
                        held = Some(dir);
                        last_move_ns = 0;
                    }
                }
                KeyEvent::Release(key) => {
                    if dir_of(key).is_some() {
                        held = None;
                    }
                }
            }
        }
        let now = crate::time::time_source().uptime_ns();
        if let Some(dir) = held
            && now.wrapping_sub(last_move_ns) >= REPEAT_PERIOD_NS
        {
            move_by(&mut window, dir, screen_w, screen_h);
            last_move_ns = now;
        }
        let time = crate::clock::time_of_day();
        if time.seconds != shown_second {
            shown_second = time.seconds;
            redraw(&window);
        }
        crate::wait_for_interrupt();
    }
}

/// Restores the console under the window's current spot before stepping it,
/// so the window never leaves an afterimage behind.
fn move_by(window: &mut ClockWindow, dir: Dir, screen_w: u32, screen_h: u32) {
    let (dx, dy) = dir.step();
    let old = window.origin();
    window.translate(dx, dy, screen_w, screen_h);
    if window.origin() != old {
        crate::console::repaint_rect(old.0 as u32, old.1 as u32, WIDTH, HEIGHT);
        redraw(window);
    }
}

fn redraw(window: &ClockWindow) {
    let time = crate::clock::time_of_day();
    crate::framebuffer::with_framebuffer(|fb| {
        let _ = window.draw(fb, time);
    });
}
