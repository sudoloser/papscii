//! The `.pscii` container format.
//!
//! Layout (all integers little-endian):
//!
//! ```text
//! magic          [u8; 4]  = b"PSCI"
//! version        u16      (2)
//! fps            u16
//! cols           u16
//! rows           u16
//! charset_len    u8
//! charset        [u8; charset_len]
//! color_mode     u8       (0 mono, 2 truecolor)
//! text_size      u16      (glyph cell height in px; 0 = fit to screen)
//! flags          u8       (bit 0 = transparent background)
//! src_width      u16
//! src_height     u16
//! cover_cols     u16
//! cover_rows     u16
//! cover_cells    [u8; cover_cols * cover_rows]
//! frame_count    u32
//! frame_table    [ { offset: u32, compressed_len: u32 }; frame_count ]
//! frame_blobs    concatenated lz4 blocks (compress_prepend_size)
//! ```
//!
//! Frame payloads depend on `color_mode`:
//!
//! * **Mono** - frame 0 is the full grid (`cols * rows` bytes). Later frames are
//!   a delta: a run of `{ index: u32, value: u8 }` pairs.
//! * **Truecolor** - frame 0 is `cols * rows` glyph bytes followed by
//!   `cols * rows * 3` RGB bytes. Later frames are a delta of
//!   `{ index: u32, value: u8, r: u8, g: u8, b: u8 }` entries (8 bytes each).
//!
//! Each payload is lz4-compressed independently so any frame can be decoded
//! without decompressing the whole file.

use crate::charset;
use crate::error::PsciiError;

pub const MAGIC: [u8; 4] = *b"PSCI";
pub const VERSION: u16 = 2;

/// Maximum glyphs a charset may hold (cells are one byte).
pub const MAX_CHARSET: usize = 256;

/// How colour is stored for each cell.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum ColorMode {
    Mono = 0,
    Ansi256 = 1,
    Truecolor = 2,
}

impl ColorMode {
    fn from_u8(v: u8) -> Option<Self> {
        match v {
            0 => Some(ColorMode::Mono),
            1 => Some(ColorMode::Ansi256),
            2 => Some(ColorMode::Truecolor),
            _ => None,
        }
    }

    /// Whether cells carry per-cell RGB data in this mode.
    pub fn is_color(self) -> bool {
        matches!(self, ColorMode::Truecolor)
    }
}

/// A grid of glyph indices into the file's charset, row-major.
///
/// For colour wallpapers [`Grid::colors`] holds three RGB bytes per cell,
/// in the same row-major order as [`Grid::cells`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Grid {
    pub cols: u16,
    pub rows: u16,
    pub cells: Vec<u8>,
    pub colors: Option<Vec<u8>>,
}

impl Grid {
    /// A blank grid (every cell = glyph index 0).
    pub fn new(cols: u16, rows: u16) -> Self {
        Grid {
            cols,
            rows,
            cells: vec![0u8; cols as usize * rows as usize],
            colors: None,
        }
    }

    pub fn from_cells(cols: u16, rows: u16, cells: Vec<u8>) -> Result<Self, PsciiError> {
        Self::from_cells_color(cols, rows, cells, None)
    }

    pub fn from_cells_color(
        cols: u16,
        rows: u16,
        cells: Vec<u8>,
        colors: Option<Vec<u8>>,
    ) -> Result<Self, PsciiError> {
        let expected = cols as usize * rows as usize;
        if cells.len() != expected {
            return Err(PsciiError::GridShape {
                cols,
                rows,
                expected_cols: cols,
                expected_rows: rows,
            });
        }
        if let Some(c) = &colors {
            if c.len() != expected * 3 {
                return Err(PsciiError::Corrupt("colour buffer size"));
            }
        }
        Ok(Grid {
            cols,
            rows,
            cells,
            colors,
        })
    }

    /// RGB for cell `idx`, or `None` for mono grids.
    #[inline]
    pub fn color_at(&self, idx: usize) -> Option<(u8, u8, u8)> {
        let c = self.colors.as_ref()?;
        let o = idx * 3;
        Some((c[o], c[o + 1], c[o + 2]))
    }

