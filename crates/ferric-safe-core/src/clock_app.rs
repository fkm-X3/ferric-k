//! The graphical clock window: a movable, closable window rendered with
//! embedded-graphics. Pure logic over a generic [`DrawTarget`], so it stays
//! host-testable; the framebuffer adapter lives in `ferric-unsafe-core`.

use embedded_graphics::{
    geometry::{Point, Size},
    mono_font::{MonoTextStyle, ascii::FONT_6X10},
    pixelcolor::Rgb888,
    prelude::*,
    primitives::{Circle, Line, PrimitiveStyle, PrimitiveStyleBuilder, Rectangle},
    text::Text,
};
use ferric_api::TimeOfDay;
// Host test builds link std, whose inherent f32::sin/cos then shadow this
// trait; only the no_std kernel target exercises it.
#[allow(unused_imports)]
use micromath::F32Ext;

pub const WIDTH: u32 = 220;
pub const HEIGHT: u32 = 180;
const TITLE_BAR_HEIGHT: u32 = 20;

const FACE_RADIUS: u32 = 42;
const HOUR_HAND: u32 = 22;
const MINUTE_HAND: u32 = 30;
const SECOND_HAND: u32 = 36;

const TITLE_BG: Rgb888 = Rgb888::new(0x28, 0x2A, 0x36);
const TITLE_TEXT: Rgb888 = Rgb888::new(0xF8, 0xF8, 0xF2);
const CLOSE_BTN_BG: Rgb888 = Rgb888::new(0xFF, 0x55, 0x55);
const CLOSE_BTN_TEXT: Rgb888 = Rgb888::new(0x28, 0x2A, 0x36);

const WINDOW_BG: Rgb888 = Rgb888::new(0x1E, 0x20, 0x29);
const WINDOW_BORDER: Rgb888 = Rgb888::new(0x44, 0x47, 0x5A);
const FACE_BG: Rgb888 = Rgb888::new(0x15, 0x16, 0x1E);
const FACE_RING: Rgb888 = Rgb888::new(0x62, 0x72, 0xA4);
const TICK: Rgb888 = Rgb888::new(0x8B, 0xEB, 0xFE);
const HAND: Rgb888 = Rgb888::new(0xF8, 0xF8, 0xF2);
const SECOND: Rgb888 = Rgb888::new(0xFF, 0x79, 0xC6);
const DIGIT: Rgb888 = Rgb888::new(0x50, 0xFA, 0x7B);

/// A rectangular window holding the analog + digital clock; drawn into any
/// `DrawTarget`, moved by the shell input loop.
pub struct ClockWindow {
    x: i32,
    y: i32,
}

impl ClockWindow {
    /// A window parked at the origin; call [`Self::place_centered`] before
    /// drawing.
    pub const fn new() -> Self {
        Self { x: 0, y: 0 }
    }

    /// Centers the window on a `screen_width` × `screen_height` surface.
    pub fn place_centered(&mut self, screen_width: u32, screen_height: u32) {
        self.x = (screen_width.saturating_sub(WIDTH) / 2) as i32;
        self.y = (screen_height.saturating_sub(HEIGHT) / 2) as i32;
    }

    /// Top-left corner of the window, for callers repainting what it covered.
    pub const fn origin(&self) -> (i32, i32) {
        (self.x, self.y)
    }

    /// Moves by `(dx, dy)`, keeping the window fully on a
    /// `screen_width` × `screen_height` surface.
    pub fn translate(&mut self, dx: i32, dy: i32, screen_width: u32, screen_height: u32) {
        let max_x = screen_width.saturating_sub(WIDTH) as i32;
        let max_y = screen_height.saturating_sub(HEIGHT) as i32;
        self.x = (self.x + dx).max(0).min(max_x);
        self.y = (self.y + dy).max(0).min(max_y);
    }

