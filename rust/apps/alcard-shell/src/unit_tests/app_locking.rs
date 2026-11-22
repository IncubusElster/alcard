use super::*;
use crate::{model::QuickAction, theme::Theme};

#[test]
fn automatic_lock_failure_preserves_an_unfocused_editor_and_never_opens_a_card() {
    let mut app = Application::new(Shell::new(Locale::En, Theme::classic()));
    app.state.enter(Screen::Menu);
    app.state.on_key(Key::Soft(3));
    app.state.on_key(Key::Text('x'));
    let editor = app.state.search_input.clone();
    assert!(editor.is_some());
    let before = app.input_anchor();
    let generation = app.input_generation;
    app.focus(false);

    // This fixture has no installed client. The safe spawn failure still
    // records the attempted lock, without moving focus or losing text.
    assert!(!app.request_idle_lock());
    assert!(!app.focused);
    assert_eq!(app.state.screen, Screen::Menu);
    assert_eq!(app.state.search_input, editor);
    assert!(app.state.system_view.is_none());
    assert_eq!(app.state.lock_state.unwrap().phase, locking::Phase::Failed);
    assert!(app.automatic_lock_request.is_none());
    assert!(before == app.input_anchor());
    assert_ne!(app.input_generation, generation);
}

#[test]
fn only_the_matching_terminal_automatic_failure_consumes_its_notice() {
    let mut app = Application::new(Shell::new(Locale::En, Theme::classic()));
    app.state.enter(Screen::Menu);
    let Request::LockSession { request: id } = app.state.begin_lock(false) else {
        panic!("Expected automatic request");
    };
    // Model the accepted asynchronous request without starting a real
    // credential client or contacting a session bus in this unit test.
    app.automatic_lock_request = Some(id);
    app.focus(false);
    app.receive_lock(locking::Message::Changed(locking::State {
        id: id + 1,
        phase: locking::Phase::Failed,
    }));
    assert_eq!(app.automatic_lock_request, Some(id));
    app.receive_lock(locking::Message::Changed(locking::State {
        id,
        phase: locking::Phase::Uncertain,
    }));
    assert_eq!(app.automatic_lock_request, Some(id));
    assert!(app.lock_inhibits_activation());
    assert!(app.state.system_view.is_none());

    app.receive_lock(locking::Message::Changed(locking::State {
        id,
        phase: locking::Phase::Failed,
    }));
    assert!(app.automatic_lock_request.is_none());
    let before = app.input_anchor();
    app.dirty = false;
    app.receive_lock(locking::Message::Changed(locking::State {
        id,
        phase: locking::Phase::Failed,
    }));
    assert!(!app.dirty);
    assert!(app.automatic_lock_request.is_none());
    assert!(!app.focused);
    assert_eq!(app.state.screen, Screen::Menu);
    assert!(before == app.input_anchor());
}

#[test]
fn successful_automatic_unlock_retires_its_notice_without_a_failure() {
    let mut app = Application::new(Shell::new(Locale::En, Theme::classic()));
    let Request::LockSession { request: id } = app.state.begin_lock(false) else {
        panic!("Expected automatic request");
    };
    app.automatic_lock_request = Some(id);
    app.receive_lock(locking::Message::Changed(locking::State {
        id,
        phase: locking::Phase::Locked,
    }));
    assert_eq!(app.automatic_lock_request, Some(id));
    app.receive_lock(locking::Message::Unlocked { id: id + 1 });
    assert_eq!(app.automatic_lock_request, Some(id));
    assert!(app.lock_inhibits_activation());
    app.receive_lock(locking::Message::Unlocked { id });
    assert!(app.automatic_lock_request.is_none());
    assert!(!app.lock_inhibits_activation());
}

#[test]
fn automatic_lock_cannot_replace_an_existing_request_or_presence() {
    for message in [
        locking::Message::Presence(true),
        locking::Message::Journal(true),
    ] {
        let mut app = Application::new(Shell::new(Locale::En, Theme::classic()));
        app.receive_lock(message);
        assert!(!app.request_idle_lock());
        assert!(app.state.lock_state.is_none());
    }
    let mut app = Application::new(Shell::new(Locale::En, Theme::classic()));
    let request = app.state.begin_lock(true);
    assert!(matches!(request, Request::LockSession { .. }));
    let before = app.input_anchor();
    assert!(!app.request_idle_lock());
    assert!(before == app.input_anchor());
}

#[test]
fn external_lock_presence_drops_home_and_softkeys_without_granting_unlock() {
    let mut app = Application::new(Shell::new(Locale::En, Theme::classic()));
    app.state.enter(Screen::Menu);
    app.receive_lock(locking::Message::Presence(true));
    for key in [
        Key::Enter,
        Key::Soft(0),
        Key::Soft(1),
        Key::Soft(2),
        Key::Soft(3),
        Key::Escape,
        Key::Home,
    ] {
        assert_eq!(app.apply(key), Request::None);
        assert_eq!(app.state.screen, Screen::Menu);
    }
    app.home();
    assert_eq!(app.state.screen, Screen::Menu);
    assert!(app.banner().is_none());
    assert!(app.state.lock_state.is_none());
    app.receive_lock(locking::Message::Presence(false));
    app.home();
    assert_eq!(app.state.screen, Screen::Home);
}

