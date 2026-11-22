//! Localized wizard rows and contextual softkeys.
use crate::{EditField, KeyboardLayout, Pane, Row, SecretField, Softkey, Step, Wizard};
use alcard_l10n::{tr, Locale};
use alcard_setup_protocol::Session;

impl Wizard {
    pub(super) fn session_label(session: Session) -> &'static str {
        match session {
            Session::Graphics => "session.graphics",
            Session::GraphicsMinimal => "session.graphics_minimal",
            Session::Console => "session.console",
        }
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
}
