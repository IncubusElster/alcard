use super::*;

fn focused() -> Input {
    let mut input = Input::default();
    input.enter(&[]);
    input.update_modifiers(Modifiers::default());
    input
}

#[test]
fn navigation_is_focus_local_and_waits_for_enter_modifier_snapshot() {
    let mut input = Input::default();
    assert!(input.press(5, Keysym::_4, Some("4"), false).is_empty());
    input.enter(&[]);
    assert!(input.press(61, Keysym::F3, None, false).is_empty());
    input.update_modifiers(Modifiers {
        alt: true,
        ..Modifiers::default()
    });
    assert!(input.press(15, Keysym::Tab, Some("\t"), false).is_empty());
    input.update_modifiers(Modifiers::default());
    input.release(61);
    assert_eq!(input.press(61, Keysym::F3, None, false), vec![Key::Soft(3)]);
    assert!(input.press(60, Keysym::F2, None, false).is_empty());
    assert!(input.press(125, Keysym::Super_L, None, false).is_empty());
    input.leave();
    assert!(input.press(9, Keysym::_8, Some("8"), false).is_empty());
    assert!(input.press(30, Keysym::a, Some("a"), true).is_empty());
}

#[test]
fn repeats_and_keys_held_at_focus_cannot_confirm_or_type() {
    let mut input = Input::default();
    input.enter(&[28, 8]);
    input.update_modifiers(Modifiers::default());
    assert!(input.press(28, Keysym::Return, Some("\r"), true).is_empty());
    assert!(input.press(8, Keysym::_7, Some("7"), true).is_empty());
    input.release(28);
    assert_eq!(
        input.press(28, Keysym::Return, Some("\r"), true),
        vec![Key::Enter]
    );
    assert!(input.press(28, Keysym::Return, Some("\r"), true).is_empty());
    input.release(8);
    assert_eq!(
        input.press(8, Keysym::_7, Some("7"), false),
        vec![Key::Soft(3)]
    );
    // The same opening key cannot become text when the dialog appears.
    assert!(input.press(8, Keysym::_7, Some("7"), true).is_empty());
    input.release(8);
    assert_eq!(
        input.press(8, Keysym::_7, Some("7"), true),
        vec![Key::Text('7')]
    );
}

#[test]
fn plain_softkeys_are_layout_independent_and_f3_has_no_letter_alias() {
    let mut input = focused();
    for code in 5..=9 {
        assert_eq!(
            input.press(code, Keysym::dollar, Some("$"), false),
            vec![Key::Soft((code - 5) as usize)]
        );
    }
    assert_eq!(
        input.press(57, Keysym::space, Some(" "), false),
        vec![Key::Soft(1)]
    );
    assert_eq!(
        input.press(15, Keysym::Tab, Some("\t"), false),
        vec![Key::Soft(2)]
    );
    // Back+Tab is not the accepted alias; Y is a gamepad label, not a key.
    input.release(15);
    assert!(input
        .press(15, Keysym::ISO_Left_Tab, None, false)
        .is_empty());
    assert!(input.press(21, Keysym::y, Some("y"), false).is_empty());
    // Fn+ESC may produce Super; only an actual Escape symbol cancels.
    assert!(input.press(1, Keysym::Super_L, None, false).is_empty());
    input.release(1);
    assert_eq!(
        input.press(1, Keysym::Escape, None, false),
        vec![Key::Escape]
    );
}

#[test]
fn modified_shortcuts_never_trigger_shell_navigation() {
    for modifiers in [
        Modifiers {
            alt: true,
            ..Modifiers::default()
        },
        Modifiers {
            ctrl: true,
            ..Modifiers::default()
        },
        Modifiers {
            shift: true,
            ..Modifiers::default()
        },
        Modifiers {
            logo: true,
            ..Modifiers::default()
        },
    ] {
        let mut input = focused();
        input.update_modifiers(modifiers);
        for (code, keysym) in [
            (5, Keysym::_4),
            (15, Keysym::Tab),
            (61, Keysym::F3),
            (28, Keysym::Return),
            (103, Keysym::Up),
        ] {
            assert!(input.press(code, keysym, None, false).is_empty());
        }
    }
}

