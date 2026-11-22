use super::*;
use std::{
    os::unix::fs::{symlink, PermissionsExt},
    sync::atomic::AtomicU64,
};

#[test]
fn prompts_and_responses_are_bound_to_one_live_generation() {
    let mut gate = Gate {
        attempt: Some(9),
        ..Gate::default()
    };
    assert!(!gate.challenge(9, 1));
    gate.ready = true;
    assert!(!gate.challenge(8, 1));
    assert!(gate.challenge(9, 1));
    assert!(!gate.challenge(9, 2));
    assert!(!gate.response(8, 1));
    assert!(!gate.response(9, 2));
    assert!(!gate.finish(9, Ok(())));
    assert!(gate.response(9, 1));
    assert!(!gate.response(9, 1));
    assert!(!gate.challenge(9, 1));
    assert!(gate.challenge(9, 2));
    assert!(gate.response(9, 2));
    assert!(gate.finish(9, Ok(())));
    assert!(!gate.challenge(9, 3));
    assert!(!gate.finish(9, Ok(())));
}

#[test]
fn cancelled_generation_cannot_authorize_a_new_attempt() {
    let mut next = Gate {
        ready: true,
        attempt: Some(10),
        ..Gate::default()
    };
    assert!(!next.finish(9, Ok(())));
    assert!(!next.response(9, 1));
    assert!(next.challenge(10, 1));
    assert!(next.response(10, 1));
    assert!(next.finish(10, Err(Failure::Denied)));
    assert!(!next.finish(10, Ok(())));
    assert_eq!(next.finished, Some(Err(Failure::Denied)));
}

#[test]
fn initialized_idle_helper_exit_preserves_explicit_retry_without_an_auth_result() {
    assert!(exited_event(true, None, Err(Failure::Unavailable)).is_none());
    assert!(matches!(
        exited_event(false, None, Err(Failure::Unavailable)),
        Some(Event::Fatal(Failure::Unavailable))
    ));
    assert!(matches!(
        exited_event(true, Some(8), Err(Failure::Unavailable)),
        Some(Event::Finished {
            attempt: 8,
            result: Err(Failure::Unavailable)
        })
    ));
}

#[test]
fn pam_errors_and_invalid_identities_never_become_success() {
    for code in [6, 7, 10, 11] {
        assert_eq!(pam_failure(code, false), Failure::Denied);
    }
    for code in [6, 7, 12, 13, 16, 27] {
        assert_eq!(pam_failure(code, true), Failure::AccountDenied);
    }
    for code in [1, 3, 4, 8, 9, 19, 26, 99] {
        assert_eq!(pam_failure(code, false), Failure::Unavailable);
    }
    for name in ["", "user\0other", "user\n", "user\u{202e}"] {
        assert!(!valid_username(name));
    }
    assert!(!valid_username(&"u".repeat(257)));
    assert!(valid_username("alcard-tester"));
    if Uid::current().is_root() {
        assert_eq!(Identity::current(), Err(Failure::IdentityChanged));
    } else {
        let identity = Identity::current().unwrap();
        assert_eq!(identity.uid, Uid::current().as_raw());
    }
}

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "alcard-lock-unit-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir(&path).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700)).unwrap();
        Self(path)
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn fixed_policy_must_be_root_owned_regular_and_not_writable_by_users() {
    let fixture = Fixture::new();
    let path = fixture.0.join("policy");
    std::fs::write(&path, "auth required pam_unix.so\n").unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
    assert_eq!(policy_file(&path).is_ok(), Uid::current().is_root());
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o666)).unwrap();
    assert!(policy_file(&path).is_err());
    let link = fixture.0.join("link");
    symlink(&path, &link).unwrap();
    assert!(policy_file(&link).is_err());
    assert!(policy_file(&fixture.0).is_err());
    let fifo = fixture.0.join("fifo");
    nix::unistd::mkfifo(
        &fifo,
        nix::sys::stat::Mode::S_IRUSR | nix::sys::stat::Mode::S_IWUSR,
    )
    .unwrap();
    assert!(policy_file(&fifo).is_err());
}

#[test]
fn cancelling_an_unresponsive_private_child_kills_and_reaps_it() {
    let fixture = Fixture::new();
    let executable = fixture.0.join("waiting-child");
    std::fs::write(&executable, "#!/bin/sh\nexec sleep 30\n").unwrap();
    std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o700)).unwrap();
    let child = Process::spawn(&executable, Some(1)).unwrap();
    let pid = child.child.id() as i32;
    drop(child);
    // This fixture never calls PAM or any system service.
    assert_eq!(
        nix::sys::signal::kill(Pid::from_raw(pid), None),
        Err(nix::errno::Errno::ESRCH)
    );
}
