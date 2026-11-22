use super::*;

#[test]
fn five_contextual_slots_work_from_keyboard_and_pad() {
    for (a, b, index) in [
        (Key::Enter, Key::Pad(PadButton::A), 0),
        (Key::Space, Key::Pad(PadButton::X), 1),
        (Key::Digit(6), Key::Pad(PadButton::Share), 2),
        (Key::Digit(7), Key::Pad(PadButton::Y), 3),
        (Key::BackTab, Key::Pad(PadButton::B), 4),
    ] {
        assert_eq!(interpret(a, false), Intent::Soft(index));
        assert_eq!(interpret(b, false), Intent::Soft(index));
    }
    assert_eq!(interpret(Key::Digit(5), false), Intent::Soft(1));
    assert_eq!(interpret(Key::Digit(8), false), Intent::Soft(4));
}

#[test]
fn typing_never_activates_softkeys() {
    assert_eq!(interpret(Key::Digit(4), true), Intent::Insert('4'));
    assert_eq!(interpret(Key::Digit(8), true), Intent::Insert('8'));
    assert_eq!(interpret(Key::Space, true), Intent::Insert(' '));
    assert_eq!(interpret(Key::Backspace, true), Intent::Erase);
    assert_eq!(interpret(Key::BackTab, true), Intent::Ignore);
    assert_eq!(interpret(Key::Pad(PadButton::X), true), Intent::Ignore);
}
