//! Cell-based text model: cursor, wrapping, scrolling, and glyph blitting
//! into a plain pixel buffer.
//!
//! [`TextGrid`] is an allocation-free console over a caller-provided
//! [`Cell`] buffer; the pixel side renders a font glyph into a [`Surface`]
//! (`[u32]` pixels packed `0x00RRGGBB`), a host-facing stand-in later routed
//! through the real framebuffer.

use crate::font::Font;
use ferric_api::Rgb;

/// One logical screen cell: a glyph plus foreground and background colors.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Cell {
    pub glyph: char,
    pub fg: Rgb,
    pub bg: Rgb,
}

impl Cell {
    /// An empty cell (`' '`) in `fg`/`bg`.
    pub const fn blank(fg: Rgb, bg: Rgb) -> Self {
        Self { glyph: ' ', fg, bg }
    }
}

/// The default tab stop width used by the cursor.
pub const TAB_WIDTH: u32 = 8;

/// How a glyph is painted: foreground/background colors and per-pixel scale.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GlyphStyle {
    pub fg: Rgb,
    pub bg: Rgb,
    pub scale: u32,
}

impl GlyphStyle {
    /// One font pixel per screen pixel in `fg`/`bg`.
    pub const fn new(fg: Rgb, bg: Rgb) -> Self {
        Self { fg, bg, scale: 1 }
    }
}

/// A mock `[u32]` pixel surface (packed `0x00RRGGBB`) with bounds-clipped
/// writes; the host-facing stand-in for the real framebuffer.
pub struct Surface<'a> {
    pixels: &'a mut [u32],
    width: usize,
    height: usize,
}

impl<'a> Surface<'a> {
    /// Wraps `pixels` laid out `width` per row; `None` when the buffer cannot
    /// hold `width * height` pixels.
    pub fn new(pixels: &'a mut [u32], width: usize, height: usize) -> Option<Self> {
        let needed = width.checked_mul(height)?;
        if width == 0 || needed > pixels.len() {
            return None;
        }
        Some(Self {
            pixels,
            width,
            height,
        })
    }

    pub fn width(&self) -> usize {
        self.width
    }

    pub fn height(&self) -> usize {
        self.height
    }

    /// Writes one pixel, clipping out-of-bounds coordinates.
    pub fn put(&mut self, x: usize, y: usize, color: u32) {
        if x < self.width && y < self.height {
            self.pixels[y * self.width + x] = color;
        }
    }

    /// Draws glyph `index` of `font` with its top-left at `(px, py)`.
    pub fn blit_glyph(&mut self, font: &Font, index: u32, px: usize, py: usize, style: GlyphStyle) {
        let fg = pack(style.fg);
        let bg = pack(style.bg);
        let scale = style.scale.clamp(1, 64) as usize;
        let (fw, fh) = (font.width() as usize, font.height() as usize);
        for gy in 0..fh {
            for gx in 0..fw {
                let Some(set) = font.glyph_pixel(index, gx as u32, gy as u32) else {
                    continue;
                };
                let color = if set { fg } else { bg };
                for sy in 0..scale {
                    let y = py + gy * scale + sy;
                    for sx in 0..scale {
                        self.put(px + gx * scale + sx, y, color);
                    }
                }
            }
        }
    }
}

fn pack(c: Rgb) -> u32 {
    u32::from(c.r) << 16 | u32::from(c.g) << 8 | u32::from(c.b)
}

/// A rectangular, cursor-tracked console over `&mut [Cell]`. Writing past the
/// last column wraps; writing past the last row scrolls.
pub struct TextGrid<'a> {
    cells: &'a mut [Cell],
    cols: u32,
    rows: u32,
    row: u32,
    col: u32,
    default_fg: Rgb,
    default_bg: Rgb,
}

impl<'a> TextGrid<'a> {
    /// Wraps `cells` as a `cols`-by-`rows` console. Returns `None` when the
    /// buffer cannot hold `cols * rows` cells.
    pub fn new(cells: &'a mut [Cell], cols: u32, rows: u32) -> Option<Self> {
        let needed = usize::try_from(cols)
            .ok()?
            .checked_mul(usize::try_from(rows).ok()?)?;
        if cols == 0 || rows == 0 || needed > cells.len() {
            return None;
        }
        let mut grid = Self {
            cells,
            cols,
            rows,
            row: 0,
            col: 0,
            default_fg: Rgb::new(0xC0, 0xC0, 0xC0),
            default_bg: Rgb::new(0, 0, 0),
        };
        grid.clear_defaults();
        Some(grid)
    }

    /// Clears every cell to `' '` in `fg`/`bg` and homes the cursor. Optionally
    /// sets the defaults used by later [`Self::put`] calls.
    pub fn clear(&mut self, fg: Rgb, bg: Rgb) {
        self.default_fg = fg;
        self.default_bg = bg;
        self.clear_defaults();
        self.row = 0;
        self.col = 0;
    }

