use papscii_core::{charset, decode, encode, EncodeOptions, Grid, PsciiError};

fn grid(cols: u16, rows: u16, fill: impl Fn(usize) -> u8) -> Grid {
    let n = cols as usize * rows as usize;
    let cells: Vec<u8> = (0..n).map(fill).collect();
    Grid::from_cells(cols, rows, cells).unwrap()
}

fn opts() -> EncodeOptions {
    EncodeOptions {
        fps: 24,
        charset: charset::default_charset(),
        ..Default::default()
    }
}

#[test]
fn roundtrip_multiple_frames() {
    let f0 = grid(8, 4, |i| (i % 10) as u8);
    let f1 = grid(8, 4, |i| ((i + 3) % 10) as u8);
    let f2 = grid(8, 4, |i| ((i + 6) % 10) as u8);
    let frames = vec![f0, f1, f2];

    let bytes = encode(&frames, opts()).unwrap();
    let (header, decoded) = decode(bytes).unwrap();

    assert_eq!(header.frame_count, 3);
    assert_eq!(header.cols, 8);
    assert_eq!(header.rows, 4);
    assert_eq!(header.fps, 24);
    assert_eq!(decoded, frames);
}

#[test]
fn delta_frames_are_smaller_than_full() {
    // A second frame that changes a single cell should compress to a handful
    // of bytes, far less than a full grid.
    let a = grid(64, 64, |i| (i % 7) as u8);
    let mut b = a.clone();
    b.cells[0] = 9;

    let one = encode(&[a.clone()], opts()).unwrap();
    let two = encode(&[a, b], opts()).unwrap();

    // Adding one near-identical frame should add well under a full grid.
    assert!(
        two.len() < one.len() + (64 * 64),
        "delta frame too large: one={} two={}",
        one.len(),
        two.len()
    );
}

#[test]
fn cover_defaults_to_first_frame() {
    let f0 = grid(4, 4, |i| (i % 10) as u8);
    let bytes = encode(&[f0.clone()], opts()).unwrap();
    let (header, _) = decode(bytes).unwrap();
    assert_eq!(header.cover, f0);
}

#[test]
fn rejects_bad_magic() {
    let err = decode(b"NOPE".to_vec()).unwrap_err();
    assert!(matches!(
        err,
        PsciiError::Truncated(_) | PsciiError::BadMagic
    ));
}

#[test]
fn rejects_mismatched_shapes() {
    let a = grid(4, 4, |_| 0);
    let b = grid(4, 5, |_| 0);
    let err = encode(&[a, b], opts()).unwrap_err();
    assert!(matches!(err, PsciiError::GridShape { .. }));
}

#[test]
fn roundtrip_truecolor() {
    let n = 8usize * 4;
    let cells: Vec<u8> = (0..n).map(|i| (i % 10) as u8).collect();
    let colors: Vec<u8> = (0..n * 3).map(|i| (i % 251) as u8).collect();
    let f0 = Grid::from_cells_color(8, 4, cells.clone(), Some(colors.clone())).unwrap();

    let mut cells1 = cells.clone();
    cells1[5] = 9;
    let mut colors1 = colors.clone();
    colors1[15] = 200;
    let f1 = Grid::from_cells_color(8, 4, cells1, Some(colors1)).unwrap();

    let options = EncodeOptions {
        color_mode: papscii_core::ColorMode::Truecolor,
        text_size: 10,
        transparent: true,
        ..opts()
    };
    let bytes = encode(&[f0.clone(), f1.clone()], options).unwrap();
    let (header, decoded) = decode(bytes).unwrap();

    assert_eq!(header.color_mode, papscii_core::ColorMode::Truecolor);
    assert_eq!(header.text_size, 10);
    assert!(header.transparent);
    assert_eq!(decoded, vec![f0, f1]);
}
