//! Slint platform backend: bridges Slint's `Platform` trait to the kernel's
//! framebuffer, time source, and input. Renders via the software renderer
//! into a CPU buffer over the real framebuffer, driven by our own event loop
//! (`gui.rs`) rather than Slint's `run_event_loop`.

use core::time::Duration;

use alloc::{boxed::Box, rc::Rc};
use slint::platform::software_renderer::{
    MinimalSoftwareWindow, PremultipliedRgbaColor, RepaintBufferType, TargetPixel,
};
use slint::platform::{self, Key as SlintKey, Platform, PlatformError, WindowEvent};

use crate::sync::{OnceLock, Spinlock};
use ferric_api::{Key, KeyEvent};

/// Cached framebuffer layout consumed by [`FbPixel`] when encoding pixels,
/// captured once at platform init so rendering needs no per-pixel lock.
struct FbLayout {
    width: u32,
    height: u32,
    bytes_per_pixel: usize,
    red_size: u8,
    red_shift: u8,
    green_size: u8,
    green_shift: u8,
    blue_size: u8,
    blue_shift: u8,
}

static FB_LAYOUT: OnceLock<FbLayout> = OnceLock::new();

/// Owning cell for the current Slint window, replaced when the active
/// component is constructed so each full-screen app renders its own window.
/// The window is `!Send + !Sync` under Slint's `unsafe-single-threaded` mode;
/// this wrapper declares that safe because the window is touched only from the
/// GUI event-loop thread, never from interrupt context.
pub struct WindowCell(Spinlock<Option<Rc<MinimalSoftwareWindow>>>);
impl WindowCell {
    const fn new() -> Self {
        Self(Spinlock::new(None))
    }

    /// Adopts `window` as the active one; a previously stored window is
    /// dropped once its owning component is.
    fn set(&self, window: Rc<MinimalSoftwareWindow>) {
        *self.0.lock() = Some(window);
    }

    /// Clones the reference to the active window, if any.
    fn get(&self) -> Option<Rc<MinimalSoftwareWindow>> {
        self.0.lock().clone()
    }
}
// SAFETY: single-threaded GUI; see the struct doc — the window is only ever
// accessed from the one GUI thread, so sharing it across threads is sound.
unsafe impl Sync for WindowCell {}

/// The active Slint window adapter, replaced on each component construction.
static WINDOW: WindowCell = WindowCell::new();

/// The platform object handed to Slint.
struct FerricPlatform;

impl Platform for FerricPlatform {
    fn create_window_adapter(&self) -> Result<Rc<dyn platform::WindowAdapter>, PlatformError> {
        let window = MinimalSoftwareWindow::new(RepaintBufferType::ReusedBuffer);
        // Adopt the latest window so a fresh component renders on a fresh
        // adapter; the previous window drops with its owning component.
        WINDOW.set(window.clone());
        window.set_size(physical_size());
        window.show()?;
        Ok(window)
    }
    fn duration_since_start(&self) -> Duration {
        Duration::from_nanos(crate::time::time_source().uptime_ns())
    }

    fn debug_log(&self, arguments: core::fmt::Arguments<'_>) {
        let mut buf = [0u8; 512];
        let len = {
            let mut w = SerialFmt {
                buf: &mut buf,
                len: 0,
            };
            let _ = core::fmt::write(&mut w, arguments);
            w.len
        };
        mirror_to_serial(&buf[..len]);
    }

    fn run_event_loop(&self) -> Result<(), PlatformError> {
        Err(PlatformError::Unsupported)
    }
}

/// A `fmt::Write` adapter that appends into a fixed byte buffer.
struct SerialFmt<'a> {
    buf: &'a mut [u8],
    len: usize,
}
impl core::fmt::Write for SerialFmt<'_> {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        let end = core::cmp::min(self.buf.len(), self.len + s.len());
        self.buf[self.len..end].copy_from_slice(&s.as_bytes()[..end - self.len]);
        self.len = end;
        Ok(())
    }
}

/// Formats and writes `s` to the arch serial sink (used by Slint `debug_log`).
fn mirror_to_serial(s: &[u8]) {
    let text = core::str::from_utf8(s).unwrap_or("");
    write_serial(text);
}

/// Writes `s` verbatim to the arch serial sink (x86_64 COM1 / aarch64 UART0).
pub(crate) fn write_serial(s: &str) {
    #[cfg(target_arch = "x86_64")]
    crate::serial::with_serial(|serial| ferric_api::TextSink::write_str(serial, s));
    #[cfg(target_arch = "aarch64")]
    crate::pl011::with_serial(|uart| ferric_api::TextSink::write_str(uart, s));
    #[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64")))]
    let _ = s;
}