    fn clear_defaults(&mut self) {
        let blank = Cell::blank(self.default_fg, self.default_bg);
        for cell in self.cells.iter_mut() {
            *cell = blank;
        }
    }

    pub fn cols(&self) -> u32 {
        self.cols
    }

    pub fn rows(&self) -> u32 {
        self.rows
    }

    pub fn cursor_row(&self) -> u32 {
        self.row
    }

    pub fn cursor_col(&self) -> u32 {
        self.col
    }

    /// Moves the cursor; coordinates are clamped into the grid.
    pub fn set_cursor(&mut self, row: u32, col: u32) {
        self.row = row.min(self.rows.saturating_sub(1));
        self.col = col.min(self.cols.saturating_sub(1));
    }

    fn index(&self, row: u32, col: u32) -> usize {
        (row * self.cols + col) as usize
    }

    /// The cell at `(row, col)`; `None` when out of range.
    pub fn cell(&self, row: u32, col: u32) -> Option<Cell> {
        if row >= self.rows || col >= self.cols {
            return None;
        }
        Some(self.cells[self.index(row, col)])
    }

    /// Overwrites the cell at `(row, col)`.
    pub fn set_cell(&mut self, row: u32, col: u32, value: Cell) {
        if row < self.rows && col < self.cols {
            self.cells[self.index(row, col)] = value;
        }
    }

    /// Writes `s` at `row`, column 0, replacing that row's cells. The cursor
    /// is left past the final character.
    pub fn set_text(&mut self, row: u32, s: &str, fg: Rgb, bg: Rgb) {
        let row = row.min(self.rows.saturating_sub(1));
        for (col, c) in s.chars().enumerate().take(self.cols as usize) {
            self.cells[self.index(row, col as u32)] = Cell { glyph: c, fg, bg };
        }
        self.set_cursor(row, (s.chars().count() as u32).min(self.cols));
    }

    /// Emits one character at the cursor, applying control codes and
    /// auto-wrapping/scrolling. `\n` moves to the start of the next row,
    /// `\r` to the start of the current one, `\t` to the next tab stop.
    pub fn put(&mut self, c: char, fg: Rgb, bg: Rgb) {
        match c {
            '\n' => {
                self.linefeed();
            }
            '\r' => {
                self.col = 0;
            }
            '\t' => {
                let next = (self.col / TAB_WIDTH + 1) * TAB_WIDTH;
                self.col = next.min(self.cols);
            }
            _ => {
                self.cells[self.index(self.row, self.col)] = Cell { glyph: c, fg, bg };
                self.col += 1;
                if self.col >= self.cols {
                    self.linefeed();
                }
            }
        }
    }

    fn linefeed(&mut self) {
        self.col = 0;
        if self.row + 1 >= self.rows {
            self.scroll_up();
        } else {
            self.row += 1;
        }
    }

