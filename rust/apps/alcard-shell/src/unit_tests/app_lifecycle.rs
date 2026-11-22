use super::*;
use crate::theme::Theme;

fn application() -> Application {
    let mut app = Application::new(Shell::new(Locale::En, Theme::classic()));
    app.state.set_applications(vec![MenuApp {
        desktop_id: "test.desktop".into(),
        name: "Test application".into(),
        icon: None,
        startup_wm_class: None,
    }]);
    app.state.screen = Screen::Menu;
    app.receive_notification(notifications::Message::AuthenticationActive(false));
    app.focus(true);
    app
}

fn window(active: bool) -> WindowInfo {
    WindowInfo {
        id: 1,
        title: "Unsaved document".into(),
        app_id: "test".into(),
        active,
        activity: if active { 2 } else { 1 },
    }
}

#[test]
fn failed_launch_opens_details_only_for_current_focused_intent() {
    for scenario in 0..4 {
        let mut app = application();
        let outcome = app.launches.start(
            "test.desktop",
            "Test application",
            &[],
            Instant::now(),
            || Err(desktop::LaunchError::MissingProgram),
        );
        let launch::Outcome::Started(attempt) = outcome else {
            panic!("expected attempt")
        };
        app.launch_intent = Some(("test.desktop".into(), attempt, app.input_generation));
        if scenario == 1 {
            app.apply(Key::Right);
        }
        if scenario == 2 {
            app.focus(false);
            app.focus(true);
        }
        if scenario == 3 {
            app.receive_notification(notifications::Message::AuthenticationActive(true));
            app.receive_notification(notifications::Message::AuthenticationActive(false));
        }
        app.sync_launches();
        assert_eq!(
            matches!(app.state.application_view, ApplicationView::Details { .. }),
            scenario == 0
        );
        assert_eq!(app.state.screen, Screen::Menu);
        assert_eq!(app.state.launches[0].phase, LaunchPhase::Failed);
        assert!(app.state.launches[0].reason.contains("executable"));
        // Consuming the same snapshot again never reopens a dismissed card.
        app.apply(Key::Escape);
        app.sync_launches();
        assert!(!matches!(
            app.state.application_view,
            ApplicationView::Details { .. }
        ));
    }
}

#[test]
fn activation_send_is_pending_and_late_ack_never_redirects_navigation() {
    let mut app = application();
    let now = Instant::now();
    app.windows(vec![window(false)]);
    let request = Request::ActivateWindow(1);
    assert_eq!(app.begin_window_operation(request.clone(), now), request);
    assert_eq!(
        app.state.window_operation(1),
        Some(WindowOperation::Activating)
    );
    assert_eq!(app.state.screen, Screen::Menu);
    assert_eq!(app.begin_window_operation(request, now), Request::None);
    app.tick_window_operations(now + WINDOW_RESPONSE_TIMEOUT);
    assert_eq!(
        app.state.window_operation(1),
        Some(WindowOperation::ActivationUnconfirmed)
    );
    app.home();
    app.windows(vec![window(true)]);
    assert_eq!(app.state.window_operation(1), None);
    assert_eq!(app.state.screen, Screen::Home);
}

#[test]
fn close_waits_for_closed_event_and_keeps_save_prompt_window_available() {
    let mut app = application();
    let now = Instant::now();
    app.windows(vec![window(false)]);
    let request = Request::CloseWindow(1);
    assert_eq!(app.begin_window_operation(request.clone(), now), request);
    app.tick_window_operations(now + WINDOW_RESPONSE_TIMEOUT);
    assert_eq!(app.state.live_windows.len(), 1);
    assert!(app.state.closed_windows.is_empty());
    assert_eq!(
        app.state.window_operation(1),
        Some(WindowOperation::CloseUnconfirmed)
    );
    // A save prompt can be reached with a new, explicit activation request.
    assert_eq!(
        app.begin_window_operation(Request::ActivateWindow(1), now + WINDOW_RESPONSE_TIMEOUT),
        Request::ActivateWindow(1)
    );
    app.windows(Vec::new());
    assert_eq!(app.state.closed_windows[0].id, 1);
    assert_eq!(app.state.window_operation(1), None);
}