    /// Paints the window: title bar, rounded border, analog face, hands,
    /// and the digital readout for `time`.
    pub fn draw<D>(&self, target: &mut D, time: TimeOfDay) -> Result<(), D::Error>
    where
        D: DrawTarget<Color = Rgb888>,
    {
        let origin = Point::new(self.x, self.y);
        // Center the clock face within the window's main body, excluding the title bar
        let center = origin
            + Point::new(
                (WIDTH as i32) / 2,
                (TITLE_BAR_HEIGHT as i32) + ((HEIGHT - TITLE_BAR_HEIGHT) as i32) / 2,
            );

        // Main window background
        Rectangle::new(origin, Size::new(WIDTH, HEIGHT))
            .into_styled(PrimitiveStyle::with_fill(WINDOW_BG))
            .draw(target)?;

        // Title bar background
        Rectangle::new(origin, Size::new(WIDTH, TITLE_BAR_HEIGHT))
            .into_styled(PrimitiveStyle::with_fill(TITLE_BG))
            .draw(target)?;

        // Title text
        Text::with_baseline(
            "Clock",
            origin + Point::new(8, 5),
            MonoTextStyle::new(&FONT_6X10, TITLE_TEXT),
            embedded_graphics::text::Baseline::Top,
        )
        .draw(target)?;

        // Close button background (top-right corner)
        let close_btn_x = (WIDTH - TITLE_BAR_HEIGHT) as i32;
        Rectangle::new(
            origin + Point::new(close_btn_x, 0),
            Size::new(TITLE_BAR_HEIGHT, TITLE_BAR_HEIGHT),
        )
        .into_styled(PrimitiveStyle::with_fill(CLOSE_BTN_BG))
        .draw(target)?;

        // Close button 'X' label (centered visually)
        Text::with_baseline(
            "X",
            origin + Point::new(close_btn_x + 7, 5),
            MonoTextStyle::new(&FONT_6X10, CLOSE_BTN_TEXT),
            embedded_graphics::text::Baseline::Top,
        )
        .draw(target)?;

        // Window border outlining the whole app
        Rectangle::new(origin, Size::new(WIDTH, HEIGHT))
            .into_styled(PrimitiveStyle::with_stroke(WINDOW_BORDER, 1))
            .draw(target)?;

        // Title bar separator line
        Line::new(
            origin + Point::new(0, TITLE_BAR_HEIGHT as i32),
            origin + Point::new((WIDTH - 1) as i32, TITLE_BAR_HEIGHT as i32),
        )
        .into_styled(PrimitiveStyle::with_stroke(WINDOW_BORDER, 1))
        .draw(target)?;

        let face_tl = center - Point::new(FACE_RADIUS as i32, FACE_RADIUS as i32);
        Circle::new(face_tl, FACE_RADIUS * 2)
            .into_styled(
                PrimitiveStyleBuilder::new()
                    .fill_color(FACE_BG)
                    .stroke_color(FACE_RING)
                    .stroke_width(1)
                    .build(),
            )
            .draw(target)?;

        for tick in 0..12 {
            let cardinal = tick % 3 == 0;
            let inner = FACE_RADIUS - if cardinal { 10 } else { 6 };
            let degrees = (tick * 30) as f32;
            Line::new(
                hand_endpoint(center, inner, degrees),
                hand_endpoint(center, FACE_RADIUS, degrees),
            )
            .into_styled(PrimitiveStyle::with_stroke(
                TICK,
                if cardinal { 2 } else { 1 },
            ))
            .draw(target)?;
        }

        let seconds = f32::from(time.seconds);
        let minutes = f32::from(time.minutes) + seconds / 60.0;
        let hours = f32::from(time.hours % 12) + f32::from(time.minutes) / 60.0;

        Line::new(center, hand_endpoint(center, HOUR_HAND, hours * 30.0))
            .into_styled(PrimitiveStyle::with_stroke(HAND, 3))
            .draw(target)?;
        Line::new(center, hand_endpoint(center, MINUTE_HAND, minutes * 6.0))
            .into_styled(PrimitiveStyle::with_stroke(HAND, 2))
            .draw(target)?;
        Line::new(center, hand_endpoint(center, SECOND_HAND, seconds * 6.0))
            .into_styled(PrimitiveStyle::with_stroke(SECOND, 1))
            .draw(target)?;

        Circle::new(center - Point::new(2, 2), 5)
            .into_styled(PrimitiveStyle::with_fill(SECOND))
            .draw(target)?;

        let text = format_time(time);
        let text_x = center.x - (8 * FONT_6X10.character_size.width as i32) / 2;
        Text::with_baseline(
            core::str::from_utf8(&text).expect("clock digits are ASCII"),
            Point::new(text_x, center.y + FACE_RADIUS as i32 + 12),
            MonoTextStyle::new(&FONT_6X10, DIGIT),
            embedded_graphics::text::Baseline::Top,
        )
        .draw(target)?;

        Ok(())
    }
}

