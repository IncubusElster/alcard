use super::*;
use crate::secrets::Invalid;
use alcard_input::Intent;
use alcard_l10n::tr;

#[test]
fn six_ordered_screens() {
    assert_eq!(Step::ALL.len(), 6);
    assert_eq!(Step::Welcome.next(), Some(Step::Account));
    assert_eq!(Step::Account.next(), Some(Step::Network));
    assert_eq!(Step::Finish.next(), None);
}

#[test]
fn language_list_is_nested_and_changes_all_labels() {
    let mut w = Wizard::new();
    w.handle(Intent::Soft(1));
    assert!(w.title().contains("language"));
    w.handle(Intent::Down);
    w.handle(Intent::Soft(1));
    assert_eq!(w.locale, Locale::Ru);
    assert_eq!(w.title(), "Язык и раскладки");
}

#[test]
fn can_select_optional_layouts_but_us_stays_primary() {
    let mut w = Wizard::new();
    w.handle(Intent::Down);
    w.handle(Intent::Soft(1)); // open keyboard list
    w.handle(Intent::Down);
    w.handle(Intent::Soft(1)); // add RU
    w.handle(Intent::Down);
    w.handle(Intent::Soft(1)); // add ES
    w.handle(Intent::Down);
    w.handle(Intent::Soft(1)); // add DE
    w.handle(Intent::Soft(2)); // inactive primary button
    w.handle(Intent::Soft(0)); // save
    assert_eq!(
        w.keyboard_layouts,
        vec![
            KeyboardLayout::Us,
            KeyboardLayout::Ru,
            KeyboardLayout::Es,
            KeyboardLayout::De
        ]
    );
}

#[test]
fn us_cannot_be_deselected_even_after_adding_ru() {
    let mut w = Wizard::new();
    w.handle(Intent::Down);
    w.handle(Intent::Soft(1));
    w.handle(Intent::Soft(1)); // attempt to uncheck US
    assert_eq!(w.status_key, "status.us_required");
    w.handle(Intent::Down);
    w.handle(Intent::Soft(1)); // add RU
    w.handle(Intent::Up);
    w.handle(Intent::Soft(1)); // US still required
    assert_eq!(w.status_key, "status.us_required");
    w.handle(Intent::Soft(4)); // cancel
    assert_eq!(w.keyboard_layouts, vec![KeyboardLayout::Us]);
}

#[test]
fn account_fields_share_a_single_screen() {
    let mut w = Wizard::new();
    w.step = Step::Account;
    let all = w
        .rows()
        .iter()
        .map(|r| r.text.clone())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(all.contains("Username"));
    assert!(all.contains("User password"));
    assert!(all.contains("doas"));
    assert!(all.contains("Root password"));
}

#[test]
fn editing_disables_softkeys_and_username_validation_applies() {
    let mut w = Wizard::new();
    w.step = Step::Account;
    w.handle(Intent::Soft(1));
    assert!(w.softkeys().iter().all(|s| !s.enabled));
    w.handle(Intent::Insert('4'));
    w.handle(Intent::CommitEdit);
    assert!(w.is_editing()); // invalid leading digit
    w.handle(Intent::CancelEdit);
    w.handle(Intent::Soft(1));
    for ch in "user48".chars() {
        w.handle(Intent::Insert(ch));
    }
    w.handle(Intent::CommitEdit);
    assert_eq!(w.username, "user48");
    w.handle(Intent::Soft(0));
    assert_eq!(w.step, Step::Account); // password is required
    w.focus = 2;
    w.handle(Intent::Soft(1));
    for c in "user-secret".chars() {
        w.handle(Intent::Insert(c));
    }
    w.handle(Intent::CommitEdit);
    w.focus = 4;
    w.handle(Intent::Soft(1));
    for c in "user-secret".chars() {
        w.handle(Intent::Insert(c));
    }
    w.handle(Intent::CommitEdit);
    w.handle(Intent::Soft(0));
    assert_eq!(w.step, Step::Network);
}