#[test]
fn window_transport_failure_clears_pending_status_without_removing_window() {
    let mut app = application();
    app.windows(vec![window(false)]);
    app.begin_window_operation(Request::CloseWindow(1), Instant::now());
    app.failed(Request::CloseWindow(1));
    assert_eq!(app.state.window_operation(1), None);
    assert_eq!(app.state.live_windows.len(), 1);
    assert!(app.state.dialog.is_some());
}

#[test]
fn a_queued_key_cannot_relaunch_a_window_closed_in_the_same_event_batch() {
    let mut app = application();
    app.state.screen = Screen::Recent;
    app.windows(vec![window(false)]);
    let before = app.input_anchor();
    app.launch_intent = Some(("test.desktop".into(), 4, app.input_generation));
    app.windows(Vec::new());
    assert!(!app.input_target_unchanged(before, Key::Enter));
    assert!(app.launch_intent.is_none());
    // A fresh user action may deliberately relaunch the closed entry.
    assert!(app.input_target_unchanged(app.input_anchor(), Key::Enter));
    assert_eq!(
        app.state.on_key(Key::Enter),
        Request::LaunchDesktop("test.desktop".into())
    );
}

#[test]
fn search_accept_is_not_a_launch_after_a_background_window_update() {
    let mut app = application();
    app.apply(Key::Soft(3));
    for ch in "test".chars() {
        app.apply(Key::Text(ch));
    }
    let before = app.input_anchor();
    app.windows(vec![window(true)]);
    assert!(app.input_target_unchanged(before, Key::Enter));
    assert_eq!(app.apply(Key::Enter), Request::None);
    assert_eq!(app.state.search_query, "test");
    assert!(!app.state.is_text_editing());
    assert!(app.state.window_operations.is_empty());
    assert!(app.state.launches.is_empty());
}

#[test]
fn a_queued_editor_key_cannot_escape_into_another_screen_or_dialog() {
    for key in [Key::Enter, Key::Text('7'), Key::Left] {
        for replaced_by_dialog in [false, true] {
            let mut app = application();
            app.apply(Key::Soft(3));
            let before = app.input_anchor();
            if replaced_by_dialog {
                app.state.request_failed(Request::ActivateWindow(999));
            } else {
                app.home();
            }
            assert!(!app.input_target_unchanged(before, key));
            assert!(!app.state.is_text_editing());
        }
    }
}

#[test]
fn queued_system_confirmation_and_result_keys_keep_their_original_context() {
    let mut app = application();
    app.home();
    app.state.on_key(Key::Soft(1));
    for _ in 0..crate::model::QuickAction::system_index(system::Operation::Logout) {
        app.state.on_key(Key::Down);
    }
    app.receive_system(system::Message::Snapshot(system::Snapshot {
        capabilities: [system::Capability::Allowed; 3],
    }));
    app.state.on_key(Key::Soft(0));
    let before = app.input_anchor();
    app.receive_system(system::Message::Snapshot(system::Snapshot {
        capabilities: [system::Capability::Blocked; 3],
    }));
    assert!(!app.input_target_unchanged(before, Key::Enter));
    assert_eq!(app.state.on_key(Key::Enter), Request::None);
    app.receive_system(system::Message::Snapshot(system::Snapshot {
        capabilities: [system::Capability::Allowed; 3],
    }));
    let Request::SystemOperation { request, operation } = app.state.on_key(Key::Enter) else {
        panic!("missing confirmed operation");
    };
    let before = app.input_anchor();
    app.receive_system(system::Message::Finished {
        request,
        operation,
        result: Err(system::Failure::Denied),
    });
    assert!(!app.input_target_unchanged(before, Key::Enter));
    assert!(matches!(
        app.state.system_view,
        Some(SystemView::Status { .. })
    ));
}

