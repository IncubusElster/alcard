use super::*;

fn notice(critical: bool, announce: bool) -> BatteryNotice {
    BatteryNotice {
        title: "Battery".into(),
        body: "Connect a charger".into(),
        critical,
        announce,
    }
}

#[test]
fn low_to_critical_reuses_one_persistent_internal_notice() {
    let mut store = Store::default();
    let now = Instant::now();
    store
        .battery_notice(Some(notice(false, true)), now, 1)
        .unwrap();
    let id = store.battery_id.unwrap();
    assert!(!store.expire(now + Duration::from_secs(3600)).changed);
    store
        .battery_notice(Some(notice(true, true)), now, 2)
        .unwrap();
    assert_eq!(store.battery_id, Some(id));
    assert_eq!(store.entries.len(), 1);
    assert_eq!(store.entries[0].notice.urgency, 2);
    assert!(store.entries[0].internal);
    assert!(store.entries[0].deadline.is_none());
    store.battery_notice(None, now, 3).unwrap();
    assert!(store.entries.is_empty());
}

#[test]
fn silent_battery_refresh_preserves_generation_read_state_and_event() {
    let mut store = Store::default();
    let now = Instant::now();
    store
        .battery_notice(Some(notice(true, true)), now, 1)
        .unwrap();
    let current = store.entries[0].notice.clone();
    store.mark_read(current.id, current.generation);
    store
        .battery_notice(Some(notice(true, false)), now, 2)
        .unwrap();
    let snapshot = store.snapshot();
    assert_eq!(
        snapshot.latest_event,
        Some(NotificationEvent {
            id: current.id,
            generation: current.generation,
        })
    );
    assert_eq!(snapshot.notices[0].id, current.id);
    assert_eq!(snapshot.notices[0].generation, current.generation);
    assert_eq!(snapshot.notices[0].created_at, current.created_at);
    assert_eq!(snapshot.event_generation, current.generation);
    assert!(!snapshot.notices[0].unread);
    assert!(snapshot.notices[0].active);
}

#[test]
fn announcement_survives_a_quiet_update_before_the_first_snapshot() {
    let mut store = Store::default();
    let now = Instant::now();
    store
        .battery_notice(Some(notice(true, true)), now, 1)
        .unwrap();
    let event = store.latest_event.unwrap();
    store
        .battery_notice(
            Some(BatteryNotice {
                title: format!("Updated\n{}", "x".repeat(TITLE_LIMIT)),
                body: format!("Charge: 4%\n{}", "y".repeat(BODY_LIMIT)),
                critical: false,
                announce: false,
            }),
            now,
            2,
        )
        .unwrap();
    let snapshot = store.snapshot();
    assert_eq!(snapshot.latest_event, Some(event));
    assert_eq!(snapshot.event_generation, event.generation);
    let current = &snapshot.notices[0];
    assert_eq!(current.generation, event.generation);
    assert_eq!(current.created_at, 1);
    assert!(current.unread);
    assert_eq!(current.urgency, 1);
    assert!(current.title.starts_with("Updated"));
    assert!(!current.title.contains('\n'));
    assert!(current.title.chars().count() <= TITLE_LIMIT);
    assert!(current.description.starts_with("Charge: 4%\n"));
    assert!(current.description.chars().count() <= BODY_LIMIT);
}

#[test]
fn clear_and_dismiss_do_not_retain_a_reusable_battery_identity() {
    let mut store = Store::default();
    let now = Instant::now();
    store
        .battery_notice(Some(notice(false, true)), now, 1)
        .unwrap();
    let id = store.battery_id.unwrap();
    assert_eq!(store.dismiss(id), Some(false));
    assert!(store.battery_id.is_none());
    store
        .battery_notice(Some(notice(false, false)), now, 2)
        .unwrap();
    assert!(
        store.entries.is_empty(),
        "silent telemetry cannot undo dismissal"
    );
    store
        .battery_notice(Some(notice(true, true)), now, 2)
        .unwrap();
    assert_ne!(store.battery_id, Some(id));
    assert!(store.clear().is_empty());
    assert!(store.battery_id.is_none());
    store
        .battery_notice(Some(notice(true, false)), now, 3)
        .unwrap();
    assert!(store.entries.is_empty(), "quiet refresh cannot undo clear");
}
#[test]
fn quiet_battery_refresh_preserves_an_unseen_external_event() {
    let mut store = Store::default();
    let now = Instant::now();
    store
        .battery_notice(Some(notice(false, true)), now, 1)
        .unwrap();
    let (external, _) = store
        .notify(
            Incoming {
                internal: false,
                replaces_id: 0,
                app_name: "Fixture",
                title: "External",
                body: "A real sender",
                actions: &[],
                hints: Hints::default(),
                expire_timeout: 0,
            },
            now,
            2,
        )
        .unwrap();
    let event = store.latest_event;
    store
        .battery_notice(Some(notice(false, false)), now, 3)
        .unwrap();
    assert_eq!(store.latest_event, event);
    assert_eq!(store.latest_event.unwrap().id, external);
    assert_eq!(
        store.latest_event.unwrap().generation,
        store.event_generation
    );
    assert_eq!(store.entries[0].notice.id, external);
    assert_eq!(store.entries[1].notice.id, store.battery_id.unwrap());
}

#[test]
fn quiet_restore_creates_no_event_and_preserves_an_unseen_external_event() {
    let mut store = Store::default();
    let now = Instant::now();
    store
        .battery_notice(Some(notice(false, false)), now, 1)
        .unwrap();
    assert!(store.latest_event.is_none());
    store.battery_notice(None, now, 2).unwrap();
    let (external, _) = store
        .notify(
            Incoming {
                internal: false,
                replaces_id: 0,
                app_name: "Fixture",
                title: "External",
                body: "A real sender",
                actions: &[],
                hints: Hints::default(),
                expire_timeout: 0,
            },
            now,
            3,
        )
        .unwrap();
    let event = store.latest_event;
    store
        .battery_notice(Some(notice(false, false)), now, 4)
        .unwrap();
    assert_eq!(store.latest_event, event);
    assert_eq!(store.latest_event.unwrap().id, external);
    assert!(store.latest_event.unwrap().generation < store.event_generation);
    assert_eq!(store.entries.len(), 2);
}
