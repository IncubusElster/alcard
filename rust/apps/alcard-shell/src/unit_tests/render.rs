use super::{
    indicators::{battery_icon, network_icon},
    *,
};
use crate::{controls, i18n::Locale, model::ControlsRequest};
fn image() -> Pixmap {
    Pixmap::new(WIDTH as u32, HEIGHT as u32).unwrap()
}
fn has_rgb(image: &Pixmap, rgb: u32) -> bool {
    image.pixels().iter().any(|p| {
        p.red() == (rgb >> 16) as u8
            && p.green() == (rgb >> 8) as u8
            && p.blue() == rgb as u8
            && p.alpha() == 255
    })
}

const CONTROLS_FIXTURES: [&str; 9] = [
    "quick",
    "quick-logout",
    "brightness",
    "volume-muted",
    "unavailable",
    "checking",
    "pending",
    "failure",
    "boost-muted",
];

fn controls_fixture(locale: Locale, theme: Theme, case: &str) -> Shell {
    let mut shell = Shell::new(locale, theme);
    shell.system_snapshot.capabilities = [Capability::Allowed; 3];
    shell.system_view = Some(SystemView::Menu {
        selected: QuickAction::BRIGHTNESS_INDEX,
    });
    shell.controls_snapshot = controls::Snapshot {
        brightness: controls::Reading {
            availability: Availability::Available,
            percent: Some(45),
            muted: None,
            above_safe_limit: false,
        },
        volume: controls::Reading {
            availability: Availability::Available,
            percent: Some(60),
            muted: Some(false),
            above_safe_limit: false,
        },
    };
    match case {
        "quick" => {
            shell.system_view = Some(SystemView::Menu {
                selected: QuickAction::LOCK_INDEX,
            })
        }
        "quick-logout" => {
            shell.system_view = Some(SystemView::Menu {
                selected: QuickAction::system_index(Operation::Logout),
            });
        }
        "brightness" => {}
        "volume-muted" => {
            shell.system_view = Some(SystemView::Menu {
                selected: QuickAction::VOLUME_INDEX,
            });
            shell.controls_snapshot.volume.muted = Some(true);
        }
        "unavailable" => {
            shell.controls_snapshot.brightness = controls::Reading {
                availability: Availability::Unavailable,
                ..controls::Reading::default()
            };
            shell.controls_snapshot.volume = controls::Reading {
                availability: Availability::Denied,
                ..controls::Reading::default()
            };
        }
        "checking" => {
            shell.controls_snapshot = controls::Snapshot::default();
            shell.controls_request = Some(ControlsRequest {
                id: 1,
                operation: None,
            });
        }
        "pending" => {
            shell.controls_request = Some(ControlsRequest {
                id: 2,
                operation: Some(controls::Operation::Adjust {
                    control: Control::Brightness,
                    direction: controls::Direction::Increase,
                }),
            });
        }
        "failure" => shell.controls_failure = Some(controls::Failure::Timeout),
        "boost-muted" => {
            shell.system_view = Some(SystemView::Menu {
                selected: QuickAction::VOLUME_INDEX,
            });
            shell.controls_snapshot.volume.percent = Some(100);
            shell.controls_snapshot.volume.muted = Some(true);
            shell.controls_snapshot.volume.above_safe_limit = true;
        }
        _ => panic!("unknown controls render fixture"),
    }
    shell
}

#[test]
fn inline_controls_and_scrolled_quick_menu_preserve_status_and_footer() {
    for locale in [Locale::En, Locale::Ru, Locale::Es] {
        for theme in [Theme::classic(), Theme::emerald()] {
            let mut home = image();
            paint(&mut home, &Shell::new(locale, theme.clone()), "12:34", None);
            let mut previous = Vec::<Vec<u8>>::new();
            for case in CONTROLS_FIXTURES {
                let shell = controls_fixture(locale, theme.clone(), case);
                let mut rendered = image();
                paint(
                    &mut rendered,
                    &shell,
                    "12:34",
                    Some("Must not cover device controls"),
                );
                assert_eq!(
                    &rendered.data()[..18 * WIDTH * 4],
                    &home.data()[..18 * WIDTH * 4]
                );
                let mut footer = image();
                softkeys(&mut footer, &shell);
                let offset = SOFTKEY_TOP as usize * WIDTH * 4;
                assert_eq!(&rendered.data()[offset..], &footer.data()[offset..]);
                assert!(rendered.pixels().iter().all(|pixel| pixel.alpha() == 255));
                assert!(
                    previous.iter().all(|other| other != rendered.data()),
                    "indistinguishable state: {case}"
                );
                previous.push(rendered.take());
            }
        }
    }
}

const RADIO_FIXTURES: [&str; 12] = [
    "radio-wifi",
    "radio-bluetooth",
    "radio-unavailable",
    "radio-blocked",
    "radio-power-pending",
    "radio-wifi-list",
    "radio-empty",
    "radio-list-pending",
    "radio-list-failed",
    "radio-bluetooth-list",
    "radio-details",
    "radio-details-scrolled",
];

