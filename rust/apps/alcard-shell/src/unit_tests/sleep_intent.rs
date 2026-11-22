use super::*;
use locking::Message as LockMessage;
use sleep::Message as SleepMessage;

fn ready(coordinator: &mut Coordinator, epoch: u64, now: Instant) {
    coordinator.receive(
        SleepMessage::Snapshot(Snapshot {
            epoch,
            capability: Capability::Allowed,
            monitor: MonitorState::Ready,
            delay_ms: 5_000,
        }),
        now,
    );
}

fn setup() -> (Coordinator, Instant) {
    let now = Instant::now();
    let mut coordinator = Coordinator::default();
    ready(&mut coordinator, 1, now);
    (coordinator, now)
}

fn prepare(coordinator: &mut Coordinator, request: u64, now: Instant) {
    assert!(coordinator.begin(request, now));
    let effects = coordinator.receive(
        SleepMessage::Prepared {
            request,
            epoch: 1,
            result: Ok(()),
        },
        now,
    );
    assert!(
        matches!(effects.as_slice(), [Effect::NeedLock(LockPurpose::Own(id))] if *id == request)
    );
}

fn bind(coordinator: &mut Coordinator, request: u64, id: u64, now: Instant) {
    prepare(coordinator, request, now);
    coordinator.lock_started(LockPurpose::Own(request), id, now);
}

fn locked(coordinator: &mut Coordinator, id: u64) {
    coordinator.receive_lock(LockMessage::Changed(State {
        id,
        phase: Phase::Locked,
    }));
}

fn sensitive(effects: &[Effect]) -> usize {
    effects
        .iter()
        .filter(|effect| matches!(effect, Effect::Commit { .. } | Effect::ReleaseDelay { .. }))
        .count()
}

fn commit(coordinator: &mut Coordinator, request: u64, id: u64, now: Instant) -> Permit {
    bind(coordinator, request, id, now);
    locked(coordinator, id);
    let effects = coordinator.flush(now);
    assert_eq!(sensitive(&effects), 1);
    effects
        .into_iter()
        .find_map(|effect| match effect {
            Effect::Commit { permit, .. } => Some(permit),
            _ => None,
        })
        .expect("one commit")
}

#[test]
fn only_ready_noninteractive_capability_starts_a_fresh_request() {
    let now = Instant::now();
    let mut coordinator = Coordinator::default();
    assert!(!coordinator.begin(1, now));
    for capability in [
        Capability::Checking,
        Capability::Authentication,
        Capability::Denied,
        Capability::Unsupported,
        Capability::Blocked,
        Capability::Unavailable,
    ] {
        coordinator.receive(
            SleepMessage::Snapshot(Snapshot {
                epoch: 1,
                capability,
                monitor: MonitorState::Ready,
                delay_ms: 5_000,
            }),
            now,
        );
        assert!(!coordinator.begin(1, now));
    }
    ready(&mut coordinator, 1, now);
    assert!(coordinator.begin(1, now));
    assert!(!coordinator.begin(2, now));
    assert_eq!(coordinator.deadline(), Some(now + PREPARE_TIMEOUT));
}

#[test]
fn preflight_precedes_lock_and_stale_prepared_messages_cannot_bind() {
    let (mut coordinator, now) = setup();
    assert!(coordinator.begin(1, now));
    for (request, epoch) in [(2, 1), (1, 0), (1, 2)] {
        assert!(coordinator
            .receive(
                SleepMessage::Prepared {
                    request,
                    epoch,
                    result: Ok(())
                },
                now
            )
            .is_empty());
    }
    assert_eq!(coordinator.progress().unwrap().stage, Stage::Preparing);
    assert_eq!(sensitive(&coordinator.flush(now)), 0);
    coordinator.receive(
        SleepMessage::Prepared {
            request: 1,
            epoch: 1,
            result: Err(Failure::Blocked),
        },
        now,
    );
    assert_eq!(
        coordinator.progress().unwrap().failure,
        Some(Failure::Blocked)
    );
    locked(&mut coordinator, 9);
    assert_eq!(sensitive(&coordinator.flush(now)), 0);
}