#[test]
fn editor_accepts_digits_spaces_layout_text_and_completed_compose() {
    let mut input = focused();
    for code in 5..=9 {
        let character = char::from_digit(code - 1, 10).unwrap();
        let text = character.to_string();
        assert_eq!(
            input.press(code, Keysym::_4, Some(&text), true),
            vec![Key::Text(character)]
        );
    }
    assert_eq!(
        input.press(57, Keysym::space, Some(" "), true),
        vec![Key::Text(' ')]
    );
    assert!(input.press(40, Keysym::dead_acute, None, true).is_empty());
    assert_eq!(
        input.press(18, Keysym::e, Some("é"), true),
        vec![Key::Text('é')]
    );
    assert_eq!(
        input.press(30, Keysym::a, Some("Яñ"), true),
        vec![Key::Text('Я'), Key::Text('ñ')]
    );
    input.update_modifiers(Modifiers {
        shift: true,
        ..Modifiers::default()
    });
    assert_eq!(
        input.press(21, Keysym::Y, Some("Y"), true),
        vec![Key::Text('Y')]
    );
    assert_eq!(
        input.press(20, Keysym::T, Some("Т"), true),
        vec![Key::Text('Т')]
    );
    input.update_modifiers(Modifiers {
        caps_lock: true,
        ..Modifiers::default()
    });
    assert_eq!(
        input.press(22, Keysym::U, Some("Ü"), true),
        vec![Key::Text('Ü')]
    );
}

#[test]
fn editor_navigation_and_control_u_are_explicit() {
    let mut input = focused();
    for (code, keysym, key) in [
        (28, Keysym::Return, Key::Enter),
        (1, Keysym::Escape, Key::Escape),
        (105, Keysym::Left, Key::Left),
        (106, Keysym::Right, Key::Right),
        (102, Keysym::Home, Key::Home),
        (107, Keysym::End, Key::End),
        (14, Keysym::BackSpace, Key::Backspace),
        (111, Keysym::Delete, Key::Delete),
    ] {
        assert_eq!(input.press(code, keysym, None, true), vec![key]);
    }
    input.update_modifiers(Modifiers {
        ctrl: true,
        ..Modifiers::default()
    });
    assert_eq!(
        input.press(22, Keysym::u, Some("\u{15}"), true),
        vec![Key::ClearText]
    );
    input.release(22);
    assert_eq!(
        input.press(22, Keysym::Cyrillic_ghe, None, true),
        vec![Key::ClearText]
    );
    assert!(input.press(30, Keysym::a, Some("\u{1}"), true).is_empty());
    input.update_modifiers(Modifiers {
        alt: true,
        ..Modifiers::default()
    });
    assert!(input.press(15, Keysym::Tab, Some("\t"), true).is_empty());
    input.update_modifiers(Modifiers {
        logo: true,
        ..Modifiers::default()
    });
    assert!(input.press(21, Keysym::y, Some("y"), true).is_empty());
}

#[test]
fn text_events_and_held_state_have_hard_bounds() {
    let mut input = focused();
    let event = "я".repeat(MAX_TEXT_EVENT + 1);
    assert_eq!(
        input.press(30, Keysym::a, Some(&event), true).len(),
        MAX_TEXT_EVENT
    );
    assert!(input
        .press(31, Keysym::s, Some("\n\r\t\0"), true)
        .is_empty());
    input.enter(&(0..MAX_HELD_KEYS as u32 + 1).collect::<Vec<_>>());
    input.update_modifiers(Modifiers::default());
    assert!(input.press(900, Keysym::F3, None, false).is_empty());
    assert!(input.held.len() <= MAX_HELD_KEYS);
}
