//! PC Screen Font (PSF) parsing and glyph rasterization, allocation-free.
//!
//! A [`Font`] borrows the raw font bytes and computes layout without copying;
//! the console passes the loader-provided buffer straight through. Both PSF v1
//! (8 px glyphs, the zap-light16 layout) and v2 (arbitrary width) are accepted.
//! Format facts cited from the OSDev "PC Screen Font" article and the kbd
//! font-formats reference.

/// Why a byte slice was rejected as a font.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FontError {
    /// The slice is too short to hold the version's header.
    TooShort,
    /// A supported magic was matched but the payload is malformed.
    Corrupt,
}

fn u16le(b: &[u8]) -> u16 {
    u16::from_le_bytes([b[0], b[1]])
}

fn u32le(b: &[u8]) -> u32 {
    u32::from_le_bytes([b[0], b[1], b[2], b[3]])
}

const PSF1_MAGIC: u16 = 0x0436;
const PSF1_HEADER: usize = 4;
const PSF2_MAGIC: u32 = 0x864a_b572;
const PSF2_HEADER: usize = 32;

/// A parsed PSF font view over borrowed bytes. No heap is used; glyph access
/// returns slices into the original data.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Font<'a> {
    data: &'a [u8],
    version: u8,
    width: u32,
    height: u32,
    bytes_per_row: u32,
    glyph_count: u32,
    glyph_bytes: usize,
    glyph_data_offset: usize,
    has_unicode: bool,
    unicode_offset: usize,
}

impl<'a> Font<'a> {
    /// Parses a PSF1 or PSF2 font from `data`.
    pub fn parse(data: &'a [u8]) -> Result<Self, FontError> {
        if data.len() < 2 {
            return Err(FontError::TooShort);
        }
        if u16le(data) == PSF1_MAGIC {
            return Self::parse_psf1(data);
        }
        if data.len() >= 4 && u32le(data) == PSF2_MAGIC {
            return Self::parse_psf2(data);
        }
        Err(FontError::Corrupt)
    }

    fn parse_psf1(data: &'a [u8]) -> Result<Self, FontError> {
        if data.len() < PSF1_HEADER {
            return Err(FontError::TooShort);
        }
        let mode = data[2];
        let char_size = data[3];
        let glyph_count: u32 = if mode & 1 != 0 { 512 } else { 256 };
        let glyph_bytes = usize::from(char_size);
        let glyph_data_offset = PSF1_HEADER;
        let glyphs_end = glyph_data_offset
            .checked_add(glyph_count as usize * glyph_bytes)
            .ok_or(FontError::Corrupt)?;
        if glyphs_end > data.len() {
            return Err(FontError::Corrupt);
        }
        Ok(Self {
            data,
            version: 1,
            width: 8,
            height: u32::from(char_size),
            bytes_per_row: 1,
            glyph_count,
            glyph_bytes,
            glyph_data_offset,
            has_unicode: mode & 2 != 0,
            unicode_offset: glyphs_end,
        })
    }

    fn parse_psf2(data: &'a [u8]) -> Result<Self, FontError> {
        if data.len() < PSF2_HEADER {
            return Err(FontError::TooShort);
        }
        let headersize = u32le(&data[8..12]) as usize;
        let flags = u32le(&data[12..16]);
        let numglyph = u32le(&data[16..20]);
        let bytesperglyph = u32le(&data[20..24]);
        let height = u32le(&data[24..28]);
        let width = u32le(&data[28..32]);
        if headersize < PSF2_HEADER
            || numglyph == 0
            || bytesperglyph == 0
            || height == 0
            || width == 0
        {
            return Err(FontError::Corrupt);
        }
        let glyph_bytes = bytesperglyph as usize;
        let glyphs_end = headersize
            .checked_add(numglyph as usize * glyph_bytes)
            .ok_or(FontError::Corrupt)?;
        if glyphs_end > data.len() {
            return Err(FontError::Corrupt);
        }
        let unicode_offset = flags & 1 != 0;
        Ok(Self {
            data,
            version: 2,
            width,
            height,
            bytes_per_row: bytesperglyph / height,
            glyph_count: numglyph,
            glyph_bytes,
            glyph_data_offset: headersize,
            has_unicode: unicode_offset,
            unicode_offset: glyphs_end,
        })
    }

    pub fn width(&self) -> u32 {
        self.width
    }

    pub fn height(&self) -> u32 {
        self.height
    }

    pub fn glyph_count(&self) -> u32 {
        self.glyph_count
    }

    /// Whether a unicode mapping table was present in the font.
    pub fn has_unicode(&self) -> bool {
        self.has_unicode
    }

