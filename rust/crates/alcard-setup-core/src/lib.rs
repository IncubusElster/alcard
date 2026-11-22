//! Pure, non-privileged first-boot UI model.
//! Holds setup choices and zeroizing edit buffers; OS changes belong to setupd.
use alcard_l10n::Locale;
use alcard_setup_protocol::Session;
use zeroize::Zeroizing;
mod input;
mod request;
mod secrets;
mod view;
use secrets::{Passwords, Slot};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Step {
    Welcome,
    Account,
    Network,
    DateTime,
    Session,
    Finish,
}
impl Step {
    pub const ALL: [Self; 6] = [
        Self::Welcome,
        Self::Account,
        Self::Network,
        Self::DateTime,
        Self::Session,
        Self::Finish,
    ];
    pub fn index(self) -> usize {
        Self::ALL
            .iter()
            .position(|item| *item == self)
            .expect("known step")
    }
    pub fn title_key(self) -> &'static str {
        match self {
            Self::Welcome => "step.welcome",
            Self::Account => "step.account",
            Self::Network => "step.network",
            Self::DateTime => "step.time",
            Self::Session => "step.session",
            Self::Finish => "step.finish",
        }
    }
    pub fn next(self) -> Option<Self> {
        Self::ALL.get(self.index() + 1).copied()
    }
    pub fn previous(self) -> Option<Self> {
        self.index()
            .checked_sub(1)
            .and_then(|i| Self::ALL.get(i))
            .copied()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum KeyboardLayout {
    Us,
    Ru,
    Es,
    De,
}
impl KeyboardLayout {
    pub const ALL: [Self; 4] = [Self::Us, Self::Ru, Self::Es, Self::De];
    pub const fn code(self) -> &'static str {
        match self {
            Self::Us => "US",
            Self::Ru => "RU",
            Self::Es => "ES",
            Self::De => "DE",
        }
    }
    pub const fn label_key(self) -> &'static str {
        match self {
            Self::Us => "keyboard.us",
            Self::Ru => "keyboard.ru",
            Self::Es => "keyboard.es",
            Self::De => "keyboard.de",
        }
    }
}

