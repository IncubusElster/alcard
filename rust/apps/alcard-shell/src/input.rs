//! Focus-local, edge-triggered navigation and non-secret text editing.
//! No global hooks, letter shortcuts, or synthetic presses on focus arrival.
use crate::model::Key;
use smithay_client_toolkit::seat::keyboard::{Keysym, Modifiers};
use std::collections::HashSet;

const MAX_HELD_KEYS: usize = 512;
const MAX_TEXT_EVENT: usize = 32;

#[derive(Default)]
pub struct Input {
    focused: bool,
    held: HashSet<u32>,
    modifiers: Modifiers,
    modifiers_ready: bool,
}

impl Input {
    pub fn enter(&mut self, held: &[u32]) {
        // wl_keyboard requires a modifiers event after every enter. Wait for
        // that fresh snapshot, including latched/locked modifiers; neither a
        // previous surface's state nor guessed raw modifier codes are enough.
        self.focused = held.len() <= MAX_HELD_KEYS;
        self.held = held.iter().copied().take(MAX_HELD_KEYS).collect();
        self.modifiers = Modifiers::default();
        self.modifiers_ready = false;
    }

    pub fn leave(&mut self) {
        self.focused = false;
        self.held.clear();
        self.modifiers = Modifiers::default();
        self.modifiers_ready = false;
    }

    pub fn update_modifiers(&mut self, modifiers: Modifiers) {
        self.modifiers = modifiers;
        self.modifiers_ready = true;
    }

    pub fn release(&mut self, code: u32) {
        self.held.remove(&code);
    }

    pub fn press(
        &mut self,
        code: u32,
        keysym: Keysym,
        utf8: Option<&str>,
        editing: bool,
    ) -> Vec<Key> {
        if !self.focused
            || self.held.len() >= MAX_HELD_KEYS
            || !self.held.insert(code)
            || !self.modifiers_ready
        {
            return Vec::new();
        }
        let modifiers = self.modifiers;
        if editing {
            if modifiers.alt || modifiers.logo {
                return Vec::new();
            }
            if modifiers.ctrl {
                // Preserve Ctrl+U on non-Latin layouts; symbol matching also
                // supports virtual keyboards with dynamically assigned codes.
                return if !modifiers.shift
                    && (code == 22 || matches!(keysym, Keysym::u | Keysym::U))
                {
                    vec![Key::ClearText]
                } else {
                    Vec::new()
                };
            }
            // Shift is useful for text, but unsupported shifted navigation
            // must not accept/cancel a dialog or pretend to select text.
            if !modifiers.shift {
                let command = match keysym {
                    Keysym::Return | Keysym::KP_Enter => Some(Key::Enter),
                    Keysym::Escape => Some(Key::Escape),
                    Keysym::BackSpace => Some(Key::Backspace),
                    Keysym::Delete | Keysym::KP_Delete => Some(Key::Delete),
                    Keysym::Left | Keysym::KP_Left => Some(Key::Left),
                    Keysym::Right | Keysym::KP_Right => Some(Key::Right),
                    Keysym::Home | Keysym::KP_Home => Some(Key::Home),
                    Keysym::End | Keysym::KP_End => Some(Key::End),
                    Keysym::F3 => Some(Key::Soft(3)),
                    _ => None,
                };
                if let Some(command) = command {
                    return vec![command];
                }
            }
            // SCTK already applies the active XKB layout and compose table.
            // A dead key has no text yet; do not invent a keysym fallback.
            return utf8
                .unwrap_or_default()
                .chars()
                .take(MAX_TEXT_EVENT)
                .filter(|character| !character.is_control())
                .map(Key::Text)
                .collect();
        }
        if modifiers.ctrl || modifiers.alt || modifiers.shift || modifiers.logo {
            return Vec::new();
        }
        // SCTK reports Linux evdev codes, not XKB's evdev+8 keycodes.
        // Physical softkeys remain layout independent only outside an editor.
        let action = match code {
            5..=9 => Some(Key::Soft((code - 5) as usize)),
            103 => Some(Key::Up),
            108 => Some(Key::Down),
            105 => Some(Key::Left),
            106 => Some(Key::Right),
            102 => Some(Key::Home),
            _ => match keysym {
                Keysym::Return | Keysym::KP_Enter => Some(Key::Enter),
                Keysym::space => Some(Key::Soft(1)),
                Keysym::Tab => Some(Key::Soft(2)),
                Keysym::F3 => Some(Key::Soft(3)),
                Keysym::Escape => Some(Key::Escape),
                Keysym::BackSpace => Some(Key::Backspace),
                _ => None,
            },
        };
        action.into_iter().collect()
    }
}

#[cfg(test)]
#[path = "unit_tests/input.rs"]
mod tests;
