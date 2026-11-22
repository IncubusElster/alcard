use super::*;
use std::{
    ffi::CString,
    fs,
    os::unix::fs::symlink,
    sync::{
        atomic::{AtomicU64, Ordering},
        mpsc,
    },
    thread,
    time::Duration,
};

struct Temp(PathBuf);
impl Temp {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let path = env::temp_dir().join(format!(
            "alcard-icons-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed),
        ));
        fs::create_dir(&path).unwrap();
        Self(path)
    }
    fn roots(&self) -> SearchRoots {
        SearchRoots {
            icons: vec![self.0.join("icons")],
            pixmaps: vec![self.0.join("pixmaps")],
        }
    }
    fn write(&self, relative: &str, bytes: &[u8]) -> PathBuf {
        let path = self.0.join(relative);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, bytes).unwrap();
        path
    }
}
impl Drop for Temp {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn rgba(width: u32, height: u32, color: [u8; 4]) -> Vec<u8> {
    let mut data = Vec::new();
    let mut encoder = png::Encoder::new(&mut data, width, height);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder.write_header().unwrap();
    writer
        .write_image_data(&color.repeat(width as usize * height as usize))
        .unwrap();
    writer.finish().unwrap();
    data
}

fn chunk(kind: &[u8; 4], payload: &[u8]) -> Vec<u8> {
    // Small test-fixture CRC, so malformed-input tests do not depend on
    // a new production dependency or on disabling PNG checksums.
    let mut crc = !0_u32;
    for byte in kind.iter().chain(payload) {
        crc ^= u32::from(*byte);
        for _ in 0..8 {
            crc = (crc >> 1) ^ (0xedb8_8320_u32 & 0_u32.wrapping_sub(crc & 1));
        }
    }
    let mut data = Vec::new();
    data.extend_from_slice(&(payload.len() as u32).to_be_bytes());
    data.extend_from_slice(kind);
    data.extend_from_slice(payload);
    data.extend_from_slice(&(!crc).to_be_bytes());
    data
}

#[test]
fn rejects_truncation_bad_checksums_oversized_dimensions_animation_and_chunk_flood() {
    let valid = rgba(2, 1, [23, 85, 180, 255]);
    assert!(decode_png(&valid).is_some());
    for length in 0..valid.len() {
        assert!(
            decode_png(&valid[..length]).is_none(),
            "accepted partial PNG of {length} bytes"
        );
    }
    let mut corrupt = valid.clone();
    corrupt[31] ^= 1;
    assert!(decode_png(&corrupt).is_none());
    for (width, height) in [
        (0, 1),
        (1, 0),
        (MAX_SIDE + 1, 1),
        (1, MAX_SIDE + 1),
        (u32::MAX, u32::MAX),
    ] {
        let mut header = valid[16..29].to_vec();
        header[..4].copy_from_slice(&width.to_be_bytes());
        header[4..8].copy_from_slice(&height.to_be_bytes());
        let mut oversized = valid[..8].to_vec();
        oversized.extend_from_slice(&chunk(b"IHDR", &header));
        oversized.extend_from_slice(&valid[33..]);
        assert!(decode_png(&oversized).is_none());
    }
    let mut trailing = valid.clone();
    trailing.push(0);
    assert!(decode_png(&trailing).is_none());
    assert!(decode_png(&vec![0; MAX_FILE_BYTES + 1]).is_none());

    let mut animated = Vec::new();
    let mut encoder = png::Encoder::new(&mut animated, 1, 1);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    encoder.set_animated(1, 0).unwrap();
    let mut writer = encoder.write_header().unwrap();
    writer.write_image_data(&[1, 2, 3, 255]).unwrap();
    writer.finish().unwrap();
    assert!(decode_png(&animated).is_none());

    let mut flooded = valid[..33].to_vec();
    for _ in 0..MAX_CHUNKS {
        flooded.extend_from_slice(&chunk(b"npAd", &[]));
    }
    flooded.extend_from_slice(&valid[33..]);
    assert!(png_dimensions(&flooded).is_none());
}

#[test]
fn alpha_palette_grayscale_and_aspect_are_preserved() {
    let image = decode_png(&rgba(8, 4, [200, 100, 50, 128])).unwrap();
    let pixel = image.pixels()[0];
    assert_eq!(
        (pixel.red(), pixel.green(), pixel.blue(), pixel.alpha()),
        (100, 50, 25, 128)
    );
    let scaled = contain(&image, 16).unwrap();
    for y in 0..16 {
        for x in 0..16 {
            let pixel = scaled.pixels()[y * 16 + x];
            assert_eq!(pixel.alpha(), if (4..12).contains(&y) { 128 } else { 0 });
            assert!(
                pixel.red() <= pixel.alpha()
                    && pixel.green() <= pixel.alpha()
                    && pixel.blue() <= pixel.alpha()
            );
        }
    }

    let mut indexed = Vec::new();
    let mut encoder = png::Encoder::new(&mut indexed, 2, 1);
    encoder.set_color(png::ColorType::Indexed);
    encoder.set_depth(png::BitDepth::Eight);
    encoder.set_palette(vec![250, 10, 30, 20, 200, 100]);
    encoder.set_trns(vec![0, 255]);
    let mut writer = encoder.write_header().unwrap();
    writer.write_image_data(&[0, 1]).unwrap();
    writer.finish().unwrap();
    let image = decode_png(&indexed).unwrap();
    assert_eq!((image.pixels()[0].red(), image.pixels()[0].alpha()), (0, 0));
    assert_eq!(
        (image.pixels()[1].green(), image.pixels()[1].alpha()),
        (200, 255)
    );

    let mut gray = Vec::new();
    let mut encoder = png::Encoder::new(&mut gray, 1, 1);
    encoder.set_color(png::ColorType::Grayscale);
    encoder.set_depth(png::BitDepth::Sixteen);
    let mut writer = encoder.write_header().unwrap();
    writer.write_image_data(&[0x80, 0]).unwrap();
    writer.finish().unwrap();
    let pixel = decode_png(&gray).unwrap().pixels()[0];
    assert_eq!(
        (pixel.red(), pixel.green(), pixel.blue(), pixel.alpha()),
        (128, 128, 128, 255)
    );
}

#[test]
fn compressed_text_is_not_expanded_into_icon_memory() {
    let mut data = Vec::new();
    let mut encoder = png::Encoder::new(&mut data, 1, 1);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    encoder
        .add_ztxt_chunk("ignored".into(), "x".repeat(DECODER_LIMIT * 2))
        .unwrap();
    let mut writer = encoder.write_header().unwrap();
    writer.write_image_data(&[1, 2, 3, 255]).unwrap();
    writer.finish().unwrap();
    assert!(data.len() < MAX_FILE_BYTES);
    let image = decode_png(&data).unwrap();
    assert_eq!((image.width(), image.height()), (1, 1));
}

#[test]
fn paths_files_and_fifo_are_rejected_without_blocking() {
    for name in [
        "",
        "../escape",
        "icons/entry",
        "./entry",
        "https://host/icon.png",
        "file:///icon.png",
        "/tmp/../icon.png",
        "thing.svg",
        "thing.svgz",
        "a\0b",
        ".hidden",
    ] {
        assert!(IconSource::parse(name).is_none(), "{name:?}");
    }
    assert!(IconSource::parse(&"n".repeat(MAX_NAME_BYTES + 1)).is_none());
    assert!(IconSource::parse(&format!("/{}.png", "n".repeat(MAX_PATH_BYTES))).is_none());
    let temp = Temp::new();
    let valid = temp.write("valid.png", &rgba(1, 1, [1, 2, 3, 255]));
    assert!(read_png(&valid).is_some());
    let linked = temp.0.join("link.png");
    symlink(&valid, &linked).unwrap();
    assert!(read_png(&linked).is_none());
    assert!(read_png(&temp.0).is_none());
    let huge = temp.0.join("huge.png");
    fs::File::create(&huge)
        .unwrap()
        .set_len(MAX_FILE_BYTES as u64 + 1)
        .unwrap();
    assert!(read_png(&huge).is_none());

    let fifo = temp.0.join("pipe.png");
    let c_path = CString::new(fifo.as_os_str().as_bytes()).unwrap();
    // SAFETY: c_path is a valid, NUL-terminated temporary path; mkfifo
    // creates only this test's FIFO with owner-only permissions.
    assert_eq!(unsafe { nix::libc::mkfifo(c_path.as_ptr(), 0o600) }, 0);
    let (send, receive) = mpsc::channel();
    thread::spawn(move || {
        let _ = send.send(read_png(&fifo).is_none());
    });
    assert!(receive
        .recv_timeout(Duration::from_secs(2))
        .expect("icon FIFO open blocked"));
}

#[test]
fn native_size_lookup_negative_cache_and_theme_invalidation_are_real() {
    let temp = Temp::new();
    temp.write(
        "icons/hicolor/16x16/apps/unusual.png",
        &rgba(16, 16, [255, 0, 0, 255]),
    );
    temp.write(
        "icons/hicolor/24x24/apps/unusual.png",
        &rgba(24, 24, [0, 0, 255, 255]),
    );
    temp.write("pixmaps/unusual.png", &rgba(32, 32, [0, 255, 0, 255]));
    let theme = Theme::classic();
    let mut cache = Cache::new(temp.roots());
    assert_eq!(
        cache.get("unusual", 16, &theme).unwrap().pixels()[0].red(),
        255
    );
    assert_eq!(
        cache.get("unusual", 24, &theme).unwrap().pixels()[0].blue(),
        255
    );

    assert!(cache.get("appears-later", 24, &theme).is_none());
    let path = temp.write(
        "pixmaps/appears-later.png",
        &rgba(24, 24, [90, 180, 40, 255]),
    );
    assert!(
        cache.get("appears-later", 24, &theme).is_none(),
        "negative cache did a fresh lookup"
    );
    let mut changed = theme.clone();
    changed.accent ^= 1;
    assert!(cache.get("appears-later", 24, &changed).is_some());
    fs::remove_file(path).unwrap();
    assert!(
        cache.get("appears-later", 24, &changed).is_some(),
        "positive cache reloaded the removed file"
    );
    assert!(
        cache.get("appears-later", 24, &theme).is_none(),
        "theme change kept an old image"
    );
}

#[test]
fn cache_entries_bytes_keys_and_lookup_candidates_stay_bounded() {
    let temp = Temp::new();
    let theme = Theme::classic();
    let mut cache = Cache::new(temp.roots());
    for index in 0..MAX_ENTRIES + 20 {
        assert!(cache.get(&format!("missing-{index}"), 24, &theme).is_none());
        assert!(cache.entries.len() <= MAX_ENTRIES);
    }
    assert_eq!(cache.bytes, 0);
    let valid = temp.write("pixmaps/base.png", &rgba(1, 1, [2, 30, 200, 255]));
    for index in 0..MAX_ENTRIES {
        fs::hard_link(&valid, temp.0.join(format!("pixmaps/positive-{index}.png"))).unwrap();
        assert!(cache
            .get(&format!("positive-{index}"), MAX_DRAW_SIZE, &theme)
            .is_some());
        assert!(cache.entries.len() <= MAX_ENTRIES);
        assert!(cache.bytes <= MAX_CACHE_BYTES);
    }
    assert_eq!(cache.bytes, MAX_CACHE_BYTES);
    assert!(cache
        .entries
        .iter()
        .any(|entry| entry.name == format!("positive-{}", MAX_ENTRIES - 1)));
    assert!(!cache.entries.iter().any(|entry| entry.name == "positive-0"));
    let count = cache.entries.len();
    assert!(cache
        .get(&"n".repeat(MAX_PATH_BYTES * 2), 24, &theme)
        .is_none());
    assert!(cache.get("valid", MAX_DRAW_SIZE + 1, &theme).is_none());
    assert_eq!(cache.entries.len(), count);

    let many_roots = SearchRoots {
        icons: (0..100)
            .map(|index| temp.0.join(format!("icons-{index}")))
            .collect(),
        pixmaps: (0..100)
            .map(|index| temp.0.join(format!("pixmaps-{index}")))
            .collect(),
    };
    let candidates = many_roots.candidates(IconSource::parse("unusual").unwrap(), 24);
    assert!(candidates.len() <= MAX_CANDIDATES);
    assert!(candidates.iter().all(|path| path.starts_with(&temp.0)));
}