#[test]
fn locked_then_unlock_or_uncertainty_in_one_batch_never_commits() {
    for later in [
        LockMessage::Unlocked { id: 8 },
        LockMessage::Changed(State {
            id: 8,
            phase: Phase::Uncertain,
        }),
    ] {
        let (mut coordinator, now) = setup();
        bind(&mut coordinator, 1, 8, now);
        locked(&mut coordinator, 8);
        coordinator.receive_lock(later);
        assert_eq!(sensitive(&coordinator.flush(now)), 0);
        assert_eq!(coordinator.progress().unwrap().stage, Stage::Failed);
        locked(&mut coordinator, 8);
        assert_eq!(sensitive(&coordinator.flush(now)), 0);
    }
}

#[test]
fn exact_id_is_required_and_a_newer_lifecycle_terminates_the_old_intent() {
    let (mut coordinator, now) = setup();
    bind(&mut coordinator, 1, 8, now);
    locked(&mut coordinator, 7);
    coordinator.receive_lock(LockMessage::Unlocked { id: 7 });
    assert_eq!(sensitive(&coordinator.flush(now)), 0);
    locked(&mut coordinator, 9);
    assert_eq!(sensitive(&coordinator.flush(now)), 0);
    assert_eq!(coordinator.progress().unwrap().stage, Stage::Failed);
    locked(&mut coordinator, 8);
    assert_eq!(sensitive(&coordinator.flush(now)), 0);
}

#[test]
fn existing_canonical_lock_and_recovered_zero_are_actual_proof_only() {
    for id in [0, 4] {
        let (mut coordinator, now) = setup();
        locked(&mut coordinator, id);
        bind(&mut coordinator, 1, id, now);
        let effects = coordinator.flush(now);
        let permit = effects
            .into_iter()
            .find_map(|effect| match effect {
                Effect::Commit { permit, .. } => Some(permit),
                _ => None,
            })
            .expect("canonical lock satisfies binding");
        assert!(permit.valid());
        locked(&mut coordinator, id + 1);
        assert!(!permit.valid());
        assert_eq!(coordinator.progress().unwrap().stage, Stage::Unknown);
    }
    let (mut coordinator, now) = setup();
    coordinator.receive_lock(LockMessage::Presence(true));
    coordinator.receive_lock(LockMessage::Journal(true));
    bind(&mut coordinator, 1, 0, now);
    assert_eq!(sensitive(&coordinator.flush(now)), 0);
    locked(&mut coordinator, 0);
    assert_eq!(sensitive(&coordinator.flush(now)), 0);
}

#[test]
fn flush_commits_once_and_cancel_revokes_an_already_queued_permit() {
    let (mut coordinator, now) = setup();
    let permit = commit(&mut coordinator, 1, 8, now);
    assert_eq!(coordinator.progress().unwrap().stage, Stage::Queued);
    assert!(coordinator.deadline().is_none());
    assert_eq!(sensitive(&coordinator.flush(now)), 0);
    let effects = coordinator.cancel(1);
    assert!(effects
        .iter()
        .any(|effect| matches!(effect, Effect::Cancel { request: 1 })));
    assert!(!permit.valid());
    assert_eq!(coordinator.progress().unwrap().stage, Stage::Unknown);
    assert!(!coordinator.begin(2, now));
    coordinator.receive(
        SleepMessage::Finished {
            request: 1,
            epoch: 1,
            result: Err(Failure::Cancelled),
        },
        now,
    );
    assert_eq!(coordinator.progress().unwrap().stage, Stage::Cancelled);
}