fn radio_fixture(locale: Locale, theme: Theme, case: &str) -> Shell {
    use crate::model::RadioRequest;
    let mut shell = controls_fixture(locale, theme, "brightness");
    shell.radios_snapshot = radios::Snapshot {
        wifi: radios::State {
            availability: radios::Availability::Available,
            powered: Some(true),
            detail: "Home network".into(),
        },
        bluetooth: radios::State {
            availability: radios::Availability::Available,
            powered: Some(false),
            detail: "Adapter 0".into(),
        },
    };
    shell.system_view = Some(SystemView::Menu {
        selected: QuickAction::WIFI_INDEX,
    });
    match case {
        "radio-wifi" => {}
        "radio-bluetooth" => {
            shell.system_view = Some(SystemView::Menu {
                selected: QuickAction::BLUETOOTH_INDEX,
            })
        }
        "radio-unavailable" => {
            shell.radios_snapshot.wifi.availability = radios::Availability::Unavailable;
            shell.radios_snapshot.wifi.powered = None;
            shell.radios_snapshot.bluetooth.availability = radios::Availability::Absent;
            shell.radios_snapshot.bluetooth.powered = None;
        }
        "radio-blocked" => {
            shell.radios_snapshot.wifi.availability = radios::Availability::Blocked;
            shell.radios_snapshot.wifi.powered = Some(false);
        }
        "radio-power-pending" => {
            shell.radios_request = Some(RadioRequest {
                id: 11,
                operation: radios::Operation::SetPowered {
                    kind: radios::Kind::Wifi,
                    powered: false,
                },
            })
        }
        "radio-wifi-list" => {
            shell.system_view = Some(SystemView::RadioList {
                kind: radios::Kind::Wifi,
                selected: 3,
            });
            shell.radio_entries = (0..5)
                .map(|index| radios::Entry {
                    id: format!("02:00:00:00:00:{index:02}"),
                    name: format!("Example network {index}"),
                    detail: "-45 dBm; 2412 MHz".into(),
                })
                .collect();
        }
        "radio-empty" | "radio-list-pending" | "radio-list-failed" => {
            shell.system_view = Some(SystemView::RadioList {
                kind: radios::Kind::Wifi,
                selected: 0,
            });
            if case == "radio-list-pending" {
                shell.radios_request = Some(RadioRequest {
                    id: 12,
                    operation: radios::Operation::List {
                        kind: radios::Kind::Wifi,
                    },
                });
            } else if case == "radio-list-failed" {
                shell.radios_failure = Some(radios::Failure::Permission);
            }
        }
        "radio-bluetooth-list" => {
            shell.radios_snapshot.bluetooth.powered = Some(true);
            shell.system_view = Some(SystemView::RadioList {
                kind: radios::Kind::Bluetooth,
                selected: 0,
            });
            shell.radio_entries = vec![radios::Entry {
                id: "02:00:00:00:00:01".into(),
                name: "Example keyboard".into(),
                detail: "Paired: yes; connected: yes".into(),
            }];
        }
        "radio-details" | "radio-details-scrolled" => {
            shell.system_view = Some(SystemView::RadioDetails {
                kind: radios::Kind::Wifi,
                entry: radios::Entry {
                    id: "02:00:00:00:00:01".into(),
                    name: "Long network name with preserved distinguishing suffix".into(),
                    detail: format!(
                        "{}\n-45 dBm; 2412 MHz\nEXACT-DEVICE-END",
                        "Long details ".repeat(25)
                    ),
                },
                scroll: if case == "radio-details" {
                    0
                } else {
                    usize::MAX
                },
            });
        }
        _ => panic!("unknown radio render fixture"),
    }
    shell
}

#[test]
fn radio_lists_details_and_failures_preserve_status_footer_and_distinct_states() {
    for locale in [Locale::En, Locale::Ru, Locale::Es] {
        for theme in [Theme::classic(), Theme::emerald()] {
            let mut home = image();
            paint(&mut home, &Shell::new(locale, theme.clone()), "12:34", None);
            let mut previous = Vec::<Vec<u8>>::new();
            for case in RADIO_FIXTURES {
                let shell = radio_fixture(locale, theme.clone(), case);
                let mut rendered = image();
                paint(
                    &mut rendered,
                    &shell,
                    "12:34",
                    Some("Must not cover radio views"),
                );
                assert_eq!(
                    &rendered.data()[..18 * WIDTH * 4],
                    &home.data()[..18 * WIDTH * 4]
                );
                let mut footer = image();
                softkeys(&mut footer, &shell);
                let offset = SOFTKEY_TOP as usize * WIDTH * 4;
                assert_eq!(&rendered.data()[offset..], &footer.data()[offset..]);
                assert!(rendered.pixels().iter().all(|pixel| pixel.alpha() == 255));
                assert!(
                    previous.iter().all(|other| other != rendered.data()),
                    "indistinguishable radio state: {case}"
                );
                previous.push(rendered.take());
            }
        }
    }
}

