use super::*;
use crate::{model::QuickAction, theme::Theme};

fn open_controls() -> (Application, u64) {
    let mut app = Application::new(Shell::new(Locale::En, Theme::classic()));
    app.state.on_key(Key::Soft(1));
    let Request::RefreshControls { request } = app.state.refresh_controls() else {
        panic!("inline controls must start with a discovery request");
    };
    (app, request)
}

fn available() -> controls::Snapshot {
    controls::Snapshot {
        brightness: controls::Reading {
            availability: controls::Availability::Available,
            percent: Some(60),
            muted: None,
            above_safe_limit: false,
        },
        volume: controls::Reading {
            availability: controls::Availability::Available,
            percent: Some(50),
            muted: Some(false),
            above_safe_limit: false,
        },
    }
}

#[test]
fn control_arrows_wait_for_the_frame_that_shows_the_current_value() {
    let (mut app, request) = open_controls();
    let checking_frame = app.input_anchor();
    app.receive_controls(controls::Message::Refreshed {
        request,
        snapshot: available(),
    });
    assert!(!app.system_input_matches_frame(Some(&checking_frame), Key::Right));
    assert!(!app.input_target_unchanged(checking_frame, Key::Left));
    let available_frame = app.input_anchor();
    assert!(app.system_input_matches_frame(Some(&available_frame), Key::Right));
    let Request::AdjustControl { request, operation } = app.state.on_key(Key::Right) else {
        panic!("expected one explicit adjustment");
    };
    let pending_frame = app.input_anchor();
    let mut after = available();
    after.brightness.percent = Some(65);
    app.receive_controls(controls::Message::Finished {
        request,
        operation,
        snapshot: after,
        result: Ok(()),
    });
    // A callback can finish before the next key callback. Its fresh model
    // alone is insufficient while the committed frame still says pending.
    assert!(app.input_target_unchanged(app.input_anchor(), Key::Left));
    assert!(!app.system_input_matches_frame(Some(&pending_frame), Key::Left));
    assert!(!app.system_input_matches_frame(Some(&pending_frame), Key::Right));
    assert!(app.system_input_matches_frame(Some(&pending_frame), Key::Escape));
}

#[test]
fn controls_reply_after_home_does_not_reopen_or_redirect_the_screen() {
    let (mut app, request) = open_controls();
    app.home();
    let home_frame = app.input_anchor();
    app.receive_controls(controls::Message::Refreshed {
        request,
        snapshot: available(),
    });
    assert_eq!(app.state.screen, Screen::Home);
    assert!(app.state.system_view.is_none());
    assert!(app.state.dialog.is_none());
    assert!(app.system_input_matches_frame(Some(&home_frame), Key::Enter));
    assert!(app.input_target_unchanged(home_frame, Key::Enter));
}

#[test]
fn missing_controls_worker_releases_pending_without_a_fake_value() {
    let mut app = Application::new(Shell::new(Locale::En, Theme::classic()));
    app.apply(Key::Soft(1));
    assert!(matches!(
        app.state.system_view,
        Some(SystemView::Menu {
            selected: QuickAction::BRIGHTNESS_INDEX
        })
    ));
    assert!(app.state.controls_request.is_none());
    assert_eq!(
        app.state.controls_failure,
        Some(controls::Failure::Unavailable)
    );
    assert_eq!(app.state.controls_snapshot.brightness.percent, None);
    assert_eq!(app.state.controls_snapshot.volume.percent, None);
    assert!(app.state.dialog.is_none());
    app.apply(Key::Escape);
    assert!(app.state.system_view.is_none());
}
