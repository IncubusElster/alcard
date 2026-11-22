use super::*;

fn new() -> Shell {
    Shell::new(Locale::Ru, Theme::classic())
}

fn window(id: u64, activity: u64) -> WindowInfo {
    WindowInfo {
        id,
        title: format!("Window {id}"),
        app_id: "editor".into(),
        active: false,
        activity,
    }
}

#[test]
fn home_has_quick_actions_and_no_demo_content() {
    let shell = new();
    assert_eq!(
        shell.actions(),
        [
            Action::Recent,
            Action::Quick,
            Action::Menu,
            Action::None,
            Action::Notifications,
        ]
    );
    assert!(shell.applications.is_empty());
    assert!(shell.notices.is_empty());
    assert!(shell.live_windows.is_empty());
}

fn ready_system(shell: &mut Shell) {
    shell.sync_system(SystemSnapshot {
        capabilities: [Capability::Allowed; 3],
    });
}

fn controls_snapshot(brightness: u8, volume: u8, muted: Option<bool>) -> controls::Snapshot {
    let reading = |percent, muted| controls::Reading {
        availability: controls::Availability::Available,
        percent: Some(percent),
        muted,
        above_safe_limit: false,
    };
    controls::Snapshot {
        brightness: reading(brightness, None),
        volume: reading(volume, muted),
    }
}

fn open_controls(shell: &mut Shell) -> u64 {
    assert_eq!(shell.on_key(Key::Soft(1)), Request::RefreshSystem);
    let Request::RefreshControls { request } = shell.refresh_controls() else {
        panic!("inline controls discovery was not requested");
    };
    request
}

fn ready_controls(shell: &mut Shell) {
    let request = open_controls(shell);
    assert!(shell.controls_message(controls::Message::Refreshed {
        request,
        snapshot: controls_snapshot(50, 20, Some(false)),
    }));
}

#[test]
fn inline_controls_reopening_preserves_an_inflight_request_and_selected_row() {
    let mut shell = new();
    let request = open_controls(&mut shell);
    assert_eq!(shell.controls_snapshot, controls::Snapshot::default());
    for key in [
        Key::Left,
        Key::Right,
        Key::Enter,
        Key::Soft(1),
        Key::Soft(2),
        Key::Soft(3),
    ] {
        assert_eq!(shell.on_key(key), Request::None);
    }
    shell.on_key(Key::Down);
    assert_eq!(shell.on_key(Key::Soft(4)), Request::None);
    assert!(shell.system_view.is_none());
    assert_eq!(shell.on_key(Key::Soft(1)), Request::RefreshSystem);
    assert_eq!(shell.refresh_controls(), Request::None);
    assert_eq!(
        shell.system_view,
        Some(SystemView::Menu {
            selected: QuickAction::VOLUME_INDEX
        })
    );
    assert_eq!(
        shell.controls_request,
        Some(ControlsRequest {
            id: request,
            operation: None
        })
    );
    assert!(shell.controls_message(controls::Message::Refreshed {
        request,
        snapshot: controls_snapshot(50, 20, Some(false)),
    }));
    assert_eq!(
        shell.system_view,
        Some(SystemView::Menu {
            selected: QuickAction::VOLUME_INDEX
        })
    );
    assert_eq!(shell.actions()[1], Action::Mute);
}

#[test]
fn controls_change_waits_for_matching_readback_and_does_not_repeat_while_pending() {
    let mut shell = new();
    ready_controls(&mut shell);
    let before = shell.controls_snapshot;
    let Request::AdjustControl { request, operation } = shell.on_key(Key::Right) else {
        panic!("brightness increase was not requested");
    };
    assert_eq!(
        operation,
        controls::Operation::Adjust {
            control: Control::Brightness,
            direction: Direction::Increase
        }
    );
    assert_eq!(shell.controls_snapshot, before);
    for key in [
        Key::Left,
        Key::Right,
        Key::Enter,
        Key::Soft(1),
        Key::Soft(2),
        Key::Soft(3),
    ] {
        assert_eq!(shell.on_key(key), Request::None);
    }
    let response = controls_snapshot(55, 20, Some(false));
    assert!(!shell.controls_message(controls::Message::Finished {
        request: request + 1,
        operation,
        snapshot: response,
        result: Ok(()),
    }));
    assert!(!shell.controls_message(controls::Message::Finished {
        request,
        operation: controls::Operation::ToggleMute,
        snapshot: response,
        result: Ok(()),
    }));
    assert!(!shell.controls_message(controls::Message::Refreshed {
        request,
        snapshot: response
    }));
    shell.on_key(Key::Home);
    assert_eq!(shell.on_key(Key::Soft(1)), Request::RefreshSystem);
    assert_eq!(shell.on_key(Key::Soft(1)), Request::None);
    assert_eq!(
        shell.controls_request,
        Some(ControlsRequest {
            id: request,
            operation: Some(operation)
        })
    );
    assert!(shell.controls_message(controls::Message::Finished {
        request,
        operation,
        snapshot: response,
        result: Ok(()),
    }));
    assert_eq!(shell.controls_snapshot, response);
    assert!(shell.controls_request.is_none());
    assert!(!shell.controls_message(controls::Message::Finished {
        request,
        operation,
        snapshot: before,
        result: Ok(()),
    }));
}

#[test]
fn controls_reply_after_home_preserves_the_current_application_view() {
    let mut shell = new();
    ready_controls(&mut shell);
    let Request::AdjustControl { request, operation } = shell.on_key(Key::Left) else {
        panic!("brightness decrease was not requested");
    };
    shell.on_key(Key::Home);
    shell.enter(Screen::Menu);
    shell.search_query = "keep this query".into();
    shell.selected = 3;
    assert!(shell.controls_message(controls::Message::Finished {
        request,
        operation,
        snapshot: controls_snapshot(45, 20, Some(false)),
        result: Ok(()),
    }));
    assert_eq!(shell.screen, Screen::Menu);
    assert_eq!(shell.selected, 3);
    assert_eq!(shell.search_query, "keep this query");
    assert!(shell.system_view.is_none());
    assert!(shell.dialog.is_none());
}

#[test]
fn controls_respect_capability_floor_ceiling_and_a_missing_mute_switch() {
    for availability in [
        controls::Availability::Checking,
        controls::Availability::Unavailable,
        controls::Availability::Denied,
    ] {
        let mut shell = new();
        shell.system_view = Some(SystemView::Menu {
            selected: QuickAction::VOLUME_INDEX,
        });
        shell.controls_snapshot = controls_snapshot(50, 20, Some(false));
        shell.controls_snapshot.volume.availability = availability;
        for key in [Key::Left, Key::Right, Key::Soft(1)] {
            assert_eq!(shell.on_key(key), Request::None);
        }
        assert_eq!(
            shell.actions(),
            [
                Action::None,
                Action::None,
                Action::None,
                Action::Retry,
                Action::Back
            ]
        );
    }
    let mut shell = new();
    shell.system_view = Some(SystemView::Menu {
        selected: QuickAction::BRIGHTNESS_INDEX,
    });
    shell.controls_snapshot = controls_snapshot(1, 0, None);
    assert_eq!(shell.on_key(Key::Left), Request::None);
    shell.controls_snapshot.brightness.percent = Some(0);
    assert_eq!(shell.on_key(Key::Left), Request::None);
    assert!(
        shell.control_operation_allowed(controls::Operation::Adjust {
            control: Control::Brightness,
            direction: Direction::Increase
        })
    );
    shell.controls_snapshot.brightness.percent = Some(100);
    assert_eq!(shell.on_key(Key::Right), Request::None);
    assert!(
        shell.control_operation_allowed(controls::Operation::Adjust {
            control: shell.system_view.as_ref().unwrap().control().unwrap(),
            direction: Direction::Decrease
        })
    );
    shell.on_key(Key::Down);
    assert_eq!(shell.on_key(Key::Left), Request::None);
    assert_eq!(shell.on_key(Key::Soft(1)), Request::None);
    shell.controls_snapshot.volume.percent = Some(100);
    shell.controls_snapshot.volume.above_safe_limit = true;
    assert_eq!(shell.on_key(Key::Right), Request::None);
    assert!(
        shell.control_operation_allowed(controls::Operation::Adjust {
            control: shell.system_view.as_ref().unwrap().control().unwrap(),
            direction: Direction::Decrease
        })
    );
    shell.controls_snapshot.volume.muted = Some(true);
    assert_eq!(shell.on_key(Key::Soft(1)), Request::None);
    shell.controls_snapshot.volume.muted = Some(false);
    assert_eq!(shell.actions()[1], Action::Mute);
    shell.controls_snapshot.volume.muted = None;
    shell.controls_snapshot.volume.percent = None;
    assert_eq!(shell.on_key(Key::Left), Request::None);
    assert_eq!(shell.on_key(Key::Right), Request::None);
}

#[test]
fn controls_mute_and_failed_readback_do_not_invent_success() {
    let mut shell = new();
    ready_controls(&mut shell);
    shell.on_key(Key::Down);
    let Request::AdjustControl { request, operation } = shell.on_key(Key::Soft(1)) else {
        panic!("mute was not requested");
    };
    assert_eq!(operation, controls::Operation::ToggleMute);
    assert_eq!(shell.controls_snapshot.volume.muted, Some(false));
    assert!(shell.controls_message(controls::Message::Finished {
        request,
        operation,
        snapshot: controls_snapshot(50, 20, Some(false)),
        result: Err(controls::Failure::Denied),
    }));
    assert_eq!(shell.controls_failure, Some(controls::Failure::Denied));
    assert_eq!(shell.actions()[1], Action::Mute);
    let Request::AdjustControl { request, operation } = shell.on_key(Key::Soft(1)) else {
        panic!("explicit retry was not requested");
    };
    assert!(shell.controls_message(controls::Message::Finished {
        request,
        operation,
        snapshot: controls_snapshot(50, 20, Some(true)),
        result: Ok(()),
    }));
    assert_eq!(shell.actions()[1], Action::Unmute);
    assert!(shell.controls_failure.is_none());
}