    pub fn len(&self) -> usize {
        self.cells.len()
    }

    pub fn is_empty(&self) -> bool {
        self.cells.is_empty()
    }
}

/// Decoded header of a `.pscii` file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Header {
    pub version: u16,
    pub fps: u16,
    pub cols: u16,
    pub rows: u16,
    pub charset: Vec<u8>,
    pub color_mode: ColorMode,
    /// Requested glyph cell height in pixels; `0` means fit to the output.
    pub text_size: u16,
    /// Draw the background transparent instead of black.
    pub transparent: bool,
    pub frame_count: u32,
    pub src_width: u16,
    pub src_height: u16,
    /// Small preview grid for the gallery, independent of the frame grid.
    pub cover: Grid,
}

/// Options passed to [`encode`].
#[derive(Debug, Clone)]
pub struct EncodeOptions {
    pub fps: u16,
    pub charset: Vec<u8>,
    pub color_mode: ColorMode,
    pub text_size: u16,
    pub transparent: bool,
    pub src_width: u16,
    pub src_height: u16,
    /// Preview grid. Defaults to frame 0 when `None`.
    pub cover: Option<Grid>,
}

impl Default for EncodeOptions {
    fn default() -> Self {
        EncodeOptions {
            fps: 24,
            charset: charset::default_charset(),
            color_mode: ColorMode::Mono,
            text_size: 0,
            transparent: false,
            src_width: 0,
            src_height: 0,
            cover: None,
        }
    }
}

/// Encode a sequence of same-shaped grids into a `.pscii` blob.
pub fn encode(frames: &[Grid], options: EncodeOptions) -> Result<Vec<u8>, PsciiError> {
    if frames.is_empty() {
        return Err(PsciiError::Corrupt("no frames"));
    }
    if options.charset.is_empty() {
        return Err(PsciiError::EmptyCharset);
    }
    if options.charset.len() > MAX_CHARSET {
        return Err(PsciiError::CharsetTooLarge(options.charset.len()));
    }

    let cols = frames[0].cols;
    let rows = frames[0].rows;
    let color = options.color_mode.is_color();
    for f in frames {
        if f.cols != cols || f.rows != rows {
            return Err(PsciiError::GridShape {
                cols: f.cols,
                rows: f.rows,
                expected_cols: cols,
                expected_rows: rows,
            });
        }
        if color && f.colors.as_ref().map(|c| c.len()) != Some(f.cells.len() * 3) {
            return Err(PsciiError::Corrupt("colour grid missing RGB data"));
        }
    }

    let cover = options.cover.clone().unwrap_or_else(|| frames[0].clone());

    // Build compressed blobs + offset table.
    let mut blobs: Vec<u8> = Vec::new();
    let mut table: Vec<(u32, u32)> = Vec::with_capacity(frames.len());
    let mut prev: Option<&Grid> = None;
    for (i, frame) in frames.iter().enumerate() {
        let payload = if i == 0 {
            full_payload(frame, color)
        } else {
            delta_payload(prev.expect("prev set"), frame, color)
        };
        let compressed = lz4_flex::block::compress_prepend_size(&payload);
        table.push((blobs.len() as u32, compressed.len() as u32));
        blobs.extend_from_slice(&compressed);
        prev = Some(frame);
    }

    let mut w = Vec::new();
    w.extend_from_slice(&MAGIC);
    push_u16(&mut w, VERSION);
    push_u16(&mut w, options.fps);
    push_u16(&mut w, cols);
    push_u16(&mut w, rows);
    w.push(options.charset.len() as u8);
    w.extend_from_slice(&options.charset);
    w.push(options.color_mode as u8);
    push_u16(&mut w, options.text_size);
    w.push(if options.transparent { 1 } else { 0 });
    push_u16(&mut w, options.src_width);
    push_u16(&mut w, options.src_height);
    push_u16(&mut w, cover.cols);
    push_u16(&mut w, cover.rows);
    w.extend_from_slice(&cover.cells);
    push_u32(&mut w, frames.len() as u32);
    for (offset, len) in &table {
        push_u32(&mut w, *offset);
        push_u32(&mut w, *len);
    }
    w.extend_from_slice(&blobs);
    Ok(w)
}