#[test]
fn system_commands_wait_for_the_committed_frame_even_after_callbacks_finish() {
    let mut app = application();
    app.home();
    app.state.on_key(Key::Soft(1));
    for _ in 0..crate::model::QuickAction::system_index(system::Operation::Logout) {
        app.state.on_key(Key::Down);
    }
    assert!(!app.system_input_matches_frame(None, Key::Soft(0)));
    let checking_frame = app.input_anchor();
    app.receive_system(system::Message::Snapshot(system::Snapshot {
        capabilities: [system::Capability::Allowed; 3],
    }));
    assert!(!app.system_input_matches_frame(Some(&checking_frame), Key::Soft(0)));
    let menu_frame = app.input_anchor();
    assert!(app.system_input_matches_frame(Some(&menu_frame), Key::Soft(0)));
    app.state.on_key(Key::Soft(0));
    assert!(!app.system_input_matches_frame(Some(&menu_frame), Key::Enter));
    assert!(!app.system_input_matches_frame(Some(&menu_frame), Key::Soft(0)));
    assert!(app.system_input_matches_frame(Some(&menu_frame), Key::Escape));
    let confirm_frame = app.input_anchor();
    assert!(app.system_input_matches_frame(Some(&confirm_frame), Key::Enter));
    let Request::SystemOperation { request, operation } = app.state.on_key(Key::Enter) else {
        panic!("missing confirmed operation");
    };
    let pending_frame = app.input_anchor();
    app.receive_system(system::Message::Finished {
        request,
        operation,
        result: Err(system::Failure::Denied),
    });
    // This is the runtime ordering that a per-key anchor alone cannot
    // protect: the channel callback finished before the key was dispatched.
    let key_callback_anchor = app.input_anchor();
    assert!(app.input_target_unchanged(key_callback_anchor, Key::Enter));
    assert!(!app.system_input_matches_frame(Some(&pending_frame), Key::Enter));
    assert!(matches!(
        app.state.system_view,
        Some(SystemView::Status { .. })
    ));
    let failure_frame = app.input_anchor();
    assert!(app.system_input_matches_frame(Some(&failure_frame), Key::Enter));
    app.home();
    assert!(!app.system_input_matches_frame(Some(&failure_frame), Key::Enter));
    let home_frame = app.input_anchor();
    assert!(app.system_input_matches_frame(Some(&home_frame), Key::Enter));
}

#[test]
fn system_reply_after_home_never_reopens_or_redirects_an_unrelated_screen() {
    let mut app = application();
    app.home();
    app.state.on_key(Key::Soft(1));
    for _ in 0..crate::model::QuickAction::system_index(system::Operation::Logout) {
        app.state.on_key(Key::Down);
    }
    app.receive_system(system::Message::Snapshot(system::Snapshot {
        capabilities: [system::Capability::Allowed; 3],
    }));
    app.state.on_key(Key::Soft(0));
    let Request::SystemOperation { request, operation } = app.state.on_key(Key::Enter) else {
        panic!("missing confirmed operation");
    };
    app.home();
    let before = app.input_anchor();
    app.receive_system(system::Message::Finished {
        request,
        operation,
        result: Err(system::Failure::Denied),
    });
    assert_eq!(app.state.screen, Screen::Home);
    assert!(app.state.system_view.is_none());
    assert!(app.state.dialog.is_none());
    assert!(app.input_target_unchanged(before, Key::Enter));
    app.apply(Key::Enter);
    assert_eq!(app.state.screen, Screen::Recent);
}

#[test]
fn unavailable_system_worker_keeps_the_error_inside_quick_actions() {
    let mut app = application();
    app.home();
    assert_eq!(app.apply(Key::Soft(1)), Request::None);
    assert_eq!(
        app.state.system_snapshot.capabilities,
        [system::Capability::Unavailable; 3]
    );
    assert!(matches!(
        app.state.system_view,
        Some(SystemView::Menu { .. })
    ));
    for _ in 0..crate::model::QuickAction::system_index(system::Operation::Logout) {
        app.apply(Key::Down);
    }
    app.apply(Key::Soft(0));
    assert!(matches!(
        app.state.system_view,
        Some(SystemView::Unavailable { .. })
    ));
    assert!(app.state.dialog.is_none());
    assert!(app.state.system_request.is_none());
}

#[test]
fn unmatched_windows_are_retained_in_launch_baseline_and_errors_are_localized() {
    let mut app = application();
    let mut unknown = window(false);
    unknown.app_id = "unknown".into();
    app.windows(vec![unknown]);
    assert_eq!(app.resolved_windows(), vec![(1, String::new())]);
    for locale in [Locale::Ru, Locale::En, Locale::Es] {
        let reason = launch_reason(
            locale,
            launch::Phase::Failed(launch::FailureReason::EarlyExit {
                code: Some(23),
                signal: None,
            }),
        );
        assert!(reason.contains("23"));
        assert!(!reason.contains("EarlyExit"));
        assert!(!launch_reason(locale, launch::Phase::Unconfirmed).is_empty());
    }
}
