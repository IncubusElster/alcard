use super::*;

#[test]
fn back_from_the_welcome_screen_exits() {
    let mut app = App::new_firstboot();
    app.handle(Intent::Soft(4));
    assert!(app.should_exit);
}

#[test]
fn navigation_during_pending_network_inventory_discards_stale_results() {
    use alcard_setup_core::Step;
    for (intent, expected_step, expect_exit) in [
        (Intent::Soft(0), Step::DateTime, false),
        (Intent::Soft(2), Step::DateTime, false),
        (Intent::Soft(4), Step::Account, false),
        (Intent::Quit, Step::Network, true),
    ] {
        let mut app = App::new_firstboot();
        app.wizard.step = Step::Network;
        let (tx, rx) = mpsc::channel();
        app.network_job = Some(NetworkJob {
            updates: rx,
            kind: NetworkJobKind::Inventory,
        });
        app.handle(intent);
        assert_eq!(app.wizard.step, expected_step);
        assert_eq!(app.should_exit, expect_exit);
        assert!(app.cancel.load(Ordering::Acquire));
        assert!(app.network_job.is_none());
        assert!(tx.send(NetworkUpdate::Inventory(None, None, None)).is_err());
        app.poll_network();
    }
}

#[test]
fn active_connection_does_not_get_abandoned_by_network_skip() {
    use alcard_setup_core::Step;
    let mut app = App::new_firstboot();
    app.wizard.step = Step::Network;
    let (_tx, rx) = mpsc::channel();
    app.network_job = Some(NetworkJob {
        updates: rx,
        kind: NetworkJobKind::Connection,
    });
    app.handle(Intent::Soft(2));
    assert_eq!(app.wizard.step, Step::Network);
    assert!(app.network_job.is_some());
    assert!(!app.cancel.load(Ordering::Acquire));
    app.handle(Intent::Soft(4));
    assert!(app.cancel.load(Ordering::Acquire));
    assert_eq!(app.wizard.step, Step::Network);
}
