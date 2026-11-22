//! Click-through notification surface on the Shell's SCTK connection.
use crate::{render, shm::Buffers, wayland::Runtime};
use smithay_client_toolkit::{
    compositor::{CompositorState, Region},
    shell::{
        wlr_layer::{Anchor, KeyboardInteractivity, Layer, LayerShell, LayerSurface},
        WaylandSurface,
    },
    shm::Shm,
};
use std::{error::Error, io};
use tiny_skia::Pixmap;
use wayland_client::QueueHandle;

pub const HEIGHT: u32 = 40;
const KEYBOARD: KeyboardInteractivity = KeyboardInteractivity::None;
const EXCLUSIVE_ZONE: i32 = -1;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Banner {
    pub title: String,
    pub background: u32,
    pub foreground: u32,
    pub accent: u32,
}

pub struct Popup {
    pub layer: LayerSurface,
    pub configured: bool,
    pub mapped: bool,
    pub dirty: bool,
    pub frame_pending: bool,
    pub size: (u32, u32),
    pub scale: i32,
    pub banner: Banner,
    pub buffers: Buffers,
    pub image: Pixmap,
}
impl Popup {
    pub fn new(
        shell: &LayerShell,
        compositor: &CompositorState,
        shm: &Shm,
        qh: &QueueHandle<Runtime>,
        banner: Banner,
    ) -> Result<Self, Box<dyn Error>> {
        let surface = compositor.create_surface(qh);
        let region = Region::new(compositor)?;
        surface.set_input_region(Some(region.wl_region()));
        let layer = shell.create_layer_surface(
            qh,
            surface,
            Layer::Overlay,
            Some("alcard-notifications"),
            None,
        );
        layer.set_size(render::WIDTH as u32, HEIGHT);
        layer.set_anchor(Anchor::TOP);
        layer.set_margin(8, 0, 0, 0);
        layer.set_keyboard_interactivity(KEYBOARD);
        layer.set_exclusive_zone(EXCLUSIVE_ZONE);
        let buffers = Buffers::new(shm)?;
        let image = Pixmap::new(render::WIDTH as u32, HEIGHT)
            .ok_or_else(|| io::Error::other("notification image allocation failed"))?;
        layer.commit(); // SCTK acknowledges configure before the first attach.
        Ok(Self {
            layer,
            configured: false,
            mapped: false,
            dirty: true,
            frame_pending: false,
            size: (render::WIDTH as u32, HEIGHT),
            scale: 1,
            banner,
            buffers,
            image,
        })
    }
    pub fn update(&mut self, banner: Banner) {
        if self.banner != banner {
            self.banner = banner;
            self.dirty = true;
        }
    }
    pub fn paint(&mut self) {
        render::paint_notification(
            &mut self.image,
            &self.banner.title,
            self.banner.background,
            self.banner.foreground,
            self.banner.accent,
        );
    }
}

#[cfg(test)]
#[path = "unit_tests/layer.rs"]
mod tests;
