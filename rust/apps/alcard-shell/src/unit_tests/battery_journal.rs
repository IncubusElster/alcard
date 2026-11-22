use super::*;
use std::{
    os::unix::fs::{DirBuilderExt, PermissionsExt},
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc, Barrier,
    },
    thread,
};

static NEXT: AtomicUsize = AtomicUsize::new(0);
struct Tmp(PathBuf);
impl Tmp {
    fn new() -> Self {
        let path = env::temp_dir().join(format!(
            "alcard-battery-journal-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::DirBuilder::new().mode(0o700).create(&path).unwrap();
        Self(path)
    }
    fn marker(&self) -> PathBuf {
        self.0.join(NAME.to_str().unwrap())
    }
}
impl Drop for Tmp {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn restart_retains_an_attempt_and_only_confirmed_clear_rearms_creation() {
    let tmp = Tmp::new();
    let first = Journal::at(&tmp.0).unwrap();
    assert_eq!(first.inhibit(), None);
    assert!(first.latch().unwrap());
    assert_eq!(first.inhibit(), Some(Inhibit::Attempt));
    let restarted = Journal::at(&tmp.0).unwrap();
    assert_eq!(restarted.inhibit(), Some(Inhibit::Attempt));
    assert!(!restarted.latch().unwrap());
    restarted.clear().unwrap();
    restarted.clear().unwrap();
    assert_eq!(first.inhibit(), None);
    assert!(first.latch().unwrap());
}

#[test]
fn invalid_runtime_permissions_or_symlink_are_rejected_without_mutation() {
    let tmp = Tmp::new();
    assert!(Journal::at(Path::new("relative/runtime")).is_err());
    fs::set_permissions(&tmp.0, fs::Permissions::from_mode(0o755)).unwrap();
    assert!(Journal::at(&tmp.0).is_err());
    fs::set_permissions(&tmp.0, fs::Permissions::from_mode(0o700)).unwrap();
    let outer = Tmp::new();
    let alias = outer.0.join("runtime");
    std::os::unix::fs::symlink(&tmp.0, &alias).unwrap();
    assert!(Journal::at(&alias).is_err());
    assert!(!tmp.marker().exists());
}

#[test]
fn symlinks_corruption_bad_modes_and_hardlinks_fail_closed_and_survive_clear() {
    for kind in 0..5 {
        let tmp = Tmp::new();
        let journal = Journal::at(&tmp.0).unwrap();
        let target = tmp.0.join("keep");
        fs::write(&target, b"unrelated data").unwrap();
        if kind == 0 {
            std::os::unix::fs::symlink(&target, tmp.marker()).unwrap();
        } else {
            fs::write(
                tmp.marker(),
                if kind == 1 {
                    b"bad".as_slice()
                } else {
                    CONTENT
                },
            )
            .unwrap();
            fs::set_permissions(
                tmp.marker(),
                fs::Permissions::from_mode(if kind == 2 { 0o644 } else { 0o600 }),
            )
            .unwrap();
            if kind == 3 {
                fs::hard_link(tmp.marker(), tmp.0.join("alias")).unwrap();
            }
            if kind == 4 {
                fs::write(tmp.marker(), vec![b'x'; 4096]).unwrap();
            }
        }
        let before = fs::symlink_metadata(tmp.marker()).unwrap().ino();
        assert_eq!(journal.inhibit(), Some(Inhibit::Attempt));
        assert!(Journal::at(&tmp.0).is_err());
        assert!(journal.latch().is_err());
        assert!(journal.clear().is_err());
        assert_eq!(fs::symlink_metadata(tmp.marker()).unwrap().ino(), before);
        assert_eq!(fs::read(&target).unwrap(), b"unrelated data");
    }
}

#[test]
fn foreign_owner_and_replaced_inode_never_pass_removal_checks() {
    let tmp = Tmp::new();
    let journal = Journal::at(&tmp.0).unwrap();
    journal.latch().unwrap();
    let old = journal.open_marker(false).unwrap();
    let metadata = old.metadata().unwrap();
    assert!(trusted_marker(&metadata, metadata.uid().wrapping_add(1), true).is_err());
    fs::rename(tmp.marker(), tmp.0.join("old")).unwrap();
    assert!(journal.latch().unwrap());
    assert!(journal.remove_matching(&old).is_err());
    assert_eq!(journal.inhibit(), Some(Inhibit::Attempt));
    assert_eq!(fs::read(tmp.marker()).unwrap(), CONTENT);
}

#[test]
fn replaced_runtime_directory_cannot_redirect_writes_or_removals() {
    let outer = Tmp::new();
    let path = outer.0.join("runtime");
    fs::DirBuilder::new().mode(0o700).create(&path).unwrap();
    let journal = Journal::at(&path).unwrap();
    journal.latch().unwrap();
    fs::rename(&path, outer.0.join("previous")).unwrap();
    fs::DirBuilder::new().mode(0o700).create(&path).unwrap();
    assert_eq!(journal.inhibit(), Some(Inhibit::Attempt));
    assert!(journal.latch().is_err());
    assert!(journal.clear().is_err());
    assert!(!path.join(NAME.to_str().unwrap()).exists());
}

#[test]
fn concurrent_latch_has_exactly_one_creator_without_blocking_readers() {
    let tmp = Tmp::new();
    let journals: Vec<_> = (0..8).map(|_| Journal::at(&tmp.0).unwrap()).collect();
    let barrier = Arc::new(Barrier::new(journals.len()));
    let handles: Vec<_> = journals
        .into_iter()
        .map(|journal| {
            let barrier = barrier.clone();
            thread::spawn(move || {
                barrier.wait();
                journal.latch()
            })
        })
        .collect();
    let mut creators = 0;
    for handle in handles {
        match handle.join().unwrap() {
            Ok(true) => creators += 1,
            Ok(false) => {}
            Err(error) => assert_eq!(error.kind(), io::ErrorKind::WouldBlock),
        }
    }
    assert_eq!(creators, 1);
    assert_eq!(
        Journal::at(&tmp.0).unwrap().inhibit(),
        Some(Inhibit::Attempt)
    );
}

#[test]
fn resume_survives_restart_without_weakening_a_recorded_attempt() {
    let tmp = Tmp::new();
    let journal = Journal::at(&tmp.0).unwrap();
    journal.resume().unwrap();
    assert_eq!(journal.inhibit(), Some(Inhibit::Resume));
    let restarted = Journal::at(&tmp.0).unwrap();
    assert_eq!(restarted.inhibit(), Some(Inhibit::Resume));
    // A still-present resume inhibition refuses an automatic attempt and
    // conservatively records that refusal as consumed, with no new sleep.
    assert!(!restarted.latch().unwrap());
    assert_eq!(journal.inhibit(), Some(Inhibit::Attempt));
    restarted.resume().unwrap();
    assert_eq!(journal.inhibit(), Some(Inhibit::Attempt));
    restarted.clear().unwrap();
    restarted.resume().unwrap();
    assert_eq!(journal.inhibit(), Some(Inhibit::Resume));
}

#[test]
fn stale_resume_clear_never_removes_a_newer_attempt() {
    let tmp = Tmp::new();
    let first = Journal::at(&tmp.0).unwrap();
    first.resume().unwrap();
    let expected = first.inhibit();
    assert_eq!(expected, Some(Inhibit::Resume));
    let other = Journal::at(&tmp.0).unwrap();
    assert!(!other.latch().unwrap());
    assert!(!first.clear_if(expected).unwrap());
    assert_eq!(first.inhibit(), Some(Inhibit::Attempt));
    assert!(!first.clear_if(None).unwrap());
    assert!(first.clear_if(Some(Inhibit::Attempt)).unwrap());
    assert!(!first.clear_if(Some(Inhibit::Attempt)).unwrap());
    assert!(first.clear_if(None).unwrap());
}
