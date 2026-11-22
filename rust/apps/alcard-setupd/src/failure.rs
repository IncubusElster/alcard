//! Secret-free diagnostics: only our typed, allowlisted static causes and
//! fixed stage names may leave the backend. Never format an arbitrary error.
use std::{error::Error, fmt, io};

#[derive(Clone, Copy, Debug)]
pub(crate) enum Stage {
    Request,
    Validation,
    Guest,
    Runtime,
    AdminPreflight,
    Account,
    UserPassword,
    RootPassword,
    AdminPolicies,
    RootLock,
    Console,
    Session,
    Complete,
}

impl Stage {
    fn name(self) -> &'static str {
        match self {
            Self::Request => "request",
            Self::Validation => "validation",
            Self::Guest => "guest",
            Self::Runtime => "runtime_preflight",
            Self::AdminPreflight => "admin_preflight",
            Self::Account => "account",
            Self::UserPassword => "user_password",
            Self::RootPassword => "root_password",
            Self::AdminPolicies => "admin_policies",
            Self::RootLock => "root_lock",
            Self::Console => "console_configuration",
            Self::Session => "session_configuration",
            Self::Complete => "completion",
        }
    }
}

#[derive(Debug)]
struct Failure {
    code: &'static str,
    stage: Option<Stage>,
}

impl fmt::Display for Failure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("firstboot operation failed")
    }
}

impl Error for Failure {}

pub(crate) fn invalid(code: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, Failure { code, stage: None })
}

fn typed(error: &io::Error) -> Option<&Failure> {
    error.get_ref()?.downcast_ref::<Failure>()
}

pub(crate) fn cause(error: &io::Error) -> &'static str {
    let Some(failure) = typed(error) else {
        return "system_io";
    };
    match failure.code {
        "invalid_plan"
        | "invalid_user_password"
        | "invalid_root_password"
        | "invalid_root_confirmation"
        | "invalid_password_character"
        | "invalid_firstboot_file_permissions"
        | "alpine_guest_required"
        | "invalid_firstboot_directory"
        | "firstboot_marker_invalid"
        | "already_completed"
        | "root_shadow_missing"
        | "root_shadow_ambiguous"
        | "root_lock_not_applied"
        | "existing_human_user"
        | "usable_account_already_present"
        | "system_command_failed"
        | "password_pipe_unavailable"
        | "password_pipe_failed"
        | "password_change_failed"
        | "invalid_output_path"
        | "different_firstboot_account"
        | "missing_progress_account"
        | "existing_unmanaged_user"
        | "original_us_required"
        | "selected_alcard_keymap_unavailable"
        | "alcard_keyboard_package_required"
        | "selected_keymap_unavailable"
        | "loadkmap_service_unavailable"
        | "timezone_not_installed"
        | "ntp_service_required"
        | "console_package_cleanup_failed"
        | "graphical_package_still_installed"
        | "unsafe_session_directory"
        | "no_admin_path"
        | "request_too_large"
        | "invalid_apply_request"
        | "invalid_admin_policy_directory"
        | "invalid_admin_policy_file"
        | "existing_admin_policy_conflict"
        | "admin_policy_changed_during_apply"
        | "invalid_policy_path"
        | "polkit_cannot_read_admin_policy"
        | "doas_package_required"
        | "doas_config_untrusted"
        | "doas_config_invalid"
        | "doas_check_timeout"
        | "doas_check_output_invalid"
        | "doas_account_invalid"
        | "doas_groups_unavailable"
        | "doas_route_unavailable"
        | "system_io" => failure.code,
        _ => "unclassified_failure",
    }
}

pub(crate) fn stage(error: &io::Error) -> &'static str {
    typed(error)
        .and_then(|failure| failure.stage)
        .map_or("unspecified", Stage::name)
}

pub(crate) fn at<T>(stage: Stage, result: io::Result<T>) -> io::Result<T> {
    result.map_err(|error| {
        io::Error::new(
            error.kind(),
            Failure {
                code: cause(&error),
                stage: Some(stage),
            },
        )
    })
}

#[cfg(test)]
#[path = "unit_tests/failure.rs"]
mod tests;
