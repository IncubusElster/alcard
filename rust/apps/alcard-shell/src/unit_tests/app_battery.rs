use super::*;
use crate::{
    status::{BatteryState, ChargeState},
    theme::Theme,
};

fn application() -> Application {
    Application::new(Shell::new(Locale::En, Theme::classic()))
}

fn observation(sequence: u64, percent: u8, charge: ChargeState) -> BatteryObservation {
    BatteryObservation {
        state: BatteryState::Present {
            percent: Some(percent),
            charge,
        },
        observed_at: Instant::now(),
        sequence,
        source_epoch: Some(1),
        external_power: Some(charge == ChargeState::Charging),
    }
}

fn ready(app: &mut Application) {
    app.receive_sleep(sleep::Message::Snapshot(sleep::Snapshot {
        epoch: 1,
        capability: system::Capability::Allowed,
        monitor: sleep::MonitorState::Ready,
        delay_ms: 5_000,
    }));
}

fn locking_battery_request() -> Application {
    let mut app = application();
    app.set_battery_preferences(BatteryPreferences {
        suspend_on_critical: true,
        ..BatteryPreferences::default()
    });
    ready(&mut app);
    let observation = observation(1, 5, ChargeState::Discharging);
    let effects = app.battery_policy.observe(observation, Instant::now());
    let episode = effects
        .iter()
        .find_map(|effect| match effect {
            battery::Effect::RequestSuspend { episode } => Some(*episode),
            _ => None,
        })
        .expect("fresh critical policy request");
    app.battery_observation = Some(observation);
    let Request::LockSession { request: lock } = app.state.begin_lock(false) else {
        panic!("lock fixture")
    };
    app.receive_lock(locking::Message::Changed(locking::State {
        id: lock,
        phase: locking::Phase::Locked,
    }));
    let Request::PrepareSuspend { request } = app.state.begin_battery_suspend() else {
        panic!("automatic request may reuse lock")
    };
    // Only the existing pure coordinator is exercised. No real client,
    // locker, bus, journal path or system operation is created here.
    assert!(app.sleep_coordinator.begin(request, Instant::now()));
    app.battery_sleep = Some(BatterySleep {
        episode,
        request: Some(request),
        refresh_deadline: Instant::now() + BATTERY_REFRESH_TIMEOUT,
        sample_barrier: None,
    });
    app.receive_sleep(sleep::Message::Prepared {
        request,
        epoch: 1,
        result: Ok(()),
    });
    app
}

#[test]
fn critical_default_only_warns_and_preserves_the_unfocused_view() {
    let mut app = application();
    app.state.enter(Screen::Menu);
    app.focus(false);
    app.observe_battery(observation(1, 5, ChargeState::Discharging));
    assert_eq!(app.battery_warning.unwrap().level, battery::Level::Critical);
    assert!(app.battery_sleep.is_none());
    assert!(app.state.lock_state.is_none());
    assert_eq!(app.state.screen, Screen::Menu);
    assert!(app.state.system_view.is_none());
    assert!(!app.focused);
}

#[test]
fn missing_journal_refuses_automatic_sleep_without_starting_lock() {
    let mut app = application();
    app.set_battery_preferences(BatteryPreferences {
        suspend_on_critical: true,
        ..BatteryPreferences::default()
    });
    ready(&mut app);
    app.observe_battery(observation(1, 4, ChargeState::Discharging));
    assert_eq!(
        app.battery_warning.unwrap().failure,
        Some(system::Failure::Unavailable)
    );
    assert!(app.battery_sleep.is_none());
    assert!(app.state.lock_state.is_none());
    assert_eq!(
        app.battery_policy.inhibit(),
        Some(battery::Inhibit::Attempt)
    );
    app.observe_battery(observation(2, 3, ChargeState::Discharging));
    assert!(app.battery_sleep.is_none());
}

#[test]
fn automatic_failure_survives_unknown_telemetry_and_silent_warning_restore() {
    let mut app = application();
    app.set_battery_preferences(BatteryPreferences {
        suspend_on_critical: true,
        ..BatteryPreferences::default()
    });
    ready(&mut app);
    app.observe_battery(observation(1, 5, ChargeState::Discharging));
    assert_eq!(
        app.battery_warning.unwrap().failure,
        Some(system::Failure::Unavailable)
    );
    app.observe_battery(BatteryObservation {
        state: BatteryState::Unknown,
        observed_at: Instant::now(),
        sequence: 2,
        source_epoch: None,
        external_power: None,
    });
    assert!(app.battery_warning.is_none());
    app.observe_battery(observation(3, 5, ChargeState::Discharging));
    assert_eq!(
        app.battery_warning.unwrap().failure,
        Some(system::Failure::Unavailable)
    );
    assert!(app.battery_sleep.is_none());
}

#[test]
fn equal_fresh_status_does_not_redraw_the_shell() {
    let mut app = application();
    let observation = observation(1, 80, ChargeState::Discharging);
    app.state.status.battery = observation.state;
    app.dirty = false;
    app.status(app.state.status);
    app.observe_battery(observation);
    app.observe_battery(BatteryObservation {
        sequence: 2,
        observed_at: Instant::now(),
        ..observation
    });
    assert!(!app.dirty);
}

#[test]
fn automatic_commit_waits_for_measurement_started_after_lock() {
    let mut app = locking_battery_request();
    let old = app.battery_observation.unwrap();
    app.flush_sleep_intent();
    assert_eq!(
        app.state.sleep_progress.unwrap().stage,
        sleep::Stage::Locking
    );
    assert!(app.take_battery_refresh_request());
    assert!(!app.battery_commit_ready(Instant::now()));
    app.observe_battery(BatteryObservation { sequence: 2, ..old });
    assert!(!app.battery_commit_ready(Instant::now()));
    app.observe_battery(observation(3, 5, ChargeState::Discharging));
    assert!(app.battery_commit_ready(Instant::now()));
    assert!(app.state.system_view.is_none());
}

#[test]
fn charger_in_the_lock_batch_revokes_the_automatic_request() {
    let mut app = locking_battery_request();
    app.observe_battery(observation(2, 5, ChargeState::Charging));
    app.flush_sleep_intent();
    assert!(app.battery_sleep.is_none());
    assert_eq!(
        app.state.sleep_progress.unwrap().stage,
        sleep::Stage::Cancelled
    );
    assert_eq!(app.state.lock_state.unwrap().phase, locking::Phase::Locked);
}

#[test]
fn resume_requires_a_new_sensor_read_and_retains_attempt_inhibition() {
    let mut app = locking_battery_request();
    app.apply_sleep_effects(vec![SleepEffect::Resume]);
    assert!(app.take_battery_refresh_request());
    assert!(app.battery_observation.is_none());
    assert_eq!(
        app.battery_policy.inhibit(),
        Some(battery::Inhibit::Attempt)
    );
    app.observe_battery(observation(2, 4, ChargeState::Discharging));
    assert!(app.battery_sleep.is_none());
    assert_eq!(app.state.lock_state.unwrap().phase, locking::Phase::Locked);
}