#[test]
fn late_request_errors_do_not_replace_another_quick_rows_status() {
    let mut shell = controls_fixture(Locale::En, Theme::classic(), "brightness");
    let operation = controls::Operation::Adjust {
        control: Control::Brightness,
        direction: controls::Direction::Increase,
    };
    shell.controls_request = Some(ControlsRequest {
        id: 14,
        operation: Some(operation),
    });
    shell.system_view = Some(SystemView::Menu {
        selected: QuickAction::VOLUME_INDEX,
    });
    let mut before = image();
    paint(&mut before, &shell, "12:34", None);
    assert!(shell.controls_message(controls::Message::Finished {
        request: 14,
        operation,
        snapshot: shell.controls_snapshot,
        result: Err(controls::Failure::Timeout),
    }));
    let mut after = image();
    paint(&mut after, &shell, "12:34", None);
    let volume = 63 * WIDTH * 4..88 * WIDTH * 4;
    assert_eq!(&before.data()[volume.clone()], &after.data()[volume]);
    let brightness = 36 * WIDTH * 4..61 * WIDTH * 4;
    assert_ne!(
        &before.data()[brightness.clone()],
        &after.data()[brightness]
    );

    let mut shell = radio_fixture(Locale::En, Theme::classic(), "radio-bluetooth");
    let operation = radios::Operation::SetPowered {
        kind: radios::Kind::Wifi,
        powered: false,
    };
    shell.radios_request = Some(crate::model::RadioRequest { id: 15, operation });
    let mut before = image();
    paint(&mut before, &shell, "12:34", None);
    assert!(shell.radios_message(radios::Message {
        request: 15,
        operation,
        snapshot: shell.radios_snapshot.clone(),
        entries: Vec::new(),
        result: Err(radios::Failure::Permission),
    }));
    let mut after = image();
    paint(&mut after, &shell, "12:34", None);
    let bluetooth = 117 * WIDTH * 4..142 * WIDTH * 4;
    assert_eq!(&before.data()[bluetooth.clone()], &after.data()[bluetooth]);
    let wifi = 90 * WIDTH * 4..115 * WIDTH * 4;
    assert_ne!(&before.data()[wifi.clone()], &after.data()[wifi]);
}

#[test]
#[ignore = "explicit local PNG export; no compositor or hardware acceptance"]
fn export_controls_render_fixtures() {
    let directory = std::env::var_os("ALCARD_CONTROLS_RENDER_DIR")
        .map(std::path::PathBuf::from)
        .expect("set ALCARD_CONTROLS_RENDER_DIR for the opt-in render export");
    std::fs::create_dir_all(&directory).unwrap();
    let mut count = 0;
    for locale in [Locale::En, Locale::Ru, Locale::Es] {
        for theme in [Theme::classic(), Theme::emerald()] {
            let fixtures = CONTROLS_FIXTURES
                .into_iter()
                .map(|case| (case, controls_fixture(locale, theme.clone(), case)))
                .chain(
                    RADIO_FIXTURES
                        .into_iter()
                        .map(|case| (case, radio_fixture(locale, theme.clone(), case))),
                );
            for (case, shell) in fixtures {
                let filename = format!("{}-{}-{case}.png", theme.id, locale.code());
                let mut rendered = image();
                paint(&mut rendered, &shell, "12:34", None);
                let file = std::fs::File::create(directory.join(filename)).unwrap();
                let mut encoder = png::Encoder::new(file, WIDTH as u32, HEIGHT as u32);
                encoder.set_color(png::ColorType::Rgba);
                encoder.set_depth(png::BitDepth::Eight);
                encoder
                    .write_header()
                    .unwrap()
                    .write_image_data(rendered.data())
                    .unwrap();
                count += 1;
            }
        }
    }
    println!("Exported {count} controls render fixtures at 320x170 to {}. Static rendering only; no device operations or compositor run.", directory.display());
}

const SUSPEND_FIXTURES: [&str; 15] = [
    "quick",
    "confirm",
    "challenge",
    "unavailable",
    "checking",
    "preparing",
    "locking",
    "queued",
    "requested",
    "ended",
    "cancelled",
    "failed",
    "unknown",
    "recovered-unknown",
    "external",
];

fn suspend_fixture(locale: Locale, theme: Theme, case: &str) -> Shell {
    let mut shell = Shell::new(locale, theme);
    shell.system_snapshot.capabilities = [Capability::Allowed; 3];
    shell.sleep_snapshot = sleep::Snapshot {
        epoch: 1,
        capability: Capability::Allowed,
        monitor: sleep::MonitorState::Ready,
        delay_ms: 5_000,
    };
    shell.system_view = Some(SystemView::SuspendStatus { scroll: 0 });
    match case {
        "quick" => {
            shell.system_view = Some(SystemView::Menu {
                selected: QuickAction::SUSPEND_INDEX,
            });
        }
        "confirm" => shell.system_view = Some(SystemView::SuspendConfirm { scroll: 0 }),
        "challenge" | "unavailable" | "checking" => {
            shell.system_view = Some(SystemView::SuspendUnavailable { scroll: 0 });
            match case {
                "challenge" => shell.sleep_snapshot.capability = Capability::Authentication,
                "unavailable" => {
                    shell.sleep_snapshot.capability = Capability::Unavailable;
                    shell.sleep_snapshot.monitor = sleep::MonitorState::Unavailable;
                }
                _ => shell.sleep_snapshot = sleep::Snapshot::default(),
            }
        }
        "external" => shell.sleep_snapshot.monitor = sleep::MonitorState::Preparing,
        _ => {
            let (stage, failure) = match case {
                "preparing" => (sleep::Stage::Preparing, None),
                "locking" => (sleep::Stage::Locking, None),
                "queued" => (sleep::Stage::Queued, None),
                "requested" => (sleep::Stage::Requested, None),
                "ended" => (sleep::Stage::Ended, None),
                "cancelled" => (sleep::Stage::Cancelled, None),
                "failed" => (sleep::Stage::Failed, Some(crate::system::Failure::Timeout)),
                "unknown" => (sleep::Stage::Unknown, Some(crate::system::Failure::Unknown)),
                "recovered-unknown" => {
                    shell.sleep_snapshot.epoch = 2;
                    (sleep::Stage::Failed, Some(crate::system::Failure::Unknown))
                }
                _ => panic!("unknown suspend render fixture"),
            };
            shell.sleep_progress = Some(sleep::Progress {
                request: 1,
                stage,
                failure,
            });
        }
    }
    shell
}