#[test]
fn controls_failed_submission_stays_inside_the_panel_and_refresh_has_a_new_id() {
    let mut shell = new();
    ready_controls(&mut shell);
    let change = shell.on_key(Key::Right);
    let Request::AdjustControl { request, operation } = change else {
        panic!("brightness request missing");
    };
    shell.request_failed(Request::AdjustControl {
        request: request + 1,
        operation,
    });
    assert!(shell.controls_request.is_some());
    shell.request_failed(change);
    assert!(matches!(shell.system_view, Some(SystemView::Menu { .. })));
    assert!(shell.dialog.is_none());
    assert!(shell.controls_request.is_none());
    assert_eq!(shell.controls_failure, Some(controls::Failure::Unavailable));
    assert_eq!(shell.controls_snapshot.brightness.percent, None);
    let Request::RefreshControls { request: refresh } = shell.on_key(Key::Soft(3)) else {
        panic!("explicit discovery retry missing");
    };
    assert!(refresh > request);
    assert!(!shell.controls_message(controls::Message::Finished {
        request,
        operation,
        snapshot: controls_snapshot(55, 20, Some(false)),
        result: Ok(()),
    }));
    assert_eq!(
        shell.controls_request,
        Some(ControlsRequest {
            id: refresh,
            operation: None
        })
    );
    assert!(shell.controls_message(controls::Message::Refreshed {
        request: refresh,
        snapshot: controls_snapshot(55, 20, Some(false)),
    }));
    assert!(shell.controls_failure.is_none());
}

fn ready_radios(shell: &mut Shell) {
    let state = |powered| radios::State {
        availability: radios::Availability::Available,
        powered: Some(powered),
        detail: String::new(),
    };
    shell.radios_snapshot = radios::Snapshot {
        wifi: state(true),
        bluetooth: state(false),
    };
}

fn radio_reply(shell: &Shell, entries: Vec<radios::Entry>) -> radios::Message {
    let pending = shell.radios_request.expect("one pending radio operation");
    radios::Message {
        request: pending.id,
        operation: pending.operation,
        snapshot: shell.radios_snapshot.clone(),
        entries,
        result: Ok(()),
    }
}

#[test]
fn quick_rows_adjust_inline_and_radio_actions_are_distinct() {
    let mut shell = new();
    ready_controls(&mut shell);
    ready_radios(&mut shell);
    assert_eq!(
        shell.system_view.as_ref().and_then(SystemView::control),
        Some(Control::Brightness)
    );
    assert_eq!(shell.on_key(Key::Enter), Request::None);
    shell.on_key(Key::Down);
    assert_eq!(
        shell.system_view.as_ref().and_then(SystemView::control),
        Some(Control::Volume)
    );
    assert_eq!(shell.actions()[1], Action::Mute);
    shell.on_key(Key::Down);
    assert_eq!(
        shell.system_view.as_ref().and_then(SystemView::radio),
        Some(radios::Kind::Wifi)
    );
    assert_eq!(shell.actions()[0], Action::Open);
    assert_eq!(shell.actions()[1], Action::TurnOff);
    for key in [Key::Left, Key::Right, Key::Soft(2)] {
        assert_eq!(shell.on_key(key), Request::None);
    }
    shell.on_key(Key::Down);
    assert_eq!(
        shell.system_view.as_ref().and_then(SystemView::radio),
        Some(radios::Kind::Bluetooth)
    );
    assert_eq!(shell.actions()[1], Action::TurnOn);
    shell.on_key(Key::Down);
    assert_eq!(
        shell.system_view,
        Some(SystemView::Menu {
            selected: QuickAction::system_index(Operation::Logout)
        })
    );
    assert!(shell.system_request.is_none());
}

#[test]
fn radio_toggle_requires_exact_readback_and_does_not_repeat_or_reopen() {
    let mut shell = new();
    ready_radios(&mut shell);
    shell.system_view = Some(SystemView::Menu {
        selected: QuickAction::WIFI_INDEX,
    });
    let Request::RadioOperation { request, operation } = shell.on_key(Key::Soft(1)) else {
        panic!("5 must request the radio toggle");
    };
    assert_eq!(
        operation,
        radios::Operation::SetPowered {
            kind: radios::Kind::Wifi,
            powered: false
        }
    );
    assert_eq!(shell.radios_snapshot.wifi.powered, Some(true));
    for key in [Key::Soft(0), Key::Soft(1), Key::Soft(3)] {
        assert_eq!(shell.on_key(key), Request::None);
    }
    let mut reply = radio_reply(&shell, Vec::new());
    reply.snapshot.wifi.powered = Some(false);
    reply.request = request + 1;
    assert!(!shell.radios_message(reply.clone()));
    reply.request = request;
    reply.operation = radios::Operation::List {
        kind: radios::Kind::Wifi,
    };
    assert!(!shell.radios_message(reply.clone()));
    reply.operation = operation;
    shell.on_key(Key::Home);
    shell.enter(Screen::Menu);
    assert!(shell.radios_message(reply.clone()));
    assert_eq!(shell.radios_snapshot.wifi.powered, Some(false));
    assert_eq!(shell.screen, Screen::Menu);
    assert!(shell.system_view.is_none());
    assert!(shell.dialog.is_none());
    assert!(!shell.radios_message(reply));
}

#[test]
fn radio_lists_return_to_the_same_row_and_refresh_preserves_device_identity() {
    let mut shell = new();
    ready_radios(&mut shell);
    shell.system_view = Some(SystemView::Menu {
        selected: QuickAction::BLUETOOTH_INDEX,
    });
    assert!(matches!(
        shell.on_key(Key::Soft(0)),
        Request::RadioOperation {
            operation: radios::Operation::List {
                kind: radios::Kind::Bluetooth
            },
            ..
        }
    ));
    let entry = |id: &str| radios::Entry {
        id: id.into(),
        name: id.into(),
        detail: "Observed device".into(),
    };
    let reply = radio_reply(&shell, vec![entry("device-b"), entry("device-a")]);
    assert!(shell.radios_message(reply));
    shell.on_key(Key::Down);
    shell.on_key(Key::Soft(0));
    assert!(
        matches!(&shell.system_view, Some(SystemView::RadioDetails { entry, .. }) if entry.id == "device-a")
    );
    shell.on_key(Key::Escape);
    assert!(matches!(
        shell.on_key(Key::Soft(3)),
        Request::RadioOperation { .. }
    ));
    let reply = radio_reply(&shell, vec![entry("device-a"), entry("device-b")]);
    assert!(shell.radios_message(reply));
    assert_eq!(
        shell.system_view,
        Some(SystemView::RadioList {
            kind: radios::Kind::Bluetooth,
            selected: 0
        })
    );
    shell.on_key(Key::Escape);
    assert_eq!(
        shell.system_view,
        Some(SystemView::Menu {
            selected: QuickAction::BLUETOOTH_INDEX
        })
    );
    assert_eq!(shell.actions()[1], Action::TurnOn);
}

#[test]
fn radio_absence_hardblock_and_failure_never_display_a_successful_toggle() {
    for availability in [
        radios::Availability::Checking,
        radios::Availability::Absent,
        radios::Availability::Unavailable,
        radios::Availability::Blocked,
    ] {
        let mut shell = new();
        ready_radios(&mut shell);
        shell.radios_snapshot.wifi.availability = availability;
        shell.system_view = Some(SystemView::Menu {
            selected: QuickAction::WIFI_INDEX,
        });
        assert_eq!(shell.actions()[1], Action::None);
        assert_eq!(shell.on_key(Key::Soft(1)), Request::None);
    }
    let mut shell = new();
    ready_radios(&mut shell);
    shell.system_view = Some(SystemView::Menu {
        selected: QuickAction::WIFI_INDEX,
    });
    let request = shell.on_key(Key::Soft(1));
    let mut failed = radio_reply(&shell, Vec::new());
    failed.result = Err(radios::Failure::Permission);
    assert!(shell.radios_message(failed));
    assert_eq!(shell.radios_snapshot.wifi.powered, Some(true));
    assert_eq!(shell.radios_failure, Some(radios::Failure::Permission));
    let retry = shell.on_key(Key::Soft(1));
    shell.request_failed(request); // A stale submission failure cannot clear the retry.
    assert!(shell.radio_busy());
    shell.request_failed(retry);
    assert!(!shell.radio_busy());
    assert_eq!(shell.radios_snapshot.wifi.powered, None);
    assert_eq!(
        shell.radios_snapshot.wifi.availability,
        radios::Availability::Unavailable
    );
    assert!(matches!(shell.system_view, Some(SystemView::Menu { .. })));
    assert!(shell.dialog.is_none());
}

fn ready_sleep(shell: &mut Shell) {
    shell.set_sleep_snapshot(sleep::Snapshot {
        epoch: 1,
        capability: Capability::Allowed,
        monitor: sleep::MonitorState::Ready,
        delay_ms: 5_000,
    });
}

fn open_suspend_confirmation(shell: &mut Shell) {
    assert_eq!(shell.on_key(Key::Soft(1)), Request::RefreshSystem);
    ready_system(shell);
    ready_sleep(shell);
    for _ in 0..QuickAction::SUSPEND_INDEX {
        shell.on_key(Key::Down);
    }
    assert_eq!(shell.actions()[0], Action::Select);
    assert_eq!(shell.on_key(Key::Soft(1)), Request::None);
    assert_eq!(shell.on_key(Key::Soft(0)), Request::None);
    assert!(matches!(
        shell.system_view,
        Some(SystemView::SuspendConfirm { .. })
    ));
    assert!(shell.sleep_progress.is_none());
}

