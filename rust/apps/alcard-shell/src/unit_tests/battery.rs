use super::*;

fn observation(now: Instant, sequence: u64, percent: u8) -> BatteryObservation {
    BatteryObservation {
        state: BatteryState::Present {
            percent: Some(percent),
            charge: ChargeState::Discharging,
        },
        observed_at: now,
        sequence,
        source_epoch: Some(1),
        external_power: None,
    }
}

fn automatic() -> Policy {
    Policy::new(BatteryPreferences {
        suspend_on_critical: true,
        ..BatteryPreferences::default()
    })
}

fn requested(effects: &[Effect]) -> u64 {
    effects
        .iter()
        .find_map(|effect| match effect {
            Effect::RequestSuspend { episode } => Some(*episode),
            _ => None,
        })
        .expect("one automatic request")
}

fn only_silent_updates(effects: &[Effect]) {
    assert!(
        effects.iter().all(|effect| matches!(
            effect,
            Effect::Warn {
                announce: false,
                ..
            }
        )),
        "unexpected action or new announcement: {effects:?}"
    );
}

#[test]
fn default_warnings_have_exact_boundaries_no_sleep_and_recovery_hysteresis() {
    let now = Instant::now();
    let mut policy = Policy::new(BatteryPreferences::default());
    assert!(policy.observe(observation(now, 1, 21), now).is_empty());
    assert!(matches!(
        policy.observe(observation(now, 2, 20), now).as_slice(),
        [Effect::Warn {
            level: Level::Low,
            percent: 20,
            ..
        }]
    ));
    for (sequence, percent) in [(3, 21), (4, 20), (5, 6)] {
        only_silent_updates(&policy.observe(observation(now, sequence, percent), now));
    }
    assert!(matches!(
        policy.observe(observation(now, 6, 5), now).as_slice(),
        [Effect::Warn {
            level: Level::Critical,
            percent: 5,
            ..
        }]
    ));
    only_silent_updates(&policy.observe(observation(now, 7, 0), now));
    only_silent_updates(&policy.observe(observation(now, 8, 22), now));
    assert_eq!(
        policy.observe(observation(now, 9, 23), now),
        [Effect::ClearWarning]
    );
    assert!(matches!(
        policy.observe(observation(now, 10, 20), now).as_slice(),
        [Effect::Warn {
            level: Level::Low,
            ..
        }]
    ));
}

#[test]
fn initially_critical_warns_once_and_a_failed_attempt_never_automatically_retries() {
    let now = Instant::now();
    let mut policy = automatic();
    let effects = policy.observe(observation(now, 1, 5), now);
    assert!(matches!(
        effects.as_slice(),
        [
            Effect::Warn {
                level: Level::Critical,
                ..
            },
            Effect::RequestSuspend { .. }
        ]
    ));
    let episode = requested(&effects);
    assert!(policy.is_action_current(episode, now));
    policy.action_finished(episode);
    assert!(!policy.is_action_current(episode, now));
    for sequence in 2..5 {
        only_silent_updates(&policy.observe(observation(now, sequence, 4), now));
    }
    assert!(policy.action_latched());
}

#[test]
fn invalid_ambiguous_external_and_nondischarging_data_cannot_start_an_action() {
    let now = Instant::now();
    let base = observation(now, 1, 0);
    for sample in [
        BatteryObservation {
            state: BatteryState::Unknown,
            ..base
        },
        BatteryObservation {
            state: BatteryState::Absent,
            ..base
        },
        BatteryObservation {
            state: BatteryState::Present {
                percent: None,
                charge: ChargeState::Discharging,
            },
            ..base
        },
        BatteryObservation {
            state: BatteryState::Present {
                percent: Some(0),
                charge: ChargeState::Unknown,
            },
            ..base
        },
        BatteryObservation {
            state: BatteryState::Present {
                percent: Some(0),
                charge: ChargeState::NotCharging,
            },
            ..base
        },
        BatteryObservation {
            state: BatteryState::Present {
                percent: Some(0),
                charge: ChargeState::Charging,
            },
            ..base
        },
        BatteryObservation {
            state: BatteryState::Present {
                percent: Some(0),
                charge: ChargeState::Full,
            },
            ..base
        },
        BatteryObservation {
            source_epoch: None,
            ..base
        },
        BatteryObservation {
            external_power: Some(true),
            ..base
        },
        BatteryObservation {
            observed_at: now - MAX_SAMPLE_AGE,
            ..base
        },
        BatteryObservation {
            observed_at: now + Duration::from_secs(1),
            ..base
        },
    ] {
        let mut policy = automatic();
        assert!(
            policy.observe(sample, now).is_empty(),
            "accepted {sample:?}"
        );
        assert!(!policy.action_latched());
    }
}

