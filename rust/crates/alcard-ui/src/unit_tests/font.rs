use super::*;
use tiny_skia::Color;

#[test]
fn spaces_are_not_missing_glyph_placeholders() {
    let mut image = Pixmap::new(16, 8).unwrap();
    text(&mut image, 0, 0, "  ", 0xffffff, 2);
    assert!(image.data().iter().all(|byte| *byte == 0));
}

#[test]
fn latin_cyrillic_and_spanish_use_bundled_geometry() {
    for ch in "AЯñ".chars() {
        assert!(
            glyphs().get(&ch).is_some_and(Option::is_some),
            "missing {ch}"
        );
    }
}

#[test]
fn glyphs_are_crisp_clipped_and_do_not_blur() {
    let mut image = Pixmap::new(8, 8).unwrap();
    image.fill(Color::BLACK);
    text(&mut image, -2, -1, "A", 0xffffff, 1);
    assert!(image.pixels().iter().any(|p| p.red() == 255));
    assert!(image
        .pixels()
        .iter()
        .all(|p| p.red() == 0 || p.red() == 255));
}