/// A parsed `.pscii` file. Holds the raw bytes; frames are decoded on demand.
pub struct Reader {
    data: Vec<u8>,
    blobs_start: usize,
    header: Header,
    table: Vec<(u32, u32)>,
}

impl Reader {
    pub fn parse(data: Vec<u8>) -> Result<Self, PsciiError> {
        let mut c = Cursor::new(&data);
        if c.take(4)? != MAGIC {
            return Err(PsciiError::BadMagic);
        }
        let version = c.u16()?;
        if version == 0 || version > VERSION {
            return Err(PsciiError::UnsupportedVersion(version));
        }
        let fps = c.u16()?;
        let cols = c.u16()?;
        let rows = c.u16()?;
        let charset_len = c.u8()? as usize;
        if charset_len == 0 {
            return Err(PsciiError::EmptyCharset);
        }
        let charset = c.take(charset_len)?.to_vec();
        let mut color_mode =
            ColorMode::from_u8(c.u8()?).ok_or(PsciiError::Corrupt("unknown color mode"))?;
        let (text_size, transparent) = if version >= 2 {
            let text_size = c.u16()?;
            let flags = c.u8()?;
            (text_size, flags & 1 != 0)
        } else {
            color_mode = ColorMode::Mono;
            (0, false)
        };
        let src_width = c.u16()?;
        let src_height = c.u16()?;
        let cover_cols = c.u16()?;
        let cover_rows = c.u16()?;
        let cover_cells = c.take(cover_cols as usize * cover_rows as usize)?.to_vec();
        let cover = Grid {
            cols: cover_cols,
            rows: cover_rows,
            cells: cover_cells,
            colors: None,
        };
        let frame_count = c.u32()?;
        let mut table = Vec::with_capacity(frame_count as usize);
        for _ in 0..frame_count {
            let offset = c.u32()?;
            let len = c.u32()?;
            table.push((offset, len));
        }
        let blobs_start = c.pos;

        Ok(Reader {
            header: Header {
                version,
                fps,
                cols,
                rows,
                charset,
                color_mode,
                text_size,
                transparent,
                frame_count,
                src_width,
                src_height,
                cover,
            },
            data,
            blobs_start,
            table,
        })
    }

    pub fn header(&self) -> &Header {
        &self.header
    }

    /// Decode every frame in order. Memory is `frame_count * grid`.
    pub fn decode_all(&self) -> Result<Vec<Grid>, PsciiError> {
        let n = self.header.frame_count as usize;
        let mut out = Vec::with_capacity(n);
        let mut prev: Option<Grid> = None;
        for i in 0..n {
            let grid = self.decode_frame(i, prev.as_ref())?;
            prev = Some(grid.clone());
            out.push(grid);
        }
        Ok(out)
    }

    /// Decode a single frame. `i == 0` ignores `prev`; later frames apply the
    /// delta on top of `prev` (a blank grid is used if `prev` is `None`).
    pub fn decode_frame(&self, i: usize, prev: Option<&Grid>) -> Result<Grid, PsciiError> {
        let (offset, len) = *self
            .table
            .get(i)
            .ok_or(PsciiError::FrameOutOfRange(i, self.table.len()))?;
        let start = self.blobs_start + offset as usize;
        let end = start + len as usize;
        let blob = self
            .data
            .get(start..end)
            .ok_or(PsciiError::Truncated("frame blob"))?;
        let payload = lz4_flex::block::decompress_size_prepended(blob)?;

        let cols = self.header.cols;
        let rows = self.header.rows;
        let expected = cols as usize * rows as usize;
        let color = self.header.color_mode.is_color();

        if i == 0 {
            let want = if color { expected * 4 } else { expected };
            if payload.len() != want {
                return Err(PsciiError::BadFrame);
            }
            let cells = payload[..expected].to_vec();
            let colors = if color {
                Some(payload[expected..].to_vec())
            } else {
                None
            };
            return Grid::from_cells_color(cols, rows, cells, colors);
        }

        let mut grid = match prev {
            Some(g) if g.cols == cols && g.rows == rows => g.clone(),
            _ => Grid::new(cols, rows),
        };
        if color && grid.colors.is_none() {
            grid.colors = Some(vec![0u8; expected * 3]);
        }
        apply_delta(&payload, &mut grid, color)?;
        Ok(grid)
    }
}