#[test]
fn suspend_views_keep_all_copy_status_and_actions_inside_the_native_layout() {
    for locale in [Locale::En, Locale::Ru, Locale::Es] {
        for theme in [Theme::classic(), Theme::emerald()] {
            let mut home = image();
            paint(&mut home, &Shell::new(locale, theme.clone()), "12:34", None);
            let mut previous = Vec::<Vec<u8>>::new();
            for case in SUSPEND_FIXTURES {
                let shell = suspend_fixture(locale, theme.clone(), case);
                let mut rendered = image();
                paint(
                    &mut rendered,
                    &shell,
                    "12:34",
                    Some("Must not cover sleep status"),
                );
                assert_eq!(
                    &rendered.data()[..18 * WIDTH * 4],
                    &home.data()[..18 * WIDTH * 4]
                );
                assert!(shell.system_lines().len() <= MODAL_ROWS);
                let mut footer = image();
                softkeys(&mut footer, &shell);
                let offset = SOFTKEY_TOP as usize * WIDTH * 4;
                assert_eq!(&rendered.data()[offset..], &footer.data()[offset..]);
                assert!(rendered.pixels().iter().all(|pixel| pixel.alpha() == 255));
                assert!(
                    previous.iter().all(|other| other != rendered.data()),
                    "indistinguishable suspend state: {case}"
                );
                previous.push(rendered.take());
            }
        }
    }
}

#[test]
#[ignore = "explicit local PNG export; no compositor, lock or sleep operation"]
fn export_suspend_render_fixtures() {
    let directory = std::env::var_os("ALCARD_SUSPEND_RENDER_DIR")
        .map(std::path::PathBuf::from)
        .expect("set ALCARD_SUSPEND_RENDER_DIR for the opt-in render export");
    std::fs::create_dir_all(&directory).unwrap();
    let mut count = 0;
    for locale in [Locale::En, Locale::Ru, Locale::Es] {
        for theme in [Theme::classic(), Theme::emerald()] {
            for case in SUSPEND_FIXTURES {
                let filename = format!("{}-{}-{case}.png", theme.id, locale.code());
                let shell = suspend_fixture(locale, theme.clone(), case);
                let mut rendered = image();
                paint(&mut rendered, &shell, "12:34", None);
                let file = std::fs::File::create(directory.join(filename)).unwrap();
                let mut encoder = png::Encoder::new(file, WIDTH as u32, HEIGHT as u32);
                encoder.set_color(png::ColorType::Rgba);
                encoder.set_depth(png::BitDepth::Eight);
                encoder
                    .write_header()
                    .unwrap()
                    .write_image_data(rendered.data())
                    .unwrap();
                count += 1;
            }
        }
    }
    println!("Exported {count} suspend render fixtures at 320x170 to {}. Static rendering only; no compositor, lock or sleep operation.", directory.display());
}

#[test]
fn compact_softkeys_are_bottom_aligned() {
    let shell = Shell::new(Locale::En, Theme::classic());
    let mut image = image();
    paint(&mut image, &shell, "12:34", None);
    assert_eq!(SOFTKEY_TOP + SOFTKEY_HEIGHT, image.height() as i32);
    assert_eq!(SOFTKEY_MAX_GLYPHS * 8, 48);
    assert_eq!(
        image.pixel(0, SOFTKEY_TOP as u32).unwrap().red(),
        (shell.theme.border >> 16) as u8
    );
}
#[test]
fn short_confirmations_keep_actions_in_the_bottom_row() {
    for locale in [Locale::En, Locale::Ru, Locale::Es] {
        for theme in [Theme::classic(), Theme::emerald()] {
            let mut shell = Shell::new(locale, theme);
            shell.live_windows = vec![crate::model::WindowInfo {
                id: 7,
                title: "Exact target".into(),
                app_id: "org.example.App".into(),
                active: false,
                activity: 1,
            }];
            for dialog in [Dialog::ConfirmClear, Dialog::ConfirmClose(7)] {
                shell.dialog = Some(dialog);
                let mut rendered = image();
                paint(&mut rendered, &shell, "12:34", None);
                let mut footer = image();
                softkeys(&mut footer, &shell);
                let offset = SOFTKEY_TOP as usize * WIDTH * 4;
                assert_eq!(&rendered.data()[offset..], &footer.data()[offset..]);
                // A short target occupies one body line. The otherwise
                // empty lower panel must not retain the old inline
                // Yes/Cancel buttons or their text.
                assert!((103..126).all(|y| (20..300).all(|x| {
                    let pixel = rendered.pixel(x, y).unwrap();
                    (u32::from(pixel.red()) << 16
                        | u32::from(pixel.green()) << 8
                        | u32::from(pixel.blue()))
                        != shell.theme.text
                })));
                assert!(has_rgb(&rendered, shell.theme.selected));
            }
        }
    }
}

