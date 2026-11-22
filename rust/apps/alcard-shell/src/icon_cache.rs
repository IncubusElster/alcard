//! Bounded, data-only application icons for the Shell's rendering thread.
//! Semantic Alcard icons take precedence over named assets. Unknown names
//! use a small, deterministic subset of local XDG/hicolor PNG locations.
//! This deliberately does not execute theme code, load SVG, or recurse through
//! directories. Rejected/missing files are negative-cached like valid images.
use alcard_ui::{
    icons::{draw_icon, IconKind},
    theme::Theme,
};
use std::{
    cell::RefCell,
    collections::VecDeque,
    env,
    fs::OpenOptions,
    io::{Cursor, Read},
    os::unix::{ffi::OsStrExt, fs::OpenOptionsExt},
    path::{Component, Path, PathBuf},
};
use tiny_skia::{ColorU8, FilterQuality, Pixmap, PixmapPaint, Transform};

const MAX_FILE_BYTES: usize = 256 * 1024;
const MAX_SIDE: u32 = 256;
const MAX_PIXELS: usize = MAX_SIDE as usize * MAX_SIDE as usize;
const MAX_DECODE_BYTES: usize = MAX_PIXELS * 4;
const DECODER_LIMIT: usize = 1024 * 1024;
const MAX_CHUNKS: usize = 256;
const MAX_PATH_BYTES: usize = 1024;
const MAX_NAME_BYTES: usize = 128;
const MAX_DATA_ROOTS: usize = 4;
const MAX_CANDIDATES: usize = 64;
const MAX_ENTRIES: usize = 64;
const MAX_CACHE_BYTES: usize = 512 * 1024;
const MAX_DRAW_SIZE: u32 = 64;

thread_local! {
    static CACHE: RefCell<Cache> = RefCell::new(Cache::new(SearchRoots::from_environment()));
}

/// Drop positive and negative entries after a successful configuration reload.
/// Search roots are rebuilt as well; no stale themed result is replayed.
pub fn clear() {
    CACHE.with(|cache| *cache.borrow_mut() = Cache::new(SearchRoots::from_environment()));
}

/// Draw a native semantic icon, a safe local PNG, or a semantic/generic
/// fallback. PNGs retain aspect ratio and transparent padding. Cached results
/// include their draw size; all cache entries are invalidated on theme change.
pub fn draw_application(
    canvas: &mut Pixmap,
    x: i32,
    y: i32,
    size: u32,
    icon: Option<&str>,
    desktop_id: &str,
    theme: &Theme,
) {
    if size == 0 || size > MAX_DRAW_SIZE {
        return;
    }
    if let Some(kind) = icon.and_then(IconKind::from_name) {
        draw_icon(canvas, x, y, size, kind, theme);
        return;
    }
    let loaded = icon.is_some_and(|name| {
        CACHE.with(|cache| {
            let mut cache = cache.borrow_mut();
            if let Some(image) = cache.get(name, size, theme) {
                canvas.draw_pixmap(
                    x,
                    y,
                    image.as_ref(),
                    &PixmapPaint::default(),
                    Transform::identity(),
                    None,
                );
                true
            } else {
                false
            }
        })
    });
    if !loaded {
        draw_icon(
            canvas,
            x,
            y,
            size,
            IconKind::from_name(desktop_id).unwrap_or(IconKind::Generic),
            theme,
        );
    }
}

fn valid_absolute_path(path: &Path) -> bool {
    path.is_absolute()
        && path.as_os_str().as_bytes().len() <= MAX_PATH_BYTES
        && !path
            .components()
            .any(|part| matches!(part, Component::ParentDir))
        && !path.as_os_str().as_bytes().contains(&0)
}