#[derive(Clone, Debug)]
enum Pane {
    Main,
    Languages {
        focus: usize,
    },
    Keyboards {
        focus: usize,
        draft: Vec<KeyboardLayout>,
    },
    WifiNetworks {
        focus: usize,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum EditField {
    Username,
    DisplayName,
    Timezone,
    WifiPassword,
    UserPassword,
    UserConfirm,
    RootPassword,
    RootConfirm,
}
impl EditField {
    fn secret_slot(self) -> Option<Slot> {
        match self {
            Self::UserPassword => Some(Slot::UserPassword),
            Self::UserConfirm => Some(Slot::UserConfirm),
            Self::RootPassword => Some(Slot::RootPassword),
            Self::RootConfirm => Some(Slot::RootConfirm),
            _ => None,
        }
    }
}
struct EditBuffer {
    field: EditField,
    text: Zeroizing<String>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SecretField {
    UserPassword,
    UserConfirm,
    RootPassword,
    RootConfirm,
}
impl SecretField {
    fn slot(self) -> Slot {
        match self {
            Self::UserPassword => Slot::UserPassword,
            Self::UserConfirm => Slot::UserConfirm,
            Self::RootPassword => Slot::RootPassword,
            Self::RootConfirm => Slot::RootConfirm,
        }
    }
}

#[derive(Clone, Debug)]
pub struct Row {
    pub text: String,
    pub selected: bool,
    /// A metadata tag, never plaintext. The renderer may borrow the secret
    /// transiently only if its corresponding Show password checkbox is checked.
    pub reveal: Option<SecretField>,
}
impl Row {
    fn plain(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            selected: false,
            reveal: None,
        }
    }
    fn focused(text: impl Into<String>, selected: bool) -> Self {
        Self {
            text: text.into(),
            selected,
            reveal: None,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Softkey {
    pub number: char,
    pub label_key: &'static str,
    pub enabled: bool,
}
impl Softkey {
    const fn disabled(number: char) -> Self {
        Self {
            number,
            label_key: "",
            enabled: false,
        }
    }
    const fn enabled(number: char, label_key: &'static str) -> Self {
        Self {
            number,
            label_key,
            enabled: true,
        }
    }
}

pub struct Wizard {
    pub step: Step,
    pub focus: usize,
    pub locale: Locale,
    /// US is permanently primary; RU ES DE are optional.
    pub keyboard_layouts: Vec<KeyboardLayout>,
    pub username: String,
    pub display_name: String,
    pub doas_for_user: bool,
    pub timezone: String,
    pub ntp_enabled: bool,
    pub network_skipped: bool,
    pub wired_link: Option<(String, bool)>,
    pub wifi_iface: Option<String>,
    wifi_networks: Vec<String>,
    wifi_selected: Option<String>,
    wifi_pending: Option<(String, String, Zeroizing<String>)>,
    pub should_exit: bool,
    status_key: &'static str,
    edit_warning_key: Option<&'static str>,
    pane: Pane,
    editing: Option<EditBuffer>,
    passwords: Passwords,
    show_user_password: bool,
    show_root_password: bool,
    pub session: Session,
    graphics_available: bool,
    minimal_available: bool,
    session_selected: bool,
    firstboot: bool,
}
impl Default for Wizard {
    fn default() -> Self {
        Self::new()
    }
}

impl Wizard {
    pub fn new() -> Self {
        Self {
            step: Step::Welcome,
            focus: 0,
            locale: Locale::En,
            keyboard_layouts: vec![KeyboardLayout::Us],
            username: String::new(),
            display_name: String::new(),
            doas_for_user: true,
            timezone: "Etc/UTC".into(),
            ntp_enabled: false,
            network_skipped: false,
            wired_link: None,
            wifi_iface: None,
            wifi_networks: Vec::new(),
            wifi_selected: None,
            wifi_pending: None,
            should_exit: false,
            status_key: "status.demo",
            edit_warning_key: None,
            pane: Pane::Main,
            editing: None,
            passwords: Passwords::default(),
            show_user_password: false,
            show_root_password: false,
            session: Session::Console,
            graphics_available: false,
            minimal_available: false,
            session_selected: false,
            firstboot: false,
        }
    }
    pub fn is_editing(&self) -> bool {
        self.editing.is_some()
    }
    pub fn set_session_availability(&mut self, graphics: bool, minimal: bool) {
        self.graphics_available = graphics;
        self.minimal_available = minimal;
        // Prefer the main Shell only before an explicit user choice, or when
        // that choice is no longer installed after an interrupted setup.
        if !self.session_selected || !self.session_available(self.session) {
            self.session = Session::ALL
                .into_iter()
                .find(|session| self.session_available(*session))
                .unwrap_or(Session::Console);
        }
    }
    fn session_available(&self, session: Session) -> bool {
        match session {
            Session::Graphics => self.graphics_available,
            Session::GraphicsMinimal => self.minimal_available,
            Session::Console => true,
        }
    }
    pub fn set_network_interfaces(
        &mut self,
        ethernet: Option<(String, bool)>,
        wifi: Option<String>,
    ) {
        self.wired_link = ethernet;
        self.wifi_iface = wifi;
    }
    pub fn wifi_menu_open(&self) -> bool {
        matches!(self.pane, Pane::WifiNetworks { .. })
    }
    pub fn set_wifi_networks(&mut self, networks: Vec<String>) {
        self.wifi_networks = networks;
        if let Pane::WifiNetworks { focus } = &mut self.pane {
            *focus = 0;
        }
    }
    pub fn take_wifi_request(&mut self) -> Option<(String, String, Zeroizing<String>)> {
        self.wifi_pending.take()
    }
    pub fn set_wifi_result(&mut self, connected: bool) {
        self.status_key = if connected {
            "network.connected"
        } else {
            "network.connect_failed"
        };
        if connected {
            self.network_skipped = false;
        }
    }

    pub fn set_apply_result(&mut self, success: bool) {
        self.status_key = if success {
            "status.apply_done"
        } else {
            "status.apply_failed"
        };
    }
}

#[cfg(test)]
#[path = "unit_tests/lib.rs"]
mod tests;