#[test]
fn filtered_menu_renders_the_matching_app_not_its_original_catalog_index() {
    use crate::model::MenuApp;
    let hidden = MenuApp {
        desktop_id: "org.example.Terminal.desktop".into(),
        name: "Terminal".into(),
        icon: Some("utilities-terminal".into()),
        startup_wm_class: None,
    };
    let matching = MenuApp {
        desktop_id: "org.example.Notes.desktop".into(),
        name: "Заметки".into(),
        icon: Some("accessories-text-editor".into()),
        startup_wm_class: None,
    };
    for locale in [Locale::En, Locale::Ru, Locale::Es] {
        let mut filtered = Shell::new(locale, Theme::classic());
        filtered.set_applications(vec![hidden.clone(), matching.clone()]);
        filtered.enter(Screen::Menu);
        filtered.search_query = "ЗАМЕТКИ".into();
        let mut expected = Shell::new(locale, Theme::classic());
        expected.set_applications(vec![matching.clone()]);
        expected.enter(Screen::Menu);
        let mut actual_image = image();
        let mut expected_image = image();
        paint(&mut actual_image, &filtered, "12:34", None);
        paint(&mut expected_image, &expected, "12:34", None);
        // The query header differs, but the complete app grid and its
        // selected icon/label must equal a catalog containing only the
        // matching desktop. A global-index renderer would draw Terminal.
        let grid = 31 * WIDTH * 4..135 * WIDTH * 4;
        assert_eq!(
            &actual_image.data()[grid.clone()],
            &expected_image.data()[grid]
        );
        filtered.search_query = "missing app".into();
        let mut no_matches = image();
        paint(&mut no_matches, &filtered, "12:34", None);
        assert_ne!(actual_image.data(), no_matches.data());
        assert_eq!(filtered.menu_apps().len(), 0);
        assert_eq!(filtered.actions()[3], Action::Search);
    }
}

#[test]
fn long_unicode_search_keeps_its_viewport_and_caret_inside_the_input() {
    use alcard_ui::dialog::TextInput;
    let query = "Начало aplicación con palabras distintas и длинный русский конец";
    for locale in [Locale::En, Locale::Ru, Locale::Es] {
        for theme in [Theme::classic(), Theme::emerald()] {
            let mut shell = Shell::new(locale, theme);
            shell.enter(Screen::Menu);
            shell.search_input = Some(TextInput::new(query, 96));
            let mut end = image();
            paint(&mut end, &shell, "12:34", Some("Must not cover the editor"));
            let mut without_banner = image();
            paint(&mut without_banner, &shell, "12:34", None);
            assert_eq!(end.data(), without_banner.data());
            shell.search_input.as_mut().unwrap().home();
            let mut start = image();
            paint(&mut start, &shell, "12:34", None);
            assert_ne!(start.data(), end.data());
            for y in 0..HEIGHT as u32 {
                for x in 0..WIDTH as u32 {
                    if start.pixel(x, y) != end.pixel(x, y) {
                        assert!((20..300).contains(&x) && (71..93).contains(&y));
                    }
                }
            }
            assert!(end.pixels().iter().all(|pixel| pixel.alpha() == 255));
            let mut footer = image();
            softkeys(&mut footer, &shell);
            let offset = SOFTKEY_TOP as usize * WIDTH * 4;
            assert_eq!(&end.data()[offset..], &footer.data()[offset..]);
            shell.search_input = None;
            let mut normal_footer = image();
            softkeys(&mut normal_footer, &shell);
            assert_ne!(&footer.data()[offset..], &normal_footer.data()[offset..]);
        }
    }
}

#[test]
fn search_preview_count_changes_without_committing_the_menu_query() {
    use crate::model::MenuApp;
    use alcard_ui::dialog::TextInput;
    let mut shell = Shell::new(Locale::Ru, Theme::classic());
    shell.set_applications(
        ["Terminal", "Notes"]
            .into_iter()
            .map(|name| MenuApp {
                desktop_id: format!("org.example.{name}.desktop"),
                name: name.into(),
                icon: None,
                startup_wm_class: None,
            })
            .collect(),
    );
    shell.enter(Screen::Menu);
    shell.search_input = Some(TextInput::new("Terminal", 96));
    let mut one_match = image();
    paint(&mut one_match, &shell, "12:34", None);
    shell.search_input = Some(TextInput::new("No matching name", 96));
    let mut no_matches = image();
    paint(&mut no_matches, &shell, "12:34", None);
    // The result counter must follow the draft while the background
    // catalog and the committed query remain unchanged until Enter.
    assert!(
        (102..111).any(|y| { (20..292).any(|x| one_match.pixel(x, y) != no_matches.pixel(x, y)) })
    );
    assert!(shell.search_query.is_empty());
    assert_eq!(shell.menu_apps().len(), 2);
}

