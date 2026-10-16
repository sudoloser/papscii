//! Papscii core: the `.pscii` container format for animated ASCII wallpapers.
//!
//! A `.pscii` file stores an animation as a sequence of **ASCII grids**, not
//! pixels and not video. Encoding happens once (video -> grids), so playback is
//! only lz4 delta-decode plus glyph rasterization. See `docs/pscii-format.md`.

pub mod charset;
pub mod config;
pub mod error;
pub mod format;
pub mod ipc;
pub mod paths;

pub use error::PsciiError;
pub use format::{decode, encode, ColorMode, EncodeOptions, Grid, Header, Reader};