#[test]
fn stale_or_unknown_telemetry_cancels_a_pending_action_without_rearming_it() {
    let now = Instant::now();
    for unknown in [false, true] {
        let mut policy = automatic();
        let episode = requested(&policy.observe(observation(now, 1, 5), now));
        assert_eq!(policy.deadline(), Some(now + MAX_SAMPLE_AGE));
        let effects = if unknown {
            policy.observe(
                BatteryObservation {
                    state: BatteryState::Unknown,
                    ..observation(now, 2, 5)
                },
                now,
            )
        } else {
            policy.tick(now + MAX_SAMPLE_AGE)
        };
        assert!(effects.contains(&Effect::CancelSuspend { episode }));
        assert!(effects.contains(&Effect::ClearWarning));
        assert!(policy.deadline().is_none());
        let later = now + MAX_SAMPLE_AGE;
        assert!(matches!(
            policy.observe(observation(later, 3, 5), later).as_slice(),
            [Effect::Warn {
                level: Level::Critical,
                announce: false,
                ..
            }]
        ));
        assert!(policy.action_latched());
    }
}

#[test]
fn source_change_cancels_the_old_token_without_transferring_a_retry_to_the_new_source() {
    let now = Instant::now();
    let mut policy = automatic();
    let episode = requested(&policy.observe(observation(now, 1, 5), now));
    let effects = policy.observe(
        BatteryObservation {
            source_epoch: Some(2),
            ..observation(now, 2, 4)
        },
        now,
    );
    assert!(effects.contains(&Effect::CancelSuspend { episode }));
    assert!(!effects
        .iter()
        .any(|effect| matches!(effect, Effect::RequestSuspend { .. })));
    assert!(!policy.is_action_current(episode, now));
    assert!(policy.action_latched());
}

#[test]
fn charging_cancels_and_a_new_discharge_episode_can_request_once() {
    let now = Instant::now();
    for external in [false, true] {
        let mut policy = automatic();
        let first = requested(&policy.observe(observation(now, 1, 5), now));
        let mut charging = observation(now, 2, 5);
        if external {
            charging.external_power = Some(true);
        } else {
            charging.state = BatteryState::Present {
                percent: Some(5),
                charge: ChargeState::Charging,
            };
        }
        let effects = policy.observe(charging, now);
        assert!(effects.contains(&Effect::CancelSuspend { episode: first }));
        assert!(!policy.action_latched());
        let second = requested(&policy.observe(observation(now, 3, 5), now));
        assert_ne!(first, second);
        policy.action_finished(first);
        assert!(policy.is_action_current(second, now));
    }
}

#[test]
fn manual_resume_blocks_even_a_first_automatic_attempt_and_rejects_pre_resume_samples() {
    let before = Instant::now();
    let after = before + Duration::from_secs(1);
    let mut policy = automatic();
    policy.observe(observation(before, 1, 50), before);
    policy.resume(after);
    assert!(policy.observe(observation(before, 2, 5), after).is_empty());
    let effects = policy.observe(observation(after, 3, 5), after);
    assert!(matches!(
        effects.as_slice(),
        [Effect::Warn {
            level: Level::Critical,
            ..
        }]
    ));
    assert_eq!(policy.inhibit(), Some(Inhibit::Resume));
    policy.observe(observation(after, 4, 8), after);
    assert_eq!(policy.inhibit(), None);
    requested(&policy.observe(observation(after, 5, 5), after));

    let mut earlier = automatic();
    earlier.resume(after);
    earlier.observe(observation(after, 1, 20), after);
    assert_eq!(earlier.inhibit(), None);
    requested(&earlier.observe(observation(after, 2, 5), after));
}

