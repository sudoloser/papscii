//! Encoding pipeline: decode a video with ffmpeg, snapshot frames, convert to
//! ASCII grids and write a single `.pscii` file. Encoding happens **once**;
//! playback then only decodes grids.
//!
//! ffmpeg is driven as a subprocess (avoids GPL linking and keeps the crate
//! pure Rust). ffprobe provides source dimensions.

use std::io::Read;
use std::path::Path;
use std::process::{Command, Stdio};

use papscii_core::{charset, encode as encode_pscii, EncodeOptions, Grid, PsciiError};

/// Character cell width:height ratio for the bundled mono font (~1:2).
pub const CELL_ASPECT: f32 = 0.5;
/// Max columns for the gallery preview cover.
const COVER_MAX_COLS: u16 = 48;

#[derive(Debug, thiserror::Error)]
pub enum EncodeError {
    #[error("could not run ffmpeg/ffprobe: {0}")]
    FfmpegMissing(String),
    #[error("ffprobe could not read input: {0}")]
    Probe(String),
    #[error("ffmpeg failed: {0}")]
    Decode(String),
    #[error("input has no decodable video frames")]
    NoFrames,
    #[error("unexpected video dimensions from ffprobe: {0:?}")]
    BadDimensions(String),
    #[error(transparent)]
    Pscii(#[from] PsciiError),
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

/// User-facing encoding settings.
#[derive(Debug, Clone)]
pub struct Settings {
    pub fps: u16,
    /// Target number of character columns.
    pub density: u16,
    pub charset: Vec<u8>,
    /// Keep per-cell colour (truecolor) instead of rendering in mono.
    pub color: bool,
    /// Glyph cell height in pixels at render time; `0` fits the output.
    pub text_size: u16,
    /// Render the wallpaper background transparent instead of black.
    pub transparent: bool,
    /// Multiplier on source pixel brightness (default 1.0).
    pub brightness: f32,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            fps: 24,
            density: 80,
            charset: charset::default_charset(),
            color: false,
            text_size: 0,
            transparent: false,
            brightness: 1.0,
        }
    }
}

/// Probe `(width, height)` of the first video stream.
pub fn probe_dimensions(input: &Path) -> Result<(u32, u32), EncodeError> {
    let out = Command::new("ffprobe")
        .args([
            "-v",
            "error",
            "-select_streams",
            "v:0",
            "-show_entries",
            "stream=width,height",
            "-of",
            "csv=p=0",
        ])
        .arg(input)
        .output()
        .map_err(|e| EncodeError::FfmpegMissing(e.to_string()))?;
    if !out.status.success() {
        return Err(EncodeError::Probe(
            String::from_utf8_lossy(&out.stderr).trim().to_string(),
        ));
    }
    let text = String::from_utf8_lossy(&out.stdout);
    let line = text.lines().next().unwrap_or("");
    let mut parts = line.split(',');
    let w: u32 = parts
        .next()
        .and_then(|s| s.trim().parse().ok())
        .ok_or_else(|| EncodeError::BadDimensions(line.to_string()))?;
    let h: u32 = parts
        .next()
        .and_then(|s| s.trim().parse().ok())
        .ok_or_else(|| EncodeError::BadDimensions(line.to_string()))?;
    if w == 0 || h == 0 {
        return Err(EncodeError::BadDimensions(line.to_string()));
    }
    Ok((w, h))
}

/// Grid dimensions (`cols`, `rows`) for a source of `src_w` x `src_h`.
pub fn grid_size(density: u16, src_w: u32, src_h: u32) -> (u16, u16) {
    let cols = density.clamp(8, 512);
    let rows = ((cols as f32) * CELL_ASPECT * (src_h as f32 / src_w as f32)).round() as i64;
    (cols, rows.max(1) as u16)
}

