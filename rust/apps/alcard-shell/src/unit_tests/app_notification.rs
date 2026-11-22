use super::*;
use crate::{i18n::Locale, model::Notice, theme::Theme};
fn notice(id: u32) -> Notice {
    Notice {
        id,
        generation: u64::from(id),
        title: "Public notification".into(),
        description: "Body".into(),
        app_name: "Test application".into(),
        desktop_entry: None,
        created_at: 0,
        urgency: 1,
        unread: true,
        actions: Vec::new(),
        active: true,
    }
}
fn snapshot(notices: Vec<Notice>, generation: u64, latest_id: u32) -> Snapshot {
    Snapshot {
        notices,
        event_generation: generation,
        latest_event: Some(NotificationEvent {
            id: latest_id,
            generation,
        }),
    }
}
fn application() -> Application {
    let mut app = Application::new(Shell::new(Locale::En, Theme::classic()));
    app.receive_notification(notifications::Message::AuthenticationActive(false));
    app
}
#[test]
fn authentication_suppresses_banners_but_preserves_notification_history() {
    let mut app = application();
    app.receive_snapshot(snapshot(vec![notice(1)], 1, 1));
    assert!(app.banner().is_some());
    app.receive_notification(notifications::Message::AuthenticationActive(true));
    assert!(app.banner().is_none());
    app.receive_snapshot(snapshot(vec![notice(2), notice(1)], 2, 2));
    assert_eq!(app.state.notices.len(), 2);
    assert!(app.banner().is_none());
    app.receive_notification(notifications::Message::AuthenticationActive(false));
    assert!(app.banner().is_none());
    app.receive_snapshot(snapshot(vec![notice(3)], 3, 3));
    assert!(app.banner().is_some());
}
#[test]
fn deleting_newest_record_never_replays_an_older_notification() {
    let mut app = application();
    app.receive_snapshot(snapshot(vec![notice(1)], 1, 1));
    app.receive_snapshot(snapshot(vec![notice(2), notice(1)], 2, 2));
    assert!(app.banner().is_some());
    app.receive_snapshot(snapshot(vec![notice(1)], 2, 2));
    assert!(app.banner().is_none());
    assert!(app.state.notices[0].unread);
}
#[test]
fn replacing_same_id_is_a_new_event_but_read_and_expiry_are_not() {
    let mut app = application();
    app.receive_snapshot(snapshot(vec![notice(1)], 1, 1));
    app.banner = None;
    let mut updated = notice(1);
    updated.generation = 2;
    updated.title = "Updated content".into();
    app.receive_snapshot(snapshot(vec![updated.clone()], 2, 1));
    assert_eq!(app.banner().unwrap().title, "Updated content");
    app.banner = None;
    updated.unread = false;
    app.receive_snapshot(snapshot(vec![updated.clone()], 2, 1));
    assert!(app.banner().is_none());
    updated.active = false;
    app.receive_snapshot(snapshot(vec![updated], 2, 1));
    assert!(app.banner().is_none());
}

#[test]
fn quiet_battery_update_preserves_the_live_banner_without_extending_or_replaying_it() {
    let mut app = application();
    let mut battery = notice(1);
    battery.title = "Critical battery".into();
    app.receive_snapshot(snapshot(vec![battery.clone()], 1, 1));
    let deadline = app.banner.as_ref().unwrap().1;

    battery.title = "Критический заряд".into();
    battery.description = "Осталось 4%".into();
    app.receive_snapshot(snapshot(vec![battery.clone()], 1, 1));
    assert_eq!(app.banner().unwrap().title, "Критический заряд");
    assert_eq!(app.banner.as_ref().unwrap().1, deadline);

    app.banner = None;
    app.receive_snapshot(snapshot(vec![battery], 1, 1));
    assert!(app.banner().is_none());
}

#[test]
fn a_quiet_snapshot_preserves_unseen_external_events_without_replaying_seen_ones() {
    let mut app = application();
    let update = Snapshot {
        notices: vec![notice(2), notice(1)],
        event_generation: 2,
        latest_event: Some(NotificationEvent {
            id: 1,
            generation: 1,
        }),
    };
    app.receive_snapshot(update.clone());
    assert_eq!(app.banner.as_ref().unwrap().0.id, 1);
    app.banner = None;
    app.receive_snapshot(Snapshot {
        event_generation: 3,
        ..update
    });
    assert!(app.banner().is_none());
}

#[test]
fn ordinary_modal_and_notification_reader_suppress_without_replaying() {
    let mut app = application();
    app.receive_snapshot(snapshot(vec![notice(1)], 1, 1));
    assert!(app.banner().is_some());
    app.failed(Request::LaunchDesktop("missing.desktop".into()));
    assert!(app.banner().is_none());
    app.apply(Key::Escape);
    assert!(app.banner().is_none());
    app.state.notification_failed();
    app.receive_snapshot(snapshot(vec![notice(1)], 1, 1));
    assert!(app.banner().is_none());
    app.apply(Key::Escape);
    assert!(app.banner().is_none());
    app.state.screen = Screen::Notifications;
    app.receive_snapshot(snapshot(vec![notice(2), notice(1)], 2, 2));
    assert!(app.banner().is_none());
    app.home();
    assert!(app.banner().is_none());
}
#[test]
fn service_failure_is_visible_and_does_not_discard_unread_history() {
    let mut app = application();
    app.receive_snapshot(snapshot(vec![notice(1)], 1, 1));
    app.receive_notification(notifications::Message::Unavailable("Test failure".into()));
    assert!(app.state.notifications_unavailable);
    assert_eq!(app.state.notices.len(), 1);
    assert!(app.state.notices[0].unread);
    assert!(app.banner().is_none());
    app.receive_notification(notifications::Message::Available);
    assert!(!app.state.notifications_unavailable);
    assert!(app.banner().is_none());
}
