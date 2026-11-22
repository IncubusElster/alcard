use super::PowerState;

#[test]
fn initial_off_is_restored_without_waiting_for_a_resumed_event() {
    let mut state = PowerState::default();
    assert!(!state.request(true)); // initial mode is not known yet
    state.observe(false);
    assert!(state.request(true));
    assert!(!state.request(true));
    state.observe(true);
    assert!(!state.needs_restore());
}

#[test]
fn input_supersedes_pending_off_even_before_off_acknowledgement() {
    let mut state = PowerState::default();
    state.observe(true);
    assert!(!state.request(true));
    assert!(state.request(false));
    assert!(state.request(true));
    state.observe(false); // acknowledgement for the earlier request
    assert_eq!(state.requested, Some(true));
    assert!(!state.request(true));
    state.observe(true);
    assert_eq!(state.requested, None);
    assert!(!state.needs_restore());
}

#[test]
fn pending_or_confirmed_off_requires_explicit_shutdown_restore() {
    let mut state = PowerState::default();
    state.observe(true);
    assert!(state.request(false));
    assert!(state.needs_restore());
    state.observe(false);
    assert!(state.needs_restore());
}

#[test]
fn compositor_mode_changes_do_not_start_a_retry_loop() {
    let mut state = PowerState::default();
    state.observe(true);
    assert!(state.request(false));
    state.observe(false);
    state.observe(true); // another controller/compositor changed its mind
    assert!(!state.request(false));
    assert!(!state.request(true)); // current mode already matches activity
    assert!(state.request(false)); // a later idle period may request again
}
