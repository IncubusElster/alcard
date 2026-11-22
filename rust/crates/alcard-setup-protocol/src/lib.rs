//! Strict, non-secret firstboot plan shared by Hello and the setup backend.
//! This plan excludes password fields. Hello sends credentials separately in the
//! firstboot request over stdin to the root-gated local setup backend.
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
pub mod session_runtime;

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SetupPlan {
    pub schema_version: u8,
    pub language: Language,
    pub account: AccountPlan,
    pub keyboard: KeyboardPlan,
    pub session: Session,
    #[serde(default = "default_timezone")]
    pub timezone: String,
    #[serde(default)]
    pub ntp_enabled: bool,
}

fn default_timezone() -> String {
    "Etc/UTC".to_owned()
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Language {
    En,
    Ru,
    Es,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AccountPlan {
    pub username: String,
    pub display_name: String,
    pub doas_enabled: bool,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct KeyboardPlan {
    /// US is mandatory and first; other layouts are optional.
    pub layouts: Vec<Layout>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Layout {
    Us,
    Ru,
    Es,
    De,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Session {
    Console,
    Graphics,
    /// The original `graphical` plan launched labwc + Foot. Keep that meaning
    /// when reading an old saved plan instead of silently opting it into Shell.
    #[serde(alias = "graphical")]
    GraphicsMinimal,
}

impl Session {
    pub const ALL: [Self; 3] = [Self::Graphics, Self::GraphicsMinimal, Self::Console];

    /// Existing installed `labwc` selections keep the diagnostic Foot session.
    pub const fn installed_name(self) -> &'static str {
        match self {
            Self::Graphics => "alcard-shell",
            Self::GraphicsMinimal => "labwc",
            Self::Console => "console",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CheckError {
    UnsupportedSchema,
    InvalidUsername,
    DisplayNameTooLong,
    MissingKeyboardLayout,
    TooManyKeyboardLayouts,
    DuplicateKeyboardLayout,
    UsKeyboardRequired,
    AdminAuthorizationUnverified,
    InvalidTimezone,
}

pub fn is_valid_username(username: &str) -> bool {
    if username.is_empty() || username.len() > 32 {
        return false;
    }
    let mut chars = username.chars();
    let first = chars.next().expect("nonempty");
    (first.is_ascii_lowercase() || first == '_')
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-')
}

/// Timezone names must be relative, canonical zoneinfo identifiers.
pub fn is_valid_timezone(name: &str) -> bool {
    if name.is_empty() || name.len() > 96 || !name.is_ascii() {
        return false;
    }
    name.split('/').all(|component| {
        !component.is_empty()
            && component != "."
            && component != ".."
            && component
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-' || b == b'+')
    })
}

/// Firstboot validation requires a verified admin route (doas or root secret).
/// This variant may only be called after validating the secret fields.
pub fn validate_for_firstboot(
    plan: &SetupPlan,
    root_password_provided: bool,
) -> Result<(), CheckError> {
    validate_common(plan)?;
    if !plan.account.doas_enabled && !root_password_provided {
        return Err(CheckError::AdminAuthorizationUnverified);
    }
    Ok(())
}

fn validate_common(plan: &SetupPlan) -> Result<(), CheckError> {
    if plan.schema_version != 1 {
        return Err(CheckError::UnsupportedSchema);
    }
    if !is_valid_username(&plan.account.username) {
        return Err(CheckError::InvalidUsername);
    }
    if plan.account.display_name.chars().count() > 64
        || plan
            .account
            .display_name
            .chars()
            .any(|c| c == ':' || c.is_control())
    {
        return Err(CheckError::DisplayNameTooLong);
    }
    if plan.keyboard.layouts.is_empty() {
        return Err(CheckError::MissingKeyboardLayout);
    }
    if plan.keyboard.layouts.first() != Some(&Layout::Us) {
        return Err(CheckError::UsKeyboardRequired);
    }
    if plan.keyboard.layouts.len() > Layout::ALL.len() {
        return Err(CheckError::TooManyKeyboardLayouts);
    }
    let mut seen = HashSet::new();
    if !plan
        .keyboard
        .layouts
        .iter()
        .all(|value| seen.insert(*value))
    {
        return Err(CheckError::DuplicateKeyboardLayout);
    }
    if !is_valid_timezone(&plan.timezone) {
        return Err(CheckError::InvalidTimezone);
    }
    // The protocol accepts all three sessions; the root-side backend must verify
    // that selected graphical packages actually exist before applying.
    Ok(())
}

/// Public JSON preflight remains conservative: without credentials a root-only
/// administrative path cannot be verified.
pub fn validate(plan: &SetupPlan) -> Result<(), CheckError> {
    validate_common(plan)?;
    if !plan.account.doas_enabled {
        return Err(CheckError::AdminAuthorizationUnverified);
    }
    Ok(())
}

impl Layout {
    pub const ALL: [Self; 4] = [Self::Us, Self::Ru, Self::Es, Self::De];
}

#[cfg(test)]
#[path = "unit_tests/lib.rs"]
mod tests;
