//! Sensitive firstboot credentials, kept exclusively in zeroizing RAM buffers.
//! Do not derive Debug, Clone, Serialize, or expose plaintext getters.
use zeroize::Zeroizing;

/// Only the UI's account controls can write these fields.
#[derive(Clone, Copy, Eq, PartialEq)]
pub(crate) enum Slot {
    UserPassword,
    UserConfirm,
    RootPassword,
    RootConfirm,
}

#[derive(Clone, Copy, Eq, PartialEq)]
pub(crate) enum Invalid {
    UserMissing,
    UserTooShort,
    UserMismatch,
    RootTooShort,
    RootMismatch,
    NoAdministrator,
}

/// All four strings are explicitly zeroed on drop (including cancellation).
/// No secret is persisted or passed to the JSON-only preflight process.
pub(crate) struct Passwords {
    user: Zeroizing<String>,
    user_confirm: Zeroizing<String>,
    root: Zeroizing<String>,
    root_confirm: Zeroizing<String>,
}

impl Default for Passwords {
    fn default() -> Self {
        Self {
            user: Zeroizing::new(String::with_capacity(256)),
            user_confirm: Zeroizing::new(String::with_capacity(256)),
            root: Zeroizing::new(String::with_capacity(256)),
            root_confirm: Zeroizing::new(String::with_capacity(256)),
        }
    }
}

impl Passwords {
    pub(crate) fn set(&mut self, slot: Slot, value: Zeroizing<String>) {
        match slot {
            Slot::UserPassword => {
                // Reopening and committing an unchanged password must not
                // unexpectedly invalidate its previously valid confirmation.
                if self.user.as_str() != value.as_str() {
                    self.user_confirm = Zeroizing::new(String::with_capacity(256));
                }
                self.user = value;
            }
            Slot::UserConfirm => self.user_confirm = value,
            Slot::RootPassword => {
                if self.root.as_str() != value.as_str() {
                    self.root_confirm = Zeroizing::new(String::with_capacity(256));
                }
                self.root = value;
            }
            Slot::RootConfirm => self.root_confirm = value,
        }
    }

    /// Create a zeroizing edit buffer containing the existing value.
    /// Cancellation drops and wipes only this temporary copy.
    pub(crate) fn copy_for_edit(&self, slot: Slot) -> Zeroizing<String> {
        let source = self.borrow_for_display(slot);
        let mut text = Zeroizing::new(String::with_capacity(source.len().max(256)));
        text.push_str(source);
        text
    }

    pub(crate) fn has_root(&self) -> bool {
        !self.root.is_empty()
    }

    /// Borrow plaintext only for explicit on-screen reveal; never allocate a copy.
    pub(crate) fn borrow_for_display(&self, slot: Slot) -> &str {
        match slot {
            Slot::UserPassword => self.user.as_str(),
            Slot::UserConfirm => self.user_confirm.as_str(),
            Slot::RootPassword => self.root.as_str(),
            Slot::RootConfirm => self.root_confirm.as_str(),
        }
    }

    /// Generate only mask characters; never copy plaintext into the UI model.
    /// The count is Unicode scalar values, not UTF-8 bytes.
    pub(crate) fn masked(&self, slot: Slot) -> String {
        let count = match slot {
            Slot::UserPassword => self.user.chars().count(),
            Slot::UserConfirm => self.user_confirm.chars().count(),
            Slot::RootPassword => self.root.chars().count(),
            Slot::RootConfirm => self.root_confirm.chars().count(),
        };
        "*".repeat(count)
    }

    pub(crate) fn validate(&self, doas_enabled: bool) -> Result<(), Invalid> {
        if self.user.is_empty() {
            return Err(Invalid::UserMissing);
        }
        if self.user.chars().count() < 8 {
            return Err(Invalid::UserTooShort);
        }
        if self.user.as_str() != self.user_confirm.as_str() {
            return Err(Invalid::UserMismatch);
        }
        if !self.root.is_empty() {
            if self.root.chars().count() < 8 {
                return Err(Invalid::RootTooShort);
            }
            if self.root.as_str() != self.root_confirm.as_str() {
                return Err(Invalid::RootMismatch);
            }
        } else if !self.root_confirm.is_empty() {
            return Err(Invalid::RootMismatch);
        }
        if !doas_enabled && self.root.is_empty() {
            return Err(Invalid::NoAdministrator);
        }
        Ok(())
    }

    #[cfg(test)]
    fn new_password(secret: &str) -> Zeroizing<String> {
        Zeroizing::new(secret.to_owned())
    }
}

#[cfg(test)]
#[path = "unit_tests/secrets.rs"]
mod tests;
