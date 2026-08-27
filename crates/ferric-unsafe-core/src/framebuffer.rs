//! Safe, bounds-checked access to the loader-provided linear framebuffer,
//! exposed as a global handle guarded by [`OnceLock`] + [`Spinlock`].

use crate::limine::{self, Framebuffer as LimineFramebuffer};
use crate::sync::{OnceLock, Spinlock};

/// An 8-bit-per-channel color independent of the surface's pixel layout.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rgb {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

impl Rgb {
    pub const fn new(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b }
    }
}

/// Vertical bars painted and verified by the boot-time self-test.
pub const BAR_COLORS: [Rgb; 7] = [
    Rgb::new(255, 255, 255),
    Rgb::new(255, 255, 0),
    Rgb::new(0, 255, 255),
    Rgb::new(0, 255, 0),
    Rgb::new(255, 0, 255),
    Rgb::new(255, 0, 0),
    Rgb::new(0, 0, 255),
];

/// Why a framebuffer was rejected or a pixel operation refused.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FramebufferError {
    NotRgb,
    UnsupportedLayout,
    OutOfBounds,
}

#[derive(Clone, Copy)]
struct ChannelMask {
    size: u8,
    shift: u8,
}

impl ChannelMask {
    fn new(size: u8, shift: u8, bpp: u16) -> Result<Self, FramebufferError> {
        if size == 0 || u16::from(shift) + u16::from(size) > bpp {
            return Err(FramebufferError::UnsupportedLayout);
        }
        Ok(Self { size, shift })
    }

    fn bit_mask(&self) -> u64 {
        ((1u64 << self.size) - 1) << self.shift
    }
}

/// Geometry handed over by the loader, awaiting validation into a
/// [`FrameBuffer`].
pub(crate) struct FrameLayout {
    ptr: *mut u8,
    width: u32,
    height: u32,
    pitch: usize,
    bpp: u16,
    red_size: u8,
    red_shift: u8,
    green_size: u8,
    green_shift: u8,
    blue_size: u8,
    blue_shift: u8,
}

impl FrameLayout {
    fn from_limine(source: &LimineFramebuffer) -> Result<Self, FramebufferError> {
        if !source.is_rgb() {
            return Err(FramebufferError::NotRgb);
        }
        if source.address().is_null()
            || source.width == 0
            || source.height == 0
            || source.width > u64::from(u32::MAX)
            || source.height > u64::from(u32::MAX)
            || source.pitch == 0
        {
            return Err(FramebufferError::UnsupportedLayout);
        }
        Ok(Self {
            ptr: source.address(),
            width: source.width as u32,
            height: source.height as u32,
            pitch: usize::try_from(source.pitch)
                .map_err(|_| FramebufferError::UnsupportedLayout)?,
            bpp: source.bpp,
            red_size: source.red_mask_size,
            red_shift: source.red_mask_shift,
            green_size: source.green_mask_size,
            green_shift: source.green_mask_shift,
            blue_size: source.blue_mask_size,
            blue_shift: source.blue_mask_shift,
        })
    }
}

/// A captured linear framebuffer; every pixel access is bounds-checked.
///
/// Pixel words follow the Limine framebuffer convention (PROTOCOL.md):
/// little-endian, channel masks counted from bit 0 of the pixel word.
pub struct FrameBuffer {
    ptr: *mut u8,
    width: u32,
    height: u32,
    pitch: usize,
    bytes_per_pixel: usize,
    red: ChannelMask,
    green: ChannelMask,
    blue: ChannelMask,
}

// SAFETY: raw-pointer plumbing stays internal; exclusive access flows only
// through the global spin lock, and the loader-mapped surface remains valid
// for the kernel lifetime because bootloader memory is never reclaimed.
unsafe impl Send for FrameBuffer {}

impl FrameBuffer {
    /// Validates `source`'s layout and captures its geometry.
    pub fn capture(source: &LimineFramebuffer) -> Result<Self, FramebufferError> {
        Self::from_layout(FrameLayout::from_limine(source)?)
    }

