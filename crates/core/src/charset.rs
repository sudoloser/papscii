//! Character ramps used to map luminance to glyphs.

/// Brightness ramp from darkest to lightest, used when converting a source
/// image luminance in `0.0..=1.0` to a glyph index in the file charset.
pub const DEFAULT_RAMP: &[u8] = b" .:-=+*#%@";

/// The charset stored in a `.pscii` file is exactly the glyph set the encoder
/// used. Most wallpapers only need the default ramp.
pub fn default_charset() -> Vec<u8> {
    DEFAULT_RAMP.to_vec()
}

/// Map a normalized luminance (`0.0` dark .. `1.0` bright) to a glyph index in
/// `charset.len()`. Index `0` is used for the darkest value.
pub fn level_to_index(level: f32, charset_len: usize) -> u8 {
    if charset_len <= 1 {
        return 0;
    }
    let clamped = level.clamp(0.0, 1.0);
    let idx = (clamped * (charset_len as f32 - 1.0)).round() as usize;
    idx.min(charset_len - 1) as u8
}