#[test]
fn dropping_public_presence_is_not_an_unlock_for_our_running_locker() {
    let mut app = Application::new(Shell::new(Locale::En, Theme::classic()));
    app.state.lock_state = Some(locking::State {
        id: 7,
        phase: locking::Phase::Locked,
    });
    app.receive_lock(locking::Message::Presence(false));
    assert!(app.lock_inhibits_shell());
    app.receive_lock(locking::Message::Unlocked { id: 8 });
    assert!(app.lock_inhibits_shell());
    app.receive_lock(locking::Message::Unlocked { id: 7 });
    assert!(!app.lock_inhibits_shell());
}

#[test]
fn journal_keeps_restarted_shell_suppressed_when_session_bus_is_lost() {
    let mut app = Application::new(Shell::new(Locale::En, Theme::classic()));
    app.state.enter(Screen::Menu);
    app.receive_lock(locking::Message::Journal(true));
    app.receive_lock(locking::Message::Presence(false));
    assert!(app.lock_inhibits_shell());
    assert!(app.lock_inhibits_activation());
    app.home();
    assert_eq!(app.apply(Key::Home), Request::None);
    assert_eq!(app.state.screen, Screen::Menu);
    assert!(app.state.lock_state.is_none());
    // An advisory clear never completes one of our own pending attempts.
    app.state.lock_state = Some(locking::State {
        id: 7,
        phase: locking::Phase::Uncertain,
    });
    app.receive_lock(locking::Message::Journal(false));
    assert!(app.lock_inhibits_activation());
    assert!(app.state.lock_state.is_some());
    app.receive_lock(locking::Message::Unlocked { id: 7 });
    assert!(!app.lock_inhibits_activation());
}

#[test]
fn starting_and_uncertain_lock_block_external_home_but_keep_local_back_without_retry() {
    for phase in [locking::Phase::Starting, locking::Phase::Uncertain] {
        let mut app = Application::new(Shell::new(Locale::En, Theme::classic()));
        app.state.on_key(Key::Soft(1));
        for _ in 0..QuickAction::LOCK_INDEX {
            app.state.on_key(Key::Down);
        }
        let Request::LockSession { request: id } = app.state.on_key(Key::Soft(0)) else {
            panic!("Expected the original lock request");
        };
        if phase == locking::Phase::Uncertain {
            app.receive_lock(locking::Message::Changed(locking::State {
                id,
                phase: locking::Phase::Locked,
            }));
            app.receive_lock(locking::Message::Presence(true));
            app.receive_lock(locking::Message::Changed(locking::State { id, phase }));
            app.receive_lock(locking::Message::Presence(false));
        }
        let expected = Some(locking::State { id, phase });
        assert!(app.lock_inhibits_activation());
        assert!(!app.lock_inhibits_shell());

        // The same guard gates external Home and cancels queued activation
        // in the Wayland runtime. Home must not silently dismiss this card.
        let card = app.input_anchor();
        app.home();
        assert!(card == app.input_anchor());
        assert_eq!(app.apply(Key::Escape), Request::None);
        assert!(app.state.system_view.is_none());
        assert_eq!(app.state.lock_state, expected);

        // Local navigation is still useful when a request is uncertain;
        // an out-of-band Home cannot replace that newly chosen screen.
        assert_eq!(app.apply(Key::Soft(2)), Request::None);
        assert_eq!(app.state.screen, Screen::Menu);
        let menu = app.input_anchor();
        app.home();
        assert!(menu == app.input_anchor());
        assert!(app.lock_inhibits_activation());
        assert_eq!(app.apply(Key::Escape), Request::None);
        assert_eq!(app.apply(Key::Soft(1)), Request::None);
        assert!(matches!(
            app.state.system_view,
            Some(SystemView::LockStatus { .. })
        ));
        for key in [Key::Enter, Key::Soft(0), Key::Soft(1)] {
            assert_eq!(app.apply(key), Request::None);
        }
        assert_eq!(app.state.lock_state, expected);
        assert!(app.state.lock_busy());
        assert!(app.state.system_request.is_none());
    }
}

#[test]
fn missing_locker_and_changed_status_cannot_turn_a_queued_key_into_a_new_action() {
    let mut app = Application::new(Shell::new(Locale::Ru, Theme::classic()));
    app.apply(Key::Soft(1));
    for _ in 0..QuickAction::LOCK_INDEX {
        app.apply(Key::Down);
    }
    let menu = app.input_anchor();
    app.apply(Key::Soft(0)); // No worker was installed in this unit fixture.
    assert_eq!(app.state.lock_state.unwrap().phase, locking::Phase::Failed);
    assert!(!app.system_input_matches_frame(Some(&menu), Key::Enter));
    let error = app.input_anchor();
    assert!(app.system_input_matches_frame(Some(&error), Key::Enter));
    assert!(app.state.dialog.is_none());
}