#[test]
fn equal_samples_refresh_age_but_duplicate_sequences_and_old_events_do_not() {
    let now = Instant::now();
    let mut policy = automatic();
    let episode = requested(&policy.observe(observation(now, 1, 5), now));
    let later = now + Duration::from_secs(10);
    policy.observe(observation(later, 2, 5), later);
    assert_eq!(policy.deadline(), Some(later + MAX_SAMPLE_AGE));
    assert!(policy.observe(observation(later, 1, 90), later).is_empty());
    assert!(policy.is_action_current(episode, now + MAX_SAMPLE_AGE));
    assert!(!policy.is_action_current(episode, later + MAX_SAMPLE_AGE));
}

#[test]
fn disabling_auto_sleep_cancels_and_reenabling_does_not_repeat_consumed_attempts() {
    let now = Instant::now();
    let mut policy = automatic();
    let episode = requested(&policy.observe(observation(now, 1, 5), now));
    let effects = policy.reconfigure(BatteryPreferences::default(), now);
    assert!(effects.contains(&Effect::CancelSuspend { episode }));
    policy.reconfigure(
        BatteryPreferences {
            suspend_on_critical: true,
            ..BatteryPreferences::default()
        },
        now,
    );
    only_silent_updates(&policy.observe(observation(now, 2, 4), now));
    assert!(policy.action_latched());
}

#[test]
fn a_low_threshold_of_one_hundred_does_not_mistake_its_boundary_for_recovery() {
    let now = Instant::now();
    let mut policy = Policy::new(BatteryPreferences {
        low_percent: 100,
        critical_percent: 99,
        suspend_on_critical: false,
    });
    assert!(matches!(
        policy.observe(observation(now, 1, 100), now).as_slice(),
        [Effect::Warn {
            level: Level::Low,
            ..
        }]
    ));
    assert!(policy.observe(observation(now, 2, 100), now).is_empty());
}

#[test]
fn restored_attempt_and_resume_have_distinct_recovery_requirements() {
    let now = Instant::now();
    for inhibit in [Inhibit::Attempt, Inhibit::Resume] {
        let mut policy = automatic();
        policy.restore_inhibit(inhibit, now);
        policy.observe(observation(now, 1, 20), now);
        let effects = policy.observe(observation(now, 2, 5), now);
        if inhibit == Inhibit::Resume {
            requested(&effects);
        } else {
            assert!(!effects
                .iter()
                .any(|effect| matches!(effect, Effect::RequestSuspend { .. })));
            assert_eq!(policy.inhibit(), Some(Inhibit::Attempt));
            policy.observe(observation(now, 3, 23), now);
            assert_eq!(policy.inhibit(), None);
        }
    }
}

#[test]
fn critical_warning_hysteresis_and_silent_restoration_do_not_repeat_announcements() {
    let now = Instant::now();
    let mut policy = Policy::new(BatteryPreferences::default());
    assert!(matches!(
        policy.observe(observation(now, 1, 5), now).as_slice(),
        [Effect::Warn {
            level: Level::Critical,
            announce: true,
            ..
        }]
    ));
    assert!(matches!(
        policy.observe(observation(now, 2, 7), now).as_slice(),
        [Effect::Warn {
            level: Level::Critical,
            announce: false,
            ..
        }]
    ));
    assert!(matches!(
        policy.observe(observation(now, 3, 8), now).as_slice(),
        [Effect::Warn {
            level: Level::Low,
            announce: false,
            ..
        }]
    ));
    assert!(matches!(
        policy.observe(observation(now, 4, 7), now).as_slice(),
        [Effect::Warn {
            level: Level::Low,
            announce: false,
            ..
        }]
    ));
    assert!(matches!(
        policy.observe(observation(now, 5, 5), now).as_slice(),
        [Effect::Warn {
            level: Level::Critical,
            announce: false,
            ..
        }]
    ));
    policy.observe(
        BatteryObservation {
            state: BatteryState::Unknown,
            ..observation(now, 6, 5)
        },
        now,
    );
    assert!(matches!(
        policy.observe(observation(now, 7, 5), now).as_slice(),
        [Effect::Warn {
            level: Level::Critical,
            announce: false,
            ..
        }]
    ));
}