fn prepare_suspend(shell: &mut Shell) -> u64 {
    open_suspend_confirmation(shell);
    let Request::PrepareSuspend { request } = shell.on_key(Key::Enter) else {
        panic!("explicit confirmation did not prepare suspend");
    };
    request
}

#[test]
fn suspend_after_radio_rows_requires_explicit_confirmation_for_one_request() {
    assert_eq!(QuickAction::ALL.len(), 9);
    assert_eq!(QuickAction::ALL[7], QuickAction::Lock);
    assert_eq!(QuickAction::ALL[8], QuickAction::Suspend);
    for key in [Key::Escape, Key::Backspace, Key::Soft(4), Key::Home] {
        let mut shell = new();
        open_suspend_confirmation(&mut shell);
        assert_eq!(shell.on_key(key), Request::None);
        assert!(shell.sleep_progress.is_none());
        assert!(shell.lock_state.is_none());
    }
    let mut shell = new();
    let request = prepare_suspend(&mut shell);
    assert_eq!(
        shell.sleep_progress,
        Some(sleep::Progress {
            request,
            stage: sleep::Stage::Preparing,
            failure: None,
        })
    );
    assert!(shell.lock_state.is_none());
    assert!(shell.system_request.is_none());
    assert!(shell.system_busy());
    assert_eq!(shell.actions()[4], Action::Cancel);
    for key in [Key::Enter, Key::Soft(0), Key::Soft(1), Key::Soft(3)] {
        assert_eq!(shell.on_key(key), Request::None);
    }
}

#[test]
fn suspend_challenge_and_unavailable_states_only_offer_information_and_refresh() {
    for capability in [
        Capability::Checking,
        Capability::Authentication,
        Capability::Denied,
        Capability::Unsupported,
        Capability::Blocked,
        Capability::Unavailable,
    ] {
        let mut shell = new();
        ready_sleep(&mut shell);
        shell.sleep_snapshot.capability = capability;
        shell.system_view = Some(SystemView::Menu {
            selected: QuickAction::SUSPEND_INDEX,
        });
        assert_eq!(shell.actions()[0], Action::Read);
        assert_eq!(shell.on_key(Key::Soft(0)), Request::None);
        assert!(matches!(
            shell.system_view,
            Some(SystemView::SuspendUnavailable { .. })
        ));
        assert_eq!(shell.actions()[0], Action::Acknowledge);
        assert_eq!(shell.on_key(Key::Soft(1)), Request::RefreshSleep);
        assert!(shell.sleep_progress.is_none());
        assert_eq!(shell.sleep_snapshot.capability, Capability::Checking);
        assert!(matches!(
            shell.system_view,
            Some(SystemView::Menu {
                selected: QuickAction::SUSPEND_INDEX
            })
        ));
    }
    for monitor in [
        sleep::MonitorState::Connecting,
        sleep::MonitorState::Preparing,
        sleep::MonitorState::Unavailable,
    ] {
        let mut shell = new();
        open_suspend_confirmation(&mut shell);
        let view = shell.system_view.clone();
        shell.set_sleep_snapshot(sleep::Snapshot {
            monitor,
            ..shell.sleep_snapshot
        });
        assert_eq!(shell.actions()[0], Action::None);
        assert_eq!(shell.on_key(Key::Enter), Request::None);
        assert_eq!(shell.system_view, view);
        assert!(shell.sleep_progress.is_none());
    }
    let mut shell = new();
    open_suspend_confirmation(&mut shell);
    shell.sleep_snapshot.capability = Capability::Authentication;
    assert_eq!(shell.on_key(Key::Enter), Request::None);
    assert!(shell.sleep_progress.is_none());
}

#[test]
fn suspend_back_or_home_cancels_precommit_without_discarding_an_actual_locker() {
    for stage in [sleep::Stage::Preparing, sleep::Stage::Locking] {
        for key in [Key::Escape, Key::Backspace, Key::Soft(4), Key::Home] {
            let mut shell = new();
            let request = prepare_suspend(&mut shell);
            shell.set_sleep_progress(Some(sleep::Progress {
                request,
                stage,
                failure: None,
            }));
            if stage == sleep::Stage::Locking {
                assert!(matches!(
                    shell.begin_sleep_lock(Some(request)),
                    Request::LockSession { .. }
                ));
            }
            let locker = shell.lock_state;
            assert_eq!(shell.on_key(key), Request::CancelSuspend { request });
            let view = shell.system_view.clone();
            assert_eq!(shell.lock_state, locker);
            // Only the coordinator confirms cancellation; hiding a card
            // must not invent its result or release the native lock.
            assert_eq!(shell.sleep_progress.unwrap().stage, stage);
            shell.set_sleep_progress(Some(sleep::Progress {
                request,
                stage: sleep::Stage::Cancelled,
                failure: None,
            }));
            assert_eq!(shell.system_view, view);
            assert_eq!(shell.lock_state, locker);
            assert_eq!(shell.begin_sleep_lock(Some(request)), Request::None);
        }
    }
}

#[test]
fn suspend_after_dispatch_back_never_claims_cancel_and_reopening_keeps_pending() {
    for stage in [
        sleep::Stage::Queued,
        sleep::Stage::Requested,
        sleep::Stage::Unknown,
    ] {
        for key in [Key::Escape, Key::Soft(4), Key::Home] {
            let mut shell = new();
            let request = prepare_suspend(&mut shell);
            shell.set_sleep_progress(Some(sleep::Progress {
                request,
                stage,
                failure: (stage == sleep::Stage::Unknown).then_some(Failure::Unknown),
            }));
            assert_eq!(shell.actions()[4], Action::Back);
            assert_eq!(shell.on_key(key), Request::None);
            assert!(shell.system_view.is_none());
            assert!(shell.system_busy());
            assert_eq!(shell.on_key(Key::Soft(1)), Request::None);
            assert!(matches!(
                shell.system_view,
                Some(SystemView::SuspendStatus { .. })
            ));
            assert_eq!(shell.sleep_progress.unwrap().stage, stage);
            assert_eq!(shell.on_key(Key::Enter), Request::None);
            assert_eq!(shell.begin_lock(true), Request::None);
        }
    }
}

#[test]
fn suspend_lock_uses_common_ids_and_bypasses_only_its_own_pending_guard() {
    let mut shell = new();
    shell.enter(Screen::Menu);
    shell.application_view = ApplicationView::Details {
        target: AppTarget::Desktop("editor.desktop".into()),
        scroll: 2,
    };
    let view = shell.application_view.clone();
    let Request::LockSession { request: first } = shell.begin_lock(false) else {
        panic!("manual lock missing");
    };
    shell.request_failed(Request::LockSession { request: first });
    ready_sleep(&mut shell);
    shell.set_sleep_progress(Some(sleep::Progress {
        request: 9,
        stage: sleep::Stage::Locking,
        failure: None,
    }));
    assert_eq!(shell.begin_lock(false), Request::None);
    assert_eq!(shell.begin_sleep_lock(Some(8)), Request::None);
    let Request::LockSession { request: second } = shell.begin_sleep_lock(Some(9)) else {
        panic!("own sleep preparation must be able to request its lock");
    };
    assert_eq!(second, first + 1);
    assert_eq!(shell.screen, Screen::Menu);
    assert_eq!(shell.application_view, view);
    assert!(shell.system_view.is_none());
    assert_eq!(shell.begin_sleep_lock(Some(9)), Request::None);
    shell.request_failed(Request::LockSession { request: second });
    shell.system_request = Some(SystemRequest {
        id: 3,
        operation: Operation::Reboot,
        phase: SystemPhase::Pending,
    });
    assert_eq!(shell.begin_sleep_lock(Some(9)), Request::None);
    shell.system_request = None;
    shell.set_sleep_progress(None);
    assert_eq!(shell.begin_sleep_lock(None), Request::None);
    shell.sleep_snapshot.monitor = sleep::MonitorState::Preparing;
    assert!(shell.system_busy());
    let Request::LockSession { request: third } = shell.begin_sleep_lock(None) else {
        panic!("external preparation must be able to request a lock");
    };
    assert_eq!(third, second + 1);
    assert_eq!(shell.application_view, view);
    assert!(shell.system_view.is_none());
}

#[test]
fn suspend_replies_do_not_steal_view_and_cycle_end_does_not_unlock() {
    let mut shell = new();
    let request = prepare_suspend(&mut shell);
    shell.set_sleep_progress(Some(sleep::Progress {
        request,
        stage: sleep::Stage::Locking,
        failure: None,
    }));
    let Request::LockSession { request: lock } = shell.begin_sleep_lock(Some(request)) else {
        panic!("sleep lock missing");
    };
    shell.lock_changed(locking::State {
        id: lock,
        phase: locking::Phase::Locked,
    });
    shell.set_sleep_progress(Some(sleep::Progress {
        request,
        stage: sleep::Stage::Requested,
        failure: None,
    }));
    shell.on_key(Key::Home);
    shell.enter(Screen::Menu);
    shell.application_view = ApplicationView::Details {
        target: AppTarget::Desktop("editor.desktop".into()),
        scroll: 3,
    };
    let view = shell.application_view.clone();
    let snapshot = sleep::Snapshot {
        epoch: 2,
        ..shell.sleep_snapshot
    };
    assert!(shell.set_sleep_snapshot(snapshot));
    assert!(!shell.set_sleep_snapshot(snapshot));
    let ended = Some(sleep::Progress {
        request,
        stage: sleep::Stage::Ended,
        failure: None,
    });
    assert!(shell.set_sleep_progress(ended));
    assert!(!shell.set_sleep_progress(ended));
    assert_eq!(shell.screen, Screen::Menu);
    assert_eq!(shell.application_view, view);
    assert!(shell.system_view.is_none());
    assert_eq!(
        shell.lock_state,
        Some(locking::State {
            id: lock,
            phase: locking::Phase::Locked,
        })
    );
    assert!(shell.lock_unlocked(lock));
    shell.go_home();
    assert_eq!(shell.on_key(Key::Soft(1)), Request::RefreshSystem);
    assert!(matches!(shell.system_view, Some(SystemView::Menu { .. })));
    // The coordinator may send the same terminal progress again; it is
    // still data, not an instruction to reopen the dismissed status.
    assert!(!shell.set_sleep_progress(ended));
    assert!(matches!(shell.system_view, Some(SystemView::Menu { .. })));
}

