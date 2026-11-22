//! Unprivileged graphical polkit agent. Authentication belongs to polkit/PAM.
mod gui;
#[cfg(test)]
mod integration;
mod native;
mod polkit;
mod protocol;
mod render;
mod shutdown;
mod ui;

use alcard_ui::preferences::{ConfigSource, Language as PreferenceLanguage, Overrides};
use std::{error::Error, io, os::unix::fs::MetadataExt, path::Path};

fn runtime_directory(path: &Path, uid: u32) -> io::Result<()> {
    let metadata = std::fs::symlink_metadata(path)?;
    if !path.is_absolute()
        || !metadata.is_dir()
        || metadata.file_type().is_symlink()
        || metadata.uid() != uid
        || metadata.mode() & 0o777 != 0o700
    {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "XDG_RUNTIME_DIR must be a private directory owned by the logged-in user",
        ));
    }
    Ok(())
}
fn main() -> std::process::ExitCode {
    match run() {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("Alcard authentication agent: {error}");
            if error.downcast_ref::<protocol::SessionLost>().is_some() {
                std::process::ExitCode::from(protocol::SESSION_LOST_EXIT)
            } else {
                std::process::ExitCode::FAILURE
            }
        }
    }
}

fn run() -> Result<(), Box<dyn Error>> {
    let mut overrides = Overrides::default();
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--help" | "-h" => {
                println!(
                    "Usage: alcard-polkit-agent [--locale auto|en|ru|es] [--theme classic|emerald|PATH]"
                );
                println!("Run as a logged-in user in a Wayland/layer-shell session. Enter submits; Escape cancels; F1 shows full action details.");
                return Ok(());
            }
            "--locale" => {
                let value = args.next().ok_or("--locale requires auto, en, ru or es")?;
                overrides.locale = Some(PreferenceLanguage::parse(&value)?);
            }
            "--theme" => {
                let name = args.next().ok_or("--theme requires a value")?;
                overrides.theme = Some(name);
            }
            _ => {
                return Err(
                    "Unknown option; credentials are never accepted on the command line".into(),
                )
            }
        }
    }
    let uid = nix::unistd::Uid::current();
    if uid.is_root() || uid != nix::unistd::Uid::effective() {
        return Err("Authentication agent must run as an ordinary logged-in user".into());
    }
    let runtime = std::env::var_os("XDG_RUNTIME_DIR").ok_or("XDG_RUNTIME_DIR is unset")?;
    runtime_directory(Path::new(&runtime), uid.as_raw())?;
    if std::env::var("WAYLAND_DISPLAY")
        .unwrap_or_default()
        .is_empty()
    {
        return Err("Wayland session required".into());
    }
    nix::sys::resource::setrlimit(nix::sys::resource::Resource::RLIMIT_CORE, 0, 0)?;
    nix::sys::prctl::set_dumpable(false)?;
    // Do not set NO_NEW_PRIVS: the standard polkit authentication helper must
    // retain its distribution-supplied privilege transition to validate PAM.
    let preferences = ConfigSource::from_environment(overrides);
    let (resolved, warning) = preferences.startup();
    if let Some(error) = warning {
        eprintln!("Authentication presentation preferences: {error}; using valid defaults");
    }
    gui::run(
        language_from_preferences(resolved.locale),
        resolved.theme,
        preferences,
    )
}

fn language_from_preferences(language: PreferenceLanguage) -> ui::Language {
    match language {
        PreferenceLanguage::Auto => ui::Language::detect(),
        PreferenceLanguage::En => ui::Language::En,
        PreferenceLanguage::Ru => ui::Language::Ru,
        PreferenceLanguage::Es => ui::Language::Es,
    }
}
