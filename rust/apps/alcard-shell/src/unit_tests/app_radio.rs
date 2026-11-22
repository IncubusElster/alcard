use super::*;
use crate::{model::QuickAction, theme::Theme};

fn wifi() -> Application {
    let mut app = Application::new(Shell::new(Locale::En, Theme::classic()));
    app.state.system_view = Some(SystemView::Menu {
        selected: QuickAction::WIFI_INDEX,
    });
    app.state.radios_snapshot.wifi = radios::State {
        availability: radios::Availability::Available,
        powered: Some(true),
        detail: "wlan0".into(),
    };
    app
}

fn reply(app: &Application, entries: Vec<radios::Entry>) -> radios::Message {
    let pending = app.state.radios_request.unwrap();
    radios::Message {
        request: pending.id,
        operation: pending.operation,
        snapshot: app.state.radios_snapshot.clone(),
        entries,
        result: Ok(()),
    }
}

#[test]
fn radio_toggle_waits_for_the_frame_showing_its_readback() {
    let mut app = wifi();
    let on_frame = app.input_anchor();
    assert!(matches!(
        app.state.on_key(Key::Soft(1)),
        Request::RadioOperation { .. }
    ));
    let pending_frame = app.input_anchor();
    let mut message = reply(&app, Vec::new());
    message.snapshot.wifi.powered = Some(false);
    app.receive_radios(message);
    for old_frame in [&on_frame, &pending_frame] {
        assert!(!app.system_input_matches_frame(Some(old_frame), Key::Soft(1)));
        assert!(app.system_input_matches_frame(Some(old_frame), Key::Escape));
    }
    let shown = app.input_anchor();
    assert!(app.system_input_matches_frame(Some(&shown), Key::Soft(1)));
    assert_eq!(app.state.actions()[1], Action::TurnOn);
}

#[test]
fn radio_list_read_key_cannot_retarget_an_unseen_entry() {
    let mut app = wifi();
    app.state.on_key(Key::Soft(0));
    let entry = |id: &str| radios::Entry {
        id: id.into(),
        name: id.into(),
        detail: String::new(),
    };
    app.receive_radios(reply(&app, vec![entry("old-network")]));
    let old_list = app.input_anchor();
    app.state.on_key(Key::Soft(3));
    app.receive_radios(reply(&app, vec![entry("new-network")]));
    assert!(!app.system_input_matches_frame(Some(&old_list), Key::Soft(0)));
    assert!(!app.input_target_unchanged(old_list, Key::Soft(0)));
    let shown = app.input_anchor();
    assert!(app.system_input_matches_frame(Some(&shown), Key::Soft(0)));
    app.state.on_key(Key::Soft(0));
    assert!(
        matches!(&app.state.system_view, Some(SystemView::RadioDetails { entry, .. }) if entry.id == "new-network")
    );
}

#[test]
fn missing_radio_worker_releases_pending_without_faking_a_switch() {
    let mut app = wifi();
    app.apply(Key::Soft(1));
    assert!(!app.state.radio_busy());
    assert_eq!(app.state.radio_state(radios::Kind::Wifi).powered, None);
    assert_eq!(
        app.state.radio_failure(radios::Kind::Wifi),
        Some(radios::Failure::Unavailable)
    );
    assert_eq!(app.state.radio_failure(radios::Kind::Bluetooth), None);
    assert!(matches!(
        app.state.system_view,
        Some(SystemView::Menu {
            selected: QuickAction::WIFI_INDEX
        })
    ));
    assert!(app.state.dialog.is_none());
}