#[test]
fn suspend_failed_submission_is_local_and_explicit_retry_uses_a_new_id() {
    let mut shell = new();
    let request = prepare_suspend(&mut shell);
    shell.request_failed(Request::PrepareSuspend {
        request: request + 1,
    });
    assert!(shell.system_busy());
    shell.request_failed(Request::PrepareSuspend { request });
    assert_eq!(
        shell.sleep_progress,
        Some(sleep::Progress {
            request,
            stage: sleep::Stage::Failed,
            failure: Some(Failure::Unavailable),
        })
    );
    assert!(!shell.system_busy());
    assert!(shell.dialog.is_none());
    assert!(matches!(
        shell.system_view,
        Some(SystemView::SuspendStatus { .. })
    ));
    assert_eq!(shell.on_key(Key::Enter), Request::RefreshSystem);
    ready_sleep(&mut shell);
    shell.on_key(Key::Soft(0));
    let Request::PrepareSuspend { request: next } = shell.on_key(Key::Enter) else {
        panic!("explicit retry did not prepare a request");
    };
    assert!(next > request);
    shell.request_failed(Request::PrepareSuspend { request });
    assert_eq!(shell.sleep_progress.unwrap().request, next);
    assert_eq!(shell.sleep_progress.unwrap().stage, sleep::Stage::Preparing);
}

#[test]
fn suspend_status_closed_by_out_of_band_home_stays_dismissed() {
    for stage in [
        sleep::Stage::Cancelled,
        sleep::Stage::Failed,
        sleep::Stage::Ended,
    ] {
        let mut shell = new();
        let request = prepare_suspend(&mut shell);
        let progress = Some(sleep::Progress {
            request,
            stage,
            failure: (stage == sleep::Stage::Failed).then_some(Failure::Unavailable),
        });
        shell.set_sleep_progress(progress);
        // Application::home may cancel the coordinator first, then enter
        // Home directly without routing IPC through keyboard text editing.
        shell.go_home();
        assert_eq!(shell.sleep_progress, progress);
        assert!(shell.system_view.is_none());
        assert_eq!(shell.on_key(Key::Soft(1)), Request::RefreshSystem);
        assert!(matches!(shell.system_view, Some(SystemView::Menu { .. })));
        assert!(!shell.set_sleep_progress(progress));
        assert!(matches!(shell.system_view, Some(SystemView::Menu { .. })));
    }
}

#[test]
fn suspend_busy_disables_other_quick_mutations_and_device_controls() {
    for stage in [
        Some(sleep::Stage::Preparing),
        Some(sleep::Stage::Requested),
        Some(sleep::Stage::Unknown),
        None,
    ] {
        let mut shell = new();
        ready_system(&mut shell);
        ready_sleep(&mut shell);
        if let Some(stage) = stage {
            shell.set_sleep_progress(Some(sleep::Progress {
                request: 1,
                stage,
                failure: None,
            }));
        } else {
            shell.sleep_snapshot.monitor = sleep::MonitorState::Preparing;
        }
        assert!(shell.system_busy());
        for selected in 0..QuickAction::ALL.len() {
            shell.system_view = Some(SystemView::Menu { selected });
            for key in [Key::Soft(0), Key::Soft(1), Key::Left, Key::Right] {
                assert_eq!(shell.on_key(key), Request::None);
            }
            assert_eq!(shell.system_view, Some(SystemView::Menu { selected }));
        }
        shell.system_view = Some(SystemView::Confirm {
            operation: Operation::PowerOff,
            scroll: 0,
        });
        assert_eq!(shell.on_key(Key::Enter), Request::None);
        shell.system_view = Some(SystemView::Menu {
            selected: QuickAction::BRIGHTNESS_INDEX,
        });
        shell.controls_snapshot = controls_snapshot(50, 20, Some(false));
        assert_eq!(shell.on_key(Key::Right), Request::None);
        assert!(shell.controls_request.is_none());
        assert!(shell.system_request.is_none());
        assert_eq!(shell.begin_lock(true), Request::None);
    }
}

fn confirm_system(shell: &mut Shell, operation: Operation) -> Request {
    assert_eq!(shell.on_key(Key::Soft(1)), Request::RefreshSystem);
    ready_system(shell);
    for _ in 0..QuickAction::system_index(operation) {
        shell.on_key(Key::Down);
    }
    assert_eq!(shell.on_key(Key::Soft(1)), Request::None);
    assert_eq!(shell.on_key(Key::Soft(0)), Request::None);
    assert!(
        matches!(shell.system_view, Some(SystemView::Confirm { operation: selected, .. }) if selected == operation)
    );
    shell.on_key(Key::Enter)
}

#[test]
fn quick_menu_uses_fifth_key_and_cancel_never_sends_system_operation() {
    let mut shell = new();
    assert_eq!(shell.on_key(Key::Soft(1)), Request::RefreshSystem);
    assert_eq!(shell.screen, Screen::Home);
    assert_eq!(shell.on_key(Key::Enter), Request::None);
    assert_eq!(shell.on_key(Key::Soft(2)), Request::None);
    assert!(matches!(
        shell.system_view,
        Some(SystemView::Menu { selected: 0 })
    ));
    ready_system(&mut shell);
    for _ in 0..QuickAction::system_index(Operation::Reboot) {
        shell.on_key(Key::Down);
    }
    shell.on_key(Key::Soft(0));
    assert_eq!(
        shell.actions(),
        [
            Action::Confirm,
            Action::None,
            Action::None,
            Action::None,
            Action::Decline
        ]
    );
    shell.on_key(Key::Soft(4));
    assert!(matches!(
        shell.system_view,
        Some(SystemView::Menu { selected: 5 })
    ));
    assert!(shell.system_request.is_none());
    shell.on_key(Key::Escape);
    assert!(shell.system_view.is_none());
    assert_eq!(shell.actions()[1], Action::Quick);
    assert_eq!(shell.actions()[3], Action::None);
}

#[test]
fn system_unavailable_and_changed_capability_cannot_be_confirmed() {
    let mut shell = new();
    shell.on_key(Key::Soft(1));
    for capability in [
        Capability::Denied,
        Capability::Unsupported,
        Capability::Blocked,
        Capability::Unavailable,
    ] {
        shell.system_view = Some(SystemView::Menu {
            selected: QuickAction::system_index(Operation::PowerOff),
        });
        shell.sync_system(SystemSnapshot {
            capabilities: [capability; 3],
        });
        assert_eq!(shell.on_key(Key::Soft(0)), Request::None);
        assert!(matches!(
            shell.system_view,
            Some(SystemView::Unavailable { .. })
        ));
        assert_eq!(shell.on_key(Key::Enter), Request::None);
        assert!(shell.system_request.is_none());
    }
    ready_system(&mut shell);
    shell.on_key(Key::Soft(0));
    assert!(matches!(
        shell.system_view,
        Some(SystemView::Confirm { .. })
    ));
    shell.sync_system(SystemSnapshot {
        capabilities: [Capability::Blocked; 3],
    });
    assert_eq!(shell.actions()[0], Action::None);
    assert_eq!(shell.on_key(Key::Enter), Request::None);
    assert!(shell.system_request.is_none());
}

#[test]
fn system_pending_acceptance_and_unknown_outcome_never_allow_duplicate_requests() {
    for result in [Ok(()), Err(Failure::Unknown)] {
        let mut shell = new();
        let Request::SystemOperation { request, operation } =
            confirm_system(&mut shell, Operation::Reboot)
        else {
            panic!("missing operation");
        };
        for key in [Key::Enter, Key::Soft(0), Key::Soft(1)] {
            assert_eq!(shell.on_key(key), Request::None);
        }
        shell.on_key(Key::Home);
        assert!(shell.system_view.is_none());
        assert!(!shell.system_finished(request + 1, operation, Ok(())));
        assert!(!shell.system_finished(request, Operation::PowerOff, Ok(())));
        assert!(shell.system_finished(request, operation, result));
        assert!(shell.system_view.is_none());
        assert!(shell.system_busy());
        assert_eq!(shell.on_key(Key::Soft(1)), Request::None);
        assert!(matches!(
            shell.system_view,
            Some(SystemView::Status {
                operation: Operation::Reboot,
                ..
            })
        ));
        assert_eq!(
            shell.actions(),
            [
                Action::None,
                Action::None,
                Action::None,
                Action::None,
                Action::Back
            ]
        );
        assert_eq!(shell.on_key(Key::Enter), Request::None);
        assert!(!shell.system_finished(request, operation, Err(Failure::Denied)));
    }
}