/// Registers `FerricPlatform` with Slint, capturing the framebuffer layout.
/// Must be called before any component is constructed.
pub fn init_platform() {
    let layout = crate::framebuffer::with_framebuffer(|fb| FbLayout {
        width: fb.width(),
        height: fb.height(),
        bytes_per_pixel: fb.bytes_per_pixel(),
        red_size: fb.channel_size(0),
        red_shift: fb.channel_shift(0),
        green_size: fb.channel_size(1),
        green_shift: fb.channel_shift(1),
        blue_size: fb.channel_size(2),
        blue_shift: fb.channel_shift(2),
    })
    .expect("GUI needs the framebuffer");
    // Only the first capture wins; a repeated call is a no-op.
    let _ = FB_LAYOUT.set(layout);
    platform::set_platform(Box::new(FerricPlatform))
        .expect("Slint platform already set or installation failed");
}

/// The active software-renderer window, already shown. Panics if accessed before
/// any component constructed the window (i.e. before `init_platform`).
pub fn window() -> Rc<MinimalSoftwareWindow> {
    WINDOW
        .get()
        .expect("Slint window created before platform init")
}
fn physical_size() -> slint::PhysicalSize {
    let layout = FB_LAYOUT
        .get()
        .expect("Slint platform initialized before capturing the framebuffer");
    slint::PhysicalSize::new(layout.width, layout.height)
}

/// Software-renderer target pixel: accumulates a premultiplied RGBA value in a
/// uniform space across frames (the renderer reuses our buffer), then encodes
/// to the framebuffer's native layout during the final blit.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FbPixel {
    red: u8,
    green: u8,
    blue: u8,
    alpha: u8,
}

impl FbPixel {
    const fn from_rgba(red: u8, green: u8, blue: u8, alpha: u8) -> Self {
        Self {
            red,
            green,
            blue,
            alpha,
        }
    }

    /// Encodes this pixel into the framebuffer's native pixel word, scaling
    /// each channel to its mask width and shifting it into place. The target
    /// is opaque because the framebuffer carries no alpha.
    fn encode(self, layout: &FbLayout) -> u64 {
        let max_r = (1u64 << layout.red_size) - 1;
        let max_g = (1u64 << layout.green_size) - 1;
        let max_b = (1u64 << layout.blue_size) - 1;
        let r = (u64::from(self.red) * max_r / 255) << layout.red_shift;
        let g = (u64::from(self.green) * max_g / 255) << layout.green_shift;
        let b = (u64::from(self.blue) * max_b / 255) << layout.blue_shift;
        debug_assert!(
            r | g | b < 1u64 << (layout.bytes_per_pixel * 8),
            "encoded pixel does not fit the framebuffer word"
        );
        r | g | b
    }
}

impl TargetPixel for FbPixel {
    fn blend(&mut self, color: PremultipliedRgbaColor) {
        let a = (u8::MAX - color.alpha) as u16;
        self.red = (self.red as u16 * a / 255) as u8 + color.red;
        self.green = (self.green as u16 * a / 255) as u8 + color.green;
        self.blue = (self.blue as u16 * a / 255) as u8 + color.blue;
        self.alpha = (self.alpha as u16 + color.alpha as u16
            - (self.alpha as u16 * color.alpha as u16) / 255) as u8;
    }

    fn from_rgb(red: u8, green: u8, blue: u8) -> Self {
        Self::from_rgba(red, green, blue, u8::MAX)
    }
}

/// Copies a rendered buffer (row-major, window packed-width `width`) into the
/// framebuffer at the top-left, encoding each [`FbPixel`] to its native word.
pub fn blit_to_framebuffer(buffer: &[FbPixel], width: u32, height: u32) {
    let layout = FB_LAYOUT
        .get()
        .expect("framebuffer layout captured before blit");
    let w = width.min(layout.width);
    let h = height.min(layout.height);
    crate::framebuffer::with_framebuffer(|fb| {
        for y in 0..h {
            let row = &buffer[(y * width) as usize..(y * width + w) as usize];
            for (x, &pixel) in row.iter().enumerate() {
                let _ = fb.write_word(x as u32, y, pixel.encode(layout));
            }
        }
    });
}

/// Maps a kernel key event to a Slint window event; `None` when the key has no
/// Slint representation (e.g. an unmapped scancode).
pub fn map_key_event(event: KeyEvent) -> Option<WindowEvent> {
    let text = match event {
        KeyEvent::Press(key) | KeyEvent::Release(key) => key_to_text(key)?,
    };
    Some(match event {
        KeyEvent::Press(_) => WindowEvent::KeyPressed { text },
        KeyEvent::Release(_) => WindowEvent::KeyReleased { text },
    })
}

