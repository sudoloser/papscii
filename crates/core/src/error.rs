//! Error type for `.pscii` parsing and encoding.

use thiserror::Error;

#[derive(Debug, Error)]
pub enum PsciiError {
    #[error("not a papscii file: bad magic")]
    BadMagic,

    #[error("unsupported .pscii version: {0}")]
    UnsupportedVersion(u16),

    #[error("unexpected end of data while reading {0}")]
    Truncated(&'static str),

    #[error("charset is empty")]
    EmptyCharset,

    #[error("charset has more than 256 glyphs ({0})")]
    CharsetTooLarge(usize),

    #[error("grid is {cols}x{rows} but expected {expected_cols}x{expected_rows}")]
    GridShape {
        cols: u16,
        rows: u16,
        expected_cols: u16,
        expected_rows: u16,
    },

    #[error("grid cell index out of range")]
    CellOutOfRange,

    #[error("frame index {0} out of range (have {1})")]
    FrameOutOfRange(usize, usize),

    #[error("frame payload is malformed")]
    BadFrame,

    #[error("corrupt .pscii: {0}")]
    Corrupt(&'static str),

    #[error("lz4: {0}")]
    Lz4(#[from] lz4_flex::block::DecompressError),

    #[error("io: {0}")]
    Io(#[from] std::io::Error),
}