    pub(crate) fn from_layout(layout: FrameLayout) -> Result<Self, FramebufferError> {
        let FrameLayout {
            ptr,
            width,
            height,
            pitch,
            bpp,
            ..
        } = layout;
        if bpp == 0 || bpp % 8 != 0 || bpp > 64 {
            return Err(FramebufferError::UnsupportedLayout);
        }
        let red = ChannelMask::new(layout.red_size, layout.red_shift, bpp)?;
        let green = ChannelMask::new(layout.green_size, layout.green_shift, bpp)?;
        let blue = ChannelMask::new(layout.blue_size, layout.blue_shift, bpp)?;
        let disjoint = (red.bit_mask() & green.bit_mask()) == 0
            && (green.bit_mask() & blue.bit_mask()) == 0
            && (blue.bit_mask() & red.bit_mask()) == 0;

        let bytes_per_pixel = usize::from(bpp / 8);
        // Rows must not overlap (writes would spill past the surface), and
        // the full-surface offset math must never wrap.
        if !disjoint
            || pitch < width as usize * bytes_per_pixel
            || pitch.checked_mul(height as usize).is_none()
        {
            return Err(FramebufferError::UnsupportedLayout);
        }

        Ok(Self {
            ptr,
            width,
            height,
            pitch,
            bytes_per_pixel,
            red,
            green,
            blue,
        })
    }

    pub fn width(&self) -> u32 {
        self.width
    }

    pub fn height(&self) -> u32 {
        self.height
    }

    fn encode_channel(mask: ChannelMask, value: u8) -> u64 {
        let max = (1u64 << mask.size) - 1;
        ((u64::from(value) * max) / 255) << mask.shift
    }

    fn encode(&self, color: Rgb) -> u64 {
        Self::encode_channel(self.red, color.r)
            | Self::encode_channel(self.green, color.g)
            | Self::encode_channel(self.blue, color.b)
    }

    fn write_unchecked(&mut self, x: u32, y: u32, pixel: u64) {
        let offset = y as usize * self.pitch + x as usize * self.bytes_per_pixel;
        for (i, byte) in pixel
            .to_le_bytes()
            .into_iter()
            .take(self.bytes_per_pixel)
            .enumerate()
        {
            // SAFETY: bounds were validated against the captured geometry
            // whose pitch covers `width * bytes_per_pixel`, keeping
            // `offset + i` inside the loader-mapped surface; that mapping
            // stays valid for the kernel lifetime because bootloader memory
            // is never reclaimed.
            unsafe { self.ptr.add(offset + i).write_volatile(byte) };
        }
    }

    /// Writes one pixel; errors when `(x, y)` lies outside the surface.
    pub fn write_pixel(&mut self, x: u32, y: u32, color: Rgb) -> Result<(), FramebufferError> {
        if x >= self.width || y >= self.height {
            return Err(FramebufferError::OutOfBounds);
        }
        self.write_unchecked(x, y, self.encode(color));
        Ok(())
    }

    /// Fills an axis-aligned rect fully inside the surface; errors otherwise.
    pub fn fill_rect(
        &mut self,
        x: u32,
        y: u32,
        w: u32,
        h: u32,
        color: Rgb,
    ) -> Result<(), FramebufferError> {
        if w == 0
            || h == 0
            || u64::from(x) + u64::from(w) > u64::from(self.width)
            || u64::from(y) + u64::from(h) > u64::from(self.height)
        {
            return Err(FramebufferError::OutOfBounds);
        }
        let pixel = self.encode(color);
        for dy in 0..h {
            for dx in 0..w {
                self.write_unchecked(x + dx, y + dy, pixel);
            }
        }
        Ok(())
    }

    /// Reads back the raw little-endian pixel word at `(x, y)`.
    pub fn read_raw_pixel(&self, x: u32, y: u32) -> Option<u64> {
        if x >= self.width || y >= self.height {
            return None;
        }
        let offset = y as usize * self.pitch + x as usize * self.bytes_per_pixel;
        let mut pixel = 0u64;
        for i in 0..self.bytes_per_pixel {
            // SAFETY: same in-surface bounds argument as `write_unchecked`.
            unsafe { pixel |= u64::from(self.ptr.add(offset + i).read_volatile()) << (8 * i) };
        }
        Some(pixel)
    }
}

static FRAMEBUFFER: OnceLock<Spinlock<FrameBuffer>> = OnceLock::new();

/// Captures the first RGB framebuffer from the boot info; false when none
/// qualifies.
pub fn init_from_boot_info(info: &limine::BootInfo) -> bool {
    let Some(source) = info.framebuffers.iter().find(|fb| fb.is_rgb()) else {
        return false;
    };
    match FrameBuffer::capture(source) {
        Ok(fb) => FRAMEBUFFER.set(Spinlock::new(fb)).is_ok(),
        Err(_) => false,
    }
}