#[test]
fn cancel_and_timeout_cannot_be_revived_by_a_late_locked_receipt() {
    for cancel in [true, false] {
        let (mut coordinator, now) = setup();
        bind(&mut coordinator, 1, 8, now);
        if cancel {
            coordinator.cancel(1);
        } else {
            coordinator.flush(now + LOCK_TIMEOUT);
        }
        locked(&mut coordinator, 8);
        assert_eq!(sensitive(&coordinator.flush(now + LOCK_TIMEOUT)), 0);
        assert!(!coordinator.progress().unwrap().busy());
        assert!(coordinator.deadline().is_none());
    }
}

#[test]
fn late_prepared_or_lock_started_cannot_extend_expired_deadlines() {
    let (mut coordinator, now) = setup();
    assert!(coordinator.begin(1, now));
    let effects = coordinator.receive(
        SleepMessage::Prepared {
            request: 1,
            epoch: 1,
            result: Ok(()),
        },
        now + PREPARE_TIMEOUT,
    );
    assert!(!effects
        .iter()
        .any(|effect| matches!(effect, Effect::NeedLock(_))));
    assert_eq!(
        coordinator.progress().unwrap().failure,
        Some(Failure::Timeout)
    );
    assert!(coordinator.begin(2, now + PREPARE_TIMEOUT));
    coordinator.receive(
        SleepMessage::Prepared {
            request: 2,
            epoch: 1,
            result: Ok(()),
        },
        now + PREPARE_TIMEOUT,
    );
    coordinator.lock_started(LockPurpose::Own(2), 8, now + PREPARE_TIMEOUT + LOCK_TIMEOUT);
    locked(&mut coordinator, 8);
    assert_eq!(
        sensitive(&coordinator.flush(now + PREPARE_TIMEOUT + LOCK_TIMEOUT)),
        0
    );
    assert_eq!(
        coordinator.progress().unwrap().failure,
        Some(Failure::Timeout)
    );
}

#[test]
fn accepted_reply_and_lone_false_never_report_cycle_completion() {
    let (mut coordinator, now) = setup();
    commit(&mut coordinator, 1, 8, now);
    coordinator.receive(
        SleepMessage::Finished {
            request: 1,
            epoch: 1,
            result: Ok(()),
        },
        now,
    );
    assert_eq!(coordinator.progress().unwrap().stage, Stage::Requested);
    let effects = coordinator.receive(
        SleepMessage::CycleEnded {
            epoch: 1,
            cycle: None,
        },
        now,
    );
    assert!(effects
        .iter()
        .any(|effect| matches!(effect, Effect::Resume)));
    assert_eq!(coordinator.progress().unwrap().stage, Stage::Requested);
    assert!(coordinator
        .receive(
            SleepMessage::CycleEnded {
                epoch: 1,
                cycle: None
            },
            now
        )
        .is_empty());
    ready(&mut coordinator, 1, now);
    assert!(!coordinator.begin(2, now));
}

#[test]
fn matching_true_false_ends_the_cycle_without_a_permanent_latch() {
    let (mut coordinator, now) = setup();
    let permit = commit(&mut coordinator, 1, 8, now);
    let effects = coordinator.receive(
        SleepMessage::Preparing {
            epoch: 1,
            cycle: 10,
        },
        now,
    );
    assert_eq!(sensitive(&effects), 0);
    assert!(!permit.valid());
    let effects = coordinator.flush(now);
    assert_eq!(sensitive(&effects), 1);
    assert!(matches!(
        effects.as_slice(),
        [Effect::ReleaseDelay {
            epoch: 1,
            cycle: 10,
            ..
        }]
    ));
    for (epoch, cycle) in [(0, Some(10)), (1, Some(9)), (1, None)] {
        assert!(coordinator
            .receive(SleepMessage::CycleEnded { epoch, cycle }, now)
            .is_empty());
        assert!(coordinator.progress().unwrap().busy());
    }
    let effects = coordinator.receive(
        SleepMessage::CycleEnded {
            epoch: 1,
            cycle: Some(10),
        },
        now,
    );
    assert!(effects
        .iter()
        .any(|effect| matches!(effect, Effect::Resume)));
    assert_eq!(coordinator.progress().unwrap().stage, Stage::Ended);
    assert_eq!(coordinator.snapshot().monitor, MonitorState::Connecting);
    coordinator.receive(
        SleepMessage::Finished {
            request: 1,
            epoch: 1,
            result: Err(Failure::Unknown),
        },
        now,
    );
    assert_eq!(coordinator.progress().unwrap().stage, Stage::Ended);
    ready(&mut coordinator, 1, now);
    assert!(coordinator.begin(2, now));
}

