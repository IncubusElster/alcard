use super::*;

fn policy() -> Policy {
    let mut policy = Policy::new(IdlePreferences::default());
    policy.add_seat(7);
    policy
}

fn changed(id: u64, phase: Phase) -> Message {
    Message::Changed(State { id, phase })
}

fn all_idle(policy: &mut Policy, seat: u32) {
    policy.notify(seat, Timer::Lock, true);
    policy.notify(seat, Timer::ScreenOff, true);
}

#[test]
fn off_waits_for_native_lock_and_never_accepts_advisory_presence() {
    let mut policy = policy();
    all_idle(&mut policy, 7);
    assert!(policy.evaluate(false).request_lock);
    for message in [
        Message::Presence(true),
        Message::Journal(true),
        changed(1, Phase::Starting),
        changed(1, Phase::Uncertain),
    ] {
        assert!(!policy.receive_lock(&message));
        assert_eq!(policy.evaluate(true), Decision::default());
    }
    assert!(!policy.receive_lock(&changed(1, Phase::Locked)));
    assert_eq!(
        policy.evaluate(true),
        Decision {
            request_lock: false,
            screen_off: true
        }
    );
    policy.receive_lock(&changed(1, Phase::Uncertain));
    assert!(!policy.evaluate(true).screen_off);
    policy.receive_lock(&changed(1, Phase::Locked));
    assert!(policy.evaluate(true).screen_off);
}

#[test]
fn resumed_activity_defeats_a_late_lock_receipt_without_cancelling_lock() {
    for timer in [Timer::Lock, Timer::ScreenOff] {
        let mut policy = policy();
        all_idle(&mut policy, 7);
        assert!(policy.evaluate(false).request_lock);
        policy.notify(7, timer, false);
        policy.receive_lock(&changed(1, Phase::Locked));
        assert_eq!(policy.evaluate(true), Decision::default());
        // A still-locked session can sleep again after fresh idle.
        all_idle(&mut policy, 7);
        assert!(policy.evaluate(true).screen_off);
        assert!(!policy.evaluate(true).request_lock);
    }
}

#[test]
fn one_attempt_per_idle_period_even_when_submission_or_lock_fails() {
    let mut policy = policy();
    all_idle(&mut policy, 7);
    assert!(policy.evaluate(false).request_lock);
    assert!(!policy.evaluate(false).request_lock);
    policy.receive_lock(&changed(1, Phase::Failed));
    all_idle(&mut policy, 7); // Duplicate idle events are not a new period.
    assert_eq!(policy.evaluate(false), Decision::default());
    policy.notify(7, Timer::Lock, false);
    all_idle(&mut policy, 7);
    assert!(policy.evaluate(false).request_lock);
    policy.receive_lock(&changed(2, Phase::Locked));
    assert!(policy.evaluate(true).screen_off);
}

#[test]
fn every_seat_must_be_idle_and_removed_seat_events_are_ignored() {
    let mut policy = policy();
    policy.add_seat(8);
    all_idle(&mut policy, 7);
    assert_eq!(policy.evaluate(false), Decision::default());
    all_idle(&mut policy, 8);
    assert!(policy.evaluate(false).request_lock);
    policy.receive_lock(&changed(1, Phase::Locked));
    assert!(policy.evaluate(true).screen_off);
    policy.notify(8, Timer::ScreenOff, false);
    assert!(!policy.evaluate(true).screen_off);
    policy.remove_seat(8);
    assert!(policy.evaluate(true).screen_off);
    policy.notify(8, Timer::Lock, false);
    assert!(policy.evaluate(true).screen_off);
    policy.remove_seat(7);
    assert_eq!(policy.evaluate(true), Decision::default());
    policy.add_seat(9);
    assert_eq!(policy.evaluate(true), Decision::default());
}

#[test]
fn canonical_unlock_rearms_and_stale_receipts_do_not_clear_a_new_lock() {
    let mut policy = policy();
    all_idle(&mut policy, 7);
    policy.receive_lock(&changed(1, Phase::Locked));
    assert!(!policy.receive_lock(&Message::Unlocked { id: 2 }));
    assert!(policy.evaluate(true).screen_off);
    assert!(policy.receive_lock(&Message::Unlocked { id: 1 }));
    assert_eq!(policy.evaluate(false), Decision::default());
    assert!(!policy.receive_lock(&changed(1, Phase::Locked)));
    assert!(!policy.receive_lock(&Message::Unlocked { id: 1 }));
    all_idle(&mut policy, 7);
    assert!(policy.evaluate(false).request_lock);
    policy.receive_lock(&changed(2, Phase::Locked));
    policy.receive_lock(&changed(1, Phase::Starting));
    assert!(!policy.receive_lock(&Message::Unlocked { id: 1 }));
    assert!(policy.evaluate(true).screen_off);
}

#[test]
fn reconfiguration_starts_a_new_period_but_preserves_confirmed_lock() {
    let mut policy = policy();
    all_idle(&mut policy, 7);
    policy.receive_lock(&changed(1, Phase::Locked));
    assert!(policy.evaluate(true).screen_off);
    policy.reconfigure(IdlePreferences::default());
    assert_eq!(policy.evaluate(true), Decision::default());
    all_idle(&mut policy, 7);
    assert!(policy.evaluate(true).screen_off);
    policy.reconfigure(IdlePreferences {
        lock_after_seconds: 0,
        screen_off_after_seconds: 0,
    });
    all_idle(&mut policy, 7);
    assert_eq!(policy.evaluate(true), Decision::default());
    assert!(policy.receive_lock(&Message::Unlocked { id: 1 }));
    all_idle(&mut policy, 7);
    assert_eq!(policy.evaluate(false), Decision::default());
}

#[test]
fn manual_and_recovery_receipts_work_without_an_automatic_request() {
    let mut policy = policy();
    all_idle(&mut policy, 7);
    policy.receive_lock(&changed(4, Phase::Locked));
    assert!(policy.evaluate(true).screen_off);
    assert!(!policy.evaluate(true).request_lock);
    assert!(policy.receive_lock(&Message::Unlocked { id: 4 }));
    all_idle(&mut policy, 7);
    policy.receive_lock(&changed(0, Phase::Locked));
    assert!(policy.evaluate(true).screen_off);
    assert!(!policy.receive_lock(&Message::Unlocked { id: 4 }));
    assert!(policy.receive_lock(&Message::Unlocked { id: 0 }));
    assert_eq!(policy.evaluate(false), Decision::default());
}

#[test]
fn busy_or_absent_seats_cannot_start_an_automatic_locker() {
    let mut policy = Policy::new(IdlePreferences::default());
    assert_eq!(policy.evaluate(false), Decision::default());
    policy.notify(42, Timer::Lock, true);
    assert_eq!(policy.evaluate(false), Decision::default());
    policy.add_seat(7);
    all_idle(&mut policy, 7);
    assert_eq!(policy.evaluate(true), Decision::default());
    assert!(policy.evaluate(false).request_lock);
}