    /// Shifts every row up one, blanking the bottom row.
    pub fn scroll_up(&mut self) {
        let cols = self.cols as usize;
        self.cells.copy_within(cols.., 0);
        let blank = Cell::blank(self.default_fg, self.default_bg);
        let start = (self.rows as usize - 1) * cols;
        for cell in self.cells[start..start + cols].iter_mut() {
            *cell = blank;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const BLACK: Rgb = Rgb::new(0, 0, 0);
    const WHITE: Rgb = Rgb::new(255, 255, 255);

    #[test]
    fn buffer_too_small_is_rejected() {
        let mut cells = [Cell::blank(BLACK, BLACK); 8];
        assert!(TextGrid::new(&mut cells, 3, 3).is_none());
        assert!(TextGrid::new(&mut cells, 2, 4).is_some());
        assert!(TextGrid::new(&mut cells, 0, 4).is_none());
    }

    #[test]
    fn put_advances_and_wraps_at_the_last_column() {
        let mut cells = vec![Cell::blank(BLACK, BLACK); 3 * 2];
        let mut g = TextGrid::new(&mut cells, 3, 2).unwrap();
        for c in ['a', 'b'] {
            g.put(c, WHITE, BLACK);
        }
        assert_eq!(g.cursor_col(), 2);
        assert_eq!(g.cell(0, 0).unwrap().glyph, 'a');

        // A char in the last column lands there, then autowraps the cursor.
        g.put('c', WHITE, BLACK);
        assert_eq!(g.cell(0, 2).unwrap().glyph, 'c');
        assert_eq!(g.cursor_row(), 1);
        assert_eq!(g.cursor_col(), 0);

        // The next char writes on the following row.
        g.put('d', WHITE, BLACK);
        assert_eq!(g.cell(1, 0).unwrap().glyph, 'd');
        assert_eq!(g.cursor_col(), 1);
    }

    #[test]
    fn newline_starts_the_next_row() {
        let mut cells = vec![Cell::blank(BLACK, BLACK); 4 * 3];
        let mut g = TextGrid::new(&mut cells, 4, 3).unwrap();
        g.put('x', WHITE, BLACK);
        g.put('\n', WHITE, BLACK);
        assert_eq!(g.cursor_row(), 1);
        assert_eq!(g.cursor_col(), 0);
        g.put('y', WHITE, BLACK);
        assert_eq!(g.cell(1, 0).unwrap().glyph, 'y');
        assert_eq!(g.cell(0, 0).unwrap().glyph, 'x');
    }

    #[test]
    fn carriage_return_returns_to_column_zero() {
        let mut cells = vec![Cell::blank(BLACK, BLACK); 4];
        let mut g = TextGrid::new(&mut cells, 4, 1).unwrap();
        g.put('a', WHITE, BLACK);
        g.put('b', WHITE, BLACK);
        g.put('\r', WHITE, BLACK);
        g.put('c', WHITE, BLACK);
        assert_eq!(g.cell(0, 0).unwrap().glyph, 'c');
        assert_eq!(g.cell(0, 1).unwrap().glyph, 'b');
    }

    #[test]
    fn tab_advances_to_the_next_stop() {
        let mut cells = vec![Cell::blank(BLACK, BLACK); 16];
        let mut g = TextGrid::new(&mut cells, 16, 1).unwrap();
        g.put('\t', WHITE, BLACK);
        assert_eq!(g.cursor_col(), 8);
        g.put('a', WHITE, BLACK);
        g.put('\t', WHITE, BLACK);
        assert_eq!(g.cursor_col(), 16);
    }

    #[test]
    fn scrolling_drops_the_top_row_and_blanks_the_bottom() {
        let mut cells = vec![Cell::blank(BLACK, BLACK); 3 * 2];
        let mut g = TextGrid::new(&mut cells, 3, 2).unwrap();
        // Fill the first row; the third char autowraps to row 1.
        for c in ['a', 'b', 'c'] {
            g.put(c, WHITE, BLACK);
        }
        assert_eq!(g.cursor_row(), 1);
        // Fill the second row; its last char scrolls everything up.
        for c in ['d', 'e', 'f'] {
            g.put(c, WHITE, BLACK);
        }
        assert_eq!(g.cell(0, 0).unwrap().glyph, 'd');
        assert_eq!(g.cell(0, 1).unwrap().glyph, 'e');
        assert_eq!(g.cell(0, 2).unwrap().glyph, 'f');
        assert_eq!(g.cell(1, 0).unwrap().glyph, ' ');
        assert_eq!(g.cell(1, 1).unwrap().glyph, ' ');
        assert_eq!(g.cell(1, 2).unwrap().glyph, ' ');
        assert_eq!(g.cursor_row(), 1);
        assert_eq!(g.cursor_col(), 0);
    }

    #[test]
    fn set_text_writes_a_row_and_leaves_the_cursor_after() {
        let mut cells = vec![Cell::blank(BLACK, BLACK); 8 * 3];
        let mut g = TextGrid::new(&mut cells, 8, 3).unwrap();
        g.set_text(1, "hi", WHITE, BLACK);
        assert_eq!(g.cell(1, 0).unwrap().glyph, 'h');
        assert_eq!(g.cell(1, 1).unwrap().glyph, 'i');
        assert_eq!(g.cursor_row(), 1);
        assert_eq!(g.cursor_col(), 2);
    }

    #[test]
    fn blit_glyph_paints_foreground_and_background() {
        // A 2x2 mock glyph: top-left pixel on only.
        let mut data = vec![0u8; 2];
        data[0] = 0b1000_0000;
        data[1] = 0b0000_0000;
        // Build a PSF1 font around it.
        let mut font_data: Vec<u8> = Vec::new();
        font_data.extend_from_slice(&0x0436u16.to_le_bytes());
        font_data.push(0); // 256 glyphs
        font_data.push(2); // char size = 2 rows
        font_data.extend(data); // glyph 0 bitmap
        font_data.resize(4 + 256 * 2, 0);
        let font = Font::parse(&font_data).unwrap();

        let mut pixels = vec![0u32; 4 * 4];
        let mut surface = Surface::new(&mut pixels, 4, 4).unwrap();
        surface.blit_glyph(&font, 0, 0, 0, GlyphStyle::new(WHITE, BLACK));
        assert_eq!(pixels[0], 0x00FF_FFFF); // (0,0) fg
        assert!(pixels[1..].iter().all(|&p| p == 0x0000_0000));

        // Scaling draws a run of `scale` pixels per font pixel.
        let mut wide = vec![0u32; 6 * 6];
        let mut surface = Surface::new(&mut wide, 6, 6).unwrap();
        surface.blit_glyph(
            &font,
            0,
            0,
            0,
            GlyphStyle {
                fg: WHITE,
                bg: BLACK,
                scale: 3,
            },
        );
        assert_eq!(wide[0], 0x00FF_FFFF);
        assert_eq!(wide[1], 0x00FF_FFFF);
        assert_eq!(wide[2], 0x00FF_FFFF);
    }
}