/// Encode `input` (anything ffmpeg decodes) into `.pscii` bytes.
pub fn encode_video(input: &Path, settings: &Settings) -> Result<Vec<u8>, EncodeError> {
    if settings.charset.is_empty() {
        return Err(EncodeError::Pscii(PsciiError::EmptyCharset));
    }
    let (src_w, src_h) = probe_dimensions(input)?;
    let (cols, rows) = grid_size(settings.density, src_w, src_h);
    let color = settings.color;

    let vf = format!(
        "fps={},scale={}:{}:flags=area",
        settings.fps.max(1),
        cols,
        rows
    );
    let pix_fmt = if color { "rgb24" } else { "gray" };
    let mut child = Command::new("ffmpeg")
        .args(["-v", "error", "-nostdin", "-i"])
        .arg(input)
        .args([
            "-an", "-vf", &vf, "-pix_fmt", pix_fmt, "-f", "rawvideo", "pipe:1",
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| EncodeError::FfmpegMissing(e.to_string()))?;

    let mut raw = Vec::new();
    child
        .stdout
        .take()
        .expect("piped stdout")
        .read_to_end(&mut raw)?;

    let status = child.wait()?;
    if !status.success() {
        let mut err = String::new();
        if let Some(mut stderr) = child.stderr.take() {
            let _ = stderr.read_to_string(&mut err);
        }
        return Err(EncodeError::Decode(err.trim().to_string()));
    }

    let frame_bytes = cols as usize * rows as usize * if color { 3 } else { 1 };
    let frame_count = raw.len() / frame_bytes;
    if frame_count == 0 {
        return Err(EncodeError::NoFrames);
    }

    let mut frames = Vec::with_capacity(frame_count);
    for chunk in raw.chunks_exact(frame_bytes) {
        frames.push(if color {
            pixels_to_grid_color(chunk, cols, rows, &settings.charset, settings.brightness)
        } else {
            pixels_to_grid(chunk, cols, rows, &settings.charset, settings.brightness)
        });
    }

    let cover = downsample(&frames[0], COVER_MAX_COLS);

    let bytes = encode_pscii(
        &frames,
        EncodeOptions {
            fps: settings.fps.max(1),
            charset: settings.charset.clone(),
            color_mode: if color {
                papscii_core::ColorMode::Truecolor
            } else {
                papscii_core::ColorMode::Mono
            },
            text_size: settings.text_size,
            transparent: settings.transparent,
            src_width: src_w.min(u16::MAX as u32) as u16,
            src_height: src_h.min(u16::MAX as u32) as u16,
            cover: Some(cover),
        },
    )?;
    Ok(bytes)
}

/// Options for adjusting an existing `.pscii` wallpaper file.
#[derive(Debug, Clone, Default)]
pub struct AdjustOptions {
    /// Multiplier on brightness (e.g. 1.5 = 50% brighter, 0.8 = dimmer).
    pub brightness: Option<f32>,
    /// Override playback rate in fps.
    pub fps: Option<u16>,
    /// Cell height in pixels at render time (0 = fit screen).
    pub text_size: Option<u16>,
    /// Draw background transparent instead of black.
    pub transparent: Option<bool>,
}

/// Modify an existing `.pscii` binary in memory.
pub fn adjust_pscii(input: &[u8], options: &AdjustOptions) -> Result<Vec<u8>, EncodeError> {
    let reader = papscii_core::Reader::parse(input.to_vec())?;
    let header = reader.header().clone();
    let mut frames = reader.decode_all()?;
    if frames.is_empty() {
        return Err(EncodeError::NoFrames);
    }

    if let Some(b) = options.brightness {
        if (b - 1.0).abs() > 0.001 {
            let clen = header.charset.len();
            for frame in &mut frames {
                if let Some(colors) = &mut frame.colors {
                    for i in 0..frame.cells.len() {
                        let r = (colors[i * 3] as f32 * b).clamp(0.0, 255.0) as u8;
                        let g = (colors[i * 3 + 1] as f32 * b).clamp(0.0, 255.0) as u8;
                        let bv = (colors[i * 3 + 2] as f32 * b).clamp(0.0, 255.0) as u8;
                        colors[i * 3] = r;
                        colors[i * 3 + 1] = g;
                        colors[i * 3 + 2] = bv;
                        let lum = 0.299 * r as f32 + 0.587 * g as f32 + 0.114 * bv as f32;
                        frame.cells[i] = papscii_core::charset::level_to_index(lum / 255.0, clen);
                    }
                } else {
                    for cell in &mut frame.cells {
                        let cur_level = *cell as f32 / (clen - 1).max(1) as f32;
                        let new_level = (cur_level * b).clamp(0.0, 1.0);
                        *cell = papscii_core::charset::level_to_index(new_level, clen);
                    }
                }
            }
        }
    }

    let cover = downsample(&frames[0], COVER_MAX_COLS);
    let bytes = encode_pscii(
        &frames,
        EncodeOptions {
            fps: options.fps.unwrap_or(header.fps).max(1),
            charset: header.charset,
            color_mode: header.color_mode,
            text_size: options.text_size.unwrap_or(header.text_size),
            transparent: options.transparent.unwrap_or(header.transparent),
            src_width: header.src_width,
            src_height: header.src_height,
            cover: Some(cover),
        },
    )?;
    Ok(bytes)
}

/// Modify an existing `.pscii` file on disk. Atomically writes via a temp file.
pub fn adjust_file(
    input: &Path,
    output: &Path,
    options: &AdjustOptions,
) -> Result<(), EncodeError> {
    let data = std::fs::read(input)?;
    let modified = adjust_pscii(&data, options)?;
    let tmp = output.with_extension("tmp.pscii");
    std::fs::write(&tmp, modified)?;
    std::fs::rename(tmp, output)?;
    Ok(())
}

/// Encode `input` and write the result to `output`.
pub fn encode_to_file(input: &Path, output: &Path, settings: &Settings) -> Result<(), EncodeError> {
    let bytes = encode_video(input, settings)?;
    std::fs::write(output, bytes)?;
    Ok(())
}

/// Map a grayscale frame (one byte per pixel) to glyph indices.
fn pixels_to_grid(pixels: &[u8], cols: u16, rows: u16, charset: &[u8], brightness: f32) -> Grid {
    let adjust = (brightness - 1.0).abs() > 0.001;
    let cells: Vec<u8> = pixels
        .iter()
        .map(|&p| {
            let val = if adjust {
                (p as f32 * brightness).clamp(0.0, 255.0)
            } else {
                p as f32
            };
            charset::level_to_index(val / 255.0, charset.len())
        })
        .collect();
    Grid::from_cells(cols, rows, cells).expect("frame size matches grid")
}

/// Map an RGB frame (three bytes per pixel) to glyph indices plus per-cell colour.
fn pixels_to_grid_color(
    pixels: &[u8],
    cols: u16,
    rows: u16,
    charset: &[u8],
    brightness: f32,
) -> Grid {
    let n = cols as usize * rows as usize;
    let mut cells = Vec::with_capacity(n);
    let mut colors = Vec::with_capacity(n * 3);
    let adjust = (brightness - 1.0).abs() > 0.001;
    for px in pixels.as_chunks::<3>().0 {
        let (mut r, mut g, mut b) = (px[0], px[1], px[2]);
        if adjust {
            r = (r as f32 * brightness).clamp(0.0, 255.0) as u8;
            g = (g as f32 * brightness).clamp(0.0, 255.0) as u8;
            b = (b as f32 * brightness).clamp(0.0, 255.0) as u8;
        }
        let lum = 0.299 * r as f32 + 0.587 * g as f32 + 0.114 * b as f32;
        cells.push(charset::level_to_index(lum / 255.0, charset.len()));
        colors.extend_from_slice(&[r, g, b]);
    }
    Grid::from_cells_color(cols, rows, cells, Some(colors)).expect("frame size matches grid")
}

/// Nearest-neighbour downsample of a grid, preserving aspect, capped width.
fn downsample(grid: &Grid, max_cols: u16) -> Grid {
    let cover_cols = grid.cols.min(max_cols).max(1);
    let cover_rows = ((cover_cols as f32) * (grid.rows as f32 / grid.cols as f32))
        .round()
        .max(1.0) as u16;
    let mut cells = Vec::with_capacity(cover_cols as usize * cover_rows as usize);
    for y in 0..cover_rows {
        let sy = (y as f32 * grid.rows as f32 / cover_rows as f32) as u16;
        for x in 0..cover_cols {
            let sx = (x as f32 * grid.cols as f32 / cover_cols as f32) as u16;
            let idx = sy.min(grid.rows - 1) as usize * grid.cols as usize
                + sx.min(grid.cols - 1) as usize;
            cells.push(grid.cells[idx]);
        }
    }
    Grid::from_cells(cover_cols, cover_rows, cells).expect("cover grid size")
}

#[cfg(test)]
mod tests {
    use super::*;
    use papscii_core::decode;

    fn ffmpeg_available() -> bool {
        Command::new("ffmpeg")
            .arg("-version")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    }

    #[test]
    fn grid_size_keeps_aspect() {
        // 16:9 source -> rows ~ cols * 0.5 * 9/16
        let (cols, rows) = grid_size(80, 1920, 1080);
        assert_eq!(cols, 80);
        assert_eq!(rows, 23);
    }

    #[test]
    fn end_to_end_from_synthetic_video() {
        if !ffmpeg_available() {
            eprintln!("skipping: ffmpeg not available");
            return;
        }
        let dir = std::env::temp_dir();
        let video = dir.join("papscii_test_in.mp4");
        let out = dir.join("papscii_test_out.pscii");

        let gen = Command::new("ffmpeg")
            .args([
                "-v",
                "error",
                "-y",
                "-f",
                "lavfi",
                "-i",
                "testsrc=size=160x120:rate=10:duration=1",
                "-pix_fmt",
                "yuv420p",
            ])
            .arg(&video)
            .status()
            .expect("run ffmpeg");
        assert!(gen.success(), "ffmpeg failed to generate test video");

        let mut settings = Settings::default();
        settings.fps = 10;
        settings.density = 40;

        encode_to_file(&video, &out, &settings).expect("encode");
        let bytes = std::fs::read(&out).unwrap();
        let (header, frames) = decode(bytes).unwrap();

        assert_eq!(header.fps, 10);
        assert_eq!(header.cols, 40);
        assert!(
            frames.len() >= 8,
            "expected ~10 frames, got {}",
            frames.len()
        );
        assert!(!header.cover.cells.is_empty());

        let _ = std::fs::remove_file(&video);
        let _ = std::fs::remove_file(&out);
    }

    #[test]
    fn end_to_end_color() {
        if !ffmpeg_available() {
            eprintln!("skipping: ffmpeg not available");
            return;
        }
        let dir = std::env::temp_dir();
        let video = dir.join("papscii_color_in.mp4");
        let out = dir.join("papscii_color_out.pscii");

        let gen = Command::new("ffmpeg")
            .args([
                "-v",
                "error",
                "-y",
                "-f",
                "lavfi",
                "-i",
                "testsrc2=size=160x120:rate=8:duration=0.5",
                "-pix_fmt",
                "yuv420p",
            ])
            .arg(&video)
            .status()
            .expect("run ffmpeg");
        assert!(gen.success(), "ffmpeg failed to generate test video");

        let settings = Settings {
            fps: 8,
            density: 32,
            color: true,
            text_size: 12,
            transparent: true,
            ..Settings::default()
        };

        encode_to_file(&video, &out, &settings).expect("encode color");
        let (header, frames) = decode(std::fs::read(&out).unwrap()).unwrap();

        assert_eq!(header.color_mode, papscii_core::ColorMode::Truecolor);
        assert_eq!(header.text_size, 12);
        assert!(header.transparent);
        assert!(frames[0].colors.is_some(), "colour grid missing");
        assert_eq!(
            frames[0].colors.as_ref().unwrap().len(),
            frames[0].cells.len() * 3
        );

        let _ = std::fs::remove_file(&video);
        let _ = std::fs::remove_file(&out);
    }

    #[test]
    fn test_adjust_pscii() {
        let grid = Grid::from_cells_color(
            4,
            2,
            vec![0, 1, 2, 3, 4, 5, 6, 7],
            Some(vec![10, 20, 30].repeat(8)),
        )
        .unwrap();
        let bytes = papscii_core::encode(
            &[grid],
            EncodeOptions {
                fps: 10,
                charset: b" .:-=+*#%@".to_vec(),
                color_mode: papscii_core::ColorMode::Truecolor,
                text_size: 0,
                transparent: false,
                src_width: 4,
                src_height: 2,
                cover: None,
            },
        )
        .unwrap();

        let adjusted = adjust_pscii(
            &bytes,
            &AdjustOptions {
                brightness: Some(2.0),
                fps: Some(15),
                text_size: Some(16),
                transparent: Some(true),
            },
        )
        .unwrap();

        let (header, frames) = decode(adjusted).unwrap();
        assert_eq!(header.fps, 15);
        assert_eq!(header.text_size, 16);
        assert!(header.transparent);
        let colors = frames[0].colors.as_ref().unwrap();
        assert_eq!(colors[0], 20); // 10 * 2.0
        assert_eq!(colors[1], 40); // 20 * 2.0
        assert_eq!(colors[2], 60); // 30 * 2.0
    }
}
