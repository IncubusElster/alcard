//! Credential UI state. Only the native worker may open a challenge or permit retry.
use crate::protocol::{plain, Command, Event, Identity, Secret};

pub use alcard_l10n::Locale as Language;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    Choosing,
    Waiting,
    Prompt,
    Retry,
    Finished,
}

pub struct Form {
    pub id: u64,
    pub action: String,
    pub reason: String,
    pub identities: Vec<Identity>,
    pub selected: Option<usize>,
    pub prompt: String,
    pub notice: String,
    pub error: bool,
    pub phase: Phase,
    pub details: bool,
    pub scroll: usize,
    pub caps_lock: bool,
    pub layout: u32,
    serial: u64,
    secret: Secret,
}
impl Form {
    pub fn new(id: u64, action: String, reason: String, identities: Vec<Identity>) -> Self {
        Self {
            id,
            action: plain(&action, 256),
            reason: plain(&reason, 2048),
            identities,
            selected: None,
            prompt: String::new(),
            notice: String::new(),
            error: false,
            phase: Phase::Choosing,
            details: false,
            scroll: 0,
            caps_lock: false,
            layout: 0,
            serial: 0,
            secret: Secret::default(),
        }
    }
    pub fn auto_select(&mut self) -> Option<Command> {
        if self.identities.len() == 1 && self.phase == Phase::Choosing {
            self.selected = Some(0);
            self.enter()
        } else {
            None
        }
    }
    pub fn receive(&mut self, event: Event, language: Language) {
        if self.phase == Phase::Finished {
            return;
        }
        match event {
            Event::Challenge {
                id,
                serial,
                prompt,
                echo,
            } if id == self.id
                && serial > self.serial
                && self.phase != Phase::Finished
                && self.selected.is_some() =>
            {
                self.secret.clear();
                self.serial = serial;
                self.prompt = plain(&prompt, 256);
                self.phase = Phase::Prompt;
                // PAM echo-on answers may include OTPs. Support the conversation,
                // but deliberately mask every response: never put entered text in SHM.
                self.notice = if echo {
                    language
                        .text("Response is hidden", "Ответ скрыт", "Respuesta oculta")
                        .into()
                } else {
                    String::new()
                };
                self.error = false;
                self.details = false;
                self.scroll = 0;
            }
            Event::Notice { id, text, error } if id == self.id => {
                self.notice = plain(&text, 2048);
                self.error = error;
                if error {
                    self.secret.clear();
                }
            }
            Event::Retry { id } if id == self.id => {
                self.secret.clear();
                self.phase = Phase::Retry;
                self.error = true;
                self.notice = language
                    .text(
                        "Authentication failed",
                        "Ошибка авторизации",
                        "Autenticacion fallida",
                    )
                    .into();
            }
            Event::Closed {
                id,
                authorized: false,
            } if id == self.id => {
                self.secret.clear();
                self.phase = Phase::Finished;
                self.error = true;
                self.notice = language
                    .text(
                        "Authorization denied or cancelled",
                        "Доступ отклонён или отменён",
                        "Autorizacion denegada o cancelada",
                    )
                    .into();
            }
            _ => {}
        }
    }
    pub fn navigate(&mut self, down: bool) {
        if self.details {
            let last = self.detail_lines().len().saturating_sub(10);
            self.scroll = if down {
                (self.scroll + 1).min(last)
            } else {
                self.scroll.saturating_sub(1)
            };
        } else if self.phase == Phase::Choosing && !self.identities.is_empty() {
            self.selected = Some(match self.selected {
                None => 0,
                Some(i) if down => (i + 1).min(self.identities.len() - 1),
                Some(i) => i.saturating_sub(1),
            });
        }
    }
    pub fn insert(&mut self, value: &str) {
        if self.phase == Phase::Prompt && !self.details {
            for ch in value.chars() {
                self.secret.insert(ch);
            }
        }
    }
    pub fn backspace(&mut self) {
        if self.phase == Phase::Prompt && !self.details {
            self.secret.backspace();
        }
    }
    pub fn clear(&mut self) {
        self.secret.clear();
    }
    pub fn masked_len(&self) -> usize {
        self.secret.count()
    }
    pub fn enter(&mut self) -> Option<Command> {
        if self.details {
            self.details = false;
            return None;
        }
        match self.phase {
            Phase::Choosing => {
                let index = self.identities.get(self.selected?)?.index;
                self.phase = Phase::Waiting;
                Some(Command::Choose { id: self.id, index })
            }
            Phase::Prompt => {
                self.phase = Phase::Waiting;
                Some(Command::Respond {
                    id: self.id,
                    serial: self.serial,
                    secret: std::mem::take(&mut self.secret),
                })
            }
            Phase::Retry => {
                self.phase = Phase::Waiting;
                Some(Command::Retry { id: self.id })
            }
            _ => None,
        }
    }
    pub fn cancel(&mut self) -> Command {
        self.secret.clear();
        self.phase = Phase::Finished;
        Command::Cancel { id: self.id }
    }
    pub fn detail_lines(&self) -> Vec<String> {
        let mut lines = Vec::new();
        for value in [&self.action, &self.reason, &self.prompt, &self.notice] {
            let chars: Vec<char> = value.chars().collect();
            for chunk in chars.chunks(37) {
                lines.push(chunk.iter().collect());
            }
            lines.push(String::new());
        }
        for user in &self.identities {
            let chars: Vec<char> = user.label.chars().collect();
            for chunk in chars.chunks(37) {
                lines.push(chunk.iter().collect());
            }
        }
        lines
    }
}

#[cfg(test)]
#[path = "unit_tests/ui.rs"]
mod tests;