/// Runs `f` with exclusive access; `None` before a successful init.
pub fn with_framebuffer<R>(f: impl FnOnce(&mut FrameBuffer) -> R) -> Option<R> {
    let mut guard = FRAMEBUFFER.get()?.lock();
    Some(f(&mut guard))
}

fn bar_bounds(index: usize, width: u32) -> (u32, u32) {
    let bars = BAR_COLORS.len() as u64;
    let start = (u64::from(width) * index as u64 / bars) as u32;
    let end = (u64::from(width) * (index as u64 + 1) / bars) as u32;
    (start, end)
}

/// Paints full-height vertical bars across the surface; false when any bar
/// fell outside it.
pub fn draw_color_bars(fb: &mut FrameBuffer) -> bool {
    let (width, height) = (fb.width(), fb.height());
    for (index, &color) in BAR_COLORS.iter().enumerate() {
        let (start, end) = bar_bounds(index, width);
        if fb.fill_rect(start, 0, end - start, height, color).is_err() {
            return false;
        }
    }
    true
}

/// Samples each bar at several heights and compares against expected colors.
pub fn verify_color_bars(fb: &FrameBuffer) -> bool {
    let (width, height) = (fb.width(), fb.height());
    let sample_ys = [height / 4, height / 2, height * 3 / 4].map(|y| y.min(height - 1));
    for (index, &color) in BAR_COLORS.iter().enumerate() {
        let (start, end) = bar_bounds(index, width);
        let center_x = start + (end - start) / 2;
        for y in sample_ys {
            if fb.read_raw_pixel(center_x, y) != Some(fb.encode(color)) {
                return false;
            }
        }
    }
    true
}

