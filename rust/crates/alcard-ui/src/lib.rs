//! Shared graphics and non-secret dialog state; no authorization or IPC.
pub mod chrome;
pub mod dialog;
pub mod font;
pub mod icons;
pub mod shm;
pub use alcard_settings_core::{preferences, theme};
