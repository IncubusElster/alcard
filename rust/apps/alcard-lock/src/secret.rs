//! Fixed-capacity UTF-8 credentials. Never Clone or Debug; never rendered.
use zeroize::Zeroize;

pub const MAX_BYTES: usize = 512;
pub const MAX_CHARS: usize = 128;

pub struct Secret {
    bytes: Box<[u8; MAX_BYTES]>,
    len: usize,
}
impl Default for Secret {
    fn default() -> Self {
        Self {
            bytes: Box::new([0; MAX_BYTES]),
            len: 0,
        }
    }
}
impl Secret {
    pub fn as_str(&self) -> &str {
        std::str::from_utf8(&self.bytes[..self.len]).expect("validated UTF-8 secret")
    }
    pub fn count(&self) -> usize {
        self.as_str().chars().count()
    }
    pub fn insert(&mut self, ch: char) -> bool {
        if ch.is_control() || self.count() >= MAX_CHARS || self.len + ch.len_utf8() > MAX_BYTES {
            return false;
        }
        let mut encoded = [0; 4];
        let length = ch.encode_utf8(&mut encoded).len();
        self.bytes[self.len..self.len + length].copy_from_slice(&encoded[..length]);
        self.len += length;
        encoded.zeroize();
        true
    }
    pub fn backspace(&mut self) {
        if let Some((offset, _)) = self.as_str().char_indices().next_back() {
            self.bytes[offset..self.len].zeroize();
            self.len = offset;
        }
    }
    pub fn clear(&mut self) {
        self.bytes.zeroize();
        self.len = 0;
    }
    pub(crate) fn from_bytes(bytes: &[u8]) -> Option<Self> {
        let text = std::str::from_utf8(bytes).ok()?;
        if bytes.len() > MAX_BYTES
            || text.chars().count() > MAX_CHARS
            || text.chars().any(char::is_control)
        {
            return None;
        }
        let mut secret = Self::default();
        secret.bytes[..bytes.len()].copy_from_slice(bytes);
        secret.len = bytes.len();
        Some(secret)
    }
}
impl Drop for Secret {
    fn drop(&mut self) {
        self.clear();
    }
}

#[cfg(test)]
#[path = "unit_tests/secret.rs"]
mod tests;
