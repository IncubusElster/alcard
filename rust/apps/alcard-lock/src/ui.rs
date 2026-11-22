//! Locker presentation and attempt binding. Secrets never become render text.
use crate::{
    protocol::{plain, Command, Event, Failure},
    secret::Secret,
};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Language {
    #[default]
    En,
    Ru,
    Es,
}

impl Language {
    pub fn detect() -> Self {
        let value = std::env::var("LC_ALL")
            .ok()
            .filter(|value| !value.is_empty())
            .or_else(|| std::env::var("LANG").ok())
            .unwrap_or_default()
            .to_lowercase();
        if value.starts_with("ru") {
            Self::Ru
        } else if value.starts_with("es") {
            Self::Es
        } else {
            Self::En
        }
    }

    pub fn text(self, en: &'static str, ru: &'static str, es: &'static str) -> &'static str {
        match self {
            Self::En => en,
            Self::Ru => ru,
            Self::Es => es,
        }
    }

    pub fn failure(self, failure: Failure) -> &'static str {
        match failure {
            Failure::Denied => self.text(
                "Authentication failed. Try again.",
                "Пароль не принят. Попробуйте снова.",
                "No se pudo verificar. Inténtalo de nuevo.",
            ),
            Failure::Cancelled => self.text(
                "Session remains locked.",
                "Сеанс остаётся заблокирован.",
                "La sesión sigue bloqueada.",
            ),
            Failure::Timeout => self.text(
                "Verification timed out. Try again.",
                "Время проверки истекло. Повторите вход.",
                "La verificación tardó demasiado. Reintenta.",
            ),
            Failure::Unavailable => self.text(
                "Password verification is unavailable.",
                "Проверка пароля недоступна.",
                "La verificación no está disponible.",
            ),
            Failure::AccountDenied => self.text(
                "This account cannot be unlocked.",
                "Вход для этой учётной записи недоступен.",
                "No se permite acceder a esta cuenta.",
            ),
            Failure::IdentityChanged => self.text(
                "Account identity changed. Session stays locked.",
                "Пользователь изменился. Сеанс заблокирован.",
                "Cambió la identidad. La sesión sigue bloqueada.",
            ),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    Securing,
    Idle,
    Waiting,
    Prompt,
    Unavailable,
    Unlocking,
}

/// The keyboard may act only on a prompt actually committed on its surface.
/// Text edits do not change this anchor, so ordinary typing needs no frame wait.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct InputAnchor {
    pub phase: Phase,
    pub attempt: Option<u64>,
    pub serial: u64,
}

pub struct Form {
    pub owner: String,
    pub phase: Phase,
    pub prompt: String,
    pub notice: String,
    pub error: bool,
    pub caps_lock: bool,
    pub layout: u32,
    locked: bool,
    next_attempt: u64,
    attempt: Option<u64>,
    serial: u64,
    secret: Secret,
}

impl Default for Form {
    fn default() -> Self {
        Self {
            owner: String::new(),
            phase: Phase::Securing,
            prompt: String::new(),
            notice: String::new(),
            error: false,
            caps_lock: false,
            layout: 0,
            locked: false,
            next_attempt: 0,
            attempt: None,
            serial: 0,
            secret: Secret::default(),
        }
    }
}

impl Form {
    pub fn ready(&mut self, user: &str) {
        self.owner = plain(user, 128);
    }

    pub fn locked(&mut self) {
        if self.phase == Phase::Securing && !self.locked {
            self.locked = true;
            self.phase = Phase::Idle;
        }
    }

    pub fn anchor(&self) -> InputAnchor {
        InputAnchor {
            phase: self.phase,
            attempt: self.attempt,
            serial: self.serial,
        }
    }

    pub fn attempt(&self) -> Option<u64> {
        self.attempt
    }

    pub fn begin(&mut self) -> Option<Command> {
        if !self.locked || self.phase != Phase::Idle {
            return None;
        }
        let attempt = self.next_attempt.checked_add(1)?;
        self.next_attempt = attempt;
        self.attempt = Some(attempt);
        self.serial = 0;
        self.secret.clear();
        self.prompt.clear();
        self.notice.clear();
        self.error = false;
        self.phase = Phase::Waiting;
        Some(Command::Begin { attempt })
    }

    pub fn insert(&mut self, value: &str) {
        if self.phase == Phase::Prompt {
            for ch in value.chars() {
                self.secret.insert(ch);
            }
        }
    }

    pub fn backspace(&mut self) {
        if self.phase == Phase::Prompt {
            self.secret.backspace();
        }
    }

    pub fn clear(&mut self) {
        self.secret.clear();
    }
    pub fn masked_len(&self) -> usize {
        self.secret.count()
    }

    pub fn submit(&mut self) -> Option<Command> {
        if !self.locked || self.phase != Phase::Prompt {
            return None;
        }
        let attempt = self.attempt?;
        self.phase = Phase::Waiting;
        Some(Command::Respond {
            attempt,
            serial: self.serial,
            secret: std::mem::take(&mut self.secret),
        })
    }

    /// Escape and focus loss cancel only authentication. Neither can release
    /// the compositor lock; a late PAM success then has no matching attempt.
    pub fn cancel(&mut self) -> Option<Command> {
        self.secret.clear();
        if matches!(
            self.phase,
            Phase::Securing | Phase::Unavailable | Phase::Unlocking
        ) {
            return None;
        }
        self.phase = Phase::Idle;
        self.prompt.clear();
        self.notice.clear();
        self.error = false;
        self.attempt
            .take()
            .map(|attempt| Command::Cancel { attempt })
    }

    pub fn unavailable(&mut self, failure: Failure, language: Language) {
        self.secret.clear();
        self.attempt = None;
        self.phase = Phase::Unavailable;
        self.notice = language.failure(failure).into();
        self.error = true;
    }

    /// Returns the authenticated attempt only once. The GUI separately requires
    /// its own real locked event before it may send unlock_and_destroy.
    pub fn receive(&mut self, event: Event, language: Language) -> Option<u64> {
        if !self.locked
            || matches!(
                self.phase,
                Phase::Securing | Phase::Unavailable | Phase::Unlocking
            )
        {
            return None;
        }
        match event {
            Event::Challenge {
                attempt,
                serial,
                prompt,
                echo,
            } if self.attempt == Some(attempt)
                && self.phase == Phase::Waiting
                && serial > self.serial =>
            {
                self.secret.clear();
                self.serial = serial;
                self.prompt = plain(&prompt, 256);
                self.notice = if echo {
                    language
                        .text("Response is hidden", "Ответ скрыт", "Respuesta oculta")
                        .into()
                } else {
                    String::new()
                };
                self.error = false;
                self.phase = Phase::Prompt;
            }
            Event::Notice {
                attempt,
                text,
                error,
            } if self.attempt == Some(attempt) => {
                self.notice = plain(&text, 512);
                self.error = error;
                if error {
                    self.secret.clear();
                }
            }
            Event::Finished { attempt, result } if self.attempt == Some(attempt) => {
                self.secret.clear();
                match result {
                    Ok(()) if self.phase == Phase::Waiting => {
                        self.phase = Phase::Unlocking;
                        return Some(attempt);
                    }
                    Ok(()) => {} // A success cannot complete an unanswered challenge.
                    Err(failure) => {
                        self.attempt = None;
                        self.phase = Phase::Idle;
                        self.notice = language.failure(failure).into();
                        self.error = failure != Failure::Cancelled;
                    }
                }
            }
            _ => {}
        }
        None
    }
}

#[cfg(test)]
#[path = "unit_tests/ui.rs"]
mod tests;
