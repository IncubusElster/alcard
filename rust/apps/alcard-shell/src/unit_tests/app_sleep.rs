use super::*;
use crate::theme::Theme;

fn application() -> Application {
    Application::new(Shell::new(Locale::En, Theme::classic()))
}

fn ready(app: &mut Application) {
    app.receive_sleep(sleep::Message::Snapshot(sleep::Snapshot {
        epoch: 1,
        capability: system::Capability::Allowed,
        monitor: sleep::MonitorState::Ready,
        delay_ms: 5_000,
    }));
}

fn pending_preflight(app: &mut Application) {
    // Model an accepted worker queue without connecting to a real bus or
    // starting a locker. All lifecycle replies below are explicit fixtures.
    assert!(app.sleep_coordinator.begin(1, Instant::now()));
    app.sync_sleep_view();
}

#[test]
fn missing_sleep_worker_fails_before_requesting_a_lock() {
    let mut app = application();
    ready(&mut app);
    app.state.system_view = Some(SystemView::SuspendConfirm { scroll: 0 });
    assert_eq!(app.apply(Key::Enter), Request::None);
    assert_eq!(
        app.state.sleep_progress,
        Some(sleep::Progress {
            request: 1,
            stage: sleep::Stage::Failed,
            failure: Some(system::Failure::Unavailable),
        })
    );
    assert!(app.state.lock_state.is_none());
    assert!(matches!(
        app.state.system_view,
        Some(SystemView::SuspendStatus { .. })
    ));
    assert!(app.state.dialog.is_none());
    assert!(!app.state.system_busy());
}

#[test]
fn home_cancels_preflight_and_a_late_reply_cannot_start_a_lock() {
    let mut app = application();
    ready(&mut app);
    pending_preflight(&mut app);
    app.state.system_view = Some(SystemView::SuspendStatus { scroll: 0 });
    app.home();
    let shown = app.input_anchor();
    app.focus(false);
    app.receive_sleep(sleep::Message::Prepared {
        request: 1,
        epoch: 1,
        result: Ok(()),
    });
    app.flush_sleep_intent();
    assert_eq!(
        app.state.sleep_progress.unwrap().stage,
        sleep::Stage::Cancelled
    );
    assert!(app.state.lock_state.is_none());
    assert!(app.state.system_view.is_none());
    assert_eq!(app.state.screen, Screen::Home);
    assert!(!app.focused);
    assert!(shown == app.input_anchor());
    app.apply(Key::Soft(1));
    assert!(matches!(
        app.state.system_view,
        Some(SystemView::Menu { .. })
    ));
}

#[test]
fn lock_launch_failure_preserves_the_underlying_unfocused_editor() {
    let mut app = application();
    ready(&mut app);
    pending_preflight(&mut app);
    app.state.enter(Screen::Menu);
    app.state.on_key(Key::Soft(3));
    app.state.on_key(Key::Text('x'));
    let editor = app.state.search_input.clone();
    let shown = app.input_anchor();
    app.focus(false);
    app.receive_sleep(sleep::Message::Prepared {
        request: 1,
        epoch: 1,
        result: Ok(()),
    });
    app.flush_sleep_intent();
    assert!(editor.is_some());
    assert_eq!(app.state.search_input, editor);
    assert_eq!(app.state.screen, Screen::Menu);
    assert!(app.state.system_view.is_none());
    assert!(!app.focused);
    assert_eq!(app.state.lock_state.unwrap().phase, locking::Phase::Failed);
    assert_eq!(
        app.state.sleep_progress.unwrap().stage,
        sleep::Stage::Failed
    );
    assert!(!app.state.system_busy());
    assert!(shown == app.input_anchor());
}

#[test]
fn external_sleep_lock_failure_does_not_retry_or_open_a_card() {
    let mut app = application();
    ready(&mut app);
    app.state.enter(Screen::Menu);
    app.focus(false);
    app.receive_sleep(sleep::Message::Preparing { epoch: 1, cycle: 1 });
    let failed = app.state.lock_state.unwrap();
    assert_eq!(failed.phase, locking::Phase::Failed);
    for _ in 0..3 {
        app.flush_sleep_intent();
    }
    assert_eq!(app.state.lock_state, Some(failed));
    assert_eq!(app.state.screen, Screen::Menu);
    assert!(app.state.system_view.is_none());
    assert!(app.state.sleep_progress.is_none());
    assert!(!app.focused);
}

#[test]
fn lock_then_unlock_in_one_batch_never_reaches_sleep_submission() {
    let mut app = application();
    ready(&mut app);
    let Request::LockSession { request: id } = app.state.begin_lock(false) else {
        panic!("fixture lock was not allocated");
    };
    pending_preflight(&mut app);
    app.receive_sleep(sleep::Message::Prepared {
        request: 1,
        epoch: 1,
        result: Ok(()),
    });
    app.receive_lock(locking::Message::Changed(locking::State {
        id,
        phase: locking::Phase::Locked,
    }));
    assert_eq!(
        app.state.sleep_progress.unwrap().stage,
        sleep::Stage::Locking
    );
    app.receive_lock(locking::Message::Unlocked { id });
    app.flush_sleep_intent();
    assert_eq!(
        app.state.sleep_progress.unwrap().failure,
        Some(system::Failure::SessionUnavailable)
    );
    assert_eq!(
        app.state.sleep_progress.unwrap().stage,
        sleep::Stage::Failed
    );
    assert!(app.state.lock_state.is_none());
}

#[test]
fn newly_allowed_sleep_confirmation_waits_for_a_committed_frame() {
    let mut app = application();
    app.state.system_view = Some(SystemView::SuspendConfirm { scroll: 0 });
    let shown = app.input_anchor();
    ready(&mut app);
    assert!(!app.system_input_matches_frame(Some(&shown), Key::Enter));
    assert!(!app.input_target_unchanged(shown, Key::Enter));
    assert!(app.state.sleep_progress.is_none());
    let shown = app.input_anchor();
    assert!(app.system_input_matches_frame(Some(&shown), Key::Enter));
}

#[test]
fn resume_invalidates_launch_intent_without_unlocking_or_changing_view() {
    let mut app = application();
    ready(&mut app);
    app.state.enter(Screen::Menu);
    app.state.on_key(Key::Soft(3));
    app.state.on_key(Key::Text('x'));
    let editor = app.state.search_input.clone();
    let Request::LockSession { request: id } = app.state.begin_lock(false) else {
        panic!("fixture lock was not allocated");
    };
    app.receive_lock(locking::Message::Changed(locking::State {
        id,
        phase: locking::Phase::Locked,
    }));
    app.focus(false);
    let generation = app.input_generation;
    app.launch_intent = Some(("fixture.desktop".into(), 1, generation));
    app.receive_sleep(sleep::Message::Preparing { epoch: 1, cycle: 1 });
    app.receive_sleep(sleep::Message::CycleEnded {
        epoch: 1,
        cycle: Some(1),
    });
    assert!(app.take_resume_request());
    assert!(!app.take_resume_request());
    assert!(app.launch_intent.is_none());
    assert_ne!(app.input_generation, generation);
    assert_eq!(app.state.search_input, editor);
    assert_eq!(app.state.screen, Screen::Menu);
    assert!(app.state.system_view.is_none());
    assert!(!app.focused);
    assert!(app.lock_inhibits_activation());
    assert_eq!(app.state.lock_state.unwrap().phase, locking::Phase::Locked);
    app.receive_sleep(sleep::Message::CycleEnded {
        epoch: 1,
        cycle: Some(1),
    });
    assert!(!app.take_resume_request());
}
