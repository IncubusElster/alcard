//! Native ARGB transport for tiny-skia images using SCTK-owned SHM slots.
//! This module does not rasterize UI or manage raw memfd/wl_buffer lifetimes.
use smithay_client_toolkit::shm::{
    slot::{Buffer, SlotPool},
    Shm,
};
use std::{error::Error, io};
use tiny_skia::{Color, FilterQuality, Pixmap, PixmapPaint, Transform};
use wayland_client::protocol::{wl_shm, wl_surface::WlSurface};

type Result<T> = std::result::Result<T, Box<dyn Error>>;

pub fn dimensions(size: (u32, u32), scale: i32) -> Result<(i32, i32, i32)> {
    let scale = u32::try_from(scale)
        .ok()
        .filter(|s| *s > 0)
        .ok_or_else(|| io::Error::other("invalid Wayland buffer scale"))?;
    let width = size
        .0
        .checked_mul(scale)
        .filter(|n| *n > 0 && *n <= 4096)
        .ok_or_else(|| io::Error::other("surface width exceeds memory limit"))?;
    let height = size
        .1
        .checked_mul(scale)
        .filter(|n| *n > 0 && *n <= 4096)
        .ok_or_else(|| io::Error::other("surface height exceeds memory limit"))?;
    // A total bound matters on the 512 MiB target, not just each dimension.
    if u64::from(width) * u64::from(height) > 4_194_304 {
        return Err(io::Error::other("surface exceeds four-megapixel memory limit").into());
    }
    Ok((width as i32, height as i32, width as i32 * 4))
}

fn copy_argb(source: &Pixmap, destination: &mut [u8]) -> Result<()> {
    if destination.len() != source.data().len() {
        return Err(io::Error::other("SHM buffer length mismatch").into());
    }
    // wl_shm ARGB8888 is a native-endian word. tiny-skia stores RGBA bytes.
    // Both are premultiplied; only channel order changes, never unpremultiply.
    for (rgba, word) in source
        .data()
        .as_chunks::<4>()
        .0
        .iter()
        .zip(destination.as_chunks_mut::<4>().0.iter_mut())
    {
        let argb = (u32::from(rgba[3]) << 24)
            | (u32::from(rgba[0]) << 16)
            | (u32::from(rgba[1]) << 8)
            | u32::from(rgba[2]);
        *word = argb.to_ne_bytes();
    }
    Ok(())
}

fn scale_image(source: &Pixmap, target: &mut Pixmap) {
    // Keep square pixels at every whole-number enlargement. Letterboxing is
    // preferable to changing glyph/icon proportions to match an arbitrary
    // compositor configure. A forced smaller size still preserves aspect.
    let ratio = (target.width() as f32 / source.width() as f32)
        .min(target.height() as f32 / source.height() as f32);
    let scale = if ratio >= 1.0 { ratio.floor() } else { ratio };
    let x = ((target.width() as f32 - source.width() as f32 * scale) / 2.0).floor();
    let y = ((target.height() as f32 - source.height() as f32 * scale) / 2.0).floor();
    let opaque = source.pixel(0, 0).is_some_and(|pixel| pixel.alpha() == 255);
    target.fill(if opaque {
        Color::BLACK
    } else {
        Color::TRANSPARENT
    });
    target.draw_pixmap(
        0,
        0,
        source.as_ref(),
        &PixmapPaint {
            quality: FilterQuality::Nearest,
            ..PixmapPaint::default()
        },
        Transform::from_row(scale, 0.0, 0.0, scale, x, y),
        None,
    );
}

struct Slot {
    buffer: Buffer,
    width: i32,
    height: i32,
}

pub struct Buffers {
    pool: SlotPool,
    slots: [Option<Slot>; 2],
    scaled: Option<Pixmap>,
}
impl Buffers {
    pub fn new(shm: &Shm) -> Result<Self> {
        Ok(Self {
            pool: SlotPool::new(320 * 170 * 4 * 2, shm)?,
            slots: [None, None],
            scaled: None,
        })
    }

    /// False means backpressure: both buffers still belong to the compositor.
    /// The caller keeps its dirty flag and retries after a Wayland event.
    pub fn attach(
        &mut self,
        source: &Pixmap,
        surface: &WlSurface,
        size: (u32, u32),
        scale: i32,
    ) -> Result<bool> {
        let (width, height, stride) = dimensions(size, scale)?;
        let available = self.slots.iter().position(|slot| match slot {
            None => true,
            Some(slot) => self.pool.canvas(&slot.buffer).is_some(),
        });
        let Some(index) = available else {
            return Ok(false);
        };
        if self.slots[index]
            .as_ref()
            .is_some_and(|s| s.width != width || s.height != height)
        {
            self.slots[index] = None; // Only an idle slot is replaced.
        }
        if self.slots[index].is_none() {
            let (buffer, _) =
                self.pool
                    .create_buffer(width, height, stride, wl_shm::Format::Argb8888)?;
            self.slots[index] = Some(Slot {
                buffer,
                width,
                height,
            });
        }
        let needs_scaling = source.width() != width as u32 || source.height() != height as u32;
        let image = if needs_scaling {
            if self
                .scaled
                .as_ref()
                .is_none_or(|p| p.width() != width as u32 || p.height() != height as u32)
            {
                self.scaled = Some(
                    Pixmap::new(width as u32, height as u32)
                        .ok_or_else(|| io::Error::other("could not allocate scaled image"))?,
                );
            }
            let target = self.scaled.as_mut().expect("allocated scaled image");
            scale_image(source, target);
            &*target
        } else {
            self.scaled = None;
            source
        };
        let slot = self.slots[index].as_ref().expect("allocated SHM slot");
        let bytes = self
            .pool
            .canvas(&slot.buffer)
            .ok_or_else(|| io::Error::other("SHM slot became active before attach"))?;
        copy_argb(image, bytes)?;
        slot.buffer.attach_to(surface)?;
        Ok(true)
    }
}

#[cfg(test)]
#[path = "unit_tests/shm.rs"]
mod tests;
