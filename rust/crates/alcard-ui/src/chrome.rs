//! Shared, compact presentation primitives. All geometry is rasterized by tiny-skia.
//! The two-pixel corners are deliberately small at the native 320×170 resolution.
use crate::theme::Theme;
use tiny_skia::{
    Color, FillRule, GradientStop, LinearGradient, Paint, Path, PathBuilder, Pixmap, Point,
    SpreadMode, Transform,
};

pub const CORNER_RADIUS: i32 = 2;
pub type Bounds = (i32, i32, i32, i32);

pub fn rgb(value: u32) -> Color {
    Color::from_rgba8((value >> 16) as u8, (value >> 8) as u8, value as u8, 255)
}

/// Integer RGB mixing for small, theme-derived highlights, not fixed-color cutouts.
pub fn mix(base: u32, other: u32, percent: u8) -> u32 {
    let amount = u32::from(percent.min(100));
    let channel = |shift: u32| {
        ((((base >> shift) & 255) * (100 - amount) + ((other >> shift) & 255) * amount) / 100)
            << shift
    };
    channel(16) | channel(8) | channel(0)
}

fn path((x, y, width, height): Bounds, radius: i32) -> Option<Path> {
    if width <= 0 || height <= 0 {
        return None;
    }
    let radius = radius.max(0).min(width / 2).min(height / 2) as f32;
    let (x, y, right, bottom) = (x as f32, y as f32, (x + width) as f32, (y + height) as f32);
    let mut path = PathBuilder::new();
    path.move_to(x + radius, y);
    path.line_to(right - radius, y);
    path.quad_to(right, y, right, y + radius);
    path.line_to(right, bottom - radius);
    path.quad_to(right, bottom, right - radius, bottom);
    path.line_to(x + radius, bottom);
    path.quad_to(x, bottom, x, bottom - radius);
    path.line_to(x, y + radius);
    path.quad_to(x, y, x + radius, y);
    path.close();
    path.finish()
}

pub fn rounded(image: &mut Pixmap, bounds: Bounds, radius: i32, color: u32) {
    let Some(path) = path(bounds, radius) else {
        return;
    };
    let mut paint = Paint {
        anti_alias: false,
        ..Paint::default()
    };
    paint.set_color(rgb(color));
    image.fill_path(
        &path,
        &paint,
        FillRule::Winding,
        Transform::identity(),
        None,
    );
}

pub fn gradient(image: &mut Pixmap, bounds: Bounds, radius: i32, top: u32, bottom: u32) {
    let Some(path) = path(bounds, radius) else {
        return;
    };
    let Some(shader) = LinearGradient::new(
        Point::from_xy(bounds.0 as f32, bounds.1 as f32),
        Point::from_xy(
            bounds.0 as f32,
            (bounds.1 + bounds.3 - 1).max(bounds.1 + 1) as f32,
        ),
        vec![
            GradientStop::new(0.0, rgb(top)),
            GradientStop::new(1.0, rgb(bottom)),
        ],
        SpreadMode::Pad,
        Transform::identity(),
    ) else {
        return;
    };
    let paint = Paint {
        anti_alias: false,
        shader,
        ..Paint::default()
    };
    image.fill_path(
        &path,
        &paint,
        FillRule::Winding,
        Transform::identity(),
        None,
    );
}

pub fn bar(image: &mut Pixmap, bounds: Bounds, theme: &Theme) {
    gradient(
        image,
        bounds,
        0,
        mix(theme.surface, theme.text, 13),
        theme.surface,
    );
}

pub fn panel(image: &mut Pixmap, bounds: Bounds, fill: u32, border: u32) {
    rounded(image, bounds, CORNER_RADIUS, border);
    rounded(
        image,
        (bounds.0 + 1, bounds.1 + 1, bounds.2 - 2, bounds.3 - 2),
        1,
        fill,
    );
}

pub fn selection(image: &mut Pixmap, bounds: Bounds, theme: &Theme) {
    rounded(image, bounds, CORNER_RADIUS, theme.accent);
    gradient(
        image,
        (bounds.0 + 1, bounds.1 + 1, bounds.2 - 2, bounds.3 - 2),
        1,
        mix(theme.selected, theme.text, 13),
        theme.selected,
    );
}

pub fn shadow(image: &mut Pixmap, bounds: Bounds) {
    let Some(path) = path(
        (bounds.0 + 2, bounds.1 + 2, bounds.2, bounds.3),
        CORNER_RADIUS,
    ) else {
        return;
    };
    let mut paint = Paint {
        anti_alias: false,
        ..Paint::default()
    };
    paint.set_color_rgba8(0, 0, 0, 96);
    image.fill_path(
        &path,
        &paint,
        FillRule::Winding,
        Transform::identity(),
        None,
    );
}