#[test]
fn definite_system_failure_requires_ack_refresh_selection_and_new_confirmation() {
    let mut shell = new();
    let Request::SystemOperation { request, operation } =
        confirm_system(&mut shell, Operation::PowerOff)
    else {
        panic!("missing operation");
    };
    assert!(shell.system_finished(request, operation, Err(Failure::Denied)));
    assert!(!shell.system_busy());
    assert_eq!(shell.on_key(Key::Enter), Request::RefreshSystem);
    assert!(shell.system_request.is_none());
    assert_eq!(shell.on_key(Key::Enter), Request::None);
    ready_system(&mut shell);
    assert_eq!(shell.on_key(Key::Soft(0)), Request::None);
    let Request::SystemOperation {
        request: next,
        operation: next_operation,
    } = shell.on_key(Key::Enter)
    else {
        panic!("missing retry");
    };
    assert!(next > request);
    assert_eq!(next_operation, operation);
}

fn app(id: &str) -> MenuApp {
    MenuApp {
        desktop_id: format!("{id}.desktop"),
        name: id.to_owned(),
        icon: None,
        startup_wm_class: None,
    }
}

#[test]
fn menu_launches_only_discovered_apps() {
    let mut shell = new();
    shell.enter(Screen::Menu);
    assert_eq!(shell.actions()[0], Action::None);
    shell.set_applications(vec![app("files"), app("terminal")]);
    assert_eq!(shell.actions()[0], Action::Open);
    assert_eq!(
        shell.on_key(Key::Soft(0)),
        Request::LaunchDesktop("files.desktop".into())
    );
    shell.on_key(Key::Right);
    assert_eq!(
        shell.on_key(Key::Enter),
        Request::LaunchDesktop("terminal.desktop".into())
    );
    assert_eq!(shell.actions()[2], Action::None);
}

#[test]
fn menu_paging_and_home_preserve_window_state() {
    let mut shell = new();
    shell.set_applications((0..10).map(|i| app(&format!("app{i}"))).collect());
    shell.enter(Screen::Menu);
    for _ in 0..8 {
        shell.on_key(Key::Right);
    }
    assert_eq!(shell.page, 1);
    shell.sync_windows(vec![window(15, 1)]);
    shell.on_key(Key::Home);
    assert_eq!(shell.screen, Screen::Home);
    assert_eq!(shell.live_windows.len(), 1);
}

#[test]
fn menu_activates_mru_window_without_duplicate_launch() {
    let mut shell = new();
    shell.set_applications(vec![app("editor")]);
    let mut w1 = window(12, 7);
    let mut w2 = window(13, 10);
    w1.app_id = "editor".into();
    w2.app_id = "editor.desktop".into();
    shell.sync_windows(vec![w1, w2]);
    shell.enter(Screen::Menu);
    assert_eq!(shell.running_count(0), 2);
    assert_eq!(shell.on_key(Key::Enter), Request::ActivateWindow(13));
}

#[test]
fn startup_wm_class_fallback_is_unambiguous() {
    let mut shell = new();
    let mut editor = app("org.example.Editor");
    editor.startup_wm_class = Some("editor-window".into());
    shell.set_applications(vec![editor]);
    let mut window = window(17, 1);
    window.app_id = "EDITOR-WINDOW".into();
    shell.sync_windows(vec![window]);
    shell.enter(Screen::Menu);
    assert_eq!(shell.on_key(Key::Enter), Request::ActivateWindow(17));

    let mut competing = app("org.other.Editor");
    competing.startup_wm_class = Some("editor-window".into());
    let mut original = app("org.example.Editor");
    original.startup_wm_class = Some("editor-window".into());
    shell.set_applications(vec![original, competing]);
    // Two apps claim the same StartupWMClass: do not guess.
    assert!(shell.resolve_app("EDITOR-WINDOW").is_none());
    assert_eq!(
        shell.on_key(Key::Enter),
        Request::LaunchDesktop("org.example.Editor.desktop".into())
    );
}

#[test]
fn unknown_app_id_is_not_guessed_from_title_or_exec() {
    let mut shell = new();
    shell.set_applications(vec![app("editor")]);
    let mut w = window(4, 2);
    w.app_id = "unrelated".into();
    w.title = "editor".into();
    shell.sync_windows(vec![w]);
    shell.enter(Screen::Menu);
    assert_eq!(
        shell.on_key(Key::Enter),
        Request::LaunchDesktop("editor.desktop".into())
    );
}

#[test]
fn menu_refresh_preserves_selected_desktop_id_across_reordering() {
    let mut shell = new();
    shell.set_applications(vec![app("alpha"), app("beta"), app("gamma")]);
    shell.enter(Screen::Menu);
    shell.selected = 1;
    shell.set_applications(vec![app("gamma"), app("alpha"), app("beta")]);
    assert_eq!(shell.selected, 2);
    assert_eq!(
        shell.on_key(Key::Enter),
        Request::LaunchDesktop("beta.desktop".into())
    );
}

#[test]
fn exact_desktop_id_has_priority_over_startup_class() {
    let mut shell = new();
    let mut wrong = app("unrelated");
    wrong.startup_wm_class = Some("editor".into());
    shell.set_applications(vec![wrong, app("editor")]);
    let mut window = window(11, 1);
    window.app_id = "editor".into();
    shell.sync_windows(vec![window]);
    shell.enter(Screen::Menu);
    shell.selected = 1;
    assert_eq!(shell.on_key(Key::Enter), Request::ActivateWindow(11));
}

#[test]
fn closed_window_can_relaunch_only_a_known_installed_app() {
    let mut shell = new();
    shell.set_applications(vec![app("editor")]);
    shell.sync_windows(vec![window(9, 12)]);
    shell.sync_windows(vec![]);
    shell.enter(Screen::Recent);
    assert_eq!(shell.actions()[0], Action::Launch);
    assert_eq!(
        shell.on_key(Key::Soft(0)),
        Request::LaunchDesktop("editor.desktop".into())
    );
    shell.set_applications(vec![]);
    assert_eq!(shell.actions()[0], Action::None);
    assert_eq!(shell.on_key(Key::Soft(0)), Request::None);
}

#[test]
fn identical_titles_keep_separate_closed_window_history() {
    let mut shell = new();
    shell.set_applications(vec![app("editor")]);
    let mut one = window(1, 3);
    let mut two = window(2, 4);
    one.title = "Untitled".into();
    two.title = "Untitled".into();
    shell.sync_windows(vec![one, two]);
    shell.sync_windows(vec![]);
    assert_eq!(shell.closed_windows.len(), 2);
    assert_ne!(shell.closed_windows[0].id, shell.closed_windows[1].id);
    shell.enter(Screen::Recent);
    assert_eq!(shell.actions()[0], Action::Launch);
    shell.selected = 1;
    assert_eq!(
        shell.on_key(Key::Soft(0)),
        Request::LaunchDesktop("editor.desktop".into())
    );
}

#[test]
fn closed_window_reactivates_other_live_window_of_same_app() {
    let mut shell = new();
    shell.set_applications(vec![app("editor")]);
    shell.sync_windows(vec![window(5, 3), window(6, 8)]);
    shell.sync_windows(vec![window(6, 8)]);
    shell.enter(Screen::Recent);
    shell.selected = 1;
    assert_eq!(shell.on_key(Key::Soft(0)), Request::ActivateWindow(6));
}

fn close_options(shell: &mut Shell) {
    if shell.application_view == ApplicationView::List {
        shell.on_key(Key::Soft(1));
    }
    assert_eq!(shell.actions()[0], Action::None);
    assert_eq!(shell.on_key(Key::Enter), Request::None);
    shell.on_key(Key::Soft(1));
    assert!(matches!(shell.dialog, Some(Dialog::ConfirmClose(_))));
}

#[test]
fn recent_activation_and_close_use_stable_window_ids() {
    let mut shell = new();
    shell.enter(Screen::Recent);
    shell.sync_windows(vec![window(11, 3), window(10, 5)]);
    assert_eq!(shell.on_key(Key::Soft(0)), Request::ActivateWindow(10));
    close_options(&mut shell);
    assert!(matches!(shell.dialog, Some(Dialog::ConfirmClose(10))));
    shell.sync_windows(vec![window(11, 8), window(10, 5)]);
    shell.on_key(Key::Left);
    assert_eq!(shell.on_key(Key::Soft(0)), Request::CloseWindow(10));
    assert_eq!(shell.live_windows.len(), 2);
    shell.sync_windows(vec![window(11, 8)]);
    assert_eq!(shell.closed_windows.len(), 1);
    assert_eq!(
        shell.window_at(1).map(|(w, running)| (w.id, running)),
        Some((10, false))
    );
    // Closing the selected window does not transfer its Close option to
    // another live window. An unknown historical app has only its card.
    assert!(matches!(
        shell.application_view,
        ApplicationView::Details { .. }
    ));
    assert_eq!(shell.actions()[0], Action::None);
    assert_eq!(shell.actions()[1], Action::Actions);
}

#[test]
fn close_confirmation_uses_direct_yes_and_no_softkeys() {
    for primary in [Key::Enter, Key::Soft(0)] {
        for cancel in [Key::Soft(4), Key::Escape, Key::Backspace] {
            let mut shell = new();
            shell.enter(Screen::Recent);
            shell.sync_windows(vec![window(10, 5)]);
            close_options(&mut shell);
            assert_eq!(
                shell.actions(),
                [
                    Action::Confirm,
                    Action::None,
                    Action::None,
                    Action::None,
                    Action::Decline
                ]
            );
            assert_eq!(shell.on_key(cancel), Request::None);
            assert!(shell.dialog.is_none());
            assert_eq!(shell.live_windows.len(), 1);
            close_options(&mut shell);
            shell.on_key(Key::Right);
            assert_eq!(shell.actions()[0], Action::Confirm);
            assert_eq!(shell.on_key(primary), Request::CloseWindow(10));
        }
    }
}