#[test]
fn quick_actions_and_system_dialogs_preserve_status_and_only_use_bottom_controls() {
    use crate::model::{SystemPhase, SystemRequest};
    use crate::system::Failure;
    for locale in [Locale::En, Locale::Ru, Locale::Es] {
        for theme in [Theme::classic(), Theme::emerald()] {
            let mut shell = Shell::new(locale, theme);
            shell.system_snapshot.capabilities = [
                Capability::Allowed,
                Capability::Authentication,
                Capability::Blocked,
            ];
            let mut home = image();
            paint(&mut home, &shell, "12:34", None);
            let mut views = vec![
                SystemView::Menu { selected: 1 },
                SystemView::Unavailable {
                    operation: Operation::PowerOff,
                    capability: Capability::Blocked,
                    scroll: usize::MAX,
                },
            ];
            views.extend(Operation::ALL.map(|operation| SystemView::Confirm {
                operation,
                scroll: usize::MAX,
            }));
            for view in views {
                shell.system_view = Some(view);
                let mut rendered = image();
                paint(
                    &mut rendered,
                    &shell,
                    "12:34",
                    Some("Must not cover quick actions"),
                );
                assert_eq!(
                    &rendered.data()[..18 * WIDTH * 4],
                    &home.data()[..18 * WIDTH * 4]
                );
                let mut footer = image();
                softkeys(&mut footer, &shell);
                let offset = SOFTKEY_TOP as usize * WIDTH * 4;
                assert_eq!(&rendered.data()[offset..], &footer.data()[offset..]);
                assert!(rendered.pixels().iter().all(|pixel| pixel.alpha() == 255));
            }
            let mut phases = Vec::new();
            for phase in [
                SystemPhase::Pending,
                SystemPhase::Accepted,
                SystemPhase::Failed(Failure::Denied),
                SystemPhase::Failed(Failure::Timeout),
                SystemPhase::Uncertain,
            ] {
                shell.system_request = Some(SystemRequest {
                    id: 7,
                    operation: Operation::Reboot,
                    phase,
                });
                shell.system_view = Some(SystemView::Status {
                    operation: Operation::Reboot,
                    scroll: 0,
                });
                let mut rendered = image();
                paint(&mut rendered, &shell, "12:34", None);
                let mut footer = image();
                softkeys(&mut footer, &shell);
                let offset = SOFTKEY_TOP as usize * WIDTH * 4;
                assert_eq!(&rendered.data()[offset..], &footer.data()[offset..]);
                // Completion, preflight failure and an uncertain result
                // must not collapse into the same visible success screen.
                assert!(phases
                    .iter()
                    .all(|previous: &Vec<u8>| previous != rendered.data()));
                phases.push(rendered.take());
            }
        }
    }
}

#[test]
fn scrolling_long_confirmation_keeps_title_and_bottom_actions_fixed() {
    let mut shell = Shell::new(Locale::En, Theme::classic());
    shell.enter(Screen::Recent);
    shell.live_windows = vec![crate::model::WindowInfo {
        id: 8,
        title: format!("{} Exact final target", "Long window title ".repeat(40)),
        app_id: "org.example.App".into(),
        active: false,
        activity: 1,
    }];
    shell.dialog = Some(Dialog::ConfirmClose(8));
    let mut first = image();
    paint(&mut first, &shell, "12:34", None);
    shell.dialog_scroll = usize::MAX;
    let mut last = image();
    paint(&mut last, &shell, "12:34", None);
    assert_ne!(first.data(), last.data());
    // Long dialogs use the maximum panel height. Scrolling changes only
    // body rows and the scroll thumb, never the title or softkey actions.
    assert_eq!(
        &first.data()[..50 * WIDTH * 4],
        &last.data()[..50 * WIDTH * 4]
    );
    let footer = SOFTKEY_TOP as usize * WIDTH * 4;
    assert_eq!(&first.data()[footer..], &last.data()[footer..]);
}

