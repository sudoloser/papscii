//! Glyph-atlas software renderer for the wallpaper grid.
//!
//! A persistent master `ARGB8888` canvas is kept in memory. Every frame only
//! cells whose glyph or colour changed are re-rasterised, and the changed cells
//! are merged into horizontal damage rectangles. The master canvas is then
//! memcpy'd into the compositor's shared-memory buffer.
//!
//! `wl_shm`'s `Argb8888` is `0xAARRGGBB`, so on little-endian machines the bytes
//! in memory are `[B, G, R, A]`.

use std::collections::HashMap;

use fontdue::{Font, FontSettings};
use papscii_core::Grid;

const FONT_BYTES: &[u8] = include_bytes!("../../../assets/fonts/NotoSansMono-Regular.ttf");

/// Character cell width:height ratio (~1:2), matching the encoder.
const CELL_ASPECT: f32 = 0.5;

/// Smallest rectangle containing all of `rects`. `rects` must not be empty.
pub fn bounding_rect(rects: &[Rect]) -> Rect {
    let (mut x0, mut y0, mut x1, mut y1) = (i32::MAX, i32::MAX, i32::MIN, i32::MIN);
    for r in rects {
        x0 = x0.min(r.x);
        y0 = y0.min(r.y);
        x1 = x1.max(r.x + r.w);
        y1 = y1.max(r.y + r.h);
    }
    Rect {
        x: x0,
        y: y0,
        w: x1 - x0,
        h: y1 - y0,
    }
}

/// A damage rectangle in surface coordinates.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

/// Per-wallpaper render options decoded from the file header.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Style {
    /// Glyph cell height in pixels; `0` fits the whole surface.
    pub text_size: u16,
    /// Leave the background transparent instead of painting it black.
    pub transparent: bool,
}

/// Resolved glyph geometry for a grid inside a surface of size `w` x `h`.
#[derive(Debug, Clone, Copy)]
pub struct Layout {
    pub origin_x: f32,
    pub origin_y: f32,
    pub cell_w: f32,
    pub cell_h: f32,
    pub content_w: u32,
    pub content_h: u32,
}

impl Layout {
    /// Where a `cols` x `rows` grid lands inside a `w` x `h` surface.
    ///
    /// `text_size == 0` fits the grid to the surface; otherwise the cell is
    /// `text_size` tall and the grid is centred, scaling down only on overflow.
    pub fn resolve(cols: u16, rows: u16, style: Style, w: u32, h: u32) -> Layout {
        let n_cols = cols.max(1) as f32;
        let n_rows = rows.max(1) as f32;
        let (mut cw, mut ch) = if style.text_size > 0 {
            let ch = style.text_size as f32;
            (ch * CELL_ASPECT, ch)
        } else {
            (w as f32 / n_cols, h as f32 / n_rows)
        };
        let scale = (w as f32 / (cw * n_cols))
            .min(h as f32 / (ch * n_rows))
            .min(1.0);
        cw *= scale;
        ch *= scale;
        Layout {
            origin_x: (w as f32 - cw * n_cols) / 2.0,
            origin_y: (h as f32 - ch * n_rows) / 2.0,
            cell_w: cw,
            cell_h: ch,
            content_w: (cw * n_cols).round() as u32,
            content_h: (ch * n_rows).round() as u32,
        }
    }
}

/// Pixel size the grid wants, clamped to a `max_w` x `max_h` area (an output).
///
/// Lets the layer surface shrink to just the art, so the compositor only blends
/// the glyph area each frame instead of a fullscreen buffer.
pub fn content_size(cols: u16, rows: u16, style: Style, max_w: u32, max_h: u32) -> (u32, u32) {
    if cols == 0 || rows == 0 || max_w == 0 || max_h == 0 {
        return (max_w.max(1), max_h.max(1));
    }
    let l = Layout::resolve(cols, rows, style, max_w, max_h);
    (l.content_w.clamp(1, max_w), l.content_h.clamp(1, max_h))
}

struct Glyph {
    w: usize,
    h: usize,
    data: Vec<u8>,
}

pub struct Renderer {
    width: u32,
    height: u32,
    cols: u16,
    rows: u16,
    origin_x: f32,
    origin_y: f32,
    cell_w: f32,
    cell_h: f32,
    style: Option<Style>,
    atlas: HashMap<u8, Glyph>,
    master: Vec<u8>,
    font: Font,
}

impl Renderer {
    pub fn new() -> Self {
        let font = Font::from_bytes(FONT_BYTES, FontSettings::default())
            .expect("bundled Noto Sans Mono is valid");
        Renderer {
            width: 0,
            height: 0,
            cols: 0,
            rows: 0,
            origin_x: 0.0,
            origin_y: 0.0,
            cell_w: 1.0,
            cell_h: 1.0,
            style: None,
            atlas: HashMap::new(),
            master: Vec::new(),
            font,
        }
    }

