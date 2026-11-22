//! Non-secret Shell dialog presentation. Actions belong exclusively to the
//! bottom softkey row; privileged authentication remains in the polkit agent.
use crate::{
    i18n::{Locale, ShellLabels},
    locking,
    model::{wrap_text, Dialog, SystemPhase, SystemRequest, WindowInfo},
    sleep,
    system::{Capability, Failure, Operation},
};

pub const MODAL_COLUMNS: usize = 34;
pub const MODAL_ROWS: usize = 6;
const MAX_CONTEXT_CHARS: usize = 4096;

#[derive(Debug, Clone)]
pub struct View {
    pub title: String,
    pub message: String,
    pub destructive: bool,
    pub information: bool,
}

impl View {
    pub fn for_dialog(
        dialog: &Dialog,
        locale: Locale,
        windows: &[WindowInfo],
        count: usize,
    ) -> Self {
        match dialog {
            Dialog::ConfirmClose(id) => {
                let title = windows
                    .iter()
                    .find(|window| window.id == *id)
                    .map(|window| window.title.as_str())
                    .filter(|title| !title.trim().is_empty())
                    .unwrap_or("?");
                Self {
                    title: locale.confirm_close().into(),
                    message: sanitize(title, MAX_CONTEXT_CHARS),
                    destructive: true,
                    information: false,
                }
            }
            Dialog::ConfirmClear => Self {
                title: locale.confirm_clear().into(),
                message: match locale {
                    Locale::Ru => format!("Записей: {count}"),
                    Locale::Es => format!("Elementos: {count}"),
                    Locale::En => format!("Entries: {count}"),
                },
                destructive: true,
                information: false,
            },
            Dialog::Info(kind) => Self {
                title: "Alcard".into(),
                message: sanitize(locale.info(*kind), MAX_CONTEXT_CHARS),
                destructive: false,
                information: true,
            },
        }
    }

    pub fn for_retry(locale: Locale, name: &str) -> Self {
        Self {
            title: locale.confirm_retry().into(),
            message: format!(
                "{}\n\n{}",
                sanitize(name, MAX_CONTEXT_CHARS),
                locale.retry_explanation()
            ),
            destructive: false,
            information: false,
        }
    }

    pub fn for_system_confirm(locale: Locale, operation: Operation) -> Self {
        Self {
            title: locale.system_confirm(operation).into(),
            message: locale.system_warning(operation).into(),
            destructive: true,
            information: false,
        }
    }

    pub fn for_system_unavailable(
        locale: Locale,
        operation: Operation,
        capability: Capability,
    ) -> Self {
        Self {
            title: locale.system_operation(operation).into(),
            message: format!(
                "{}\n\n{}",
                locale.system_capability(capability),
                locale.system_refresh_hint()
            ),
            destructive: matches!(capability, Capability::Denied | Capability::Blocked),
            information: !matches!(capability, Capability::Denied | Capability::Blocked),
        }
    }

    pub fn for_system_status(locale: Locale, request: &SystemRequest) -> Self {
        let (message, warning) = match request.phase {
            SystemPhase::Pending => (locale.system_waiting(), false),
            SystemPhase::Accepted => (locale.system_accepted(), false),
            SystemPhase::Failed(failure) => (locale.system_failure(failure), true),
            SystemPhase::Uncertain => (locale.system_failure(Failure::Unknown), true),
        };
        Self {
            title: locale.system_operation(request.operation).into(),
            message: message.into(),
            destructive: warning,
            information: !warning,
        }
    }

    pub fn for_lock_status(locale: Locale, phase: locking::Phase) -> Self {
        let warning = matches!(phase, locking::Phase::Failed | locking::Phase::Uncertain);
        Self {
            title: locale.lock_title().into(),
            message: locale.lock_status(phase).into(),
            destructive: warning,
            information: !warning,
        }
    }

    pub fn for_suspend_confirm(locale: Locale) -> Self {
        Self {
            title: locale.suspend_confirm().into(),
            message: locale.suspend_warning().into(),
            destructive: false,
            information: false,
        }
    }

    pub fn for_suspend_unavailable(locale: Locale, snapshot: sleep::Snapshot) -> Self {
        let warning = matches!(
            snapshot.capability,
            Capability::Denied | Capability::Blocked | Capability::Authentication
        );
        Self {
            title: locale.suspend_title().into(),
            message: locale.suspend_unavailable(snapshot).into(),
            destructive: warning,
            information: !warning,
        }
    }

    pub fn for_suspend_status(
        locale: Locale,
        snapshot: sleep::Snapshot,
        progress: Option<sleep::Progress>,
    ) -> Self {
        let external = snapshot.monitor == sleep::MonitorState::Preparing
            && progress.is_none_or(|progress| !progress.busy());
        let warning = !external
            && progress.is_some_and(|progress| {
                matches!(progress.stage, sleep::Stage::Failed | sleep::Stage::Unknown)
            });
        let message = if external {
            // An external cycle may win before our own request was sent.
            // Its continuing preparation takes precedence over the retained
            // Cancelled/Failed/Ended result of an earlier local intention.
            locale.suspend_external()
        } else {
            match progress {
                Some(sleep::Progress {
                    stage: sleep::Stage::Failed,
                    failure,
                    ..
                }) => locale.suspend_failure(failure.unwrap_or(Failure::Unknown)),
                Some(progress) => locale.suspend_status(progress.stage),
                None => locale.suspend_unavailable(snapshot),
            }
        };
        Self {
            title: locale.suspend_title().into(),
            message: message.into(),
            destructive: warning,
            information: !warning,
        }
    }

    /// Model and renderer share these rows when clamping scroll, so a long
    /// target title cannot hide its distinguishing suffix.
    pub fn lines(&self) -> Vec<String> {
        wrap_text(&self.message, MODAL_COLUMNS)
    }
}

/// Bounded plain text, never a command or terminal escape. Retain full normal
/// titles, with an explicit ellipsis only beyond the defensive length limit.
fn sanitize(value: &str, limit: usize) -> String {
    let mut chars = value.chars().filter(|ch| !ch.is_control());
    let mut result: String = chars.by_ref().take(limit).collect();
    if chars.next().is_some() && limit > 0 {
        result.pop();
        result.push('…');
    }
    result
}

#[cfg(test)]
#[path = "unit_tests/modal.rs"]
mod tests;
