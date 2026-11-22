//! In-process messages only. No cookies, credentials or PAM replies cross Shell IPC.
use zeroize::Zeroize;

pub const MAX_BYTES: usize = 512;
pub const MAX_CHARS: usize = 128;

/// The session supervisor must end this login, rather than retry an agent
/// against an authentication context that no longer belongs to the PAM login.
pub const SESSION_LOST_EXIT: u8 = 75;

#[derive(Debug)]
pub struct SessionLost;

impl std::fmt::Display for SessionLost {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("Authenticated login service was lost; ending this desktop session")
    }
}

impl std::error::Error for SessionLost {}

/// Fixed allocation: edits never reallocate or leave removed bytes behind.
/// Deliberately neither Clone nor Debug. The owner wipes it on every exit path.
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
        std::str::from_utf8(&self.bytes[..self.len]).expect("secret contains validated UTF-8")
    }
    pub fn count(&self) -> usize {
        self.as_str().chars().count()
    }
    pub fn insert(&mut self, ch: char) -> bool {
        if ch.is_control() || self.count() >= MAX_CHARS || self.len + ch.len_utf8() > MAX_BYTES {
            return false;
        }
        let mut encoded = [0_u8; 4];
        let n = ch.encode_utf8(&mut encoded).len();
        self.bytes[self.len..self.len + n].copy_from_slice(&encoded[..n]);
        self.len += n;
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
}
impl Drop for Secret {
    fn drop(&mut self) {
        self.clear();
    }
}

#[derive(Clone)]
pub struct Identity {
    /// Index into the exact authority-supplied identity vector, not a username lookup.
    pub index: usize,
    pub label: String,
}
pub enum Event {
    Ready,
    Open {
        id: u64,
        action: String,
        reason: String,
        identities: Vec<Identity>,
    },
    Challenge {
        id: u64,
        serial: u64,
        prompt: String,
        echo: bool,
    },
    Notice {
        id: u64,
        text: String,
        error: bool,
    },
    Retry {
        id: u64,
    },
    Closed {
        id: u64,
        authorized: bool,
    },
    Fatal(String),
    SessionLost,
}
pub enum Command {
    Choose {
        id: u64,
        index: usize,
    },
    Respond {
        id: u64,
        serial: u64,
        secret: Secret,
    },
    Retry {
        id: u64,
    },
    Cancel {
        id: u64,
    },
    Shutdown,
}

pub fn plain(value: &str, limit: usize) -> String {
    value.chars().filter(|c| !c.is_control() && !matches!(*c,
        '\u{061c}' | '\u{200e}' | '\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}'
    )).take(limit).collect()
}

#[cfg(test)]
#[path = "unit_tests/protocol.rs"]
mod tests;
