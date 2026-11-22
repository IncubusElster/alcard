//! Device-independent Alcard input mapping.
//! Crossterm events are translated to Key by the TUI frontend;
//! future gamepad backends may translate to Key::Pad without changing UI logic.

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PadButton {
    A,
    X,
    Share,
    Y,
    B,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Key {
    Enter,
    Space,
    Digit(u8),
    Text(char),
    Backspace,
    Escape,
    BackTab,
    Tab,
    Up,
    Down,
    Pad(PadButton),
    CtrlC,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Intent {
    /// Zero-based index for one of the five contextual softkeys.
    Soft(usize),
    FocusNext,
    Up,
    Down,
    Insert(char),
    Erase,
    CommitEdit,
    CancelEdit,
    Quit,
    Ignore,
}

/// The same physical key has a different meaning while text is being edited.
/// No text-input character can accidentally trigger a softkey.
pub fn interpret(key: Key, editing: bool) -> Intent {
    if key == Key::CtrlC {
        return Intent::Quit;
    }

    if editing {
        return match key {
            Key::Enter | Key::Pad(PadButton::A) => Intent::CommitEdit,
            Key::Escape | Key::Pad(PadButton::B) => Intent::CancelEdit,
            Key::Backspace => Intent::Erase,
            Key::Space => Intent::Insert(' '),
            Key::Digit(n @ 0..=9) => Intent::Insert(char::from(b'0' + n)),
            Key::Text(c) if !c.is_control() => Intent::Insert(c),
            _ => Intent::Ignore,
        };
    }

    match key {
        Key::Enter | Key::Digit(4) | Key::Pad(PadButton::A) => Intent::Soft(0),
        Key::Space | Key::Digit(5) | Key::Pad(PadButton::X) => Intent::Soft(1),
        Key::Digit(6) | Key::Pad(PadButton::Share) => Intent::Soft(2),
        Key::Digit(7) | Key::Pad(PadButton::Y) => Intent::Soft(3),
        Key::Escape | Key::Backspace | Key::BackTab | Key::Digit(8) | Key::Pad(PadButton::B) => {
            Intent::Soft(4)
        }
        Key::Tab => Intent::FocusNext,
        Key::Up => Intent::Up,
        Key::Down => Intent::Down,
        _ => Intent::Ignore,
    }
}

#[cfg(test)]
#[path = "unit_tests/lib.rs"]
mod tests;