#[test]
fn modal_focus_does_not_mutate_underlying_selection_or_trigger_repeat() {
    let mut shell = new();
    shell.enter(Screen::Recent);
    shell.sync_windows(vec![window(10, 5), window(11, 3)]);
    shell.selected = 1;
    close_options(&mut shell);
    // Repeated Options/Select is inert in the confirmation.
    assert_eq!(shell.on_key(Key::Soft(1)), Request::None);
    shell.on_key(Key::Right);
    assert_eq!(shell.selected, 1);
    assert_eq!(shell.on_key(Key::Escape), Request::None);
    assert_eq!(shell.selected, 1);
    assert!(shell.dialog.is_none());
}

#[test]
fn closed_window_before_confirmation_does_not_close_another() {
    let mut shell = new();
    shell.enter(Screen::Recent);
    shell.sync_windows(vec![window(10, 5)]);
    close_options(&mut shell);
    shell.on_key(Key::Left);
    shell.sync_windows(vec![window(20, 6)]);
    assert_eq!(shell.on_key(Key::Soft(0)), Request::None);
}

#[test]
fn recent_selection_survives_reordering_and_service_failure() {
    let mut shell = new();
    shell.enter(Screen::Recent);
    shell.sync_windows(vec![window(1, 2), window(2, 1)]);
    shell.selected = 1;
    shell.sync_windows(vec![window(1, 2), window(2, 3)]);
    assert_eq!(shell.window_at(shell.selected).unwrap().0.id, 2);
    shell.lose_window_service();
    assert!(shell.toplevel_unavailable);
    assert!(shell.live_windows.is_empty());
    assert!(shell.closed_windows.is_empty());
}

fn notice(id: u32, generation: u64, active: bool) -> Notice {
    Notice {
        id,
        generation,
        title: format!("Notice {id}"),
        description: "Complete body".into(),
        app_name: "Example".into(),
        desktop_entry: Some("org.example.App".into()),
        created_at: 1_700_000_000,
        urgency: 1,
        unread: true,
        actions: vec![NoticeAction {
            key: "default".into(),
            label: "Open".into(),
        }],
        active,
    }
}

#[test]
fn expired_notifications_remain_readable_and_read_is_explicit() {
    let mut shell = new();
    let mut item = notice(7, 10, false);
    item.description = format!("First line\n\n{}\nLast line", "Long body ".repeat(400));
    shell.sync_notices(vec![item]);
    shell.enter(Screen::Notifications);
    assert_eq!(shell.unread_count(), 1); // Entering the list is not reading.
    assert_eq!(shell.actions()[0], Action::Read);
    assert_eq!(
        shell.on_key(Key::Soft(0)),
        Request::MarkNotificationRead {
            id: 7,
            generation: 10
        }
    );
    assert_eq!(shell.actions()[0], Action::None); // An expired action cannot run.
    assert_eq!(shell.unread_count(), 1); // Only the canonical backend clears unread.
    for _ in 0..1_000 {
        shell.on_key(Key::Down);
    }
    let lines = shell.viewed_notice().unwrap().text_lines(shell.locale);
    assert_eq!(lines.last().unwrap(), "Last line");
    assert!(
        matches!(shell.notification_view, NotificationView::Detail { scroll, .. } if scroll == lines.len() - NOTICE_ROWS)
    );
    assert_eq!(shell.on_key(Key::Enter), Request::None);
    shell.on_key(Key::Soft(4));
    assert_eq!(shell.notification_view, NotificationView::List);
    assert_eq!(shell.notices[shell.selected].id, 7);
}

#[test]
fn every_named_action_is_reachable_and_generation_bound() {
    let mut shell = new();
    let mut item = notice(7, 22, true);
    item.actions = (0..64)
        .map(|i| NoticeAction {
            key: format!("action-{i}"),
            label: format!("Action {i}"),
        })
        .collect();
    shell.sync_notices(vec![item]);
    shell.enter(Screen::Notifications);
    assert_eq!(
        shell.on_key(Key::Enter),
        Request::MarkNotificationRead {
            id: 7,
            generation: 22
        }
    );
    assert_eq!(shell.actions()[0], Action::None);
    assert_eq!(shell.actions()[1], Action::Actions);
    assert_eq!(shell.on_key(Key::Enter), Request::None);
    shell.on_key(Key::Soft(1)); // Open context through Options.
    assert_eq!(shell.on_key(Key::Enter), Request::None); // Select belongs to 5.
    for _ in 0..63 {
        shell.on_key(Key::Down);
    }
    assert_eq!(
        shell.on_key(Key::Soft(1)),
        Request::InvokeNotification {
            id: 7,
            generation: 22,
            action: "action-63".into(),
        }
    );
    let mut expired = shell.notices[0].clone();
    expired.active = false;
    shell.sync_notices(vec![expired]);
    assert!(matches!(
        shell.notification_view,
        NotificationView::Detail { .. }
    ));
    assert_eq!(shell.actions()[0], Action::None);
    assert_eq!(shell.on_key(Key::Enter), Request::None);
}

#[test]
fn notification_updates_preserve_selection_but_never_retarget_open_content() {
    let mut shell = new();
    shell.sync_notices(vec![notice(7, 1, true), notice(8, 2, true)]);
    shell.enter(Screen::Notifications);
    shell.selected = 1;
    shell.on_key(Key::Soft(0));
    shell.sync_notices(vec![
        notice(9, 3, true),
        notice(7, 1, true),
        notice(8, 2, true),
    ]);
    assert_eq!(shell.notices[shell.selected].id, 8);
    assert!(matches!(
        shell.notification_view,
        NotificationView::Detail {
            id: 8,
            generation: 2,
            ..
        }
    ));
    shell.on_key(Key::Soft(4));
    assert_eq!(shell.notices[shell.selected].id, 8);
    shell.on_key(Key::Soft(0));
    shell.sync_notices(vec![notice(9, 3, true), notice(8, 4, true)]);
    assert_eq!(shell.notification_view, NotificationView::List);
    assert_eq!(shell.notices[shell.selected].id, 8);
    assert!(shell.notices[shell.selected].unread);
    shell.on_key(Key::Soft(0));
    shell.sync_notices(vec![notice(9, 3, true)]);
    assert_eq!(shell.notification_view, NotificationView::List);
    assert_eq!(shell.selected, 0);
}

#[test]
fn unavailable_notifications_keep_history_readable_and_offer_retry() {
    let mut shell = new();
    shell.sync_notices(vec![notice(7, 1, true)]);
    shell.enter(Screen::Notifications);
    shell.notification_service(false);
    assert_eq!(shell.actions()[0], Action::Read);
    assert_eq!(shell.actions()[1], Action::Actions);
    assert_eq!(shell.actions()[2], Action::None);
    assert_eq!(shell.on_key(Key::Soft(0)), Request::None);
    assert!(matches!(
        shell.notification_view,
        NotificationView::Detail { .. }
    ));
    assert_eq!(shell.actions()[0], Action::None);
    shell.on_key(Key::Soft(1));
    assert_eq!(shell.on_key(Key::Soft(1)), Request::RetryNotifications);
    shell.notification_service(true);
    assert!(matches!(
        shell.notification_view,
        NotificationView::Detail { .. }
    ));
    shell.on_key(Key::Soft(1));
    shell.notification_service(false);
    assert!(matches!(
        shell.notification_view,
        NotificationView::Detail { .. }
    ));
    shell.sync_notices(vec![]);
    assert_eq!(shell.actions()[0], Action::None);
    shell.on_key(Key::Soft(1));
    assert_eq!(shell.on_key(Key::Soft(1)), Request::RetryNotifications);
}

#[test]
fn notification_delete_and_clear_wait_for_the_backend() {
    let mut shell = new();
    shell.sync_notices(vec![notice(7, 1, true), notice(8, 2, false)]);
    shell.enter(Screen::Notifications);
    shell.on_key(Key::Soft(1));
    shell.on_key(Key::Down); // Named action, Delete, Clear.
    assert_eq!(shell.on_key(Key::Soft(1)), Request::DismissNotification(7));
    assert_eq!(shell.notices.len(), 2);
    shell.on_key(Key::Down);
    shell.on_key(Key::Soft(1));
    assert!(matches!(shell.dialog, Some(Dialog::ConfirmClear)));
    assert_eq!(shell.actions()[0], Action::Clear);
    assert_eq!(shell.on_key(Key::Escape), Request::None);
    shell.on_key(Key::Soft(1));
    assert_eq!(shell.on_key(Key::Soft(0)), Request::ClearNotifications);
    assert_eq!(shell.notices.len(), 2);
    shell.sync_notices(vec![]);
    assert_eq!(shell.unread_count(), 0);
}

#[test]
fn wrapping_preserves_unicode_long_words_and_paragraphs() {
    let value = format!("Первая строка\n\n{}\nКонец", "Я".repeat(75));
    let lines = wrap_text(&value, 36);
    assert_eq!(lines[0], "Первая строка");
    assert_eq!(lines[1], "");
    assert_eq!(lines[2].chars().count(), 36);
    assert_eq!(lines[3].chars().count(), 36);
    assert_eq!(lines[4].chars().count(), 3);
    assert_eq!(lines[5], "Конец");
    assert!(lines.iter().all(|line| line.chars().count() <= 36));
    let mut item = notice(1, 1, false);
    item.title = "Ж".repeat(128);
    item.app_name = "Source".repeat(10);
    item.description = "Body".repeat(1_024);
    let all = item.text_lines(Locale::Ru);
    assert!(all.len() > 100);
    assert!(all
        .iter()
        .all(|line| line.chars().count() <= NOTICE_COLUMNS));
}

fn launch(phase: LaunchPhase, attempt: u64) -> LaunchStatus {
    LaunchStatus {
        desktop_id: "editor.desktop".into(),
        name: "Editor".into(),
        attempt,
        phase,
        reason: "A precise reason from the launcher".into(),
    }
}

