//! The compositor owns the lock; only this client's current-user PAM result
//! permits its explicit unlock request. Shell never receives credentials.
mod authentication;
mod gui;
#[cfg(test)]
mod integration;
mod protocol;
mod render;
mod secret;
mod ui;

use alcard_ui::preferences::{ConfigSource, Language as PreferenceLanguage, Overrides};
use std::{
    error::Error,
    io::{self, Write},
    os::unix::fs::MetadataExt,
    path::Path,
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread,
};

static LOCK_REQUESTED: AtomicBool = AtomicBool::new(false);

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

fn harden() -> Result<(), Box<dyn Error>> {
    let uid = nix::unistd::Uid::current();
    if uid.is_root()
        || uid != nix::unistd::Uid::effective()
        || nix::unistd::Gid::current() != nix::unistd::Gid::effective()
    {
        return Err("Locker must run as an ordinary logged-in user, never setuid/setgid".into());
    }
    nix::sys::resource::setrlimit(nix::sys::resource::Resource::RLIMIT_CORE, 0, 0)?;
    nix::sys::prctl::set_dumpable(false)?;
    // Do not set NO_NEW_PRIVS: pam_unix's distribution-provided unix_chkpwd
    // needs its normal setgid-shadow/setuid transition to verify this UID.
    Ok(())
}

fn main() -> Result<(), Box<dyn Error>> {
    let arguments: Vec<_> = std::env::args().skip(1).collect();
    if arguments == ["--pam-worker"] {
        harden()?;
        authentication::worker_main()?;
        return Ok(());
    }
    let notify = arguments.iter().any(|argument| argument == "--notify");
    // Only an ordinary returned error before any compositor lock request can
    // attest FAILED. No Drop/unwind handler may emit that statement: a crash
    // between requesting the lock and receiving LOCKED is indeterminate.
    run_reported(notify, &LOCK_REQUESTED, &mut io::stdout(), || {
        run(arguments, notify)
    })
}

fn run(arguments: Vec<String>, notify: bool) -> Result<(), Box<dyn Error>> {
    let mut overrides = Overrides::default();
    let mut args = arguments.into_iter();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--help" | "-h" => {
                if notify {
                    return Err(
                        "--help cannot be combined with the lifecycle notification protocol".into(),
                    );
                }
                println!("Usage: alcard-lock [--notify] [--locale auto|en|ru|es] [--theme classic|emerald|PATH]");
                println!("Locks all outputs through ext-session-lock-v1. Enter submits a masked PAM response; Escape clears or cancels authentication, never the lock.");
                return Ok(());
            }
            "--notify" => {}
            "--locale" => {
                let value = args.next().ok_or("--locale requires auto, en, ru or es")?;
                overrides.locale = Some(PreferenceLanguage::parse(&value)?);
            }
            "--theme" => overrides.theme = Some(args.next().ok_or("--theme requires a value")?),
            _ => return Err(
                "Unknown option; user, PAM service and credentials are not command-line options"
                    .into(),
            ),
        }
    }
    harden()?;
    let runtime = std::env::var_os("XDG_RUNTIME_DIR").ok_or("XDG_RUNTIME_DIR is unset")?;
    runtime_directory(Path::new(&runtime), nix::unistd::Uid::current().as_raw())?;
    if std::env::var("WAYLAND_DISPLAY")
        .unwrap_or_default()
        .is_empty()
    {
        return Err("Wayland session required".into());
    }
    let preferences = ConfigSource::from_environment(overrides);
    let (resolved, warning) = preferences.startup();
    if let Some(error) = warning {
        eprintln!("Lock presentation preferences: {error}; using valid defaults");
    }
    let language = match resolved.locale {
        PreferenceLanguage::Auto => ui::Language::detect(),
        PreferenceLanguage::En => ui::Language::En,
        PreferenceLanguage::Ru => ui::Language::Ru,
        PreferenceLanguage::Es => ui::Language::Es,
    };
    gui::run(language, resolved.theme, notify)
}

fn mark_lock_requested() {
    LOCK_REQUESTED.store(true, Ordering::Release);
}

fn run_reported<E>(
    notify: bool,
    requested: &AtomicBool,
    output: &mut impl Write,
    operation: impl FnOnce() -> Result<(), E>,
) -> Result<(), E> {
    let result = operation();
    if notify && result.is_err() && !requested.load(Ordering::Acquire) {
        let _ = output.write_all(b"FAILED\n");
        let _ = output.flush();
    }
    result
}

fn report_locked(notify: bool) {
    report_status(notify, b"LOCKED\n");
}

fn report_unlocked(notify: bool) {
    report_status(notify, b"UNLOCKED\n");
}

fn report_status(notify: bool, value: &[u8]) {
    if notify {
        // The Shell may have exited after launching us. A broken reporting
        // pipe must never terminate a locker or release its compositor lock.
        let mut output = io::stdout().lock();
        let _ = output.write_all(value);
        let _ = output.flush();
    }
}

/// Presentation-only presence: no methods, no passwords and no unlock command.
/// An absent or unresponsive session bus cannot decide lock security.
struct Presence {
    stop: mpsc::SyncSender<()>,
}
impl Presence {
    fn start() -> Self {
        let (stop, receive) = mpsc::sync_channel(1);
        let _ = thread::Builder::new()
            .name("alcard-lock-presence".into())
            .spawn(move || {
                let mut retry = std::time::Duration::from_secs(1);
                loop {
                    if !matches!(receive.try_recv(), Err(mpsc::TryRecvError::Empty)) {
                        return;
                    }
                    let connection = zbus::block_on(async {
                        let connection = zbus::Connection::session().await?;
                        connection
                            .request_name_with_flags(
                                "org.alcard.SessionLock",
                                zbus::fdo::RequestNameFlags::DO_NOT_QUEUE,
                            )
                            .await?;
                        Ok::<_, zbus::Error>(connection)
                    });
                    if let Ok(connection) = connection {
                        retry = std::time::Duration::from_secs(1);
                        while !connection.is_closed() {
                            match receive.recv_timeout(std::time::Duration::from_secs(1)) {
                                Err(mpsc::RecvTimeoutError::Timeout) => {}
                                _ => return,
                            }
                        }
                    }
                    // This is presentation only. Bus loss never cancels PAM,
                    // clears recovery intent or releases the compositor lock.
                    match receive.recv_timeout(retry) {
                        Err(mpsc::RecvTimeoutError::Timeout) => {}
                        _ => return,
                    }
                    retry = (retry * 2).min(std::time::Duration::from_secs(30));
                }
            });
        Self { stop }
    }
}
impl Drop for Presence {
    fn drop(&mut self) {
        let _ = self.stop.try_send(());
    }
}

#[cfg(test)]
#[path = "unit_tests/main_lifecycle.rs"]
mod lifecycle_tests;
