use super::*;
use std::{
    cell::Cell,
    ops::{Deref, DerefMut},
    process::{Command, Stdio},
};

fn completed_child(success: bool) -> Child {
    // BusyBox installations need not expose true/false in /usr/bin. The
    // child script is a fixed test literal, never application/user input.
    let mut child = Command::new("/bin/sh")
        .args(["-c", if success { "exit 0" } else { "exit 1" }])
        .spawn()
        .unwrap();
    child.wait().unwrap();
    child
}

// A fixed test-only child remains alive until its stdin closes. Tests never
// sleep or kill processes, and production has no test command or fixture mode.
fn waiting_child(exit_code: i32) -> Child {
    Command::new("/bin/sh")
        .args(["-c", "read -r marker; exit \"$1\"", "alcard-launch-test"])
        .arg(exit_code.to_string())
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap()
}

struct Running(Tracker);
impl Running {
    fn new() -> Self {
        Self(Tracker::new())
    }
    fn finish(&mut self, index: usize) {
        self.children[index].child.stdin.take();
        self.children[index].child.wait().unwrap();
    }
}
impl Deref for Running {
    type Target = Tracker;
    fn deref(&self) -> &Tracker {
        &self.0
    }
}
impl DerefMut for Running {
    fn deref_mut(&mut self) -> &mut Tracker {
        &mut self.0
    }
}
impl Drop for Running {
    fn drop(&mut self) {
        for tracked in &mut self.0.children {
            tracked.child.stdin.take();
            let _ = tracked.child.wait();
        }
    }
}

fn phase(tracker: &Tracker, id: &str) -> Phase {
    tracker.records[id].status.phase
}
fn attempt(outcome: Outcome) -> u64 {
    match outcome {
        Outcome::Started(attempt) => attempt,
        other => panic!("{other:?}"),
    }
}
fn never_spawn() -> Result<Child, LaunchError> {
    panic!("unexpected spawn")
}

#[test]
fn duplicates_stay_guarded_after_the_launch_becomes_uncertain() {
    let now = Instant::now();
    let count = Cell::new(0);
    let mut tracker = Tracker::new();
    let first = attempt(tracker.start("a.desktop", "A", &[], now, || {
        count.set(count.get() + 1);
        Ok(completed_child(true))
    }));
    assert_eq!(
        tracker.start("a.desktop", "A", &[], now, never_spawn),
        Outcome::Suppressed(first)
    );
    tracker.tick(now);
    assert_eq!(phase(&tracker, "a.desktop"), Phase::Starting);
    tracker.tick(now + START_TIMEOUT - Duration::from_millis(1));
    assert_eq!(phase(&tracker, "a.desktop"), Phase::Starting);
    tracker.tick(now + START_TIMEOUT);
    assert_eq!(phase(&tracker, "a.desktop"), Phase::Unconfirmed);
    assert_eq!(
        tracker.start("a.desktop", "A", &[], now, never_spawn),
        Outcome::Suppressed(first)
    );
    assert_eq!(count.get(), 1);
    assert_eq!(tracker.next_wakeup(now + START_TIMEOUT), None);
}

#[test]
fn existing_window_returns_its_id_without_spawning_or_focusing() {
    let mut tracker = Tracker::new();
    let now = Instant::now();
    let windows = vec![(17, "a.desktop".into()), (12, "a.desktop".into())];
    assert_eq!(
        tracker.start("a.desktop", "A", &windows, now, never_spawn),
        Outcome::ExistingWindow(17)
    );
    assert!(tracker.records.is_empty());
    assert_eq!(tracker.take_update(), None);
}

#[test]
fn only_a_new_stably_matched_window_confirms_a_launch() {
    let now = Instant::now();
    let mut tracker = Tracker::new();
    tracker.start("a.desktop", "A", &[(1, String::new())], now, || {
        Ok(completed_child(true))
    });
    tracker.take_update();
    tracker.windows(&[(1, "a.desktop".into()), (2, "other.desktop".into())]);
    assert_eq!(tracker.take_update(), None);
    assert_eq!(phase(&tracker, "a.desktop"), Phase::Starting);
    tracker.windows(&[(1, "a.desktop".into()), (2, "a.desktop".into())]);
    assert_eq!(tracker.take_update(), Some(Vec::new()));
    assert_eq!(tracker.children.len(), 1); // Readiness does not abandon the child.
    tracker.tick(now);
    assert!(tracker.children.is_empty());
    assert_eq!(tracker.next_wakeup(now), None);
}

