//! Foreign toplevel state on the Shell's single Wayland connection/queue.
//! This is a protocol extension to SCTK, not an independent client or poller.
use crate::model::WindowInfo;
use std::collections::HashMap;
use wayland_client::{backend::ObjectId, protocol::wl_seat::WlSeat, Proxy};
use wayland_protocols_wlr::foreign_toplevel::v1::client::{
    zwlr_foreign_toplevel_handle_v1::{self as handle, ZwlrForeignToplevelHandleV1},
    zwlr_foreign_toplevel_manager_v1::Event as ManagerEvent,
};

struct Entry {
    handle: ZwlrForeignToplevelHandleV1,
    window: WindowInfo,
    ready: bool,
}

pub struct State {
    windows: HashMap<ObjectId, Entry>,
    sequence: u64,
    next_id: u64,
    dirty: bool,
    finished: bool,
    pending_shell_activation: bool,
}
impl State {
    pub fn new() -> Self {
        Self {
            windows: HashMap::new(),
            sequence: 0,
            next_id: 1,
            dirty: false,
            finished: false,
            pending_shell_activation: false,
        }
    }
    pub fn stop(&mut self) {
        for (_, entry) in self.windows.drain() {
            entry.handle.destroy();
        }
        self.finished = true;
        self.dirty = true;
        self.pending_shell_activation = false;
    }
    pub fn take_update(&mut self) -> Option<Result<Vec<WindowInfo>, ()>> {
        if !std::mem::take(&mut self.dirty) {
            return None;
        }
        if self.finished {
            return Some(Err(()));
        }
        let mut windows: Vec<_> = self
            .windows
            .values()
            .filter(|entry| entry.ready && !is_own_window(&entry.window))
            .map(|entry| entry.window.clone())
            .collect();
        windows.sort_by(|a, b| b.activity.cmp(&a.activity).then_with(|| b.id.cmp(&a.id)));
        windows.truncate(128);
        Some(Ok(windows))
    }
    fn handle_for(&self, id: u64) -> Option<&ZwlrForeignToplevelHandleV1> {
        self.windows
            .values()
            .find(|entry| entry.window.id == id && entry.ready)
            .map(|entry| &entry.handle)
    }
    pub fn activate(&self, id: u64, seat: Option<&WlSeat>) -> Result<(), &'static str> {
        if self.finished {
            return Err("window manager unavailable");
        }
        let handle = self.handle_for(id).ok_or("window no longer exists")?;
        let seat = seat.ok_or("no Wayland seat for activation")?;
        handle.activate(seat);
        Ok(())
    }
    pub fn close(&self, id: u64) -> Result<(), &'static str> {
        if self.finished {
            return Err("window manager unavailable");
        }
        self.handle_for(id)
            .ok_or("window no longer exists")?
            .close();
        Ok(())
    }
    pub fn request_home(&mut self) {
        if !self.finished {
            self.pending_shell_activation = true;
        }
    }
    pub fn cancel_shell_activation(&mut self) {
        // A command issued during/before a lock must not steal application
        // focus when the compositor later restores the unlocked session.
        self.pending_shell_activation = false;
    }
    pub fn request_shell_activation(&mut self, seat: Option<&WlSeat>) {
        if !self.pending_shell_activation {
            return;
        }
        let Some(seat) = seat else { return };
        if let Some(entry) = self
            .windows
            .values()
            .find(|entry| entry.ready && is_own_window(&entry.window))
        {
            entry.handle.activate(seat);
            self.pending_shell_activation = false;
        }
    }
    pub fn manager_event(&mut self, event: ManagerEvent) {
        match event {
            ManagerEvent::Toplevel { toplevel } => {
                let id = self.next_id;
                self.next_id = self.next_id.wrapping_add(1);
                self.windows.insert(
                    toplevel.id(),
                    Entry {
                        handle: toplevel,
                        window: WindowInfo {
                            id,
                            title: String::new(),
                            app_id: String::new(),
                            active: false,
                            activity: 0,
                        },
                        ready: false,
                    },
                );
            }
            ManagerEvent::Finished => self.stop(),
            _ => {}
        }
    }
    pub fn handle_event(&mut self, proxy: &ZwlrForeignToplevelHandleV1, event: handle::Event) {
        let key = proxy.id();
        if let handle::Event::Closed = event {
            if let Some(entry) = self.windows.remove(&key) {
                entry.handle.destroy();
                self.dirty = true;
            }
            return;
        }
        let mut activated_now = false;
        if let Some(entry) = self.windows.get_mut(&key) {
            match event {
                handle::Event::Title { title } => entry.window.title = bounded(&title, 256),
                handle::Event::AppId { app_id } => entry.window.app_id = identity(app_id),
                handle::Event::State { state: bytes } => {
                    let active = bytes
                        .as_chunks::<4>()
                        .0
                        .iter()
                        .any(|chunk| u32::from_ne_bytes(*chunk) == 2);
                    activated_now = active && !entry.window.active;
                    entry.window.active = active;
                }
                handle::Event::Done => {
                    entry.ready = true;
                    self.dirty = true;
                }
                _ => {}
            }
        }
        if activated_now {
            self.sequence = self.sequence.saturating_add(1);
            if let Some(entry) = self.windows.get_mut(&key) {
                entry.window.activity = self.sequence;
            }
        }
    }
}
fn bounded(text: &str, max: usize) -> String {
    let mut chars = text.chars().filter(|c| !c.is_control());
    let mut value: String = chars.by_ref().take(max).collect();
    if max > 0 && chars.next().is_some() {
        value.pop();
        value.push('…');
    }
    value
}
fn identity(value: String) -> String {
    // Never truncate an identity into another application's valid prefix.
    if value.len() > 4096 || value.chars().any(char::is_control) {
        String::new()
    } else {
        value
    }
}
fn is_own_window(window: &WindowInfo) -> bool {
    window.app_id == "alcard-shell"
}

#[cfg(test)]
#[path = "unit_tests/toplevel.rs"]
mod tests;