#[test]
fn menu_and_recent_remember_identity_across_home_and_background_refresh() {
    let mut shell = new();
    shell.set_applications((0..12).map(|i| app(&format!("app{i}"))).collect());
    shell.enter(Screen::Menu);
    for _ in 0..9 {
        shell.on_key(Key::Right);
    }
    shell.go_home();
    shell.sync_windows(vec![window(1, 4), window(2, 3)]);
    shell.enter(Screen::Recent);
    shell.on_key(Key::Down);
    shell.go_home();
    let mut apps: Vec<_> = (0..12).map(|i| app(&format!("app{i}"))).collect();
    apps.rotate_left(2);
    shell.set_applications(apps);
    shell.sync_windows(vec![window(2, 10), window(1, 4)]);
    shell.enter(Screen::Menu);
    assert_eq!(
        shell.applications[shell.selected].desktop_id,
        "app9.desktop"
    );
    assert_eq!(shell.selected, 7);
    assert_eq!(shell.page, 0);
    shell.enter(Screen::Recent);
    assert_eq!(shell.window_at(shell.selected).unwrap().0.id, 2);
    shell.go_home();
    shell.set_applications(vec![app("only")]);
    shell.enter(Screen::Menu);
    assert_eq!((shell.selected, shell.page), (0, 0));
}

#[test]
fn simultaneous_closed_windows_preserve_mru_order_and_history_bound() {
    let mut shell = new();
    shell.sync_windows((1..=15).map(|id| window(id, id)).collect());
    shell.sync_windows(Vec::new());
    assert_eq!(
        shell
            .closed_windows
            .iter()
            .map(|window| window.id)
            .collect::<Vec<_>>(),
        (4..=15).rev().collect::<Vec<_>>()
    );
    shell.sync_windows(vec![window(16, 16), window(17, 17)]);
    shell.sync_windows(Vec::new());
    assert_eq!(
        shell
            .closed_windows
            .iter()
            .take(4)
            .map(|window| window.id)
            .collect::<Vec<_>>(),
        [17, 16, 15, 14]
    );
    assert_eq!(shell.closed_windows.len(), 12);
    shell.sync_windows(vec![window(18, 1)]);
    shell.sync_windows(Vec::new());
    assert_eq!(shell.closed_windows[0].id, 17);
    assert!(!shell.closed_windows.iter().any(|window| window.id == 18));
}

#[test]
fn pending_launch_requires_an_explicit_generation_bound_retry() {
    let mut shell = new();
    shell.set_applications(vec![app("editor")]);
    shell.enter(Screen::Menu);
    shell.sync_launches(vec![launch(LaunchPhase::Starting, 7)]);
    assert_eq!(shell.on_key(Key::Enter), Request::None);
    assert_eq!(shell.on_key(Key::Soft(0)), Request::None);
    shell.on_key(Key::Soft(1));
    assert_eq!(shell.actions()[1], Action::None); // Retry remains disabled while starting.
    shell.sync_launches(vec![launch(LaunchPhase::Unconfirmed, 7)]);
    for cancel in [Key::Escape, Key::Soft(4)] {
        shell.on_key(Key::Soft(1));
        assert!(shell.modal_active());
        assert_eq!(shell.actions()[0], Action::Retry);
        assert_eq!(shell.on_key(cancel), Request::None);
        assert!(!shell.modal_active());
    }
    shell.on_key(Key::Soft(1));
    assert_eq!(
        shell.on_key(Key::Soft(0)),
        Request::RetryLaunch {
            desktop_id: "editor.desktop".into(),
            attempt: 7
        }
    );
    shell.on_key(Key::Soft(1)); // Old confirmation still names attempt 7.
    shell.sync_launches(vec![launch(LaunchPhase::Starting, 8)]);
    assert!(!shell.modal_active());
    assert_eq!(shell.on_key(Key::Enter), Request::None);
    let before = shell.application_view.clone();
    shell.show_launch_error("editor.desktop", 7);
    assert_eq!(shell.application_view, before);
    shell.sync_launches(vec![launch(LaunchPhase::Failed, 8)]);
    shell.enter(Screen::Menu);
    shell.on_key(Key::Soft(1));
    assert_eq!(
        shell.on_key(Key::Soft(1)),
        Request::RetryLaunch {
            desktop_id: "editor.desktop".into(),
            attempt: 8
        }
    );
}

#[test]
fn nested_options_and_window_picker_keep_targets_and_return_positions() {
    let mut shell = new();
    shell.set_applications(vec![app("editor"), app("other")]);
    shell.sync_windows(vec![window(1, 9), window(2, 5)]);
    shell.enter(Screen::Menu);
    shell.on_key(Key::Soft(1));
    assert_eq!(shell.on_key(Key::Enter), Request::None);
    shell.on_key(Key::Soft(1)); // Windows
    shell.on_key(Key::Down);
    shell.sync_windows(vec![window(2, 20), window(1, 9)]);
    assert!(matches!(
        &shell.application_view,
        ApplicationView::Windows { choice, .. } if choice.selected() == Some(&2)
    ));
    shell.on_key(Key::Soft(0)); // Read the selected window's full card.
    assert!(matches!(
        shell.application_view,
        ApplicationView::Details {
            target: AppTarget::Window { id: 2, .. },
            ..
        }
    ));
    shell.on_key(Key::Soft(4));
    assert_eq!(shell.on_key(Key::Soft(1)), Request::ActivateWindow(2));
    shell.sync_windows(vec![window(1, 9)]);
    assert!(matches!(
        shell.application_view,
        ApplicationView::Options { .. }
    ));
    // The disappearing picker selection did not become window 1.
    assert_eq!(shell.on_key(Key::Enter), Request::None);
    shell.on_key(Key::Down); // Details
    shell.on_key(Key::Soft(1));
    shell.set_applications(vec![app("other"), app("editor")]);
    assert!(
        matches!(shell.application_view, ApplicationView::Details { target: AppTarget::Desktop(ref id), .. } if id == "editor.desktop")
    );
    shell.set_applications(vec![app("other")]);
    assert_eq!(shell.application_view, ApplicationView::List);
}

#[test]
fn window_operations_disable_repeats_and_close_confirmation_checks_identity() {
    let mut shell = new();
    shell.sync_windows(vec![window(1, 1)]);
    shell.enter(Screen::Recent);
    shell.sync_window_operations(vec![WindowOperationStatus {
        id: 1,
        kind: WindowOperation::Closing,
    }]);
    assert_eq!(shell.on_key(Key::Enter), Request::None);
    shell.on_key(Key::Soft(1));
    assert_eq!(shell.actions()[1], Action::None);
    shell.sync_window_operations(vec![WindowOperationStatus {
        id: 1,
        kind: WindowOperation::CloseUnconfirmed,
    }]);
    shell.on_key(Key::Soft(1));
    shell.on_key(Key::Left);
    let mut replacement = window(1, 1);
    replacement.app_id = "different-app".into();
    shell.sync_windows(vec![replacement]);
    assert!(matches!(
        shell.dialog,
        Some(Dialog::Info(InfoKind::TargetUnavailable))
    ));
    assert_eq!(shell.on_key(Key::Enter), Request::None);
}

#[test]
fn full_application_card_preserves_text_and_clamps_after_status_changes() {
    let mut shell = new();
    let mut editor = app("editor");
    editor.name = format!("{} FIN", "Полное имя aplicación ".repeat(30));
    shell.set_applications(vec![editor]);
    let mut failure = launch(LaunchPhase::Failed, 1);
    failure.reason = format!("{}\nREASON-END", "Причина / explicación ".repeat(100));
    shell.sync_launches(vec![failure]);
    shell.enter(Screen::Menu);
    shell.show_launch_error("editor.desktop", 1);
    let target = AppTarget::Desktop("editor.desktop".into());
    let lines = shell.application_lines(&target);
    assert_eq!(lines.last().unwrap(), "REASON-END");
    assert!(lines
        .iter()
        .all(|line| line.chars().count() <= NOTICE_COLUMNS));
    for _ in 0..100 {
        shell.on_key(Key::Right);
    }
    assert!(
        matches!(shell.application_view, ApplicationView::Details { scroll, .. } if scroll == lines.len() - NOTICE_ROWS)
    );
    shell.sync_launches(Vec::new());
    assert!(
        matches!(shell.application_view, ApplicationView::Details { scroll, .. } if scroll == shell.application_lines(&target).len().saturating_sub(NOTICE_ROWS))
    );
}

#[test]
fn notification_context_replacement_cannot_retarget_delete_or_a_named_action() {
    let mut shell = new();
    shell.sync_notices(vec![notice(1, 1, true)]);
    shell.enter(Screen::Notifications);
    shell.on_key(Key::Soft(1));
    shell.on_key(Key::Down); // Delete generation 1.
    shell.sync_notices(vec![notice(1, 2, true)]);
    assert_eq!(shell.notification_view, NotificationView::List);
    assert_eq!(shell.on_key(Key::Soft(2)), Request::None);
    assert_eq!(shell.on_key(Key::Soft(1)), Request::None); // Only opens a new context.
    assert_eq!(shell.on_key(Key::Enter), Request::None);
    assert_eq!(shell.notices[0].generation, 2);
    assert!(shell.notices[0].unread);
}

fn type_query(shell: &mut Shell, query: &str) {
    assert!(shell.is_text_editing());
    shell.on_key(Key::ClearText);
    for ch in query.chars() {
        assert_eq!(shell.on_key(Key::Text(ch)), Request::None);
    }
}