#[test]
fn password_edit_is_enabled_but_apply_is_still_disabled() {
    let mut w = Wizard::new();
    w.step = Step::Account;
    w.focus = 2;
    assert!(w.softkeys()[1].enabled); // The account page allows password editing.
    w.step = Step::Finish;
    assert!(!w.softkeys()[0].enabled);
    w.handle(Intent::Soft(0));
    assert_eq!(w.step, Step::Finish);
}

#[test]
fn username_normalizes_uppercase_on_every_keystroke() {
    let mut w = Wizard::new();
    w.step = Step::Account;
    w.handle(Intent::Soft(1));
    for c in "Elster_42".chars() {
        w.handle(Intent::Insert(c));
    }
    assert!(w.rows().iter().any(|r| r.text.contains("elster_42_")));
    w.handle(Intent::CommitEdit);
    assert_eq!(w.username, "elster_42");
}

#[test]
fn rejected_characters_never_reach_username_field_and_warn_immediately() {
    for locale in Locale::ALL {
        let mut w = Wizard::new();
        w.locale = locale;
        w.step = Step::Account;
        w.handle(Intent::Soft(1));
        w.handle(Intent::Insert('5'));
        assert_eq!(
            w.editing.as_ref().expect("username editor").text.as_str(),
            ""
        );
        assert_eq!(w.edit_warning_key, Some("account.username_first"));
        assert_eq!(w.status(), tr(locale, "account.username_first"));
        w.handle(Intent::Insert('A'));
        assert_eq!(
            w.editing.as_ref().expect("username editor").text.as_str(),
            "a"
        );
        for rejected in [' ', 'Ё', '漢', '💡', '.', '/', '@', '\n'] {
            w.handle(Intent::Insert(rejected));
            assert_eq!(w.edit_warning_key, Some("account.username_unsupported"));
            assert_eq!(w.status(), tr(locale, "account.username_unsupported"));
            // Assert against the editor's buffer, not all UI rows: labels and
            // help messages may legitimately contain spaces or punctuation.
            assert_eq!(
                w.editing.as_ref().expect("username editor").text.as_str(),
                "a"
            );
        }
        w.handle(Intent::Insert('_'));
        assert_eq!(w.edit_warning_key, None);
        w.handle(Intent::CommitEdit);
        assert_eq!(w.username, "a_");
    }
}

#[test]
fn username_at_32_ascii_bytes_disallows_extra_input() {
    let mut w = Wizard::new();
    w.step = Step::Account;
    w.handle(Intent::Soft(1));
    for c in "A".repeat(32).chars() {
        w.handle(Intent::Insert(c));
    }
    w.handle(Intent::Insert('b'));
    assert_eq!(w.edit_warning_key, Some("account.username_limit"));
    w.handle(Intent::CommitEdit);
    assert_eq!(w.username, "a".repeat(32));
}

#[test]
fn password_mask_shows_exact_character_count_during_input_and_after_save() {
    for field in [2usize, 4, 6, 8] {
        let mut wizard = Wizard::new();
        wizard.step = Step::Account;
        wizard.focus = field;
        wizard.handle(Intent::Soft(1));
        assert!(wizard.rows().iter().any(|r| r.text.contains(": _")));
        for (i, character) in "aЁ💡7".chars().enumerate() {
            wizard.handle(Intent::Insert(character));
            let shown = wizard
                .rows()
                .into_iter()
                .find(|r| r.selected)
                .expect("focused secret field")
                .text;
            assert!(shown.ends_with(&format!("{}_", "*".repeat(i + 1))));
            assert!(!shown.contains("Ё") && !shown.contains("💡"));
        }
        wizard.handle(Intent::Erase);
        let shown = wizard
            .rows()
            .into_iter()
            .find(|r| r.selected)
            .expect("focused secret field")
            .text;
        assert!(shown.ends_with("***_"));
        wizard.handle(Intent::CommitEdit);
        let shown = wizard
            .rows()
            .into_iter()
            .find(|r| r.selected)
            .expect("focused secret field")
            .text;
        assert!(shown.ends_with("***"));
    }
}

