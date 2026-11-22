//! Alcard Pixel 8x8 glyph geometry. Rasterization is exclusively tiny-skia.
//! Empty glyphs stay empty; missing characters use the bundled '?' glyph.
use std::{collections::HashMap, sync::OnceLock};
use tiny_skia::{FillRule, Paint, Path, PathBuilder, Pixmap, Rect, Transform};

fn glyphs() -> &'static HashMap<char, Option<Path>> {
    static GLYPHS: OnceLock<HashMap<char, Option<Path>>> = OnceLock::new();
    GLYPHS.get_or_init(|| {
        let mut result = HashMap::new();
        for line in
            include_str!("../../../../fonts/alcard-pixel/source/AlcardPixel-Regular.hex").lines()
        {
            let Some((code, bits)) = line.trim().split_once(':') else {
                continue;
            };
            let Ok(code) = u32::from_str_radix(code, 16) else {
                continue;
            };
            let Some(ch) = char::from_u32(code) else {
                continue;
            };
            if bits.len() != 16 || !bits.is_ascii() {
                continue;
            }
            let mut rows = [0_u8; 8];
            let mut valid = true;
            for (index, row) in rows.iter_mut().enumerate() {
                match u8::from_str_radix(&bits[index * 2..index * 2 + 2], 16) {
                    Ok(value) => *row = value,
                    Err(_) => {
                        valid = false;
                        break;
                    }
                }
            }
            if !valid {
                continue;
            }
            let mut path = PathBuilder::new();
            for (y, row) in rows.iter().enumerate() {
                let mut x = 0;
                while x < 8 {
                    if row & (0x80 >> x) == 0 {
                        x += 1;
                        continue;
                    }
                    let start = x;
                    while x < 8 && row & (0x80 >> x) != 0 {
                        x += 1;
                    }
                    // A glyph is cached as vector rectangles, not as a second
                    // handwritten rasterizer or a mutable pixel atlas.
                    path.push_rect(
                        Rect::from_xywh(start as f32, y as f32, (x - start) as f32, 1.0).unwrap(),
                    );
                }
            }
            result.insert(ch, path.finish());
        }
        result
    })
}

pub fn text(target: &mut Pixmap, x: i32, y: i32, value: &str, color: u32, max_columns: usize) {
    text_scaled(target, (x, y), value, color, max_columns, 1);
}

/// Render the same cached glyphs at a whole-pixel scale, for the Home clock.
pub fn text_scaled(
    target: &mut Pixmap,
    origin: (i32, i32),
    value: &str,
    color: u32,
    max_columns: usize,
    scale: u8,
) {
    let scale = f32::from(scale.clamp(1, 4));
    let mut paint = Paint {
        anti_alias: false,
        ..Paint::default()
    };
    paint.set_color_rgba8((color >> 16) as u8, (color >> 8) as u8, color as u8, 255);
    let glyphs = glyphs();
    for (index, ch) in value.chars().take(max_columns).enumerate() {
        if let Some(Some(path)) = glyphs.get(&ch).or_else(|| glyphs.get(&'?')) {
            target.fill_path(
                path,
                &paint,
                FillRule::Winding,
                Transform::from_row(
                    scale,
                    0.0,
                    0.0,
                    scale,
                    origin.0 as f32 + index as f32 * 8.0 * scale,
                    origin.1 as f32,
                ),
                None,
            );
        }
    }
}

pub fn centered(
    target: &mut Pixmap,
    center: i32,
    y: i32,
    value: &str,
    color: u32,
    max_columns: usize,
) {
    let count = value.chars().count().min(max_columns);
    text(
        target,
        center - count as i32 * 4,
        y,
        value,
        color,
        max_columns,
    );
}

#[cfg(test)]
#[path = "unit_tests/font.rs"]
mod tests;