    pub fn canvas(&self) -> &[u8] {
        &self.master
    }

    /// Fill the whole canvas with opaque black (used when no wallpaper is set).
    pub fn clear_black(&mut self, w: u32, h: u32) -> Vec<Rect> {
        if w != self.width || h != self.height {
            self.width = w;
            self.height = h;
            self.cols = 0;
            self.rows = 0;
            self.master = vec![0u8; w as usize * h as usize * 4];
        }
        for p in self.master.as_chunks_mut::<4>().0 {
            p[0] = 0;
            p[1] = 0;
            p[2] = 0;
            p[3] = 255;
        }
        vec![Rect {
            x: 0,
            y: 0,
            w: w as i32,
            h: h as i32,
        }]
    }

    fn background(&self) -> [u8; 4] {
        match self.style {
            Some(s) if s.transparent => [0, 0, 0, 0],
            _ => [0, 0, 0, 255],
        }
    }

    fn build(&mut self, w: u32, h: u32, cols: u16, rows: u16, charset: &[u8], style: Style) {
        self.width = w;
        self.height = h;
        self.cols = cols.max(1);
        self.rows = rows.max(1);
        self.style = Some(style);

        let layout = Layout::resolve(self.cols, self.rows, style, w, h);
        self.cell_w = layout.cell_w;
        self.cell_h = layout.cell_h;
        self.origin_x = layout.origin_x;
        self.origin_y = layout.origin_y;

        let px = (layout.cell_h * 0.9).clamp(4.0, 512.0);

        self.atlas.clear();
        for (i, &b) in charset.iter().enumerate() {
            let ch = b as char;
            let (m, bitmap) = self.font.rasterize(ch, px);
            self.atlas.insert(
                i as u8,
                Glyph {
                    w: m.width,
                    h: m.height,
                    data: bitmap,
                },
            );
        }

        self.master = vec![0u8; w as usize * h as usize * 4];
        let bg = self.background();
        for p in self.master.as_chunks_mut::<4>().0 {
            p.copy_from_slice(&bg);
        }
    }

    #[inline]
    fn x_of(&self, c: u16) -> i32 {
        (self.origin_x + c as f32 * self.cell_w).round() as i32
    }

    #[inline]
    fn y_of(&self, r: u16) -> i32 {
        (self.origin_y + r as f32 * self.cell_h).round() as i32
    }

    fn span_rect(&self, r: u16, start: u16, end: u16) -> Rect {
        Rect {
            x: self.x_of(start),
            y: self.y_of(r),
            w: self.x_of(end) - self.x_of(start),
            h: self.y_of(r + 1) - self.y_of(r),
        }
    }

    fn fill_cell(&mut self, c: u16, r: u16) {
        let x0 = self.x_of(c).max(0) as u32;
        let x1 = self.x_of(c + 1).min(self.width as i32).max(0) as u32;
        let y0 = self.y_of(r).max(0) as u32;
        let y1 = self.y_of(r + 1).min(self.height as i32).max(0) as u32;
        if x1 <= x0 || y1 <= y0 {
            return;
        }
        let bg = self.background();
        for y in y0..y1 {
            let row = y as usize * self.width as usize;
            for x in x0..x1 {
                let i = (row + x as usize) * 4;
                self.master[i..i + 4].copy_from_slice(&bg);
            }
        }
    }

    fn blit_cell(&mut self, c: u16, r: u16, glyph_index: u8, rgb: (u8, u8, u8)) {
        self.fill_cell(c, r);
        let (cr, cg, cb) = rgb;
        let glyph = match self.atlas.get(&glyph_index) {
            Some(g) if g.w > 0 && g.h > 0 => g,
            _ => return,
        };
        let cell_x = self.x_of(c);
        let cell_y = self.y_of(r);
        let cell_w = (self.x_of(c + 1) - cell_x).max(0) as usize;
        let cell_h = (self.y_of(r + 1) - cell_y).max(0) as usize;
        let ox = cell_x + ((cell_w.saturating_sub(glyph.w)) / 2) as i32;
        let oy = cell_y + ((cell_h.saturating_sub(glyph.h)) / 2) as i32;
        let transparent = matches!(self.style, Some(s) if s.transparent);

        for gy in 0..glyph.h {
            let dy = oy + gy as i32;
            if dy < 0 || dy >= self.height as i32 {
                continue;
            }
            let row = dy as usize * self.width as usize;
            for gx in 0..glyph.w {
                let a = glyph.data[gy * glyph.w + gx];
                if a == 0 {
                    continue;
                }
                let dx = ox + gx as i32;
                if dx < 0 || dx >= self.width as i32 {
                    continue;
                }
                let af = a as u32;
                // Premultiplied over the (transparent or black) cell background.
                let r = (cr as u32 * af / 255) as u8;
                let g = (cg as u32 * af / 255) as u8;
                let b = (cb as u32 * af / 255) as u8;
                let alpha = if transparent { a } else { 255 };
                let i = (row + dx as usize) * 4;
                self.master[i] = b;
                self.master[i + 1] = g;
                self.master[i + 2] = r;
                self.master[i + 3] = alpha;
            }
        }
    }