#[test]
fn account_requires_matching_password_and_redacts_contents() {
    let mut wizard = Wizard::new();
    wizard.step = Step::Account;
    wizard.username = "operator".into();
    wizard.focus = 2;
    wizard.handle(Intent::Soft(1));
    for c in "account_password".chars() {
        wizard.handle(Intent::Insert(c));
    }
    let rendered = wizard
        .rows()
        .iter()
        .map(|row| row.text.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(!rendered.contains("account_password"));
    assert!(rendered.contains(&"*".repeat("account_password".chars().count())));
    wizard.handle(Intent::CommitEdit);
    wizard.handle(Intent::Soft(0));
    assert_eq!(wizard.step, Step::Account);
    assert_eq!(wizard.status_key, "account.password_mismatch");

    wizard.focus = 4;
    wizard.handle(Intent::Soft(1));
    for c in "account_password".chars() {
        wizard.handle(Intent::Insert(c));
    }
    wizard.handle(Intent::CommitEdit);
    wizard.handle(Intent::Soft(0));
    assert_eq!(wizard.step, Step::Network);
}

#[test]
fn root_password_allows_admin_when_doas_is_off() {
    let mut wizard = Wizard::new();
    wizard.step = Step::Account;
    wizard.username = "operator".into();
    let fill = |wizard: &mut Wizard, field: usize, value: &str| {
        wizard.focus = field;
        wizard.handle(Intent::Soft(1));
        for c in value.chars() {
            wizard.handle(Intent::Insert(c));
        }
        wizard.handle(Intent::CommitEdit);
    };
    fill(&mut wizard, 2, "user_password");
    fill(&mut wizard, 4, "user_password");
    wizard.focus = 5;
    wizard.handle(Intent::Soft(1)); // no doas
    wizard.handle(Intent::Soft(0));
    assert_eq!(wizard.step, Step::Account);
    assert_eq!(wizard.status_key, "account.admin_required");
    fill(&mut wizard, 6, "root_password");
    fill(&mut wizard, 8, "root_password");
    wizard.handle(Intent::Soft(0));
    assert_eq!(wizard.step, Step::Network);
}

#[test]
fn cancelled_password_edit_keeps_confirmed_value() {
    let mut wizard = Wizard::new();
    wizard.step = Step::Account;
    wizard.focus = 2;
    wizard.handle(Intent::Soft(1));
    for c in "original_secret".chars() {
        wizard.handle(Intent::Insert(c));
    }
    wizard.handle(Intent::CommitEdit);
    wizard.handle(Intent::Soft(1));
    for c in "replacement_secret".chars() {
        wizard.handle(Intent::Insert(c));
    }
    wizard.handle(Intent::CancelEdit);
    assert_eq!(
        wizard.passwords.masked(Slot::UserPassword),
        "*".repeat("original_secret".chars().count())
    );
    assert_eq!(wizard.passwords.masked(Slot::UserConfirm), "");
}

#[test]
fn two_password_visibility_toggles_are_independent() {
    let mut wizard = Wizard::new();
    wizard.step = Step::Account;
    wizard.focus = 2;
    wizard.handle(Intent::Soft(1));
    for c in "user-secret".chars() {
        wizard.handle(Intent::Insert(c));
    }
    wizard.handle(Intent::CommitEdit);
    wizard.focus = 6;
    wizard.handle(Intent::Soft(1));
    for c in "root-secret".chars() {
        wizard.handle(Intent::Insert(c));
    }
    wizard.handle(Intent::CommitEdit);

    let rows = wizard.rows();
    assert!(rows.iter().all(|row| row.reveal.is_none()));
    assert!(rows.iter().any(|row| row.text.contains("***********")));
    assert!(!rows
        .iter()
        .any(|row| row.text.contains("user-secret") || row.text.contains("root-secret")));

    wizard.focus = 3;
    wizard.handle(Intent::Soft(1)); // Show user password
    assert!(wizard.show_user_password);
    assert!(!wizard.show_root_password);
    let rows = wizard.rows();
    assert_eq!(
        wizard.revealed_secret(&rows[4]),
        Some(("user-secret", false))
    );
    assert!(wizard.revealed_secret(&rows[8]).is_none());
    assert!(rows.iter().all(|row| !row.text.contains("user-secret")));

    wizard.focus = 7;
    wizard.handle(Intent::Soft(1)); // Show root password
    assert!(wizard.show_root_password);
    let rows = wizard.rows();
    assert_eq!(
        wizard.revealed_secret(&rows[8]),
        Some(("root-secret", false))
    );

    wizard.focus = 3;
    wizard.handle(Intent::Soft(1));
    assert!(!wizard.show_user_password);
    assert!(wizard.show_root_password);
    let rows = wizard.rows();
    assert!(wizard.revealed_secret(&rows[4]).is_none());
    assert_eq!(
        wizard.revealed_secret(&rows[8]),
        Some(("root-secret", false))
    );
}

#[test]
fn visible_password_is_available_during_edit_without_storing_it_in_rows() {
    let mut wizard = Wizard::new();
    wizard.step = Step::Account;
    wizard.focus = 3;
    wizard.handle(Intent::Soft(1)); // Show user
    wizard.focus = 2;
    wizard.handle(Intent::Soft(1)); // Edit password
    for c in "abc".chars() {
        wizard.handle(Intent::Insert(c));
    }
    let rows = wizard.rows();
    assert_eq!(wizard.revealed_secret(&rows[4]), Some(("abc", true)));
    assert!(rows.iter().all(|row| !row.text.contains("abc")));
    wizard.handle(Intent::CancelEdit);
    let rows = wizard.rows();
    assert_eq!(wizard.revealed_secret(&rows[4]), Some(("", false)));
}

#[test]
fn checkbox_focus_and_localized_labels_are_correct() {
    for locale in Locale::ALL {
        let mut wizard = Wizard::new();
        wizard.locale = locale;
        wizard.step = Step::Account;
        wizard.focus = 3;
        assert_eq!(wizard.softkeys()[1].label_key, "action.toggle");
        assert!(wizard
            .rows()
            .iter()
            .any(|row| { row.selected && row.text.contains(tr(locale, "account.show_password")) }));
        wizard.handle(Intent::Soft(1));
        assert!(wizard
            .rows()
            .iter()
            .any(|row| { row.selected && row.text.starts_with("[x]") }));
    }
}

#[test]
fn reopening_user_password_edits_saved_value_not_an_empty_field() {
    let mut wizard = Wizard::new();
    wizard.step = Step::Account;
    wizard.focus = 2;
    wizard.handle(Intent::Soft(1));
    for c in "old-password".chars() {
        wizard.handle(Intent::Insert(c));
    }
    wizard.handle(Intent::CommitEdit);
    wizard.focus = 4;
    wizard.handle(Intent::Soft(1));
    for c in "old-password".chars() {
        wizard.handle(Intent::Insert(c));
    }
    wizard.handle(Intent::CommitEdit);

    wizard.focus = 2;
    wizard.handle(Intent::Soft(1));
    let selected = wizard
        .rows()
        .into_iter()
        .find(|r| r.selected)
        .expect("password row");
    assert!(selected.text.ends_with(&format!("{}_", "*".repeat(12))));
    wizard.handle(Intent::CommitEdit); // unchanged edit preserves confirmation
    assert!(wizard.passwords.validate(true).is_ok());

    wizard.handle(Intent::Soft(1));
    wizard.handle(Intent::Erase);
    wizard.handle(Intent::Insert('!'));
    let selected = wizard
        .rows()
        .into_iter()
        .find(|r| r.selected)
        .expect("password row");
    assert!(selected.text.ends_with(&format!("{}_", "*".repeat(12))));
    wizard.handle(Intent::CommitEdit);
    assert!(matches!(
        wizard.passwords.validate(true),
        Err(Invalid::UserMismatch)
    ));
}

#[test]
fn reopening_root_confirmation_can_edit_and_cancel_without_modifying_saved_value() {
    let mut wizard = Wizard::new();
    wizard.step = Step::Account;
    wizard.focus = 8;
    wizard.handle(Intent::Soft(1));
    for c in "root-confirmation".chars() {
        wizard.handle(Intent::Insert(c));
    }
    wizard.handle(Intent::CommitEdit);
    let original_mask = wizard.passwords.masked(Slot::RootConfirm);
    wizard.handle(Intent::Soft(1)); // reopen existing confirmation
    assert!(wizard
        .rows()
        .iter()
        .any(|r| r.selected && r.text.ends_with(&format!("{}_", original_mask))));
    wizard.handle(Intent::Erase);
    wizard.handle(Intent::CancelEdit);
    assert_eq!(wizard.passwords.masked(Slot::RootConfirm), original_mask);
}

#[test]
fn firstboot_payload_is_separate_from_public_plan_and_demo_cannot_apply() {
    let mut wizard = Wizard::new();
    wizard.step = Step::Account;
    wizard.username = "operator".into();
    let fill = |wizard: &mut Wizard, focus: usize, secret: &str| {
        wizard.focus = focus;
        wizard.handle(Intent::Soft(1));
        for c in secret.chars() {
            wizard.handle(Intent::Insert(c));
        }
        wizard.handle(Intent::CommitEdit);
    };
    fill(&mut wizard, 2, "test-password");
    fill(&mut wizard, 4, "test-password");
    wizard.step = Step::Finish;
    assert!(!wizard.softkeys()[0].enabled);
    assert!(wizard.firstboot_request().is_err());
    wizard.enable_firstboot();
    assert!(wizard.softkeys()[0].enabled);
    let bytes = wizard.firstboot_request().expect("validated payload");
    let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(json["plan"]["account"]["username"], "operator");
    assert_eq!(json["plan"]["timezone"], "Etc/UTC");
    assert!(json["plan"]["account"].get("user_password").is_none());
    assert_eq!(json["credentials"]["user_password"], "test-password");
    assert!(wizard
        .rows()
        .iter()
        .all(|row| !row.text.contains("test-password")));
}

#[test]
fn timezone_input_and_ntp_toggle_are_editable() {
    let mut wizard = Wizard::new();
    wizard.step = Step::DateTime;
    wizard.handle(Intent::Soft(1));
    for _ in 0..7 {
        wizard.handle(Intent::Erase);
    } // Etc/UTC
    for ch in "America/Montevideo".chars() {
        wizard.handle(Intent::Insert(ch));
    }
    wizard.handle(Intent::CommitEdit);
    assert_eq!(wizard.timezone, "America/Montevideo");
    wizard.handle(Intent::Down);
    wizard.handle(Intent::Soft(1));
    assert!(wizard.ntp_enabled);
}

#[test]
fn network_displays_wired_link_above_wifi_settings_in_all_languages() {
    for locale in Locale::ALL {
        let mut wizard = Wizard::new();
        wizard.locale = locale;
        wizard.step = Step::Network;
        wizard.set_network_interfaces(Some(("eth0".into(), true)), Some("wlan0".into()));
        let rows = wizard.rows();
        let ethernet = rows
            .iter()
            .position(|r| r.text.contains(tr(locale, "network.ethernet")))
            .unwrap();
        let wifi = rows
            .iter()
            .position(|r| r.text.contains(tr(locale, "network.wifi_settings")))
            .unwrap();
        assert!(ethernet < wifi);
        assert!(rows[ethernet].text.contains("eth0"));
        assert!(rows[wifi].text.contains("wlan0"));
    }
}

#[test]
fn wifi_network_selection_masks_password_then_clears_transfer() {
    let mut wizard = Wizard::new();
    wizard.step = Step::Network;
    wizard.set_network_interfaces(None, Some("wlan0".into()));
    wizard.focus = 1;
    wizard.handle(Intent::Soft(1));
    assert!(wizard.wifi_menu_open());
    wizard.set_wifi_networks(vec!["Test Network".into(), "Other".into()]);
    wizard.handle(Intent::Soft(1));
    assert!(wizard.is_editing());
    for ch in "wifi-secret".chars() {
        wizard.handle(Intent::Insert(ch));
    }
    let rows = wizard.rows();
    assert!(rows.iter().any(|r| r.text.contains("***********_")));
    assert!(!rows.iter().any(|r| r.text.contains("wifi-secret")));
    wizard.handle(Intent::CommitEdit);
    assert!(!wizard.is_editing());
    let (iface, ssid, secret) = wizard.take_wifi_request().expect("pending Wi-Fi request");
    assert_eq!(iface, "wlan0");
    assert_eq!(ssid, "Test Network");
    assert_eq!(secret.as_str(), "wifi-secret");
    assert!(wizard.take_wifi_request().is_none());
}

#[test]
fn session_default_prefers_shell_then_minimal_then_console() {
    let mut wizard = Wizard::new();
    assert_eq!(wizard.session, Session::Console);
    wizard.set_session_availability(false, true);
    assert_eq!(wizard.session, Session::GraphicsMinimal);
    wizard.set_session_availability(true, true);
    assert_eq!(wizard.session, Session::Graphics);
    wizard.set_session_availability(false, false);
    assert_eq!(wizard.session, Session::Console);
}

#[test]
fn explicit_session_selection_survives_availability_refresh() {
    let mut wizard = Wizard::new();
    wizard.set_session_availability(true, true);
    wizard.step = Step::Session;
    wizard.focus = 2;
    wizard.handle(Intent::Soft(1));
    assert_eq!(wizard.session, Session::Console);
    wizard.set_session_availability(true, true);
    assert_eq!(wizard.session, Session::Console);
    wizard.focus = 1;
    wizard.handle(Intent::Soft(1));
    assert_eq!(wizard.session, Session::GraphicsMinimal);
    wizard.set_session_availability(true, true);
    assert_eq!(wizard.session, Session::GraphicsMinimal);
    wizard.set_session_availability(false, false);
    assert_eq!(wizard.session, Session::Console);
}

#[test]
fn firstboot_plan_carries_each_session_without_legacy_ambiguity() {
    let mut w = Wizard::new();
    w.step = Step::Account;
    w.username = "operator".into();
    let fill = |w: &mut Wizard, focus: usize| {
        w.focus = focus;
        w.handle(Intent::Soft(1));
        for c in "test-password".chars() {
            w.handle(Intent::Insert(c));
        }
        w.handle(Intent::CommitEdit);
    };
    fill(&mut w, 2);
    fill(&mut w, 4);
    w.set_session_availability(true, true);
    w.enable_firstboot();
    for (focus, expected) in ["graphics", "graphics_minimal", "console"]
        .into_iter()
        .enumerate()
    {
        w.step = Step::Session;
        w.focus = focus;
        w.handle(Intent::Soft(1));
        w.step = Step::Finish;
        let frame = w.firstboot_request().unwrap();
        let value: serde_json::Value = serde_json::from_slice(&frame).unwrap();
        assert_eq!(value["plan"]["session"], expected);
    }
}

#[test]
fn unavailable_session_preserves_the_available_selection() {
    let mut w = Wizard::new();
    w.set_session_availability(false, true);
    w.step = Step::Session;
    w.handle(Intent::Soft(1));
    assert_eq!(w.status_key, "status.session_unavailable");
    assert_eq!(w.session, Session::GraphicsMinimal);
    w.handle(Intent::Down);
    w.handle(Intent::Down);
    w.handle(Intent::Soft(1));
    assert_eq!(w.session, Session::Console);
}

#[test]
fn session_picker_and_summary_expose_all_three_localized_choices() {
    let mut w = Wizard::new();
    w.set_session_availability(true, true);
    for locale in Locale::ALL {
        w.locale = locale;
        for (index, session) in Session::ALL.into_iter().enumerate() {
            w.step = Step::Session;
            w.focus = index;
            w.handle(Intent::Soft(1));
            let label = tr(locale, Wizard::session_label(session));
            let rows = w.rows();
            assert!(rows
                .iter()
                .any(|row| row.text.starts_with("(*)") && row.text.contains(label)));
            assert!(rows.iter().all(|row| row.text.chars().count() <= 40));
            w.step = Step::Finish;
            assert!(w.rows().iter().any(|row| row.text.ends_with(label)));
        }
    }
}