#[test]
fn external_transition_waits_for_exact_lock_and_releases_only_at_flush() {
    for allow_own_commit in [true, false] {
        let (mut coordinator, now) = setup();
        let purpose = LockPurpose::External {
            epoch: 1,
            cycle: 10,
        };
        let effects = coordinator.receive(
            SleepMessage::Preparing {
                epoch: 1,
                cycle: 10,
            },
            now,
        );
        assert!(matches!(effects.as_slice(), [Effect::NeedLock(found)] if *found == purpose));
        coordinator.lock_started(purpose, 8, now);
        coordinator.receive_lock(LockMessage::Presence(true));
        coordinator.receive_lock(LockMessage::Journal(true));
        assert_eq!(sensitive(&coordinator.flush_when(now, allow_own_commit)), 0);
        locked(&mut coordinator, 7);
        assert_eq!(sensitive(&coordinator.flush_when(now, allow_own_commit)), 0);
        locked(&mut coordinator, 8);
        // Waiting for a battery sample must never hold an external cycle's
        // bounded delay once its exact canonical lock has been confirmed.
        let effects = coordinator.flush_when(now, allow_own_commit);
        let permit = effects
            .into_iter()
            .find_map(|effect| match effect {
                Effect::ReleaseDelay {
                    epoch: 1,
                    cycle: 10,
                    permit,
                } => Some(permit),
                _ => None,
            })
            .expect("one matching release");
        assert!(permit.valid());
        assert_eq!(sensitive(&coordinator.flush_when(now, allow_own_commit)), 0);
        coordinator.receive_lock(LockMessage::Unlocked { id: 8 });
        assert!(!permit.valid());
    }
}

#[test]
fn external_same_batch_unlock_or_false_revokes_the_pending_release() {
    for end_cycle in [true, false] {
        let (mut coordinator, now) = setup();
        coordinator.receive(
            SleepMessage::Preparing {
                epoch: 1,
                cycle: 10,
            },
            now,
        );
        coordinator.lock_started(
            LockPurpose::External {
                epoch: 1,
                cycle: 10,
            },
            8,
            now,
        );
        locked(&mut coordinator, 8);
        if end_cycle {
            coordinator.receive(
                SleepMessage::CycleEnded {
                    epoch: 1,
                    cycle: Some(10),
                },
                now,
            );
        } else {
            coordinator.receive_lock(LockMessage::Unlocked { id: 8 });
        }
        assert_eq!(sensitive(&coordinator.flush(now)), 0);
    }
}

#[test]
fn failed_external_lock_is_not_retried_by_flush_or_duplicate_messages() {
    let (mut coordinator, now) = setup();
    let purpose = LockPurpose::External {
        epoch: 1,
        cycle: 10,
    };
    coordinator.receive(
        SleepMessage::Preparing {
            epoch: 1,
            cycle: 10,
        },
        now,
    );
    coordinator.lock_started(purpose, 8, now);
    coordinator.receive_lock(LockMessage::Changed(State {
        id: 8,
        phase: Phase::Failed,
    }));
    coordinator.lock_failed(purpose);
    for _ in 0..4 {
        assert!(coordinator.flush(now).is_empty());
        assert!(coordinator
            .receive(
                SleepMessage::Preparing {
                    epoch: 1,
                    cycle: 10
                },
                now
            )
            .is_empty());
    }
    // A genuinely new lifecycle can still protect the already external
    // transition, without this coordinator launching another locker.
    locked(&mut coordinator, 9);
    assert!(matches!(
        coordinator.flush(now).as_slice(),
        [Effect::ReleaseDelay { .. }]
    ));
}