/// Convenience: parse and decode all frames.
pub fn decode(data: Vec<u8>) -> Result<(Header, Vec<Grid>), PsciiError> {
    let reader = Reader::parse(data)?;
    let frames = reader.decode_all()?;
    Ok((reader.header().clone(), frames))
}

// --- delta helpers -----------------------------------------------------------

fn full_payload(frame: &Grid, color: bool) -> Vec<u8> {
    let mut out = Vec::with_capacity(frame.cells.len() * if color { 4 } else { 1 });
    out.extend_from_slice(&frame.cells);
    if color {
        if let Some(c) = &frame.colors {
            out.extend_from_slice(c);
        }
    }
    out
}

fn delta_payload(prev: &Grid, cur: &Grid, color: bool) -> Vec<u8> {
    let mut changed: Vec<u8> = Vec::new();
    for i in 0..cur.cells.len() {
        let glyph_changed = prev.cells.get(i) != cur.cells.get(i);
        let color_changed = if color {
            prev.color_at(i) != cur.color_at(i)
        } else {
            false
        };
        if glyph_changed || color_changed {
            changed.extend_from_slice(&(i as u32).to_le_bytes());
            changed.push(*cur.cells.get(i).unwrap_or(&0));
            if color {
                let (r, g, b) = cur.color_at(i).unwrap_or((0, 0, 0));
                changed.push(r);
                changed.push(g);
                changed.push(b);
            }
        }
    }
    changed
}

fn apply_delta(payload: &[u8], grid: &mut Grid, color: bool) -> Result<(), PsciiError> {
    let stride = if color { 8 } else { 5 };
    if !payload.len().is_multiple_of(stride) {
        return Err(PsciiError::BadFrame);
    }
    for chunk in payload.chunks_exact(stride) {
        let idx = u32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]) as usize;
        let value = chunk[4];
        let cell = grid.cells.get_mut(idx).ok_or(PsciiError::CellOutOfRange)?;
        *cell = value;
        if color {
            if let Some(colors) = grid.colors.as_mut() {
                let o = idx * 3;
                if o + 3 > colors.len() {
                    return Err(PsciiError::CellOutOfRange);
                }
                colors[o] = chunk[5];
                colors[o + 1] = chunk[6];
                colors[o + 2] = chunk[7];
            }
        }
    }
    Ok(())
}

// --- primitives --------------------------------------------------------------

fn push_u16(buf: &mut Vec<u8>, v: u16) {
    buf.extend_from_slice(&v.to_le_bytes());
}

fn push_u32(buf: &mut Vec<u8>, v: u32) {
    buf.extend_from_slice(&v.to_le_bytes());
}

struct Cursor<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> Cursor<'a> {
    fn new(data: &'a [u8]) -> Self {
        Cursor { data, pos: 0 }
    }

    fn take(&mut self, n: usize) -> Result<&'a [u8], PsciiError> {
        let end = self
            .pos
            .checked_add(n)
            .ok_or(PsciiError::Truncated("overflow"))?;
        let slice = self
            .data
            .get(self.pos..end)
            .ok_or(PsciiError::Truncated("buffer"))?;
        self.pos = end;
        Ok(slice)
    }

    fn u8(&mut self) -> Result<u8, PsciiError> {
        Ok(self.take(1)?[0])
    }

    fn u16(&mut self) -> Result<u16, PsciiError> {
        let b = self.take(2)?;
        Ok(u16::from_le_bytes([b[0], b[1]]))
    }

    fn u32(&mut self) -> Result<u32, PsciiError> {
        let b = self.take(4)?;
        Ok(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }
}
