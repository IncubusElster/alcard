use super::*;
use std::{
    fs,
    os::unix::fs::PermissionsExt,
    sync::atomic::{AtomicU64, Ordering},
};

static NEXT: AtomicU64 = AtomicU64::new(0);

struct Fixture {
    directory: std::path::PathBuf,
    identity: Identity,
}
impl Fixture {
    fn new() -> Self {
        let directory = std::env::temp_dir().join(format!(
            "alcard-lock-state-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&directory).unwrap();
        fs::set_permissions(&directory, fs::Permissions::from_mode(0o700)).unwrap();
        Self {
            directory,
            identity: Identity {
                // SAFETY: identity getter has no pointer preconditions.
                uid: unsafe { libc::getuid() },
                pid: std::process::id(),
                start: start_time(std::process::id()).unwrap(),
            },
        }
    }
    fn session(&self) -> Session {
        Session::open(&self.directory, self.identity).unwrap()
    }
    fn path(&self) -> std::path::PathBuf {
        self.directory.join(format!(
            "alcard-lock-v1-{}-{}.state",
            self.identity.pid, self.identity.start
        ))
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.directory);
    }
}

#[test]
fn owner_loss_retains_intent_until_explicit_authenticated_completion() {
    let fixture = Fixture::new();
    let original = fixture.session();
    let restarted_shell = fixture.session();
    assert_eq!(original.observe().unwrap(), Observation::Clear);
    let locker = original.try_lease().unwrap().unwrap();
    assert_eq!(restarted_shell.observe().unwrap(), Observation::Held);
    assert!(restarted_shell.try_lease().unwrap().is_none());
    locker.mark_requested().unwrap();
    drop(locker); // Like process exit: no implicit unlink or clear.
    assert_eq!(restarted_shell.observe().unwrap(), Observation::Abandoned);
    let replacement = restarted_shell.try_lease().unwrap().unwrap();
    drop(replacement); // Pre-request failure during recovery must retain intent.
    assert_eq!(original.observe().unwrap(), Observation::Abandoned);
    let replacement = original.try_lease().unwrap().unwrap();
    replacement.mark_requested().unwrap();
    replacement.authenticated_unlock_acknowledged().unwrap();
    assert_eq!(restarted_shell.observe().unwrap(), Observation::Held);
    drop(replacement);
    assert_eq!(restarted_shell.observe().unwrap(), Observation::Clear);
}

#[test]
fn initial_pre_request_failure_does_not_create_recovery_intent() {
    let fixture = Fixture::new();
    let session = fixture.session();
    drop(session.try_lease().unwrap().unwrap());
    assert_eq!(session.observe().unwrap(), Observation::Clear);
}

#[test]
fn new_compositor_lifetime_cannot_adopt_previous_intent() {
    let fixture = Fixture::new();
    let previous = fixture.session();
    let owner = previous.try_lease().unwrap().unwrap();
    owner.mark_requested().unwrap();
    drop(owner);
    let next = Session::open(
        &fixture.directory,
        Identity {
            start: fixture.identity.start + 1,
            ..fixture.identity
        },
    )
    .unwrap();
    assert_eq!(next.observe().unwrap(), Observation::Clear);
    assert!(!next.is_live());
    assert!(next.try_lease().is_err());
    assert_eq!(previous.observe().unwrap(), Observation::Abandoned);
}

#[test]
fn unsafe_runtime_and_file_replacements_are_rejected() {
    let fixture = Fixture::new();
    fs::set_permissions(&fixture.directory, fs::Permissions::from_mode(0o755)).unwrap();
    assert!(Session::open(&fixture.directory, fixture.identity).is_err());
    fs::set_permissions(&fixture.directory, fs::Permissions::from_mode(0o700)).unwrap();
    let session = fixture.session();
    fs::rename(fixture.path(), fixture.directory.join("old")).unwrap();
    fs::write(fixture.path(), b"0").unwrap();
    fs::set_permissions(fixture.path(), fs::Permissions::from_mode(0o600)).unwrap();
    assert!(session.observe().is_err());
    assert!(session.try_lease().is_err());
    fs::remove_file(fixture.path()).unwrap();
    std::os::unix::fs::symlink(fixture.directory.join("old"), fixture.path()).unwrap();
    assert!(Session::open(&fixture.directory, fixture.identity).is_err());
    fs::remove_file(fixture.path()).unwrap();
    fs::hard_link(fixture.directory.join("old"), fixture.path()).unwrap();
    assert!(Session::open(&fixture.directory, fixture.identity).is_err());
}

#[test]
fn malformed_records_and_permissive_files_are_not_treated_as_clear() {
    let fixture = Fixture::new();
    let session = fixture.session();
    fs::write(fixture.path(), b"?").unwrap();
    assert!(session.observe().is_err());
    assert!(session.try_lease().is_err());
    fs::write(fixture.path(), b"00").unwrap();
    assert!(session.observe().is_err());
    fs::write(fixture.path(), b"0").unwrap();
    fs::set_permissions(fixture.path(), fs::Permissions::from_mode(0o644)).unwrap();
    assert!(session.observe().is_err());
}

#[test]
fn process_identity_parser_handles_comm_and_rejects_dead_processes() {
    let tail = (4..22).map(|_| "0").collect::<Vec<_>>().join(" ");
    assert_eq!(
        parse_start_time(&format!("7 (name ) with spaces) S {tail} 123 0")).unwrap(),
        123
    );
    assert!(parse_start_time(&format!("7 (name) Z {tail} 123")).is_err());
    assert!(parse_start_time("7 (name) S 0").is_err());
}
