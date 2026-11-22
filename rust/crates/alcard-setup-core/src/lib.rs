//! Pure, non-privileged first-boot UI model.
//! The H0-H2 prototype never writes OS state or collects passwords.
use alcard_input::Intent;
use alcard_l10n::{tr, Locale};
use alcard_setup_protocol::{
    is_valid_timezone, AccountPlan, KeyboardPlan, Language, Layout, Session, SetupPlan,
};
use serde::Serialize;
use zeroize::Zeroizing;
mod secrets;
use secrets::{Invalid, Passwords, Slot};

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
    fn session_label(session: Session) -> &'static str {
        match session {
            Session::Graphics => "session.graphics",
            Session::GraphicsMinimal => "session.graphics_minimal",
            Session::Console => "session.console",
        }
    }
    fn session_focus(&self) -> usize {
        Session::ALL
            .iter()
            .position(|session| *session == self.session)
            .unwrap_or(2)
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

    /// Only the explicit --firstboot CLI selects a privileged apply flow.
    pub fn enable_firstboot(&mut self) {
        self.firstboot = true;
    }

    /// Serialize a borrowed secret payload directly into one zeroizing buffer.
    /// The public SetupPlan itself contains no passwords or password hashes.
    pub fn firstboot_request(&self) -> Result<Zeroizing<Vec<u8>>, &'static str> {
        if !self.firstboot || self.step != Step::Finish || self.editing.is_some() {
            return Err("firstboot_not_ready");
        }
        if !is_valid_timezone(&self.timezone)
            || self.username.is_empty()
            || self.passwords.validate(self.doas_for_user).is_err()
        {
            return Err("firstboot_validation_failed");
        }
        if !self.session_available(self.session) {
            return Err("selected_session_unavailable");
        }
        let locale = match self.locale {
            Locale::En => Language::En,
            Locale::Ru => Language::Ru,
            Locale::Es => Language::Es,
        };
        let layouts = self
            .keyboard_layouts
            .iter()
            .map(|layout| match layout {
                KeyboardLayout::Us => Layout::Us,
                KeyboardLayout::Ru => Layout::Ru,
                KeyboardLayout::Es => Layout::Es,
                KeyboardLayout::De => Layout::De,
            })
            .collect::<Vec<_>>();
        let plan = SetupPlan {
            schema_version: 1,
            language: locale,
            account: AccountPlan {
                username: self.username.clone(),
                display_name: self.display_name.clone(),
                doas_enabled: self.doas_for_user,
            },
            keyboard: KeyboardPlan { layouts },
            session: self.session,
            timezone: self.timezone.clone(),
            ntp_enabled: self.ntp_enabled,
        };
        #[derive(Serialize)]
        struct Credentials<'a> {
            user_password: &'a str,
            user_confirm: &'a str,
            root_password: &'a str,
            root_confirm: &'a str,
        }
        #[derive(Serialize)]
        struct Request<'a> {
            plan: SetupPlan,
            credentials: Credentials<'a>,
        }
        let credentials = Credentials {
            user_password: self.passwords.borrow_for_display(Slot::UserPassword),
            user_confirm: self.passwords.borrow_for_display(Slot::UserConfirm),
            root_password: self.passwords.borrow_for_display(Slot::RootPassword),
            root_confirm: self.passwords.borrow_for_display(Slot::RootConfirm),
        };
        serde_json::to_vec(&Request { plan, credentials })
            .map(Zeroizing::new)
            .map_err(|_| "firstboot_serialization_failed")
    }
    pub fn set_apply_result(&mut self, success: bool) {
        self.status_key = if success {
            "status.apply_done"
        } else {
            "status.apply_failed"
        };
    }
    pub fn title(&self) -> &'static str {
        let key = match self.pane {
            Pane::Main => self.step.title_key(),
            Pane::Languages { .. } => "pane.languages",
            Pane::Keyboards { .. } => "pane.keyboards",
            Pane::WifiNetworks { .. } => "pane.wifi",
        };
        tr(self.locale, key)
    }
    pub fn status(&self) -> &'static str {
        if self.is_editing() {
            tr(
                self.locale,
                self.edit_warning_key.unwrap_or("status.typing"),
            )
        } else {
            let key = if self.firstboot && self.status_key == "status.demo" {
                "status.firstboot"
            } else {
                self.status_key
            };
            tr(self.locale, key)
        }
    }
    pub fn rows(&self) -> Vec<Row> {
        match &self.pane {
            Pane::Languages { focus } => {
                let mut rows = vec![
                    Row::plain(tr(self.locale, "language.intro")),
                    Row::plain(""),
                ];
                for (i, locale) in Locale::ALL.iter().enumerate() {
                    let mark = if *locale == self.locale { "(*)" } else { "( )" };
                    rows.push(Row::focused(
                        format!("{mark} {}", tr(self.locale, locale.name_key())),
                        i == *focus,
                    ));
                }
                rows.push(Row::plain(""));
                rows.push(Row::plain(tr(self.locale, "language.hint")));
                rows
            }
            Pane::Keyboards { focus, draft } => {
                let mut rows = vec![
                    Row::plain(tr(self.locale, "keyboard.intro")),
                    Row::plain(""),
                ];
                for (i, layout) in KeyboardLayout::ALL.iter().enumerate() {
                    let selected = draft.contains(layout);
                    let check = if selected { "[x]" } else { "[ ]" };
                    let primary = if draft.first() == Some(layout) {
                        format!(" ({})", tr(self.locale, "keyboard.primary"))
                    } else {
                        String::new()
                    };
                    rows.push(Row::focused(
                        format!("{check} {}{primary}", tr(self.locale, layout.label_key())),
                        *focus == i,
                    ));
                }
                rows.push(Row::plain(""));
                rows.push(Row::plain(tr(self.locale, "keyboard.hint")));
                rows
            }
            Pane::WifiNetworks { focus } => {
                let mut rows = vec![
                    Row::plain(tr(self.locale, "network.wifi_choose")),
                    Row::plain(""),
                ];
                if self.wifi_networks.is_empty() {
                    rows.push(Row::plain(tr(self.locale, "network.wifi_empty")));
                }
                for (i, name) in self.wifi_networks.iter().enumerate() {
                    rows.push(Row::focused(name.as_str(), *focus == i));
                }
                if let Some(edit) = &self.editing {
                    if edit.field == EditField::WifiPassword {
                        rows.push(Row::plain(format!(
                            "{}: {}_",
                            tr(self.locale, "network.wifi_password"),
                            "*".repeat(edit.text.chars().count())
                        )));
                    }
                }
                rows.push(Row::plain(""));
                rows.push(Row::plain(tr(self.locale, "network.wifi_refresh_hint")));
                rows
            }
            Pane::Main => self.main_rows(),
        }
    }
    fn main_rows(&self) -> Vec<Row> {
        let t = |key| tr(self.locale, key);
        match self.step {
            Step::Welcome => vec![
                Row::plain(t("welcome.intro")),
                Row::plain(""),
                self.main_row(
                    0,
                    format!("{}: {}", t("label.language"), t(self.locale.name_key())),
                ),
                self.main_row(
                    1,
                    format!("{}: {}", t("label.keyboards"), self.layout_codes()),
                ),
                Row::plain(""),
                Row::plain(t("welcome.note")),
            ],
            Step::Account => vec![
                Row::plain(t("account.intro")),
                Row::plain(""),
                self.main_row(
                    0,
                    format!(
                        "{}: {}",
                        t("account.username"),
                        self.editable_text(EditField::Username)
                    ),
                ),
                self.main_row(
                    1,
                    format!(
                        "{}: {}",
                        t("account.display_name"),
                        self.editable_text(EditField::DisplayName)
                    ),
                ),
                self.secret_row(2, "account.password", EditField::UserPassword),
                self.main_row(
                    3,
                    format!(
                        "[{}] {}",
                        if self.show_user_password { 'x' } else { ' ' },
                        t("account.show_password")
                    ),
                ),
                self.secret_row(4, "account.confirm", EditField::UserConfirm),
                self.main_row(
                    5,
                    format!(
                        "[{}] {}",
                        if self.doas_for_user { 'x' } else { ' ' },
                        t("account.doas")
                    ),
                ),
                self.secret_row(6, "account.root_password", EditField::RootPassword),
                self.main_row(
                    7,
                    format!(
                        "[{}] {}",
                        if self.show_root_password { 'x' } else { ' ' },
                        t("account.show_password")
                    ),
                ),
                self.secret_row(8, "account.root_confirm", EditField::RootConfirm),
                Row::plain(""),
                Row::plain(t("account.secret_hint")),
            ],
            Step::Network => {
                let ethernet = match &self.wired_link {
                    Some((name, true)) => format!("{name} ({})", t("network.ethernet_connected")),
                    Some((name, false)) => format!("{name} ({})", t("network.ethernet_no_link")),
                    None => t("network.ethernet_missing").into(),
                };
                vec![
                    Row::plain(t("network.intro")),
                    Row::plain(""),
                    self.main_row(0, format!("{}: {ethernet}", t("network.ethernet"))),
                    self.main_row(
                        1,
                        format!(
                            "{}: {}",
                            t("network.wifi_settings"),
                            self.wifi_iface
                                .as_deref()
                                .unwrap_or(t("network.wifi_missing"))
                        ),
                    ),
                    Row::plain(""),
                    Row::plain(t("network.note")),
                ]
            }
            Step::DateTime => vec![
                Row::plain(t("time.intro")),
                Row::plain(""),
                self.main_row(
                    0,
                    format!(
                        "{}: {}",
                        t("time.timezone"),
                        self.editable_text(EditField::Timezone)
                    ),
                ),
                self.main_row(
                    1,
                    format!(
                        "[{}] {}",
                        if self.ntp_enabled { 'x' } else { ' ' },
                        t("time.ntp")
                    ),
                ),
                Row::plain(""),
                Row::plain(t("time.note")),
            ],
            Step::Session => {
                let mut rows = vec![Row::plain(t("session.intro")), Row::plain("")];
                for (index, session) in Session::ALL.into_iter().enumerate() {
                    rows.push(self.main_row(
                        index,
                        format!(
                            "({}) {} - {}",
                            if self.session == session { '*' } else { ' ' },
                            t(Self::session_label(session)),
                            t(if self.session_available(session) {
                                "session.available"
                            } else {
                                "session.unavailable"
                            })
                        ),
                    ));
                }
                rows.push(Row::plain(""));
                rows.push(Row::plain(t("session.note")));
                rows
            }
            Step::Finish => vec![
                Row::plain(t(if self.firstboot {
                    "finish.apply_intro"
                } else {
                    "finish.intro"
                })),
                Row::plain(""),
                Row::plain(format!(
                    "{}: {}",
                    t("finish.user"),
                    self.placeholder(&self.username)
                )),
                Row::plain(format!("{}: {}", t("finish.keyboard"), self.layout_codes())),
                Row::plain(format!("{}: {}", t("finish.doas"), self.doas_value())),
                Row::plain(format!(
                    "{}: {}",
                    t("finish.root"),
                    if self.passwords.has_root() {
                        t("value.enabled")
                    } else {
                        t("value.not_set")
                    }
                )),
                Row::plain(format!(
                    "{}: {}",
                    t("finish.network"),
                    self.network_status()
                )),
                Row::plain(format!(
                    "{}: {}",
                    t("finish.session"),
                    t(Self::session_label(self.session))
                )),
                Row::plain(format!("{}: {}", t("time.timezone"), self.timezone)),
                Row::plain(""),
                Row::plain(t(if self.firstboot {
                    "finish.apply_warning"
                } else {
                    "finish.disabled"
                })),
            ],
        }
    }
    fn main_row(&self, index: usize, text: impl Into<String>) -> Row {
        Row::focused(text, self.focus == index)
    }

    fn secret_row(&self, index: usize, label_key: &str, field: EditField) -> Row {
        let revealed = if index == 2 || index == 4 {
            self.show_user_password
        } else {
            self.show_root_password
        };
        let mut row = Row::focused(
            format!(
                "{}: {}",
                tr(self.locale, label_key),
                if revealed {
                    String::new()
                } else {
                    self.editable_text(field)
                }
            ),
            self.focus == index,
        );
        if revealed {
            row.reveal = Some(match field {
                EditField::UserPassword => SecretField::UserPassword,
                EditField::UserConfirm => SecretField::UserConfirm,
                EditField::RootPassword => SecretField::RootPassword,
                EditField::RootConfirm => SecretField::RootConfirm,
                _ => unreachable!("secret row requires a password field"),
            });
        }
        row
    }

    /// UI-only, explicitly disclosed view. Never put this reference into Row.text,
    /// JSON setup plans, logs, or serialized application state.
    pub fn revealed_secret<'a>(&'a self, row: &Row) -> Option<(&'a str, bool)> {
        if self.step != Step::Account {
            return None;
        }
        let field = row.reveal?;
        if let Some(editor) = &self.editing {
            let matching = matches!(
                (field, editor.field),
                (SecretField::UserPassword, EditField::UserPassword)
                    | (SecretField::UserConfirm, EditField::UserConfirm)
                    | (SecretField::RootPassword, EditField::RootPassword)
                    | (SecretField::RootConfirm, EditField::RootConfirm)
            );
            if matching {
                return Some((editor.text.as_str(), true));
            }
        }
        Some((self.passwords.borrow_for_display(field.slot()), false))
    }
    fn layout_codes(&self) -> String {
        self.keyboard_layouts
            .iter()
            .map(|layout| layout.code())
            .collect::<Vec<_>>()
            .join(", ")
    }
    fn placeholder<'a>(&self, value: &'a str) -> &'a str {
        if value.is_empty() {
            tr(self.locale, "value.unset")
        } else {
            value
        }
    }
    fn editable_text(&self, field: EditField) -> String {
        if let Some(edit) = &self.editing {
            if edit.field == field {
                if field.secret_slot().is_some() {
                    return format!("{}_", "*".repeat(edit.text.chars().count()));
                }
                return format!("{}_", edit.text.as_str());
            }
        }
        if let Some(slot) = field.secret_slot() {
            let masked = self.passwords.masked(slot);
            return if masked.is_empty() {
                tr(self.locale, "value.not_set").into()
            } else {
                masked
            };
        }
        let value = match field {
            EditField::Username => &self.username,
            EditField::DisplayName => &self.display_name,
            EditField::Timezone => &self.timezone,
            _ => unreachable!("secrets handled above"),
        };
        self.placeholder(value).into()
    }
    fn doas_value(&self) -> &'static str {
        tr(
            self.locale,
            if self.doas_for_user {
                "value.enabled"
            } else {
                "value.disabled"
            },
        )
    }
    fn network_status(&self) -> &'static str {
        tr(
            self.locale,
            if self.network_skipped {
                "network.skipped"
            } else {
                "network.no_connection"
            },
        )
    }

    fn main_fields(step: Step) -> usize {
        match step {
            Step::Welcome => 2,
            Step::Session => Session::ALL.len(),
            Step::Account => 9,
            Step::Network => 2,
            Step::DateTime => 2,
            Step::Finish => 0,
        }
    }

    pub fn softkeys(&self) -> [Softkey; 5] {
        let mut keys = [
            Softkey::disabled('4'),
            Softkey::disabled('5'),
            Softkey::disabled('6'),
            Softkey::disabled('7'),
            Softkey::disabled('8'),
        ];
        if self.is_editing() {
            return keys;
        }
        match &self.pane {
            Pane::Languages { .. } => {
                keys[0] = Softkey::enabled('4', "action.select");
                keys[1] = Softkey::enabled('5', "action.select");
                keys[4] = Softkey::enabled('8', "action.back");
            }
            Pane::Keyboards { .. } => {
                keys[0] = Softkey::enabled('4', "action.done");
                keys[1] = Softkey::enabled('5', "action.toggle");

                keys[3] = Softkey::enabled('7', "action.info");
                keys[4] = Softkey::enabled('8', "action.back");
            }
            Pane::WifiNetworks { .. } => {
                keys[0] = Softkey::enabled('4', "action.select");
                keys[1] = Softkey::enabled('5', "action.select");
                keys[2] = Softkey::enabled('6', "action.refresh");
                keys[4] = Softkey::enabled('8', "action.back");
            }
            Pane::Main => {
                if self.step != Step::Finish {
                    keys[0] = Softkey::enabled('4', "action.next");
                } else if self.firstboot {
                    keys[0] = Softkey::enabled('4', "action.apply");
                }
                keys[4] = Softkey::enabled('8', "action.back");
                keys[3] = Softkey::enabled('7', "action.info");
                match self.step {
                    Step::Welcome | Step::Session => {
                        keys[1] = Softkey::enabled('5', "action.select")
                    }
                    Step::Account => {
                        if [3, 5, 7].contains(&self.focus) {
                            keys[1] = Softkey::enabled('5', "action.toggle");
                        } else {
                            keys[1] = Softkey::enabled('5', "action.edit");
                        }
                    }
                    Step::Network => {
                        keys[2] = Softkey::enabled('6', "action.skip");
                        if self.focus == 1 && self.wifi_iface.is_some() {
                            keys[1] = Softkey::enabled('5', "action.select");
                        }
                    }
                    Step::DateTime => {
                        keys[1] = Softkey::enabled(
                            '5',
                            if self.focus == 0 {
                                "action.edit"
                            } else {
                                "action.toggle"
                            },
                        );
                    }
                    _ => {}
                }
            }
        }
        keys
    }

    pub fn handle(&mut self, intent: Intent) {
        if intent == Intent::Quit {
            self.should_exit = true;
            return;
        }
        if self.editing.is_some() {
            self.handle_edit(intent);
            return;
        }
        match intent {
            Intent::FocusNext | Intent::Down => self.move_focus(true),
            Intent::Up => self.move_focus(false),
            Intent::Soft(index) if index < 5 && self.softkeys()[index].enabled => {
                self.activate(index);
            }
            _ => {}
        }
    }
    fn handle_edit(&mut self, intent: Intent) {
        let Some(mut edit) = self.editing.take() else {
            return;
        };
        match intent {
            Intent::Insert(input) => {
                if edit.field == EditField::Username {
                    let c = input.to_ascii_lowercase();
                    let is_first = edit.text.is_empty();
                    let allowed = c.is_ascii_lowercase()
                        || c == '_'
                        || (!is_first && (c.is_ascii_digit() || c == '-'));
                    if !allowed {
                        self.edit_warning_key =
                            Some(if is_first && (c.is_ascii_digit() || c == '-') {
                                "account.username_first"
                            } else {
                                "account.username_unsupported"
                            });
                    } else if edit.text.len() >= 32 {
                        self.edit_warning_key = Some("account.username_limit");
                    } else {
                        edit.text.push(c);
                        self.edit_warning_key = None;
                    }
                } else {
                    let max = if edit.field.secret_slot().is_some() {
                        128
                    } else {
                        64
                    };
                    if input.is_control() {
                        self.edit_warning_key = Some("account.invalid_character");
                    } else if edit.text.chars().count() >= max {
                        self.edit_warning_key = Some("account.field_limit");
                    } else {
                        edit.text.push(input);
                        self.edit_warning_key = None;
                    }
                }
            }
            Intent::Erase => {
                edit.text.pop();
                self.edit_warning_key = None;
            }
            Intent::CommitEdit => {
                match edit.field {
                    EditField::Username => {
                        if edit.text.is_empty() {
                            self.edit_warning_key = Some("account.no_username");
                            self.editing = Some(edit);
                            return;
                        }
                        self.username = edit.text.as_str().to_owned();
                    }
                    EditField::DisplayName => self.display_name = edit.text.as_str().to_owned(),
                    EditField::WifiPassword => {
                        if let (Some(iface), Some(ssid)) =
                            (self.wifi_iface.as_ref(), self.wifi_selected.as_ref())
                        {
                            self.wifi_pending = Some((iface.clone(), ssid.clone(), edit.text));
                            self.pane = Pane::Main;
                            self.status_key = "network.connecting";
                        }
                    }
                    EditField::Timezone => {
                        if !is_valid_timezone(edit.text.as_str()) {
                            self.edit_warning_key = Some("time.bad_timezone");
                            self.editing = Some(edit);
                            return;
                        }
                        self.timezone = edit.text.as_str().to_owned();
                    }
                    secret => self
                        .passwords
                        .set(secret.secret_slot().expect("secret field"), edit.text),
                }
                self.edit_warning_key = None;
                self.status_key = "status.edit_saved";
                return;
            }
            Intent::CancelEdit => {
                // edit is dropped and zeroized, leaving the old stored value unchanged.
                self.edit_warning_key = None;
                self.status_key = "status.edit_cancel";
                return;
            }
            _ => {}
        }
        self.editing = Some(edit);
    }
    fn move_focus(&mut self, forward: bool) {
        let (focus, count) = match &mut self.pane {
            Pane::Main => (&mut self.focus, Self::main_fields(self.step)),
            Pane::Languages { focus } => (focus, Locale::ALL.len()),
            Pane::Keyboards { focus, .. } => (focus, KeyboardLayout::ALL.len()),
            Pane::WifiNetworks { focus } => (focus, self.wifi_networks.len().max(1)),
        };
        if count > 0 {
            *focus = if forward {
                (*focus + 1) % count
            } else {
                (*focus + count - 1) % count
            };
        }
    }
    fn activate(&mut self, index: usize) {
        match self.pane.clone() {
            Pane::WifiNetworks { focus } => match index {
                0 | 1 if !self.wifi_networks.is_empty() => {
                    self.wifi_selected = self.wifi_networks.get(focus).cloned();
                    self.editing = Some(EditBuffer {
                        field: EditField::WifiPassword,
                        text: Zeroizing::new(String::new()),
                    });
                    self.status_key = "network.password_hint";
                }
                4 => self.pane = Pane::Main,
                _ => {}
            },
            Pane::Main => self.activate_main(index),
            Pane::Languages { focus } => match index {
                0 | 1 => {
                    self.locale = Locale::ALL[focus];
                    self.pane = Pane::Main;
                    self.status_key = "status.language_saved";
                }
                4 => {
                    self.pane = Pane::Main;
                    self.status_key = "status.edit_cancel";
                }
                _ => {}
            },
            Pane::Keyboards { focus, .. } => match index {
                0 => {
                    if let Pane::Keyboards { draft, .. } = &self.pane {
                        self.keyboard_layouts = draft.clone();
                    }
                    self.pane = Pane::Main;
                    self.status_key = "status.layouts_saved";
                }
                1 => self.toggle_layout(focus),
                2 => {} // Original US stays primary.
                3 => self.status_key = "keyboard.hint",
                4 => {
                    self.pane = Pane::Main;
                    self.status_key = "status.layouts_cancel";
                }
                _ => {}
            },
        }
    }
    fn toggle_layout(&mut self, index: usize) {
        let layout = KeyboardLayout::ALL[index];
        if let Pane::Keyboards { draft, .. } = &mut self.pane {
            if layout == KeyboardLayout::Us {
                self.status_key = "status.us_required";
                return;
            }
            if let Some(old) = draft.iter().position(|value| *value == layout) {
                draft.remove(old);
            } else {
                draft.push(layout);
            }
        }
        self.status_key = "status.open_keyboard";
    }
    fn activate_main(&mut self, index: usize) {
        match index {
            0 => self.advance(),
            1 => self.context_action(),
            2 if self.step == Step::Network => {
                self.network_skipped = true;
                self.status_key = "status.skipped";
                self.advance();
            }
            3 => self.status_key = "status.info",
            4 => self.back(),
            _ => {}
        }
    }
    fn context_action(&mut self) {
        match self.step {
            Step::Welcome if self.focus == 0 => {
                let focus = Locale::ALL
                    .iter()
                    .position(|value| *value == self.locale)
                    .unwrap_or(0);
                self.pane = Pane::Languages { focus };
                self.status_key = "status.open_language";
            }
            Step::Welcome => {
                self.pane = Pane::Keyboards {
                    focus: 0,
                    draft: self.keyboard_layouts.clone(),
                };
                self.status_key = "status.open_keyboard";
            }
            Step::Account if self.focus == 3 => {
                self.show_user_password = !self.show_user_password;
                self.status_key = "status.demo";
            }
            Step::Account if self.focus == 7 => {
                self.show_root_password = !self.show_root_password;
                self.status_key = "status.demo";
            }
            Step::Account if self.focus == 5 => {
                self.doas_for_user = !self.doas_for_user;
                self.status_key = "status.demo";
            }
            Step::Account => {
                let field = match self.focus {
                    0 => EditField::Username,
                    1 => EditField::DisplayName,
                    2 => EditField::UserPassword,
                    4 => EditField::UserConfirm,
                    6 => EditField::RootPassword,
                    8 => EditField::RootConfirm,
                    _ => return,
                };
                // Editing starts with the saved value; Esc discards the temporary
                // copy and preserves the old password/confirmation unchanged.
                let text = match field {
                    EditField::Username => Zeroizing::new(self.username.clone()),
                    EditField::DisplayName => Zeroizing::new(self.display_name.clone()),
                    _ => self
                        .passwords
                        .copy_for_edit(field.secret_slot().expect("account password field")),
                };
                self.editing = Some(EditBuffer { field, text });
                self.edit_warning_key = None;
                self.status_key = "status.edit";
            }
            Step::Network if self.focus == 1 => {
                if self.wifi_iface.is_some() {
                    self.pane = Pane::WifiNetworks { focus: 0 };
                    self.status_key = "network.wifi_choose";
                }
            }
            Step::DateTime if self.focus == 1 => {
                self.ntp_enabled = !self.ntp_enabled;
                self.status_key = "status.demo";
            }
            Step::DateTime => {
                self.editing = Some(EditBuffer {
                    field: EditField::Timezone,
                    text: Zeroizing::new(self.timezone.clone()),
                });
                self.edit_warning_key = None;
                self.status_key = "status.edit";
            }
            Step::Session => {
                let selected = Session::ALL
                    .get(self.focus)
                    .copied()
                    .unwrap_or(Session::Console);
                if self.session_available(selected) {
                    self.session = selected;
                    self.session_selected = true;
                    self.status_key = match selected {
                        Session::Graphics => "status.graphics",
                        Session::GraphicsMinimal => "status.graphics_minimal",
                        Session::Console => "status.console",
                    };
                } else {
                    self.status_key = "status.session_unavailable";
                }
            }
            _ => self.status_key = "status.unavailable",
        }
    }
    fn advance(&mut self) {
        if self.step == Step::Account {
            if self.username.is_empty() {
                self.status_key = "account.no_username";
                return;
            }
            self.status_key = match self.passwords.validate(self.doas_for_user) {
                Ok(()) => "status.demo",
                Err(Invalid::UserMissing) => "account.user_password_required",
                Err(Invalid::UserTooShort) => "account.password_short",
                Err(Invalid::UserMismatch) => "account.password_mismatch",
                Err(Invalid::RootTooShort) => "account.root_password_short",
                Err(Invalid::RootMismatch) => "account.root_password_mismatch",
                Err(Invalid::NoAdministrator) => "account.admin_required",
            };
            if self.passwords.validate(self.doas_for_user).is_err() {
                return;
            }
        }
        if let Some(next) = self.step.next() {
            self.step = next;
            self.focus = if next == Step::Session {
                self.session_focus()
            } else {
                0
            };
            self.status_key = if self.firstboot && next == Step::Finish {
                "status.firstboot_ready"
            } else {
                "status.demo"
            };
        }
    }
    fn back(&mut self) {
        if let Some(previous) = self.step.previous() {
            self.step = previous;
            self.focus = if previous == Step::Session {
                self.session_focus()
            } else {
                0
            };
            self.status_key = "status.back";
        } else {
            self.should_exit = true;
        }
    }
}

#[cfg(test)]
#[path = "unit_tests/lib.rs"]
mod tests;
