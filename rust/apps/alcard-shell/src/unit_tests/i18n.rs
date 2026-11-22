use super::*;
use crate::model::Action;

#[test]
fn all_softkey_labels_fit_inside_64_pixels_with_padding() {
    let actions = [
        Action::Recent,
        Action::Quick,
        Action::Menu,
        Action::Notifications,
        Action::Open,
        Action::Launch,
        Action::Back,
        Action::Confirm,
        Action::Cancel,
        Action::Decline,
        Action::Read,
        Action::Actions,
        Action::Select,
        Action::Acknowledge,
        Action::Search,
        Action::Clear,
        Action::Retry,
        Action::ApplyText,
        Action::CancelText,
        Action::Mute,
        Action::Unmute,
        Action::TurnOn,
        Action::TurnOff,
    ];
    for locale in [Locale::Ru, Locale::En, Locale::Es] {
        for action in actions {
            let label = locale.action(action);
            assert!(
                label.chars().count() <= 6,
                "{}: {:?} ({}) is too wide for a softkey",
                locale.code(),
                action,
                label
            );
        }
    }
}

#[test]
fn search_copy_fits_its_native_field_and_names_the_actual_keyboard_keys() {
    for locale in [Locale::Ru, Locale::En, Locale::Es] {
        assert!(locale.search_title().chars().count() <= 35);
        assert!(locale.search_hint().chars().count() <= 34);
        assert!(locale.search_clear_hint().chars().count() <= 34);
        assert!(locale.search_clear_hint().starts_with("Ctrl+U:"));
        assert!(locale.search_apply_key().starts_with("Enter:"));
        assert!(locale.search_cancel_key().starts_with("Esc:"));
        assert!(locale.search_apply_key().chars().count() <= 18);
        assert!(locale.search_cancel_key().chars().count() <= 18);
    }
}

#[test]
fn controls_copy_fits_inline_quick_rows_and_footer() {
    for locale in [Locale::Ru, Locale::En, Locale::Es] {
        for control in [Control::Brightness, Control::Volume] {
            assert!(locale.control_title(control).chars().count() <= 24);
            for availability in [
                Availability::Checking,
                Availability::Available,
                Availability::Unavailable,
                Availability::Denied,
            ] {
                for muted in [None, Some(false), Some(true)] {
                    for above_safe_limit in [false, true] {
                        let reading = Reading {
                            availability,
                            percent: Some(50),
                            muted,
                            above_safe_limit,
                        };
                        assert!(locale.control_hint(control, reading).chars().count() <= 35);
                    }
                }
            }
        }
        for failure in [
            ControlFailure::Unavailable,
            ControlFailure::Denied,
            ControlFailure::Timeout,
            ControlFailure::Unknown,
            ControlFailure::Changed,
            ControlFailure::Invalid,
        ] {
            assert!(locale.controls_failure(failure).chars().count() <= 36);
        }
        for text in [
            locale.controls_adjust_hint(),
            locale.controls_pending(true),
            locale.controls_pending(false),
        ] {
            assert!(text.chars().count() <= 36);
        }
    }
}

#[test]
fn radio_copy_and_details_fit_native_rows_and_preserve_the_target() {
    for locale in [Locale::Ru, Locale::En, Locale::Es] {
        for kind in [RadioKind::Wifi, RadioKind::Bluetooth] {
            assert!(locale.radio_title(kind).chars().count() <= 24);
            assert!(locale.radio_list_title(kind).chars().count() <= 35);
            assert!(locale.radio_details_title(kind).chars().count() <= 31);
            assert!(locale.radio_empty(kind).chars().count() <= 35);
            for operation in [
                radios::Operation::Refresh,
                radios::Operation::List { kind },
                radios::Operation::SetPowered {
                    kind,
                    powered: true,
                },
                radios::Operation::SetPowered {
                    kind,
                    powered: false,
                },
            ] {
                assert!(locale.radio_pending(operation).chars().count() <= 35);
            }
        }
        for availability in [
            RadioAvailability::Checking,
            RadioAvailability::Available,
            RadioAvailability::Absent,
            RadioAvailability::Unavailable,
            RadioAvailability::Blocked,
        ] {
            for powered in [None, Some(false), Some(true)] {
                let state = radios::State {
                    availability,
                    powered,
                    detail: String::new(),
                };
                assert!(locale.radio_state(&state).chars().count() <= 35);
                assert!(locale.radio_value(&state).chars().count() <= 6);
                if availability != RadioAvailability::Available || powered.is_none() {
                    assert_eq!(locale.radio_value(&state), "--");
                }
            }
        }
        for failure in [
            RadioFailure::Unavailable,
            RadioFailure::Permission,
            RadioFailure::Timeout,
            RadioFailure::Failed,
            RadioFailure::InvalidResponse,
        ] {
            assert!(locale.radio_failure(failure).chars().count() <= 35);
        }
        assert!(locale.radio_list_hint().chars().count() <= 36);
        let entry = radios::Entry {
            id: "02:00:00:00:00:01".into(),
            name: format!("{} UNIQUE-NAME-END", "Network имя red ".repeat(20)),
            detail: "Facts\n\u{1b}signal: -45 dBm\nUNIQUE-DETAIL-END".into(),
        };
        let text = locale.radio_details_text(&entry);
        assert!(text.contains("UNIQUE-NAME-END"));
        assert!(text.contains(&entry.id));
        assert!(!text.contains('\u{1b}'));
        let lines = locale.radio_details_lines(&entry);
        assert!(lines.len() > crate::modal::MODAL_ROWS);
        assert!(lines
            .iter()
            .all(|line| line.chars().count() <= crate::modal::MODAL_COLUMNS));
        assert!(lines.last().unwrap().ends_with("UNIQUE-DETAIL-END"));
    }
}

#[test]
fn system_action_labels_and_capabilities_fit_the_native_rows() {
    let capabilities = [
        Capability::Checking,
        Capability::Allowed,
        Capability::Authentication,
        Capability::Denied,
        Capability::Unsupported,
        Capability::Blocked,
        Capability::Unavailable,
    ];
    for locale in [Locale::Ru, Locale::En, Locale::Es] {
        assert!(locale.quick_title().chars().count() <= 36);
        for operation in Operation::ALL {
            assert!(locale.system_operation(operation).chars().count() <= 35);
            assert!(locale.system_confirm(operation).chars().count() <= 31);
        }
        for capability in capabilities {
            assert!(locale.system_capability(capability).chars().count() <= 35);
        }
    }
}

#[test]
fn suspend_title_and_every_monitor_capability_hint_fit_the_quick_menu() {
    for locale in [Locale::Ru, Locale::En, Locale::Es] {
        assert!(locale.suspend_title().chars().count() <= 35);
        assert!(locale.suspend_confirm().chars().count() <= 31);
        for monitor in [
            MonitorState::Connecting,
            MonitorState::Ready,
            MonitorState::Preparing,
            MonitorState::Unavailable,
        ] {
            for capability in [
                Capability::Checking,
                Capability::Allowed,
                Capability::Authentication,
                Capability::Denied,
                Capability::Unsupported,
                Capability::Blocked,
                Capability::Unavailable,
            ] {
                let hint = locale.suspend_hint(SleepSnapshot {
                    epoch: 1,
                    capability,
                    monitor,
                    delay_ms: 5_000,
                });
                assert!(!hint.is_empty() && hint.chars().count() <= 35);
            }
        }
    }
}