    /// Render `grid`, returning the damage rectangles for what changed.
    ///
    /// `last` holds the previously rendered grid; it is updated in place.
    pub fn render(
        &mut self,
        grid: &Grid,
        charset: &[u8],
        style: Style,
        last: &mut Option<Grid>,
        w: u32,
        h: u32,
    ) -> Vec<Rect> {
        if charset.is_empty() {
            return Vec::new();
        }
        let dims_changed = w != self.width
            || h != self.height
            || grid.cols != self.cols
            || grid.rows != self.rows
            || self.style != Some(style)
            || self.atlas.is_empty();
        if dims_changed {
            self.build(w, h, grid.cols, grid.rows, charset, style);
        }
        let full = dims_changed || last.is_none();

        let cols = grid.cols;
        let rows = grid.rows;
        let mut damage = Vec::new();

        for r in 0..rows {
            let mut start: Option<u16> = None;
            for c in 0..cols {
                let idx = r as usize * cols as usize + c as usize;
                let changed = full
                    || match last.as_ref() {
                        Some(prev) => {
                            prev.cells.get(idx) != grid.cells.get(idx)
                                || prev.color_at(idx) != grid.color_at(idx)
                        }
                        None => true,
                    };
                if changed {
                    let gi = *grid.cells.get(idx).unwrap_or(&0);
                    let rgb = grid.color_at(idx).unwrap_or((255, 255, 255));
                    self.blit_cell(c, r, gi, rgb);
                    if start.is_none() {
                        start = Some(c);
                    }
                } else if let Some(s) = start.take() {
                    damage.push(self.span_rect(r, s, c));
                }
            }
            if let Some(s) = start.take() {
                damage.push(self.span_rect(r, s, cols));
            }
        }

        if full {
            damage = vec![Rect {
                x: 0,
                y: 0,
                w: self.width as i32,
                h: self.height as i32,
            }];
        }

        *last = Some(grid.clone());
        damage
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use papscii_core::Grid;

    #[test]
    fn transparent_background_has_zero_alpha() {
        let grid = Grid::new(4, 2);
        let mut r = Renderer::new();
        let mut last = None;
        r.render(
            &grid,
            b" @",
            Style {
                text_size: 0,
                transparent: true,
            },
            &mut last,
            64,
            32,
        );
        assert_eq!(r.canvas()[3], 0, "corner should be transparent");
    }

    #[test]
    fn opaque_background_has_full_alpha() {
        let grid = Grid::new(4, 2);
        let mut r = Renderer::new();
        let mut last = None;
        r.render(
            &grid,
            b" @",
            Style {
                text_size: 0,
                transparent: false,
            },
            &mut last,
            64,
            32,
        );
        assert_eq!(r.canvas()[3], 255, "corner should be opaque black");
    }

    #[test]
    fn fit_mode_wants_the_whole_surface() {
        let s = Style {
            text_size: 0,
            transparent: false,
        };
        assert_eq!(content_size(40, 15, s, 1920, 1200), (1920, 1200));
    }

    #[test]
    fn fixed_text_size_shrinks_to_the_art() {
        let s = Style {
            text_size: 8,
            transparent: false,
        };
        assert_eq!(content_size(40, 15, s, 1920, 1200), (160, 120));
    }

    #[test]
    fn oversized_text_scales_down_to_fit() {
        let s = Style {
            text_size: 200,
            transparent: false,
        };
        assert_eq!(content_size(40, 15, s, 1920, 1200), (1600, 1200));
    }

    #[test]
    fn colour_glyph_is_tinted_red() {
        let mut grid = Grid::new(1, 1);
        grid.cells = vec![1];
        grid.colors = Some(vec![255, 0, 0]);
        let mut r = Renderer::new();
        let mut last = None;
        r.render(
            &grid,
            b" @",
            Style {
                text_size: 8,
                transparent: true,
            },
            &mut last,
            80,
            80,
        );
        let red = r
            .canvas()
            .as_chunks::<4>()
            .0
            .iter()
            .any(|p| p[3] > 0 && p[2] > p[1] && p[2] > p[0]);
        assert!(red, "expected a red-tinted glyph pixel");
    }

    #[test]
    fn bounding_rect_covers_all_rects() {
        let rs = [
            Rect {
                x: 10,
                y: 20,
                w: 5,
                h: 5,
            },
            Rect {
                x: 0,
                y: 30,
                w: 4,
                h: 2,
            },
        ];
        assert_eq!(
            bounding_rect(&rs),
            Rect {
                x: 0,
                y: 20,
                w: 15,
                h: 12
            }
        );
    }
}