#[test]
fn known_pending_lock_is_reused_for_external_sleep_without_a_duplicate() {
    let (mut coordinator, now) = setup();
    coordinator.receive_lock(LockMessage::Changed(State {
        id: 8,
        phase: Phase::Starting,
    }));
    assert!(coordinator
        .receive(
            SleepMessage::Preparing {
                epoch: 1,
                cycle: 10
            },
            now
        )
        .is_empty());
    assert_eq!(sensitive(&coordinator.flush(now)), 0);
    locked(&mut coordinator, 8);
    assert!(matches!(
        coordinator.flush(now).as_slice(),
        [Effect::ReleaseDelay { .. }]
    ));
}

#[test]
fn external_cycle_winning_before_commit_cancels_the_duplicate_own_request() {
    let (mut coordinator, now) = setup();
    bind(&mut coordinator, 1, 8, now);
    locked(&mut coordinator, 8);
    coordinator.receive(
        SleepMessage::Preparing {
            epoch: 1,
            cycle: 10,
        },
        now,
    );
    assert_eq!(coordinator.progress().unwrap().stage, Stage::Cancelled);
    let effects = coordinator.flush(now);
    assert!(!effects
        .iter()
        .any(|effect| matches!(effect, Effect::Commit { .. })));
    assert!(effects
        .iter()
        .any(|effect| matches!(effect, Effect::ReleaseDelay { .. })));
}

#[test]
fn epoch_loss_revokes_queued_work_and_new_ready_recovers_without_success() {
    let (mut coordinator, now) = setup();
    let permit = commit(&mut coordinator, 1, 8, now);
    coordinator.receive(SleepMessage::Lost { epoch: 1 }, now);
    assert!(!permit.valid());
    assert_eq!(coordinator.progress().unwrap().stage, Stage::Unknown);
    coordinator.receive(
        SleepMessage::Finished {
            request: 1,
            epoch: 1,
            result: Ok(()),
        },
        now,
    );
    assert_eq!(coordinator.progress().unwrap().stage, Stage::Unknown);
    ready(&mut coordinator, 2, now);
    assert_eq!(
        coordinator.progress(),
        Some(Progress {
            request: 1,
            stage: Stage::Failed,
            failure: Some(Failure::Unknown),
        })
    );
    assert!(coordinator
        .receive(
            SleepMessage::Preparing {
                epoch: 1,
                cycle: 10
            },
            now
        )
        .is_empty());
    assert!(coordinator
        .receive(
            SleepMessage::CycleEnded {
                epoch: 1,
                cycle: Some(10)
            },
            now
        )
        .is_empty());
    assert!(coordinator.begin(2, now));
}

#[test]
fn preparing_snapshot_revokes_commit_before_its_cycle_message_arrives() {
    let (mut coordinator, now) = setup();
    let permit = commit(&mut coordinator, 1, 8, now);
    let mut snapshot = coordinator.snapshot();
    snapshot.monitor = MonitorState::Preparing;
    coordinator.receive(SleepMessage::Snapshot(snapshot), now);
    assert!(!permit.valid());
    assert_eq!(sensitive(&coordinator.flush(now)), 0);
}

#[test]
fn rejected_submission_and_coordinator_drop_revoke_queued_permissions() {
    let (mut coordinator, now) = setup();
    let permit = commit(&mut coordinator, 1, 8, now);
    coordinator.submission_failed(1, Failure::Unavailable);
    assert!(!permit.valid());
    assert_eq!(coordinator.progress().unwrap().stage, Stage::Failed);
    let (mut coordinator, now) = setup();
    let permit = commit(&mut coordinator, 1, 8, now);
    drop(coordinator);
    assert!(!permit.valid());
}
