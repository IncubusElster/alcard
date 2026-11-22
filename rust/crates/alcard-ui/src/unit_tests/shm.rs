use super::*;

#[test]
fn rgba_is_converted_to_native_argb_without_losing_premultiplication() {
    let source = Pixmap::from_vec(
        vec![128, 32, 16, 128, 0, 0, 255, 255],
        tiny_skia::IntSize::from_wh(2, 1).unwrap(),
    )
    .unwrap();
    let mut out = [0_u8; 8];
    copy_argb(&source, &mut out).unwrap();
    assert_eq!(u32::from_ne_bytes(out[..4].try_into().unwrap()), 0x80802010);
    assert_eq!(u32::from_ne_bytes(out[4..].try_into().unwrap()), 0xff0000ff);
}

#[test]
fn buffer_bounds_reject_zero_negative_scale_overflow_and_excessive_memory() {
    assert_eq!(dimensions((320, 170), 2).unwrap(), (640, 340, 2560));
    for (size, scale) in [
        ((0, 170), 1),
        ((320, 170), 0),
        ((320, 170), -1),
        ((u32::MAX, 170), 2),
        ((4096, 4096), 1),
    ] {
        assert!(dimensions(size, scale).is_err());
    }
}

#[test]
fn transport_rejects_mismatched_lengths_and_preserves_transparency() {
    let source = Pixmap::new(1, 1).unwrap();
    assert!(copy_argb(&source, &mut [0; 3]).is_err());
    let mut out = [255; 4];
    copy_argb(&source, &mut out).unwrap();
    assert_eq!(out, [0; 4]);
}

#[test]
fn nearest_scaling_is_done_by_tiny_skia_without_blurring() {
    let source = Pixmap::from_vec(
        vec![255, 0, 0, 255, 0, 0, 255, 255],
        tiny_skia::IntSize::from_wh(2, 1).unwrap(),
    )
    .unwrap();
    let mut target = Pixmap::new(4, 2).unwrap();
    scale_image(&source, &mut target);
    for y in 0..2 {
        assert_eq!(target.pixel(0, y), source.pixel(0, 0));
        assert_eq!(target.pixel(1, y), source.pixel(0, 0));
        assert_eq!(target.pixel(2, y), source.pixel(1, 0));
        assert_eq!(target.pixel(3, y), source.pixel(1, 0));
    }
    // A square configure must not stretch the two source pixels vertically.
    let mut square = Pixmap::new(5, 5).unwrap();
    scale_image(&source, &mut square);
    assert_eq!(
        square
            .pixels()
            .iter()
            .filter(|pixel| pixel.blue() == 255)
            .count(),
        4
    );
    assert_eq!(square.pixel(0, 0).unwrap().alpha(), 255);
}
