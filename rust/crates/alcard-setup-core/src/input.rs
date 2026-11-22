//! Wizard navigation, field editing and validation transitions.
use crate::{secrets::Invalid, EditBuffer, EditField, KeyboardLayout, Pane, Step, Wizard};
use alcard_input::Intent;
use alcard_l10n::Locale;
use alcard_setup_protocol::{is_valid_timezone, Session};
use zeroize::Zeroizing;

impl Wizard {
    fn session_focus(&self) -> usize {
        Session::ALL
            .iter()
            .position(|session| *session == self.session)
            .unwrap_or(2)
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
            let validation = self.passwords.validate(self.doas_for_user);
            self.status_key = match validation {
                Ok(()) => "status.demo",
                Err(Invalid::UserMissing) => "account.user_password_required",
                Err(Invalid::UserTooShort) => "account.password_short",
                Err(Invalid::UserMismatch) => "account.password_mismatch",
                Err(Invalid::RootTooShort) => "account.root_password_short",
                Err(Invalid::RootMismatch) => "account.root_password_mismatch",
                Err(Invalid::NoAdministrator) => "account.admin_required",
            };
            if validation.is_err() {
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
