# The `.pscii` format

Papscii animations are encoded **once**, at creation time, from a video source
into a single `.pscii` file. The file stores **ASCII grids**, never pixels and
never video, so playback is only lz4 delta-decode plus glyph rasterization.

All integers are little-endian.

## Layout

| field        | type                    | notes                                   |
| ------------ | ----------------------- | --------------------------------------- |
| magic        | `[u8; 4]`               | `b"PSCI"`                               |
| version      | `u16`                   | currently `2`                           |
| fps          | `u16`                   | playback frames per second              |
| cols, rows   | `u16`                   | frame grid size (characters)            |
| charset_len  | `u8`                    | 1..=256                                 |
| charset      | `[u8; charset_len]`     | glyphs, index 0 = darkest               |
| color_mode   | `u8`                    | 0 = mono, 2 = truecolor                 |
| text_size    | `u16`                   | glyph cell height in px; 0 = fit screen |
| flags        | `u8`                    | bit 0 = transparent background          |
| src_width    | `u16`                   | original video width                    |
| src_height   | `u16`                   | original video height                   |
| cover_cols   | `u16`                   | preview grid width                      |
| cover_rows   | `u16`                   | preview grid height                     |
| cover_cells  | `[u8; cc*cr]`           | preview grid (mono), for the gallery    |
| frame_count  | `u32`                   | number of frames                        |
| frame_table  | `[{u32,u32}; count]`    | `{ offset, compressed_len }` per frame  |
| frame_blobs  | bytes                   | concatenated compressed frame payloads  |

`offset` is relative to the start of `frame_blobs`.

Version `1` files (mono, no text size/transparency) are still read: the extra
`text_size`/`flags` fields are absent and `color_mode` is forced to mono.

## Frame payloads

Each frame payload is lz4-compressed independently (lz4_flex
`compress_prepend_size`, i.e. the uncompressed size is stored in the block
header). This lets any frame be seeked without touching the rest of the file.

**Mono** (`color_mode = 0`):

- Frame 0 is the full grid: `cols * rows` glyph bytes, row-major.
- Frame N > 0 is a **delta**: a run of `{ index: u32, value: u8 }` pairs
  (5 bytes each) for the cells that changed.

**Truecolor** (`color_mode = 2`):

- Frame 0 is `cols * rows` glyph bytes followed by `cols * rows * 3` RGB bytes
  (one per cell, same order).
- Frame N > 0 is a delta of `{ index: u32, value: u8, r: u8, g: u8, b: u8 }`
  entries (8 bytes each) for cells whose glyph **or** colour changed.

Playback decodes sequentially (apply frame 0, then each delta) and loops. Only
changed cells are drawn, matching the delta encoding.

## Render options

`text_size` and the `flags` transparent bit are per-wallpaper render hints:

- `text_size` is the glyph cell height in pixels. `0` fits the whole output
  surface; a positive value draws the grid at that size and centres it, scaling
  down only if it would overflow. Smaller text => smaller rendered wallpaper.
- `transparent` leaves the background un-painted (alpha 0) so whatever is behind
  the surface shows through, instead of painting it black.