/// Converts a kernel key to its Slint `SharedString` representation. Plain
/// characters pass through as their glyph; control keys use Slint's `Key`.
fn key_to_text(key: Key) -> Option<slint::SharedString> {
    Some(match key {
        Key::Char(c) if c.is_ascii_graphic() || c == ' ' => c.into(),
        Key::Enter => SlintKey::Return.into(),
        Key::Tab => SlintKey::Tab.into(),
        Key::Backspace => SlintKey::Backspace.into(),
        Key::Escape => SlintKey::Escape.into(),
        Key::Up => SlintKey::UpArrow.into(),
        Key::Down => SlintKey::DownArrow.into(),
        Key::Left => SlintKey::LeftArrow.into(),
        Key::Right => SlintKey::RightArrow.into(),
        Key::Home => SlintKey::Home.into(),
        Key::End => SlintKey::End.into(),
        Key::PageUp => SlintKey::PageUp.into(),
        Key::PageDown => SlintKey::PageDown.into(),
        Key::Insert => SlintKey::Insert.into(),
        Key::Delete => SlintKey::Delete.into(),
        Key::LeftShift | Key::RightShift => SlintKey::Shift.into(),
        Key::LeftControl | Key::RightControl => SlintKey::Control.into(),
        Key::LeftAlt | Key::RightAlt => SlintKey::Alt.into(),
        Key::CapsLock => SlintKey::CapsLock.into(),
        Key::F(n) if (1..=24).contains(&n) => match n {
            1 => SlintKey::F1.into(),
            2 => SlintKey::F2.into(),
            3 => SlintKey::F3.into(),
            4 => SlintKey::F4.into(),
            5 => SlintKey::F5.into(),
            6 => SlintKey::F6.into(),
            7 => SlintKey::F7.into(),
            8 => SlintKey::F8.into(),
            9 => SlintKey::F9.into(),
            10 => SlintKey::F10.into(),
            11 => SlintKey::F11.into(),
            12 => SlintKey::F12.into(),
            13 => SlintKey::F13.into(),
            14 => SlintKey::F14.into(),
            15 => SlintKey::F15.into(),
            16 => SlintKey::F16.into(),
            17 => SlintKey::F17.into(),
            18 => SlintKey::F18.into(),
            19 => SlintKey::F19.into(),
            20 => SlintKey::F20.into(),
            21 => SlintKey::F21.into(),
            22 => SlintKey::F22.into(),
            23 => SlintKey::F23.into(),
            _ => SlintKey::F24.into(),
        },
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn layout_bgrx() -> FbLayout {
        FbLayout {
            width: 2,
            height: 2,
            bytes_per_pixel: 4,
            red_size: 8,
            red_shift: 16,
            green_size: 8,
            green_shift: 8,
            blue_size: 8,
            blue_shift: 0,
        }
    }

    fn layout_rgb565() -> FbLayout {
        FbLayout {
            width: 2,
            height: 2,
            bytes_per_pixel: 2,
            red_size: 5,
            red_shift: 11,
            green_size: 6,
            green_shift: 5,
            blue_size: 5,
            blue_shift: 0,
        }
    }

    #[test]
    fn fb_pixel_from_rgb_encodes_to_bgrx() {
        let l = layout_bgrx();
        let p = FbPixel::from_rgb(0xFF, 0x12, 0x34);
        assert_eq!(p.encode(&l), 0x00FF_1234);
    }

    #[test]
    fn fb_pixel_from_rgb_encodes_to_rgb565() {
        let l = layout_rgb565();
        let p = FbPixel::from_rgb(0xFF, 0x00, 0x00);
        assert_eq!(p.encode(&l), 0xF800);
        let gray = FbPixel::from_rgb(0x80, 0x80, 0x80);
        assert_eq!(gray.encode(&l), 0x7BEF);
    }

    #[test]
    fn fb_pixel_blend_composites_over_existing() {
        let mut p = FbPixel {
            red: 0,
            green: 0,
            blue: 0,
            alpha: 0,
        };
        p.blend(PremultipliedRgbaColor {
            red: 0x7F,
            green: 0x7F,
            blue: 0x7F,
            alpha: 0x80,
        });
        assert_eq!(p.red, 0x7F);
        assert_eq!(p.green, 0x7F);
        assert_eq!(p.blue, 0x7F);
        assert_eq!(p.alpha, 0x80);
    }

    #[test]
    fn opaque_blend_overwrites_below() {
        let mut p = FbPixel::from_rgb(0xFF, 0, 0);
        p.blend(PremultipliedRgbaColor {
            red: 0,
            green: 0,
            blue: 0xFF,
            alpha: 0xFF,
        });
        assert_eq!(p, FbPixel::from_rgb(0, 0, 0xFF));
    }

    #[test]
    fn key_events_map_to_slint_window_events() {
        assert!(matches!(
            map_key_event(KeyEvent::Press(Key::Escape)),
            Some(WindowEvent::KeyPressed { .. })
        ));
        assert!(matches!(
            map_key_event(KeyEvent::Release(Key::Escape)),
            Some(WindowEvent::KeyReleased { .. })
        ));
        assert!(map_key_event(KeyEvent::Press(Key::Char('A'))).is_some());
        assert!(map_key_event(KeyEvent::Press(Key::Unknown(0x9F))).is_none());
    }

    #[test]
    fn text_rep_of_char_and_control_keys() {
        assert_eq!(key_to_text(Key::Char('x')).unwrap(), "x");
        assert_eq!(
            key_to_text(Key::Enter).unwrap(),
            slint::SharedString::from(SlintKey::Return)
        );
        assert!(key_to_text(Key::Unknown(0)).is_none());
    }
}
