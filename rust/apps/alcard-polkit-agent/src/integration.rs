//! Opt-in integration test, compiled only by cargo test. It uses real polkitd,
//! libpolkit-agent, the distribution PAM stack, SCTK and labwc.
//! The disposable container supplies the accounts/policies. No test mode is
//! exposed by the installed agent binary and no host policy is changed.
use crate::gui::{self, TestProbe};
use glib::variant::ToVariant;
use std::{
    collections::HashMap,
    io::Read,
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::{
        atomic::Ordering::{Acquire, Release},
        Arc,
    },
    thread,
    time::{Duration, Instant},
};

fn wait(description: &str, mut condition: impl FnMut() -> bool) {
    eprintln!("Checking: {description}");
    let deadline = Instant::now() + Duration::from_secs(25);
    while !condition() {
        assert!(Instant::now() < deadline, "Timed out: {description}");
        thread::sleep(Duration::from_millis(20));
    }
}
fn key(name: &str) {
    assert!(Command::new("wtype")
        .args(["-k", name])
        .status()
        .expect("wtype key")
        .success());
}
fn answer(text: &str) {
    // These are fixed throwaway test-fixture values, never real credentials.
    // The production agent neither takes nor emits passwords through argv.
    use std::io::Write;
    let mut input = Command::new("wtype")
        .args(["-d", "10", "-"])
        .stdin(Stdio::piped())
        .spawn()
        .expect("wtype input");
    input
        .stdin
        .take()
        .unwrap()
        .write_all(text.as_bytes())
        .unwrap();
    assert!(input.wait().unwrap().success());
    key("Return");
}
fn request(action: &str) -> Child {
    let stat = std::fs::read_to_string("/proc/self/stat").unwrap();
    let start = stat
        .rsplit_once(')')
        .unwrap()
        .1
        .split_whitespace()
        .nth(19)
        .unwrap();
    let subject = format!(
        "{},{},{}",
        std::process::id(),
        start,
        nix::unistd::Uid::current()
    );
    Command::new("pkcheck")
        .args([
            "--allow-user-interaction",
            "--process",
            &subject,
            "--action-id",
            action,
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::inherit())
        .spawn()
        .expect("pkcheck")
}
fn result(child: &mut Child) -> bool {
    let mut status = None;
    wait("authority result", || {
        status = child.try_wait().unwrap();
        status.is_some()
    });
    status.unwrap().success()
}
fn presence() -> bool {
    let bus = gio::bus_get_sync(gio::BusType::Session, gio::Cancellable::NONE).unwrap();
    bus.call_sync(
        Some("org.freedesktop.DBus"),
        "/org/freedesktop/DBus",
        "org.freedesktop.DBus",
        "NameHasOwner",
        Some(&(crate::native::PRESENCE,).to_variant()),
        None,
        gio::DBusCallFlags::NONE,
        3000,
        gio::Cancellable::NONE,
    )
    .unwrap()
    .get::<(bool,)>()
    .unwrap()
    .0
}
fn capture_empty_prompt(directory: &Path) {
    assert!(
        directory.is_absolute(),
        "Authentication screenshot directory must be absolute"
    );
    std::fs::create_dir_all(directory).expect("authentication screenshot directory");
    // The real PAM challenge and keyboard focus are already observed. Allow
    // the GUI thread to commit that empty form before the compositor capture.
    // No response or fixture-secret read has happened at this point.
    thread::sleep(Duration::from_millis(200));
    let path = directory.join("01-auth-empty.png");
    assert!(Command::new("timeout")
        .args(["5", "grim", "-t", "png"])
        .arg(&path)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .status()
        .expect("capture empty authentication form")
        .success());
    let mut header = [0_u8; 24];
    std::fs::File::open(&path)
        .expect("authentication screenshot")
        .read_exact(&mut header)
        .expect("authentication screenshot PNG header");
    assert_eq!(&header[..8], b"\x89PNG\r\n\x1a\n");
    assert_eq!(u32::from_be_bytes(header[16..20].try_into().unwrap()), 320);
    assert_eq!(u32::from_be_bytes(header[20..24].try_into().unwrap()), 170);
}

fn roundtrip(
    probe: &TestProbe,
    action: &str,
    wrong_attempts: usize,
    multiple: bool,
    mut screenshot_directory: Option<&Path>,
) {
    eprintln!(
        "Roundtrip: {action}, wrong attempts: {wrong_attempts}, identity selection: {multiple}"
    );
    let mut prompt = probe.prompt.load(Acquire);
    let mut retries = probe.retry.load(Acquire);
    let closed = probe.closed.load(Acquire);
    let mut attempt = 0;
    let mut child = request(action);
    if multiple {
        wait("explicit identity selection", || {
            probe.choosing.load(Acquire) && probe.focused.load(Acquire)
        });
        assert_eq!(
            probe.prompt.load(Acquire),
            prompt,
            "An identity was selected implicitly"
        );
        key("Down");
        key("Down");
        key("Return");
    }
    let deadline = Instant::now() + Duration::from_secs(45);
    loop {
        assert!(Instant::now() < deadline, "PAM roundtrip timed out");
        if let Some(status) = child.try_wait().unwrap() {
            assert_eq!(status.success(), wrong_attempts < 3);
            break;
        }
        let new_prompt = probe.prompt.load(Acquire);
        if new_prompt > prompt {
            prompt = new_prompt;
            wait("credential keyboard focus", || probe.focused.load(Acquire));
            assert!(
                presence(),
                "Shell must suppress banners during authentication"
            );
            let echo = probe.echo.load(Acquire);
            if let (false, Some(directory)) = (echo, screenshot_directory) {
                capture_empty_prompt(directory);
                screenshot_directory = None;
            }
            if echo {
                answer("45678");
            } else if attempt < wrong_attempts {
                answer("deliberately-incorrect");
            } else {
                let home = std::env::var("HOME").unwrap();
                let path = if multiple {
                    ".auth-test-second"
                } else {
                    ".auth-test-secret"
                };
                let secret = zeroize::Zeroizing::new(
                    std::fs::read_to_string(format!("{home}/{path}")).unwrap(),
                );
                answer(&secret);
            }
        }
        let new_retry = probe.retry.load(Acquire);
        if new_retry > retries {
            retries = new_retry;
            attempt += 1;
            key("Return");
        }
        thread::sleep(Duration::from_millis(20));
    }
    wait("closed challenge", || probe.closed.load(Acquire) > closed);
    wait("authentication presence release", || !presence());
    assert!(prompt > 0, "No real PAM prompt was observed");
}
fn forged_request_is_denied() {
    let system = gio::bus_get_sync(gio::BusType::System, gio::Cancellable::NONE).unwrap();
    let identities = vec![(
        "unix-user".to_string(),
        HashMap::from([(
            "uid".to_string(),
            nix::unistd::Uid::current().as_raw().to_variant(),
        )]),
    )];
    let args = (
        "org.alcard.test.self",
        "Forged prompt",
        "",
        HashMap::<String, String>::new(),
        "fake-cookie",
        identities,
    )
        .to_variant();
    let error = system
        .call_sync(
            system.unique_name().as_deref(),
            "/org/alcard/PolicyKit1/AuthenticationAgent",
            "org.freedesktop.PolicyKit1.AuthenticationAgent",
            "BeginAuthentication",
            Some(&args),
            None,
            gio::DBusCallFlags::NONE,
            3000,
            gio::Cancellable::NONE,
        )
        .expect_err("Non-authority caller was accepted");
    assert!(error.to_string().contains("AccessDenied"));
}
#[test]
#[ignore = "requires disposable polkit/PAM container and headless labwc; scripts/test-polkit-native.sh"]
fn real_polkit_pam_graphical_roundtrips() {
    assert_eq!(
        std::env::var("ALCARD_AUTH_TEST_CONTAINER").as_deref(),
        Ok("1"),
        "Never run native fixtures on a real user system"
    );
    assert!(!nix::unistd::Uid::current().is_root());
    let probe = Arc::new(TestProbe::default());
    let state = probe.clone();
    let driver = thread::spawn(move || {
        let result = std::panic::catch_unwind(|| {
            wait("native agent registration", || state.ready.load(Acquire));
            forged_request_is_denied();
            let screenshot_directory = std::env::var_os("ALCARD_AUTH_SCREENSHOT_DIR")
                .filter(|value| !value.is_empty())
                .map(PathBuf::from);
            roundtrip(
                &state,
                "org.alcard.test.self",
                0,
                false,
                screenshot_directory.as_deref(),
            );
            roundtrip(&state, "org.alcard.test.self", 1, false, None);
            roundtrip(&state, "org.alcard.test.admin", 0, true, None);
            // Escape must cancel before any response and leave the agent usable.
            let prompt = state.prompt.load(Acquire);
            let closed = state.closed.load(Acquire);
            let mut first = request("org.alcard.test.self");
            wait("cancel prompt", || {
                state.prompt.load(Acquire) > prompt && state.focused.load(Acquire)
            });
            // Competing requests cannot replace the active prompt or its identity.
            let mut second = request("org.alcard.test.self");
            assert!(!result(&mut second));
            key("Escape");
            assert!(!result(&mut first));
            wait("cancel completion", || state.closed.load(Acquire) > closed);
            wait("cancel presence release", || !presence());
            // Caller death causes the real authority's CancelAuthentication.
            let prompt = state.prompt.load(Acquire);
            let closed = state.closed.load(Acquire);
            let mut abandoned = request("org.alcard.test.self");
            wait("abandoned prompt", || state.prompt.load(Acquire) > prompt);
            abandoned.kill().unwrap();
            let _ = abandoned.wait();
            wait("authority cancellation", || {
                state.closed.load(Acquire) > closed
            });
            roundtrip(&state, "org.alcard.test.self", 3, false, None);
            roundtrip(&state, "org.alcard.test.self", 0, false, None);
        });
        state.stop.store(true, Release);
        result
    });
    let gui = gui::run_test(probe);
    let driven = driver.join().expect("driver thread");
    gui.expect("graphical agent runtime");
    if let Err(panic) = driven {
        std::panic::resume_unwind(panic);
    }
}
