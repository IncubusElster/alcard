//! System settings shared by the console and graphical applications.
pub mod network;
#[cfg(feature = "pairing")]
pub mod pairing;
pub mod preferences;
pub mod settings;
pub mod theme;