#[test]
fn nonzero_exit_has_a_short_wayland_race_grace() {
    let now = Instant::now();
    let mut tracker = Tracker::new();
    tracker.start("a.desktop", "A", &[], now, || Ok(completed_child(false)));
    tracker.take_update();
    tracker.tick(now);
    assert_eq!(phase(&tracker, "a.desktop"), Phase::Starting);
    assert_eq!(tracker.next_wakeup(now), Some(EXIT_GRACE));
    tracker.tick(now + EXIT_GRACE - Duration::from_millis(1));
    assert_eq!(tracker.take_update(), None);
    tracker.tick(now + EXIT_GRACE);
    assert_eq!(
        phase(&tracker, "a.desktop"),
        Phase::Failed(FailureReason::EarlyExit {
            code: Some(1),
            signal: None
        })
    );
    assert_eq!(tracker.take_update().unwrap().len(), 1);
    assert_eq!(tracker.next_wakeup(now + EXIT_GRACE), None);
}

#[test]
fn a_window_arriving_during_exit_grace_cancels_the_failure() {
    let now = Instant::now();
    let mut tracker = Tracker::new();
    tracker.start("a.desktop", "A", &[], now, || Ok(completed_child(false)));
    tracker.tick(now);
    tracker.windows(&[(1, "a.desktop".into())]);
    tracker.tick(now + EXIT_GRACE);
    assert!(tracker.records.is_empty());
    assert_eq!(tracker.take_update(), Some(Vec::new()));
}

#[test]
fn spawn_failure_is_typed_and_retry_requires_the_current_failed_attempt() {
    let now = Instant::now();
    let mut tracker = Tracker::new();
    let first = attempt(tracker.start("a.desktop", "A", &[], now, || {
        Err(LaunchError::MissingProgram)
    }));
    assert_eq!(
        phase(&tracker, "a.desktop"),
        Phase::Failed(FailureReason::Spawn(LaunchError::MissingProgram))
    );
    assert_eq!(
        tracker.start("a.desktop", "A", &[], now, never_spawn),
        Outcome::Suppressed(first)
    );
    assert_eq!(
        tracker.retry("a.desktop", "A", first + 1, &[], now, never_spawn),
        Outcome::Rejected(Rejection::StaleAttempt)
    );
    let second = attempt(tracker.retry("a.desktop", "A", first, &[], now, || {
        Ok(completed_child(true))
    }));
    assert!(second > first);
    assert_eq!(
        tracker.retry("a.desktop", "A", first, &[], now, never_spawn),
        Outcome::Rejected(Rejection::StaleAttempt)
    );
    assert_eq!(
        tracker.retry("a.desktop", "A", second, &[], now, never_spawn),
        Outcome::Rejected(Rejection::RetryUnavailable)
    );
    tracker.tick(now);
    assert_eq!(phase(&tracker, "a.desktop"), Phase::Starting);
}

#[test]
fn an_old_child_cannot_fail_the_retried_attempt() {
    let now = Instant::now();
    let mut tracker = Running::new();
    let first = attempt(tracker.start("a.desktop", "A", &[], now, || Ok(waiting_child(7))));
    tracker.tick(now + START_TIMEOUT);
    assert_eq!(phase(&tracker, "a.desktop"), Phase::Unconfirmed);
    let second = attempt(
        tracker.retry("a.desktop", "A", first, &[], now + START_TIMEOUT, || {
            Ok(waiting_child(0))
        }),
    );
    tracker.finish(0);
    tracker.tick(now + START_TIMEOUT + REAP_INTERVAL);
    tracker.tick(now + START_TIMEOUT + REAP_INTERVAL + EXIT_GRACE);
    assert_eq!(tracker.records["a.desktop"].status.attempt, second);
    assert_eq!(phase(&tracker, "a.desktop"), Phase::Starting);
    assert!(tracker.records["a.desktop"].failure.is_none());
    assert_eq!(tracker.children.len(), 1);
}

#[test]
fn retry_rechecks_windows_before_spawning() {
    let now = Instant::now();
    let mut tracker = Tracker::new();
    let first = attempt(tracker.start("a.desktop", "A", &[], now, || {
        Err(LaunchError::MissingProgram)
    }));
    assert_eq!(
        tracker.retry(
            "a.desktop",
            "A",
            first,
            &[(7, "a.desktop".into())],
            now,
            never_spawn
        ),
        Outcome::ExistingWindow(7)
    );
    assert_eq!(tracker.take_update(), Some(Vec::new()));
}

#[test]
fn uncertain_records_have_a_hard_bound() {
    let now = Instant::now();
    let mut tracker = Tracker::new();
    for index in 0..MAX_RECORDS {
        tracker.start(&format!("{index}.desktop"), "A", &[], now, || {
            Ok(completed_child(true))
        });
    }
    tracker.tick(now + START_TIMEOUT);
    assert!(tracker
        .records
        .values()
        .all(|record| record.status.phase == Phase::Unconfirmed));
    assert_eq!(
        tracker.start("overflow.desktop", "A", &[], now, never_spawn),
        Outcome::Rejected(Rejection::TooManyRecords)
    );
    assert_eq!(tracker.records.len(), MAX_RECORDS);
}