enum IconSource<'a> {
    Absolute(&'a Path),
    Named(&'a str),
}
impl<'a> IconSource<'a> {
    fn parse(value: &'a str) -> Option<Self> {
        if value.is_empty() || value.len() > MAX_PATH_BYTES {
            return None;
        }
        let path = Path::new(value);
        if path.is_absolute() {
            return (valid_absolute_path(path)
                && path
                    .extension()
                    .is_some_and(|ext| ext.eq_ignore_ascii_case("png")))
            .then_some(Self::Absolute(path));
        }
        if value.len() > MAX_NAME_BYTES
            || value.starts_with('.')
            || !value
                .chars()
                .all(|ch| ch.is_alphanumeric() || matches!(ch, '-' | '_' | '.'))
        {
            return None;
        }
        let extension = value.rsplit('.').next().unwrap_or_default();
        if ["svg", "svgz", "xpm", "jpg", "jpeg", "gif", "bmp", "ico"]
            .iter()
            .any(|suffix| extension.eq_ignore_ascii_case(suffix))
        {
            return None;
        }
        Some(Self::Named(value))
    }
}

#[derive(Default)]
struct SearchRoots {
    icons: Vec<PathBuf>,
    pixmaps: Vec<PathBuf>,
}
impl SearchRoots {
    fn from_environment() -> Self {
        let mut data = Vec::new();
        let home = env::var_os("HOME")
            .map(PathBuf::from)
            .filter(|path| valid_absolute_path(path));
        let user_data = env::var_os("XDG_DATA_HOME")
            .map(PathBuf::from)
            .filter(|path| valid_absolute_path(path))
            .or_else(|| home.as_ref().map(|path| path.join(".local/share")));
        if let Some(path) = user_data {
            data.push(path);
        }
        let dirs = env::var_os("XDG_DATA_DIRS")
            .filter(|dirs| !dirs.is_empty() && dirs.as_bytes().len() <= 8192)
            .unwrap_or_else(|| "/usr/local/share:/usr/share".into());
        // Even malformed environment input cannot create unbounded root lists.
        for path in env::split_paths(&dirs).take(16) {
            if data.len() == MAX_DATA_ROOTS {
                break;
            }
            if valid_absolute_path(&path) && !data.contains(&path) {
                data.push(path);
            }
        }
        let mut icons = Vec::new();
        if let Some(home) = home {
            icons.push(home.join(".icons"));
        }
        icons.extend(data.iter().map(|path| path.join("icons")));
        Self {
            icons,
            pixmaps: data.into_iter().map(|path| path.join("pixmaps")).collect(),
        }
    }

    fn candidates(&self, source: IconSource<'_>, size: u32) -> Vec<PathBuf> {
        if let IconSource::Absolute(path) = source {
            return vec![path.to_owned()];
        }
        let IconSource::Named(name) = source else {
            unreachable!()
        };
        let filename = if name.to_ascii_lowercase().ends_with(".png") {
            name.to_owned()
        } else {
            format!("{name}.png")
        };
        let native = if size < 20 { [16, 24] } else { [24, 16] };
        let mut candidates = Vec::new();
        let mut add = |path: PathBuf| {
            if candidates.len() < MAX_CANDIDATES
                && valid_absolute_path(&path)
                && !candidates.contains(&path)
            {
                candidates.push(path);
            }
        };
        // Exact 16/24 assets have priority, before any resampling fallback.
        for dimension in native {
            for root in self.icons.iter().take(MAX_DATA_ROOTS + 1) {
                for category in ["apps", "places"] {
                    add(root
                        .join(format!("hicolor/{dimension}x{dimension}/{category}"))
                        .join(&filename));
                }
            }
        }
        for root in self.icons.iter().take(MAX_DATA_ROOTS + 1) {
            add(root.join(&filename));
        }
        for root in self.pixmaps.iter().take(MAX_DATA_ROOTS) {
            add(root.join(&filename));
        }
        for dimension in [32, 48, 64, 128, 256] {
            for root in self.icons.iter().take(MAX_DATA_ROOTS + 1) {
                add(root
                    .join(format!("hicolor/{dimension}x{dimension}/apps"))
                    .join(&filename));
            }
        }
        for root in self.icons.iter().take(MAX_DATA_ROOTS + 1) {
            for category in ["categories", "devices", "status", "mimetypes", "actions"] {
                add(root
                    .join(format!("hicolor/{}x{}/{category}", native[0], native[0]))
                    .join(&filename));
            }
        }
        candidates
    }

    fn load(&self, source: IconSource<'_>, size: u32) -> Option<Pixmap> {
        self.candidates(source, size)
            .into_iter()
            .find_map(|path| read_png(&path).and_then(|image| contain(&image, size)))
    }
}

struct Entry {
    name: String,
    size: u32,
    image: Option<Pixmap>,
}
impl Entry {
    fn bytes(&self) -> usize {
        self.image.as_ref().map_or(0, |image| image.data().len())
    }
}
struct Cache {
    roots: SearchRoots,
    entries: VecDeque<Entry>,
    bytes: usize,
    theme: Option<Theme>,
}
impl Cache {
    fn new(roots: SearchRoots) -> Self {
        Self {
            roots,
            entries: VecDeque::new(),
            bytes: 0,
            theme: None,
        }
    }
    fn get(&mut self, name: &str, size: u32, theme: &Theme) -> Option<&Pixmap> {
        if size == 0 || size > MAX_DRAW_SIZE {
            return None;
        }
        let source = IconSource::parse(name)?;
        if self.theme.as_ref() != Some(theme) {
            self.entries.clear();
            self.bytes = 0;
            self.theme = Some(theme.clone());
        }
        if let Some(index) = self
            .entries
            .iter()
            .position(|entry| entry.name == name && entry.size == size)
        {
            let entry = self.entries.remove(index)?;
            self.entries.push_back(entry);
        } else {
            let entry = Entry {
                name: name.to_owned(),
                size,
                image: self.roots.load(source, size),
            };
            let bytes = entry.bytes();
            while self.entries.len() >= MAX_ENTRIES || self.bytes + bytes > MAX_CACHE_BYTES {
                let oldest = self.entries.pop_front()?;
                self.bytes -= oldest.bytes();
            }
            self.bytes += bytes;
            self.entries.push_back(entry);
        }
        self.entries.back().and_then(|entry| entry.image.as_ref())
    }
}

fn read_png(path: &Path) -> Option<Pixmap> {
    if !valid_absolute_path(path) {
        return None;
    }
    // Inspect the opened descriptor: no symlink/FIFO/device TOCTOU, and no
    // blocking FIFO open. A symlink at the final component is a fallback.
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(nix::libc::O_NONBLOCK | nix::libc::O_NOFOLLOW | nix::libc::O_CLOEXEC)
        .open(path)
        .ok()?;
    let metadata = file.metadata().ok()?;
    if !metadata.is_file() || metadata.len() < 33 || metadata.len() > MAX_FILE_BYTES as u64 {
        return None;
    }
    let mut data = Vec::with_capacity(metadata.len() as usize);
    file.take(MAX_FILE_BYTES as u64 + 1)
        .read_to_end(&mut data)
        .ok()?;
    decode_png(&data)
}

/// Validate dimensions before constructing decoder buffers. A bounded chunk
/// walk also rejects animation, missing IEND, partial chunks and trailing data.
fn png_dimensions(data: &[u8]) -> Option<(u32, u32)> {
    if data.len() < 33
        || data.len() > MAX_FILE_BYTES
        || &data[..8] != b"\x89PNG\r\n\x1a\n"
        || &data[8..16] != b"\0\0\0\rIHDR"
    {
        return None;
    }
    let width = u32::from_be_bytes(data[16..20].try_into().ok()?);
    let height = u32::from_be_bytes(data[20..24].try_into().ok()?);
    if width == 0
        || height == 0
        || width > MAX_SIDE
        || height > MAX_SIDE
        || width as usize * height as usize > MAX_PIXELS
    {
        return None;
    }
    let mut at = 8_usize;
    let mut seen_data = false;
    for _ in 0..MAX_CHUNKS {
        let header = data.get(at..at.checked_add(8)?)?;
        let length = u32::from_be_bytes(header[..4].try_into().ok()?) as usize;
        let end = at.checked_add(length)?.checked_add(12)?;
        if end > data.len() {
            return None;
        }
        match &header[4..] {
            b"acTL" | b"fcTL" | b"fdAT" => return None,
            b"IHDR" if at != 8 => return None,
            b"IDAT" => seen_data = true,
            b"IEND" => {
                return (seen_data && length == 0 && end == data.len()).then_some((width, height))
            }
            _ => {}
        }
        at = end;
    }
    None
}

fn decode_png(data: &[u8]) -> Option<Pixmap> {
    let (width, height) = png_dimensions(data)?;
    let mut decoder = png::Decoder::new_with_limits(
        Cursor::new(data),
        png::Limits {
            bytes: DECODER_LIMIT,
        },
    );
    decoder.set_ignore_text_chunk(true);
    decoder.set_ignore_iccp_chunk(true);
    decoder.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
    let mut reader = decoder.read_info().ok()?;
    if reader.info().animation_control.is_some() || reader.info().frame_control.is_some() {
        return None;
    }
    let length = reader.output_buffer_size()?;
    if length == 0 || length > MAX_DECODE_BYTES {
        return None;
    }
    let mut raw = vec![0; length];
    let frame = reader.next_frame(&mut raw).ok()?;
    if frame.width != width
        || frame.height != height
        || frame.bit_depth != png::BitDepth::Eight
        || frame.color_type == png::ColorType::Indexed
        || frame.buffer_size() != width as usize * height as usize * frame.color_type.samples()
    {
        return None;
    }
    // Validate final chunks/checksums rather than accepting a partial image.
    reader.finish().ok()?;
    let mut image = Pixmap::new(width, height)?;
    for (target, sample) in image
        .pixels_mut()
        .iter_mut()
        .zip(raw[..frame.buffer_size()].chunks_exact(frame.color_type.samples()))
    {
        let (r, g, b, a) = match frame.color_type {
            png::ColorType::Grayscale => (sample[0], sample[0], sample[0], 255),
            png::ColorType::GrayscaleAlpha => (sample[0], sample[0], sample[0], sample[1]),
            png::ColorType::Rgb => (sample[0], sample[1], sample[2], 255),
            png::ColorType::Rgba => (sample[0], sample[1], sample[2], sample[3]),
            png::ColorType::Indexed => return None,
        };
        *target = ColorU8::from_rgba(r, g, b, a).premultiply();
    }
    Some(image)
}

fn contain(source: &Pixmap, size: u32) -> Option<Pixmap> {
    if size == 0 || size > MAX_DRAW_SIZE {
        return None;
    }
    let (width, height) = (source.width(), source.height());
    let (target_width, target_height) = if width >= height {
        (size, ((height * size + width / 2) / width).max(1))
    } else {
        (((width * size + height / 2) / height).max(1), size)
    };
    let mut target = Pixmap::new(size, size)?;
    target.draw_pixmap(
        0,
        0,
        source.as_ref(),
        &PixmapPaint {
            quality: if target_width == width && target_height == height {
                FilterQuality::Nearest
            } else {
                FilterQuality::Bilinear
            },
            ..PixmapPaint::default()
        },
        Transform::from_row(
            target_width as f32 / width as f32,
            0.0,
            0.0,
            target_height as f32 / height as f32,
            ((size - target_width) / 2) as f32,
            ((size - target_height) / 2) as f32,
        ),
        None,
    );
    Some(target)
}

#[cfg(test)]
#[path = "unit_tests/icon_cache.rs"]
mod tests;