    /// The raw bitmap of glyph `index` (top row first; pixels MSB-first
    /// within each row byte). `None` when `index` is out of range.
    pub fn glyph(&self, index: u32) -> Option<&'a [u8]> {
        if index >= self.glyph_count {
            return None;
        }
        let start = self.glyph_data_offset + index as usize * self.glyph_bytes;
        Some(&self.data[start..start + self.glyph_bytes])
    }

    /// Whether `(x, y)` inside glyph `index` is part of the glyph. Pixel
    /// bits are laid out MSB-first, top row first.
    pub fn glyph_pixel(&self, index: u32, x: u32, y: u32) -> Option<bool> {
        if index >= self.glyph_count || x >= self.width || y >= self.height {
            return None;
        }
        let row =
            usize::try_from(y).ok()? * self.bytes_per_row as usize + usize::try_from(x / 8).ok()?;
        let byte = self.glyph(index)?[row];
        Some(byte & (1 << (7 - (x % 8))) != 0)
    }

    /// Resolves `c` to a glyph index: the unicode table first, then its code
    /// point as a direct index when in range, else `None` for the caller to
    /// fall back to a replacement glyph.
    pub fn glyph_index_for(&self, c: char) -> Option<u32> {
        if self.has_unicode
            && let Some(g) = self.unicode_lookup(c)
        {
            return Some(g);
        }
        let u = c as u32;
        if u < self.glyph_count { Some(u) } else { None }
    }

    fn unicode_lookup(&self, c: char) -> Option<u32> {
        match self.version {
            1 => self.unicode_lookup_v1(c),
            _ => self.unicode_lookup_v2(c),
        }
    }

    fn unicode_lookup_v1(&self, c: char) -> Option<u32> {
        let mut pos = self.unicode_offset;
        let target = u32::from(c);
        for glyph in 0..self.glyph_count {
            let count = *self.data.get(pos)?;
            if count == 0xFF {
                break;
            }
            let seq = pos + 1;
            for i in 0..usize::from(count) {
                let off = seq + i * 2;
                let cp = u32::from(u16le(self.data.get(off..off + 2)?));
                if cp == target {
                    return Some(glyph);
                }
            }
            pos += 1 + usize::from(count) * 2;
        }
        None
    }

    fn unicode_lookup_v2(&self, c: char) -> Option<u32> {
        let mut pos = 0;
        let mut glyph = 0u32;
        let bytes = self.data.get(self.unicode_offset..)?;
        let target = c as u32;
        while pos < bytes.len() {
            let b = bytes[pos];
            if b == 0xFF {
                glyph += 1;
                pos += 1;
                continue;
            }
            let (cp, width) = decode_utf8(&bytes[pos..])?;
            pos += width;
            if cp == target {
                return Some(glyph);
            }
        }
        None
    }
}