/// Draws the bars and proves them by reading pixels back.
pub fn run_color_bar_self_test() -> bool {
    with_framebuffer(|fb| draw_color_bars(fb) && verify_color_bars(fb)).unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    const BLACK: Rgb = Rgb::new(0, 0, 0);

    /// Builds a [`FrameBuffer`] over a mock byte buffer, or explains why the
    /// layout was rejected.
    fn try_fb_over(
        width: u32,
        height: u32,
        bpp: u16,
        (rs, rsh): (u8, u8),
        (gs, gsh): (u8, u8),
        (bs, bsh): (u8, u8),
        pitch_override: Option<usize>,
    ) -> Result<(FrameBuffer, Vec<u8>), FramebufferError> {
        let bytes_per_pixel = usize::from(bpp / 8);
        let pitch = pitch_override.unwrap_or(width as usize * bytes_per_pixel);
        let mut bytes = vec![0xEE; pitch * height as usize];
        let fb = FrameBuffer::from_layout(FrameLayout {
            ptr: bytes.as_mut_ptr(),
            width,
            height,
            pitch,
            bpp,
            red_size: rs,
            red_shift: rsh,
            green_size: gs,
            green_shift: gsh,
            blue_size: bs,
            blue_shift: bsh,
        })?;
        Ok((fb, bytes))
    }

    /// Unwrapping variant for layouts every test assumes valid.
    fn fb_over(
        width: u32,
        height: u32,
        bpp: u16,
        channels: ((u8, u8), (u8, u8), (u8, u8)),
    ) -> (FrameBuffer, Vec<u8>) {
        try_fb_over(width, height, bpp, channels.0, channels.1, channels.2, None)
            .expect("test layout must validate")
    }

    /// BGRX 32bpp (blue in the lowest byte), QEMU's common linear mode.
    fn surface_bgrx(width: u32, height: u32) -> (FrameBuffer, Vec<u8>) {
        fb_over(width, height, 32, ((8, 16), (8, 8), (8, 0)))
    }

    #[test]
    fn layouts_outside_the_supported_shape_are_rejected() {
        let rejects = [
            // Zero or non-byte-multiple bpp.
            (4, 2, (0u16, (8, 16), (8, 8), (8, 0)), None),
            (4, 2, (33, (8, 16), (8, 8), (8, 0)), None),
            (4, 2, (72, (8, 16), (8, 8), (8, 0)), None),
            // Mask reaching past the pixel word.
            (4, 2, (32, (8, 29), (8, 8), (8, 0)), None),
            // Zero-sized channel.
            (4, 2, (32, (0, 16), (8, 8), (8, 0)), None),
            // Overlapping channels.
            (4, 2, (32, (8, 12), (8, 8), (8, 0)), None),
            // Pitch smaller than one scanline.
            (4, 2, (32, (8, 16), (8, 8), (8, 0)), Some(4)),
        ];
        for (width, height, spec, pitch) in rejects {
            let result = try_fb_over(width, height, spec.0, spec.1, spec.2, spec.3, pitch);
            assert_eq!(result.err(), Some(FramebufferError::UnsupportedLayout));
        }
    }

    #[test]
    fn write_pixel_places_bytes_in_bgrx_order() {
        let (mut fb, bytes) = surface_bgrx(2, 2);

        fb.write_pixel(1, 0, Rgb::new(255, 255, 255)).unwrap();
        fb.write_pixel(0, 1, Rgb::new(0x12, 0x34, 0x56)).unwrap();

        // Pixel word 0x00FFFFFF lands little-endian: B, G, R, pad.
        assert_eq!(&bytes[4..8], &[0xFF, 0xFF, 0xFF, 0x00]);
        assert_eq!(&bytes[8..12], &[0x56, 0x34, 0x12, 0x00]);
        // Untouched pixels keep the sentinel fill.
        assert_eq!(&bytes[0..4], &[0xEE; 4]);
    }

    #[test]
    fn narrow_channels_scale_instead_of_truncating() {
        // RGB565: r[15:11] g[10:5] b[4:0].
        let (mut fb, _buffer) = fb_over(2, 2, 16, ((5, 11), (6, 5), (5, 0)));

        fb.write_pixel(0, 0, Rgb::new(255, 255, 255)).unwrap();
        assert_eq!(fb.read_raw_pixel(0, 0), Some(0xFFFF));

        fb.write_pixel(1, 1, Rgb::new(128, 128, 128)).unwrap();
        // Classic RGB565 mid-gray.
        assert_eq!(fb.read_raw_pixel(1, 1), Some(0x7BEF));
    }

    #[test]
    fn out_of_bounds_operations_are_refused() {
        let (mut fb, _buffer) = surface_bgrx(4, 4);

        assert_eq!(
            fb.write_pixel(4, 0, BLACK),
            Err(FramebufferError::OutOfBounds)
        );
        assert_eq!(
            fb.write_pixel(0, 4, BLACK),
            Err(FramebufferError::OutOfBounds)
        );
        assert_eq!(fb.read_raw_pixel(4, 4), None);

        assert_eq!(
            fb.fill_rect(0, 0, 0, 4, BLACK),
            Err(FramebufferError::OutOfBounds)
        );
        assert_eq!(
            fb.fill_rect(2, 2, 3, 1, BLACK),
            Err(FramebufferError::OutOfBounds)
        );
        assert_eq!(
            fb.fill_rect(2, 2, 1, 3, BLACK),
            Err(FramebufferError::OutOfBounds)
        );

        // Exact-fit operations stay legal.
        assert_eq!(fb.write_pixel(3, 3, BLACK), Ok(()));
        assert_eq!(fb.fill_rect(0, 0, 4, 4, BLACK), Ok(()));
    }

    #[test]
    fn fill_rect_touches_only_its_own_region() {
        let (mut fb, bytes) = surface_bgrx(4, 4);
        let pitch = 16usize;

        fb.fill_rect(1, 1, 2, 2, Rgb::new(255, 0, 0)).unwrap();

        for y in 0..4u32 {
            for x in 0..4u32 {
                let inside = (1..3).contains(&x) && (1..3).contains(&y);
                let expected_word = if inside {
                    0x00FF0000 // BGRX red sits above green/blue bytes
                } else {
                    0xEEEEEEEE // untouched sentinel
                };
                let base = y as usize * pitch + x as usize * 4;
                let word = u32::from_le_bytes(bytes[base..base + 4].try_into().unwrap()) as u64;
                assert_eq!(word, expected_word, "pixel ({x},{y})");
            }
        }
    }

    #[test]
    fn drawn_bars_verify_until_a_pixel_is_corrupted() {
        let (mut fb, _buffer) = surface_bgrx(14, 8);
        assert_eq!(fb.width(), 14);
        assert_eq!(fb.height(), 8);

        assert!(draw_color_bars(&mut fb));
        assert!(verify_color_bars(&fb));

        // Bar 2 spans columns 4..6; poison one sampled point inside it.
        fb.write_pixel(5, 4, Rgb::new(1, 2, 3)).unwrap();
        assert!(!verify_color_bars(&fb));
    }

    #[test]
    fn degenerate_surfaces_still_draw_and_verify() {
        let (mut fb, _buffer) = surface_bgrx(7, 1);
        assert!(draw_color_bars(&mut fb));
        assert!(verify_color_bars(&fb));
    }
}