#[test]
fn overlay_notification_is_premultiplied_and_fully_bounded() {
    let mut image = Pixmap::new(WIDTH as u32, 40).unwrap();
    paint_notification(&mut image, &"X".repeat(250), 0x102030, 0xffffff, 0x6677aa);
    assert_eq!(image.data().len(), WIDTH * 40 * 4);
    assert_eq!(image.pixel(0, 0).unwrap().alpha(), 0);
    let accent = image.pixel(8, 4).unwrap();
    assert_eq!(
        (accent.red(), accent.green(), accent.blue(), accent.alpha()),
        (0x66, 0x77, 0xaa, 255)
    );
    let background = image.pixel(6, 33).unwrap();
    assert_eq!(
        (background.red(), background.green(), background.blue()),
        (0x10, 0x20, 0x30)
    );
}
#[test]
fn every_screen_renders_in_both_themes_and_all_locales() {
    for locale in [Locale::En, Locale::Ru, Locale::Es] {
        for theme in [Theme::classic(), Theme::emerald()] {
            let mut shell = Shell::new(locale, theme);
            shell.applications = (0..12)
                .map(|index| crate::model::MenuApp {
                    desktop_id: format!("app-{index}.desktop"),
                    name: "Приложение / Aplicación".into(),
                    icon: None,
                    startup_wm_class: None,
                })
                .collect();
            shell.notices = vec![notice()];
            shell.status.battery = BatteryState::Present {
                percent: Some(47),
                charge: ChargeState::Charging,
            };
            shell.status.wifi = NetworkStatus {
                state: LinkState::Routed,
                strength: Some(3),
            };
            shell.status.ethernet = NetworkStatus {
                state: LinkState::LinkOnly,
                strength: None,
            };
            for screen in [
                Screen::Home,
                Screen::Menu,
                Screen::Recent,
                Screen::Notifications,
            ] {
                shell.screen = screen;
                let mut image = image();
                paint(&mut image, &shell, "12:34", None);
                assert!(has_rgb(&image, shell.theme.background));
                assert!(has_rgb(&image, shell.theme.accent));
                assert!(image.pixels().iter().all(|pixel| pixel.alpha() == 255));
            }
            shell.screen = Screen::Notifications;
            shell.notification_view = NotificationView::Detail {
                id: 1,
                generation: 1,
                scroll: 0,
            };
            let mut image = image();
            paint(&mut image, &shell, "12:34", None);
            assert!(image.pixels().iter().all(|pixel| pixel.alpha() == 255));
            shell.on_key(crate::model::Key::Soft(1));
            paint(&mut image, &shell, "12:34", None);
            assert!(has_rgb(&image, shell.theme.selected));
        }
    }
}
#[test]
fn application_navigation_and_long_cards_fit_native_screen_in_all_locales() {
    use crate::model::{
        AppTarget, ApplicationOption, ApplicationView, LaunchStatus, MenuApp, WindowInfo,
    };
    for locale in [Locale::En, Locale::Ru, Locale::Es] {
        for theme in [Theme::classic(), Theme::emerald()] {
            let mut shell = Shell::new(locale, theme);
            shell.set_applications(vec![MenuApp {
                desktop_id: "org.example.Editor.desktop".into(),
                name: "Полное название aplicación muy larga ".repeat(20),
                icon: None,
                startup_wm_class: None,
            }]);
            shell.sync_windows(
                (1..=8)
                    .map(|id| WindowInfo {
                        id,
                        title: format!("Окно {id}: {}", "Documento без сокращений ".repeat(15)),
                        app_id: "org.example.Editor".into(),
                        active: false,
                        activity: id,
                    })
                    .collect(),
            );
            shell.sync_launches(vec![LaunchStatus {
                desktop_id: "org.example.Editor.desktop".into(),
                name: "Editor".into(),
                attempt: 2,
                phase: LaunchPhase::Unconfirmed,
                reason: "Полная причина / explicación completa ".repeat(50),
            }]);
            shell.enter(Screen::Menu);
            let target = AppTarget::Desktop("org.example.Editor.desktop".into());
            let views = [
                ApplicationView::List,
                ApplicationView::Options {
                    target: target.clone(),
                    entries: vec![
                        ApplicationOption::Windows,
                        ApplicationOption::Retry { attempt: 2 },
                        ApplicationOption::Details,
                    ],
                    selected: 1,
                },
                ApplicationView::Windows {
                    desktop_id: "org.example.Editor.desktop".into(),
                    choice: alcard_ui::dialog::ChoiceList::new(1..=8),
                },
                ApplicationView::Details {
                    target,
                    scroll: usize::MAX,
                },
                ApplicationView::ConfirmRetry {
                    desktop_id: "org.example.Editor.desktop".into(),
                    attempt: 2,
                    scroll: usize::MAX,
                },
            ];
            for view in views {
                shell.application_view = view;
                let mut rendered = image();
                paint(
                    &mut rendered,
                    &shell,
                    "12:34",
                    Some("Do not cover a nested view"),
                );
                assert_eq!((rendered.width(), rendered.height()), (320, 170));
                assert!(rendered.pixels().iter().all(|pixel| pixel.alpha() == 255));
                let mut footer = image();
                softkeys(&mut footer, &shell);
                let offset = SOFTKEY_TOP as usize * WIDTH * 4;
                assert_eq!(&rendered.data()[offset..], &footer.data()[offset..]);
                assert_eq!(shell.actions()[2], Action::None);
                assert_eq!(
                    shell.actions()[3],
                    if shell.application_view == ApplicationView::List {
                        Action::Search
                    } else {
                        Action::None
                    }
                );
            }
        }
    }
}
fn notice() -> crate::model::Notice {
    crate::model::Notice {
        id: 1,
        generation: 1,
        title: "Пример длинного заголовка / Un aviso largo".into(),
        description: "Строка с полным текстом.\n\nInformación completa. ".repeat(40),
        app_name: "Example sender".into(),
        desktop_entry: Some("org.example.App".into()),
        created_at: 1_700_000_000,
        urgency: 2,
        unread: true,
        actions: vec![crate::model::NoticeAction {
            key: "default".into(),
            label: "Открыть приложение / Abrir la aplicación".into(),
        }],
        active: true,
    }
}
#[test]
fn unread_indicator_includes_expired_history_and_clears_only_when_read() {
    let mut shell = Shell::new(Locale::En, Theme::classic());
    let mut notice = notice();
    notice.active = false;
    shell.notices = vec![notice];
    let mut unread = image();
    status(&mut unread, &shell, "12:34");
    shell.notices[0].unread = false;
    let mut read = image();
    status(&mut read, &shell, "12:34");
    assert!((3..15).any(|y| (127..158).any(|x| unread.pixel(x, y) != read.pixel(x, y))));
    assert_eq!(shell.unread_count(), 0);
}
#[test]
fn unknown_battery_is_not_rendered_as_empty_and_states_stay_in_the_status_bar() {
    let theme = Theme::classic();
    let mut unknown = image();
    battery_icon(
        &mut unknown,
        BatteryState::Unknown,
        &theme,
        BatteryPreferences::default(),
    );
    let mut empty = image();
    battery_icon(
        &mut empty,
        BatteryState::Present {
            percent: Some(0),
            charge: ChargeState::Discharging,
        },
        &theme,
        BatteryPreferences::default(),
    );
    assert_ne!(unknown.data(), empty.data());
    for state in [
        LinkState::Unknown,
        LinkState::Absent,
        LinkState::Disabled,
        LinkState::Disconnected,
        LinkState::Connecting,
        LinkState::LinkOnly,
        LinkState::Addressed,
        LinkState::Routed,
    ] {
        let mut canvas = image();
        network_icon(
            &mut canvas,
            175,
            NetworkStatus {
                state,
                strength: None,
            },
            true,
            &theme,
        );
        network_icon(
            &mut canvas,
            205,
            NetworkStatus {
                state,
                strength: None,
            },
            false,
            &theme,
        );
        assert!(canvas.pixels()[WIDTH * 18..]
            .iter()
            .all(|pixel| pixel.alpha() == 0));
    }
}