/// Decodes one UTF-8 code point from the front of `b`; `None` on truncation.
fn decode_utf8(b: &[u8]) -> Option<(u32, usize)> {
    let first = *b.first()?;
    let (len, bits) = if first < 0x80 {
        (1usize, 0u32)
    } else if first & 0xE0 == 0xC0 {
        (2usize, 0xC0u32)
    } else if first & 0xF0 == 0xE0 {
        (3usize, 0xE0u32)
    } else if first & 0xF8 == 0xF0 {
        (4usize, 0xF0u32)
    } else {
        (1usize, 0x80u32)
    };
    if b.len() < len {
        return None;
    }
    let mut cp = u32::from(first) ^ bits;
    for &byte in &b[1..len] {
        cp = (cp << 6) | u32::from(byte & 0x3F);
    }
    Some((cp, len))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Builds a PSF1 byte buffer (256 glyphs, no unicode table) whose glyph
    /// `i` is all pixels on.
    fn psf1_glyphs(glyph_bytes: usize, glyph_count: usize, unicode: bool) -> Vec<u8> {
        let mut data = Vec::new();
        // Magic + mode (512-glyph bit 0, unicode bit 1) + char size.
        let mode = u8::from(glyph_count > 256) | if unicode { 2 } else { 0 };
        data.extend_from_slice(&0x0436u16.to_le_bytes());
        data.push(mode);
        data.push(glyph_bytes as u8);
        for i in 0..glyph_count {
            data.extend(core::iter::repeat_n(i as u8, glyph_bytes));
        }
        if unicode {
            // One u16 codepoint per glyph, then the 0xFF terminator.
            for g in 0..glyph_count as u16 {
                data.push(1);
                data.extend_from_slice(&g.to_le_bytes());
            }
            data.push(0xFF);
        }
        data
    }

    #[test]
    fn psf1_geometry_is_derived_from_the_header() {
        let data = psf1_glyphs(16, 256, false);
        let font = Font::parse(&data).unwrap();
        assert_eq!(font.version, 1);
        assert_eq!(font.width(), 8);
        assert_eq!(font.height(), 16);
        assert_eq!(font.glyph_count(), 256);
        assert!(!font.has_unicode());
    }

    #[test]
    fn psf1_512_glyph_mode_is_honoured() {
        let data = psf1_glyphs(8, 512, false);
        let font = Font::parse(&data).unwrap();
        assert_eq!(font.glyph_count(), 512);
        assert_eq!(font.glyph(511).unwrap().len(), 8);
        assert_eq!(font.glyph(512), None);
    }

    #[test]
    fn glyph_slices_are_presented_row_first() {
        let data = psf1_glyphs(4, 256, false);
        let font = Font::parse(&data).unwrap();
        let g = font.glyph(7).unwrap();
        assert_eq!(g, &[7, 7, 7, 7]);
    }

    #[test]
    fn glyph_pixel_reads_msb_first_bits() {
        let data = psf1_glyphs(16, 256, false);
        let font = Font::parse(&data).unwrap();
        // Glyph 0 is all-zero bits -> no pixel set.
        assert_eq!(font.glyph_pixel(0, 0, 0), Some(false));
        assert_eq!(font.glyph_pixel(0, 7, 15), Some(false));
        // Glyph 0xFF is all-one bits -> every pixel set.
        assert_eq!(font.glyph_pixel(0xFF, 0, 0), Some(true));
        assert_eq!(font.glyph_pixel(0xFF, 7, 15), Some(true));
        // Out-of-range queries.
        assert_eq!(font.glyph_pixel(0, 8, 0), None);
        assert_eq!(font.glyph_pixel(0, 0, 16), None);
        assert_eq!(font.glyph_pixel(256, 0, 0), None);
    }

    #[test]
    fn unicode_table_resolves_codepoints_to_glyphs() {
        let data = psf1_glyphs(16, 256, true);
        let font = Font::parse(&data).unwrap();
        assert!(font.has_unicode());
        // Codepoint 'A' (65) maps to glyph 65.
        assert_eq!(font.glyph_index_for('A'), Some(65));
        // Without a table the codepoint falls back to the direct index.
        let plain_data = psf1_glyphs(16, 256, false);
        let plain = Font::parse(&plain_data).unwrap();
        assert_eq!(plain.glyph_index_for('A'), Some(65));
        // A codepoint beyond the table falls back, then fails.
        assert_eq!(font.glyph_index_for('\u{4FFF}'), None);
    }

    #[test]
    fn a_synthetic_psf2_font_parses() {
        // 16 wide, 32 tall, 256 glyphs, no unicode.
        let bytes_per_glyph = (16 / 8) * 32;
        let mut data = Vec::new();
        data.extend_from_slice(&0x864a_b572u32.to_le_bytes());
        data.extend_from_slice(&0u32.to_le_bytes()); // version
        data.extend_from_slice(&32u32.to_le_bytes()); // headersize
        data.extend_from_slice(&0u32.to_le_bytes()); // flags
        data.extend_from_slice(&256u32.to_le_bytes()); // numglyph
        data.extend_from_slice(&(bytes_per_glyph as u32).to_le_bytes());
        data.extend_from_slice(&32u32.to_le_bytes()); // height
        data.extend_from_slice(&16u32.to_le_bytes()); // width
        data.resize(32 + 256 * bytes_per_glyph, 0);
        data[32] = 0x80; // first pixel of glyph 0 set
        let font = Font::parse(&data).unwrap();
        assert_eq!(font.version, 2);
        assert_eq!(font.width(), 16);
        assert_eq!(font.height(), 32);
        assert_eq!(font.bytes_per_row, 2);
        assert_eq!(font.glyph_pixel(0, 0, 0), Some(true));
        assert_eq!(font.glyph_pixel(0, 1, 0), Some(false));
        assert_eq!(font.glyph_pixel(0, 8, 0), Some(false));
    }

    #[test]
    fn malformed_and_non_psf_input_is_rejected() {
        assert_eq!(Font::parse(&[]), Err(FontError::TooShort));
        assert_eq!(Font::parse(&[0xFF]), Err(FontError::TooShort));
        // A non-PSF magic that is long enough to fail header matching.
        assert_eq!(Font::parse(&[0xAA, 0xBB]), Err(FontError::Corrupt));
        // PSF1 header but truncated glyph data.
        let mut data = psf1_glyphs(16, 256, false);
        data.truncate(100);
        assert_eq!(Font::parse(&data), Err(FontError::Corrupt));
    }

    #[test]
    fn the_vendored_zap_light16_font_parses() {
        // Guards the shipped font: if this fails the font is corrupt or the
        // header layout drifted.
        let data = include_bytes!("../../../fonts/zap-light16.psf");
        let font = Font::parse(data).unwrap();
        assert_eq!(font.version, 1);
        assert_eq!(font.width(), 8);
        assert_eq!(font.height(), 16);
        assert_eq!(font.glyph_count(), 256);
        assert!(font.has_unicode());
        assert_eq!(font.glyph(0).unwrap().len(), 16);

        // 'A' is glyph 65, a 16-byte bitmap whose crossbar row is 0x7E.
        let a = font.glyph(65).unwrap();
        assert_eq!(a[8], 0x7E);
        assert_eq!(a[0], 0x00);
        assert_eq!(a[15], 0x00);
        // The unicode table must resolve 'A' back to glyph 65.
        assert_eq!(font.glyph_index_for('A'), Some(65));
        // Pixels inside and outside the 'A' vertical stroke.
        assert!(font.glyph_pixel(65, 6, 6).unwrap());
        assert!(!font.glyph_pixel(65, 0, 0).unwrap());
    }
}