impl Default for ClockWindow {
    fn default() -> Self {
        Self::new()
    }
}

/// `HH:MM:SS`, each field zero-padded to two digits.
fn format_time(time: TimeOfDay) -> [u8; 8] {
    let mut buf = [0u8; 8];
    two_digits(time.hours, &mut buf[0..2]);
    buf[2] = b':';
    two_digits(time.minutes, &mut buf[3..5]);
    buf[5] = b':';
    two_digits(time.seconds, &mut buf[6..8]);
    buf
}

fn two_digits(value: u8, out: &mut [u8]) {
    out[0] = b'0' + value / 10;
    out[1] = b'0' + value % 10;
}

/// Endpoint of a hand `radius` px long at `degrees` clockwise from 12 o'clock,
/// drawn on a screen space where `+y` points down.
fn hand_endpoint(center: Point, radius: u32, degrees: f32) -> Point {
    let radians = degrees * core::f32::consts::PI / 180.0;
    Point::new(
        center.x + (radians.sin() * radius as f32) as i32,
        center.y - (radians.cos() * radius as f32) as i32,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use embedded_graphics::geometry::OriginDimensions;

    const SCREEN_W: u32 = 320;
    const SCREEN_H: u32 = 240;
    const SENTINEL: Rgb888 = Rgb888::new(0x12, 0x34, 0x56);

    #[test]
    fn hand_endpoint_traces_a_clockwise_circle() {
        let center = Point::new(10, 10);
        let r = 10;
        assert_eq!(hand_endpoint(center, r, 0.0), Point::new(10, 0));
        assert_eq!(hand_endpoint(center, r, 90.0), Point::new(20, 10));
        assert_eq!(hand_endpoint(center, r, 180.0), Point::new(10, 20));
        assert_eq!(hand_endpoint(center, r, 270.0), Point::new(0, 10));
    }

    #[test]
    fn translate_clamps_the_window_to_the_screen() {
        let mut window = ClockWindow::new();
        window.translate(-100, -100, SCREEN_W, SCREEN_H);
        assert_eq!((window.x, window.y), (0, 0));

        window.translate(1000, 1000, SCREEN_W, SCREEN_H);
        assert_eq!(
            (window.x, window.y),
            (SCREEN_W as i32 - 220, SCREEN_H as i32 - 180)
        );

        // A screen smaller than the window pins it at the origin instead of
        // panicking on an inverted clamp bound.
        let mut tiny = ClockWindow::new();
        tiny.translate(50, 50, 100, 100);
        assert_eq!((tiny.x, tiny.y), (0, 0));
    }

    #[test]
    fn place_centered_centers_the_window() {
        let mut window = ClockWindow::new();
        window.place_centered(SCREEN_W, SCREEN_H);
        assert_eq!((window.x, window.y), (50, 30));
    }

    #[test]
    fn format_time_pads_each_field_to_two_digits() {
        assert_eq!(format_time(TimeOfDay::new(0, 0, 0)), *b"00:00:00");
        assert_eq!(format_time(TimeOfDay::new(1, 2, 3)), *b"01:02:03");
        assert_eq!(format_time(TimeOfDay::new(23, 59, 59)), *b"23:59:59");
    }

    #[test]
    fn midnight_points_all_hands_up() {
        let mut window = ClockWindow::new();
        window.place_centered(SCREEN_W, SCREEN_H);
        let mut target = PixelBuffer::new(SENTINEL);
        window.draw(&mut target, TimeOfDay::new(0, 0, 0)).unwrap();

        let cx = window.x + (WIDTH as i32) / 2;
        let cy = window.y + (TITLE_BAR_HEIGHT as i32) + ((HEIGHT - TITLE_BAR_HEIGHT) as i32) / 2;

        // Second hand is on top, pointing straight up, and the center cap
        // covers the hand origins.
        assert_eq!(target.get(cx as u32, cy as u32 - SECOND_HAND), SECOND);
        assert_eq!(target.get(cx as u32, cy as u32), SECOND);
    }

    #[test]
    fn window_paints_inside_its_bounds_only() {
        let mut window = ClockWindow::new();
        window.place_centered(SCREEN_W, SCREEN_H);
        let mut target = PixelBuffer::new(SENTINEL);
        window.draw(&mut target, TimeOfDay::new(9, 30, 15)).unwrap();

        // Border ring.
        assert_eq!(target.get(window.x as u32, window.y as u32), WINDOW_BORDER);
        // Interior corner is now title bar fill.
        assert_eq!(
            target.get(window.x as u32 + 5, window.y as u32 + 5),
            TITLE_BG
        );
        // Window body background is strictly below the title bar.
        assert_eq!(
            target.get(window.x as u32 + 5, window.y as u32 + TITLE_BAR_HEIGHT + 5),
            WINDOW_BG
        );
        // A point inside the face that no hand or tick reaches stays face fill.
        let cx = window.x + (WIDTH as i32) / 2;
        let cy = window.y + (TITLE_BAR_HEIGHT as i32) + ((HEIGHT - TITLE_BAR_HEIGHT) as i32) / 2;
        assert_eq!(target.get((cx + 20) as u32, (cy + 20) as u32), FACE_BG);
        // Pixels outside the window are untouched.
        assert_eq!(
            target.get(window.x as u32 - 1, window.y as u32 - 1),
            SENTINEL
        );
    }

    #[test]
    fn digital_readout_is_rendered_below_the_face() {
        let mut window = ClockWindow::new();
        window.place_centered(SCREEN_W, SCREEN_H);
        let mut target = PixelBuffer::new(SENTINEL);
        window
            .draw(&mut target, TimeOfDay::new(12, 34, 56))
            .unwrap();

        let cx = window.x + (WIDTH as i32) / 2;
        let cy = window.y + (TITLE_BAR_HEIGHT as i32) + ((HEIGHT - TITLE_BAR_HEIGHT) as i32) / 2;

        let text_top = cy + FACE_RADIUS as i32 + 12;
        let text_left = cx - (8 * FONT_6X10.character_size.width as i32) / 2;
        let mut lit = 0;

        for y in text_top..text_top + FONT_6X10.character_size.height as i32 {
            for x in text_left..text_left + 48 {
                if target.get(x as u32, y as u32) == DIGIT {
                    lit += 1;
                }
            }
        }
        assert!(lit > 20, "expected text pixels, found {lit}");
    }

    /// A `DrawTarget` over a fixed screen for host tests; mirrors the shape of
    /// the framebuffer adapter in `ferric-unsafe-core`.
    struct PixelBuffer {
        pixels: [Rgb888; (SCREEN_W * SCREEN_H) as usize],
    }

    impl PixelBuffer {
        fn new(fill: Rgb888) -> Self {
            Self {
                pixels: [fill; (SCREEN_W * SCREEN_H) as usize],
            }
        }

        fn get(&self, x: u32, y: u32) -> Rgb888 {
            self.pixels[y as usize * SCREEN_W as usize + x as usize]
        }

        fn set(&mut self, x: u32, y: u32, color: Rgb888) {
            self.pixels[y as usize * SCREEN_W as usize + x as usize] = color;
        }
    }

    impl OriginDimensions for PixelBuffer {
        fn size(&self) -> Size {
            Size::new(SCREEN_W, SCREEN_H)
        }
    }

    impl DrawTarget for PixelBuffer {
        type Color = Rgb888;
        type Error = core::convert::Infallible;

        fn draw_iter<I>(&mut self, pixels: I) -> Result<(), Self::Error>
        where
            I: IntoIterator<Item = Pixel<Self::Color>>,
        {
            for Pixel(point, color) in pixels {
                if point.x >= 0 && point.y >= 0 {
                    self.set(point.x as u32, point.y as u32, color);
                }
            }
            Ok(())
        }
    }
}