#[test]
fn every_critical_percentage_has_a_visible_danger_outline_in_both_themes() {
    for theme in [Theme::classic(), Theme::emerald()] {
        let danger = (
            (theme.error >> 16) as u8,
            (theme.error >> 8) as u8,
            theme.error as u8,
        );
        for charge in [
            ChargeState::Discharging,
            ChargeState::NotCharging,
            ChargeState::Unknown,
        ] {
            for percent in 0..=5 {
                let mut canvas = image();
                battery_icon(
                    &mut canvas,
                    BatteryState::Present {
                        percent: Some(percent),
                        charge,
                    },
                    &theme,
                    BatteryPreferences::default(),
                );
                let tip = canvas.pixel(256, 8).unwrap();
                assert_eq!((tip.red(), tip.green(), tip.blue()), danger, "{percent}%");
                assert_eq!(tip.alpha(), 255);
                assert!(canvas.pixels()[WIDTH * 18..]
                    .iter()
                    .all(|pixel| pixel.alpha() == 0));
            }
        }
        for state in [BatteryState::Unknown, BatteryState::Absent] {
            let mut canvas = image();
            battery_icon(&mut canvas, state, &theme, BatteryPreferences::default());
            let tip = canvas.pixel(256, 8).unwrap();
            assert_ne!((tip.red(), tip.green(), tip.blue()), danger);
        }
    }
}

#[test]
fn battery_colors_follow_configured_thresholds_and_charging_or_full_are_positive() {
    let theme = Theme::classic();
    let preferences = BatteryPreferences {
        low_percent: 30,
        critical_percent: 10,
        ..BatteryPreferences::default()
    };
    let cases = [
        (0, ChargeState::Discharging, theme.error),
        (10, ChargeState::Discharging, theme.error),
        (11, ChargeState::Discharging, theme.warning),
        (30, ChargeState::Discharging, theme.warning),
        (31, ChargeState::Discharging, theme.muted),
        (0, ChargeState::Charging, theme.success),
        (5, ChargeState::Full, theme.success),
    ];
    for (percent, charge, expected) in cases {
        let mut canvas = image();
        battery_icon(
            &mut canvas,
            BatteryState::Present {
                percent: Some(percent),
                charge,
            },
            &theme,
            preferences,
        );
        let tip = canvas.pixel(256, 8).unwrap();
        assert_eq!(
            (tip.red(), tip.green(), tip.blue()),
            (
                (expected >> 16) as u8,
                (expected >> 8) as u8,
                expected as u8
            ),
            "{percent}% {charge:?}",
        );
    }
}

#[test]
fn notification_previews_make_truncation_explicit() {
    assert_eq!(abbreviated("Short", 5), "Short");
    assert_eq!(abbreviated("Длинная строка", 6), "Длинн…");
    assert_eq!(abbreviated("One\nTwo", 20), "One Two");
}
#[test]
fn translucent_dimming_preserves_opaque_destination_and_clips() {
    let mut image = Pixmap::new(8, 8).unwrap();
    image.fill(Color::WHITE);
    dim(&mut image, -2, -2, 6, 6);
    let shaded = image.pixel(1, 1).unwrap();
    assert_eq!(shaded.alpha(), 255);
    assert!((126..=128).contains(&shaded.red()));
    assert_eq!(image.pixel(7, 7).unwrap().red(), 255);
}