#[test]
fn search_uses_seventh_slot_and_filtered_identity_for_launch() {
    let mut shell = new();
    let mut editor = app("editor");
    editor.name = "Редактор Español".into();
    shell.set_applications(vec![app("other"), editor, app("last")]);
    shell.on_key(Key::Soft(3));
    assert!(!shell.is_text_editing());
    shell.enter(Screen::Menu);
    assert_eq!(shell.actions()[2], Action::None);
    assert_eq!(shell.actions()[3], Action::Search);
    shell.on_key(Key::Soft(3));
    type_query(&mut shell, "РЕДАКТОР");
    assert_eq!(shell.count(), 3); // A draft does not filter the catalog.
    assert_eq!(shell.search_match_count(), 1);
    assert_eq!(shell.actions()[0], Action::ApplyText);
    assert_eq!(shell.on_key(Key::Enter), Request::None);
    assert_eq!(shell.count(), 1);
    assert_eq!(shell.menu_app(0).unwrap().desktop_id, "editor.desktop");
    assert_eq!(
        shell.on_key(Key::Enter),
        Request::LaunchDesktop("editor.desktop".into())
    );
    shell.on_key(Key::Soft(3));
    type_query(&mut shell, "ESPAÑOL");
    shell.on_key(Key::Enter);
    assert_eq!(shell.count(), 1);
    shell.on_key(Key::Soft(3));
    type_query(&mut shell, "LAST.DESKTOP");
    shell.on_key(Key::Enter);
    assert_eq!(
        shell.on_key(Key::Enter),
        Request::LaunchDesktop("last.desktop".into())
    );
}

#[test]
fn search_cancel_empty_results_and_clear_preserve_the_committed_view() {
    let mut shell = new();
    shell.set_applications((0..12).map(|i| app(&format!("app{i}"))).collect());
    shell.enter(Screen::Menu);
    for _ in 0..9 {
        shell.on_key(Key::Right);
    }
    shell.on_key(Key::Soft(3));
    type_query(&mut shell, "app1");
    shell.on_key(Key::Escape);
    assert_eq!(shell.search_query, "");
    assert_eq!((shell.selected, shell.page), (9, 1));
    shell.on_key(Key::Soft(3));
    type_query(&mut shell, "APP1");
    shell.on_key(Key::Enter);
    assert_eq!(shell.count(), 3);
    shell.on_key(Key::Right);
    assert_eq!(
        shell.menu_app(shell.selected).unwrap().desktop_id,
        "app10.desktop"
    );
    shell.on_key(Key::Soft(3));
    type_query(&mut shell, "no matches");
    shell.on_key(Key::Escape);
    assert_eq!(shell.search_query, "APP1");
    assert_eq!(shell.selected, 1);
    shell.on_key(Key::Soft(3));
    type_query(&mut shell, "no matches");
    shell.on_key(Key::Enter);
    assert_eq!(shell.count(), 0);
    assert_eq!(shell.on_key(Key::Enter), Request::None);
    assert_eq!(shell.actions()[1], Action::None);
    assert_eq!(shell.actions()[3], Action::Search);
    shell.on_key(Key::Soft(3));
    type_query(&mut shell, "");
    shell.on_key(Key::Enter);
    assert_eq!(shell.count(), 12);
}

#[test]
fn search_draft_edits_unicode_digits_and_survives_background_refresh() {
    let mut shell = new();
    let mut target = app("target");
    target.name = "Игра 45678 ñ".into();
    shell.set_applications(vec![app("other"), target.clone()]);
    shell.enter(Screen::Menu);
    shell.on_key(Key::Soft(3));
    type_query(&mut shell, "Игра 45678 X");
    shell.on_key(Key::Left);
    shell.on_key(Key::Delete);
    shell.on_key(Key::Text('ñ'));
    let draft = shell.search_input.clone();
    shell.set_applications(vec![target, app("other")]);
    shell.apply_preferences(Locale::Es, Theme::emerald());
    assert_eq!(shell.search_input, draft);
    shell.on_key(Key::Soft(0)); // Stale semantic buttons cannot accept text.
    shell.on_key(Key::Soft(4));
    assert!(shell.is_text_editing());
    shell.on_key(Key::Enter);
    assert_eq!(shell.search_query, "Игра 45678 ñ");
    assert_eq!(
        shell.on_key(Key::Enter),
        Request::LaunchDesktop("target.desktop".into())
    );
    shell.go_home();
    shell.enter(Screen::Menu);
    assert_eq!(shell.count(), 1);
    assert_eq!(
        shell.menu_app(shell.selected).unwrap().desktop_id,
        "target.desktop"
    );
}

#[test]
fn long_confirmation_scroll_does_not_change_the_lower_action_or_target() {
    let mut shell = new();
    let mut target = window(7, 1);
    target.title = format!("{}END", "Длинное название ".repeat(40));
    shell.sync_windows(vec![target]);
    shell.enter(Screen::Recent);
    shell.on_key(Key::Soft(1));
    shell.on_key(Key::Soft(1));
    assert!(shell.dialog_lines().len() > MODAL_ROWS);
    for _ in 0..20 {
        shell.on_key(Key::Right);
    }
    assert!(shell.dialog_scroll > 0);
    assert_eq!(shell.actions()[0], Action::Confirm);
    assert_eq!(shell.actions()[4], Action::Decline);
    assert_eq!(shell.on_key(Key::Enter), Request::CloseWindow(7));
}

fn choose_lock(shell: &mut Shell) -> u64 {
    shell.on_key(Key::Soft(1));
    for _ in 0..QuickAction::LOCK_INDEX {
        shell.on_key(Key::Down);
    }
    // This operation does not depend on login1 capability replies.
    assert_eq!(shell.on_key(Key::Soft(1)), Request::None);
    let Request::LockSession { request } = shell.on_key(Key::Soft(0)) else {
        panic!("Expected the independent lock request")
    };
    request
}

#[test]
fn automatic_lock_preserves_navigation_and_uses_the_manual_request_sequence() {
    let mut shell = new();
    shell.enter(Screen::Menu);
    shell.on_key(Key::Soft(3));
    shell.on_key(Key::Text('x'));
    let editor = shell.search_input.clone();
    let Request::LockSession { request: first } = shell.begin_lock(false) else {
        panic!("Expected the automatic lock request");
    };
    assert_eq!(shell.screen, Screen::Menu);
    assert_eq!(shell.search_input, editor);
    assert!(shell.system_view.is_none());
    assert_eq!(shell.begin_lock(false), Request::None);
    assert_eq!(shell.begin_lock(true), Request::None);
    assert!(shell.lock_changed(locking::State {
        id: first,
        phase: locking::Phase::Locked,
    }));
    assert!(shell.lock_unlocked(first));
    assert_eq!(shell.screen, Screen::Menu);
    assert_eq!(shell.search_input, editor);
    let Request::LockSession { request: second } = shell.begin_lock(true) else {
        panic!("Expected the subsequent manual lock request");
    };
    assert_eq!(second, first + 1);
    assert!(matches!(
        shell.system_view,
        Some(SystemView::LockStatus { .. })
    ));
}

#[test]
fn automatic_lock_never_overlaps_a_pending_system_operation() {
    let mut shell = new();
    shell.system_request = Some(SystemRequest {
        id: 42,
        operation: Operation::Reboot,
        phase: SystemPhase::Pending,
    });
    assert_eq!(shell.begin_lock(false), Request::None);
    assert!(shell.lock_state.is_none());
    assert_eq!(shell.system_request.unwrap().id, 42);
}

#[test]
fn lock_is_independent_of_power_policy_and_waits_for_its_exact_lifecycle() {
    let mut shell = new();
    let id = choose_lock(&mut shell);
    assert!(shell.system_request.is_none());
    assert!(shell.system_busy());
    assert!(!shell.lock_unlocked(id)); // process creation is not a lock.
    assert!(!shell.lock_changed(locking::State {
        id: id + 1,
        phase: locking::Phase::Locked
    }));
    assert!(shell.lock_changed(locking::State {
        id,
        phase: locking::Phase::Locked
    }));
    shell.on_key(Key::Escape); // A hidden status card never cancels a lock.
    assert!(shell.lock_busy());
    shell.on_key(Key::Soft(1));
    assert_eq!(shell.on_key(Key::Soft(1)), Request::None);
    assert!(!shell.lock_unlocked(id + 1));
    assert!(shell.lock_unlocked(id));
    assert!(!shell.system_busy());
    assert!(shell.system_view.is_none());
    assert!(!shell.lock_unlocked(id));
}

#[test]
fn failed_lock_requires_acknowledgement_but_unknown_lock_never_repeats() {
    let mut shell = new();
    let id = choose_lock(&mut shell);
    shell.request_failed(Request::LockSession { request: id });
    assert!(!shell.lock_busy());
    assert_eq!(shell.on_key(Key::Enter), Request::RefreshSystem);
    let Request::LockSession { request: next } = shell.on_key(Key::Soft(0)) else {
        panic!("Expected explicit retry on the remembered Lock item")
    };
    assert_ne!(id, next);
    assert!(shell.lock_changed(locking::State {
        id: next,
        phase: locking::Phase::Uncertain
    }));
    shell.go_home();
    assert_eq!(shell.on_key(Key::Soft(1)), Request::None);
    assert_eq!(shell.on_key(Key::Enter), Request::None);
    assert_eq!(shell.on_key(Key::Soft(1)), Request::None);
    assert!(shell.lock_busy());
    assert!(!shell.lock_changed(locking::State {
        id,
        phase: locking::Phase::Locked
    }));
}

#[test]
fn late_unlock_keeps_the_users_new_screen_and_selection() {
    let mut shell = new();
    let id = choose_lock(&mut shell);
    shell.lock_changed(locking::State {
        id,
        phase: locking::Phase::Locked,
    });
    shell.go_home();
    shell.enter(Screen::Menu);
    assert!(shell.lock_unlocked(id));
    assert_eq!(shell.screen, Screen::Menu);
    assert!(shell.system_view.is_none());
}
