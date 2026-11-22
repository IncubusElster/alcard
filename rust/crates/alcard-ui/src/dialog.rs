//! Bounded state for non-secret, single-line dialogs.
//!
//! These values deliberately implement `Clone` and `Debug`: credentials must
//! use their own secret-buffer/form implementation. Character positions are
//! Unicode scalar indexes, matching the fixed-cell UI font, not UTF-8 bytes.

/// Hard limits apply even when a caller requests a larger text or list state.
pub const MAX_TEXT_CHARS: usize = 1024;
pub const MAX_CHOICES: usize = 512;
const MAX_TEXT_SCAN: usize = MAX_TEXT_CHARS * 4;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TextInput {
    value: String,
    cursor: usize,
    max_chars: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TextViewport {
    pub text: String,
    /// Cursor column relative to the visible text; a blank cell at the end is
    /// included in the available columns. Zero for a zero-column viewport.
    pub cursor: usize,
    /// Index of the first visible character in the complete value.
    pub offset: usize,
}

impl TextInput {
    pub fn new(value: &str, max_chars: usize) -> Self {
        let mut input = Self {
            value: String::new(),
            cursor: 0,
            max_chars: max_chars.min(MAX_TEXT_CHARS),
        };
        input.insert(value);
        input
    }

    pub fn value(&self) -> &str {
        &self.value
    }

    pub fn cursor(&self) -> usize {
        self.cursor
    }

    /// Insert a bounded prefix, omitting control characters (including line
    /// breaks). No allocation or scan grows with an unbounded incoming value.
    pub fn insert(&mut self, value: &str) -> bool {
        let remaining = self.max_chars - self.value.chars().count();
        if remaining == 0 {
            return false;
        }
        let added: String = value
            .chars()
            .take(MAX_TEXT_SCAN)
            .filter(|ch| !ch.is_control())
            .take(remaining)
            .collect();
        if added.is_empty() {
            return false;
        }
        let byte = self.byte_at(self.cursor);
        self.value.insert_str(byte, &added);
        self.cursor += added.chars().count();
        true
    }

    pub fn backspace(&mut self) -> bool {
        if self.cursor == 0 {
            return false;
        }
        let start = self.byte_at(self.cursor - 1);
        let end = self.byte_at(self.cursor);
        self.value.drain(start..end);
        self.cursor -= 1;
        true
    }

    pub fn delete(&mut self) -> bool {
        let start = self.byte_at(self.cursor);
        if start == self.value.len() {
            return false;
        }
        let end = self.byte_at(self.cursor + 1);
        self.value.drain(start..end);
        true
    }

    pub fn left(&mut self) -> bool {
        self.move_to(self.cursor.saturating_sub(1))
    }

    pub fn right(&mut self) -> bool {
        self.move_to((self.cursor + 1).min(self.value.chars().count()))
    }

    pub fn home(&mut self) -> bool {
        self.move_to(0)
    }

    pub fn end(&mut self) -> bool {
        self.move_to(self.value.chars().count())
    }

    pub fn clear(&mut self) -> bool {
        if self.value.is_empty() {
            return false;
        }
        self.value.clear();
        self.cursor = 0;
        true
    }

    /// Return only visible cells, keeping the insertion cursor inside the
    /// viewport. A zero-column viewport contains no text or cursor cell.
    pub fn viewport(&self, columns: usize) -> TextViewport {
        let columns = columns.min(self.max_chars + 1);
        if columns == 0 {
            return TextViewport {
                text: String::new(),
                cursor: 0,
                offset: self.cursor,
            };
        }
        let offset = self.cursor.saturating_sub(columns - 1);
        TextViewport {
            text: self.value.chars().skip(offset).take(columns).collect(),
            cursor: self.cursor - offset,
            offset,
        }
    }

    fn byte_at(&self, character: usize) -> usize {
        self.value
            .char_indices()
            .nth(character)
            .map_or(self.value.len(), |(byte, _)| byte)
    }

    fn move_to(&mut self, cursor: usize) -> bool {
        let changed = self.cursor != cursor;
        self.cursor = cursor;
        changed
    }
}

/// A choice is identified by a stable key. Display labels belong to the
/// caller, so localization or reordering cannot silently retarget selection.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChoiceList<K> {
    keys: Vec<K>,
    selected: usize,
}

impl<K: Eq> ChoiceList<K> {
    pub fn new(keys: impl IntoIterator<Item = K>) -> Self {
        Self {
            keys: bounded_keys(keys),
            selected: 0,
        }
    }

    /// Keep the selected key when it still exists; otherwise clamp the old
    /// index to the new list. Duplicate keys retain their first occurrence.
    pub fn reconcile(&mut self, keys: impl IntoIterator<Item = K>) -> bool {
        let keys = bounded_keys(keys);
        let selected = self
            .selected()
            .and_then(|old| keys.iter().position(|key| key == old))
            .unwrap_or_else(|| self.selected.min(keys.len().saturating_sub(1)));
        let changed = self.keys != keys || self.selected != selected;
        self.keys = keys;
        self.selected = selected;
        changed
    }

    pub fn selected(&self) -> Option<&K> {
        self.keys.get(self.selected)
    }

    pub fn selected_index(&self) -> Option<usize> {
        (!self.keys.is_empty()).then_some(self.selected)
    }

    pub fn up(&mut self) -> bool {
        if self.selected == 0 {
            return false;
        }
        self.selected -= 1;
        true
    }

    pub fn down(&mut self) -> bool {
        if self.selected + 1 >= self.keys.len() {
            return false;
        }
        self.selected += 1;
        true
    }

    pub fn select(&mut self, key: &K) -> bool {
        let Some(selected) = self.keys.iter().position(|candidate| candidate == key) else {
            return false;
        };
        let changed = selected != self.selected;
        self.selected = selected;
        changed
    }
}

fn bounded_keys<K: Eq>(keys: impl IntoIterator<Item = K>) -> Vec<K> {
    let mut result = Vec::new();
    // Bound enumeration before deduplication: even an infinite iterator of one
    // repeated key must finish, without allocating an intermediate full list.
    for key in keys.into_iter().take(MAX_CHOICES) {
        if !result.contains(&key) {
            result.push(key);
        }
    }
    result
}

#[cfg(test)]
#[path = "unit_tests/dialog.rs"]
mod tests;