#[test]
fn the_oldest_finished_failure_can_be_evicted_at_the_record_bound() {
    let now = Instant::now();
    let mut tracker = Tracker::new();
    for index in 0..MAX_RECORDS {
        tracker.start(&format!("{index}.desktop"), "A", &[], now, || {
            Err(LaunchError::MissingProgram)
        });
    }
    let newest = attempt(tracker.start("new.desktop", "A", &[], now, || {
        Err(LaunchError::MissingProgram)
    }));
    assert_eq!(newest, MAX_RECORDS as u64 + 1);
    assert_eq!(tracker.records.len(), MAX_RECORDS);
    assert!(!tracker.records.contains_key("0.desktop"));
    assert!(tracker.records.contains_key("new.desktop"));
}

#[test]
fn surviving_children_bound_retries_per_application() {
    let mut now = Instant::now();
    let mut tracker = Running::new();
    let mut current = attempt(tracker.start("a.desktop", "A", &[], now, || Ok(waiting_child(0))));
    for _ in 1..MAX_APP_CHILDREN {
        now += START_TIMEOUT;
        tracker.tick(now);
        current =
            attempt(tracker.retry("a.desktop", "A", current, &[], now, || Ok(waiting_child(0))));
    }
    now += START_TIMEOUT;
    tracker.tick(now);
    assert_eq!(
        tracker.retry("a.desktop", "A", current, &[], now, never_spawn),
        Outcome::Rejected(Rejection::TooManyAttempts)
    );
    assert_eq!(tracker.children.len(), MAX_APP_CHILDREN);
    // The guard observes children; it never terminates an older process.
    assert!(tracker
        .children
        .iter_mut()
        .all(|tracked| tracked.child.try_wait().unwrap().is_none()));
}

#[test]
fn all_surviving_children_share_one_global_bound() {
    let now = Instant::now();
    let mut tracker = Running::new();
    let mut attempts = Vec::new();
    for index in 0..MAX_RECORDS {
        let id = format!("{index}.desktop");
        let first = attempt(tracker.start(&id, "A", &[], now, || Ok(waiting_child(0))));
        attempts.push((id, first));
    }
    tracker.tick(now + START_TIMEOUT);
    for (id, first) in &mut attempts {
        *first = attempt(
            tracker.retry(id, "A", *first, &[], now + START_TIMEOUT, || {
                Ok(waiting_child(0))
            }),
        );
    }
    assert_eq!(tracker.children.len(), MAX_CHILDREN);
    tracker.tick(now + START_TIMEOUT * 2);
    assert_eq!(
        tracker.retry(
            &attempts[0].0,
            "A",
            attempts[0].1,
            &[],
            now + START_TIMEOUT * 2,
            never_spawn
        ),
        Outcome::Rejected(Rejection::TooManyChildren)
    );
}

#[test]
fn snapshots_coalesce_and_names_are_bounded_without_hiding_identity_errors() {
    let now = Instant::now();
    let mut tracker = Tracker::new();
    for id in ["", "bad\nidentity"] {
        assert_eq!(
            tracker.start(id, "Name", &[], now, never_spawn),
            Outcome::Rejected(Rejection::InvalidIdentity)
        );
    }
    assert_eq!(
        tracker.start(&"x".repeat(4097), "Name", &[], now, never_spawn),
        Outcome::Rejected(Rejection::InvalidIdentity)
    );
    tracker.start(
        "b.desktop",
        &format!("{}\n", "Я".repeat(200)),
        &[],
        now,
        || Err(LaunchError::MissingDesktopEntry),
    );
    tracker.start("a.desktop", "Name", &[], now, || {
        Err(LaunchError::MissingProgram)
    });
    let update = tracker.take_update().unwrap();
    assert_eq!(
        update
            .iter()
            .map(|status| status.desktop_id.as_str())
            .collect::<Vec<_>>(),
        ["a.desktop", "b.desktop"]
    );
    assert_eq!(update[1].name.chars().count(), 96);
    assert!(!update[1].name.chars().any(char::is_control));
    assert_eq!(tracker.take_update(), None);
    assert_eq!(tracker.next_wakeup(now), None);
}

#[test]
fn attempt_ids_cannot_wrap_or_reuse_stale_guards() {
    let now = Instant::now();
    let mut tracker = Tracker::new();
    tracker.next_attempt = u64::MAX;
    assert_eq!(
        tracker.start("a.desktop", "A", &[], now, never_spawn),
        Outcome::Rejected(Rejection::AttemptOverflow)
    );
    assert!(tracker.records.is_empty());
    assert_eq!(tracker.take_update(), None);
}
