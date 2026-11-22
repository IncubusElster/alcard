//! Quick-setting rows and their bounded detail/status presentation.

use super::{
    abbreviated, chrome, font, modal_view, radios, rect, scroll_indicator, sleep, Availability,
    Capability, Control, Pixmap, QuickAction, Shell, SystemView, View, SOFTKEY_TOP, WIDTH,
};
use crate::i18n::ShellLabels;

pub(super) fn system_view(canvas: &mut Pixmap, s: &Shell, view: &SystemView) {
    match view {
        SystemView::Menu { selected } => quick_menu(canvas, s, *selected),
        SystemView::Confirm { operation, scroll } => {
            modal_view(
                canvas,
                s,
                &View::for_system_confirm(s.locale, *operation),
                *scroll,
            );
        }
        SystemView::Unavailable {
            operation,
            capability,
            scroll,
        } => {
            modal_view(
                canvas,
                s,
                &View::for_system_unavailable(s.locale, *operation, *capability),
                *scroll,
            );
        }
        SystemView::Status { operation, scroll } => {
            if let Some(request) = s
                .system_request
                .as_ref()
                .filter(|request| request.operation == *operation)
            {
                modal_view(
                    canvas,
                    s,
                    &View::for_system_status(s.locale, request),
                    *scroll,
                );
            }
        }
        SystemView::LockStatus { scroll } => {
            if let Some(state) = s.lock_state {
                modal_view(
                    canvas,
                    s,
                    &View::for_lock_status(s.locale, state.phase),
                    *scroll,
                );
            }
        }
        SystemView::RadioList { kind, selected } => radio_list(canvas, s, *kind, *selected),
        SystemView::RadioDetails {
            kind,
            entry,
            scroll,
        } => {
            let view = View {
                title: s.locale.radio_details_title(*kind).into(),
                message: s.locale.radio_details_text(entry),
                destructive: false,
                information: true,
            };
            modal_view(canvas, s, &view, *scroll);
        }
        SystemView::SuspendConfirm { scroll } => {
            modal_view(canvas, s, &View::for_suspend_confirm(s.locale), *scroll);
        }
        SystemView::SuspendUnavailable { scroll } => {
            modal_view(
                canvas,
                s,
                &View::for_suspend_unavailable(s.locale, s.sleep_snapshot),
                *scroll,
            );
        }
        SystemView::SuspendStatus { scroll } => {
            modal_view(
                canvas,
                s,
                &View::for_suspend_status(s.locale, s.sleep_snapshot, s.sleep_progress),
                *scroll,
            );
        }
    }
}

pub(super) fn quick_menu(canvas: &mut Pixmap, s: &Shell, selected: usize) {
    let t = &s.theme;
    rect(canvas, 0, 18, WIDTH as i32, SOFTKEY_TOP - 18, t.background);
    font::text(canvas, 16, 22, s.locale.quick_title(), t.accent, 36);
    let selected = selected.min(QuickAction::ALL.len() - 1);
    let first = selected.saturating_sub(3);
    for (index, action) in QuickAction::ALL.into_iter().enumerate().skip(first).take(4) {
        let y = 36 + (index - first) as i32 * 27;
        if index == selected {
            chrome::selection(canvas, (8, y, 300, 25), t);
        } else {
            chrome::rounded(canvas, (8, y, 300, 25), 2, t.surface);
        }
        match action {
            QuickAction::Control(control) => quick_control(canvas, s, control, y),
            QuickAction::Radio(kind) => quick_radio(canvas, s, kind, y),
            _ => {
                let (title, hint, capability) = match action {
                    QuickAction::System(operation) => {
                        let capability = s.system_snapshot.capability(operation);
                        (
                            s.locale.system_operation(operation),
                            s.locale.system_capability(capability),
                            capability,
                        )
                    }
                    QuickAction::Lock => (
                        s.locale.lock_title(),
                        s.locale.lock_hint(),
                        Capability::Authentication,
                    ),
                    QuickAction::Suspend => (
                        s.locale.suspend_title(),
                        s.locale.suspend_hint(s.sleep_snapshot),
                        match s.sleep_snapshot.monitor {
                            sleep::MonitorState::Connecting => Capability::Checking,
                            sleep::MonitorState::Preparing => Capability::Blocked,
                            sleep::MonitorState::Unavailable => Capability::Unavailable,
                            sleep::MonitorState::Ready
                                if s.sleep_snapshot.capability == Capability::Authentication =>
                            {
                                Capability::Denied
                            }
                            sleep::MonitorState::Ready => s.sleep_snapshot.capability,
                        },
                    ),
                    QuickAction::Control(_) | QuickAction::Radio(_) => unreachable!(),
                };
                let color = match capability {
                    Capability::Allowed => t.success,
                    Capability::Authentication => t.accent,
                    Capability::Denied | Capability::Blocked => t.warning,
                    Capability::Checking | Capability::Unsupported | Capability::Unavailable => {
                        t.muted
                    }
                };
                font::text(canvas, 16, y + 3, title, t.text, 35);
                font::text(canvas, 16, y + 15, hint, color, 35);
            }
        }
    }
    scroll_indicator(canvas, 36, 106, first, 4, QuickAction::ALL.len(), t);
    if matches!(QuickAction::ALL[selected], QuickAction::Control(_)) {
        font::text(
            canvas,
            16,
            145,
            s.locale.controls_adjust_hint(),
            t.muted,
            36,
        );
    }
}

pub(super) fn quick_control(canvas: &mut Pixmap, s: &Shell, control: Control, y: i32) {
    let t = &s.theme;
    let reading = s.control_reading(control);
    font::text(
        canvas,
        16,
        y + 3,
        s.locale.control_title(control),
        t.text,
        24,
    );
    // Keep the last confirmed value visible during a change. A higher external
    // gain must not look like exactly 100%; unknown readings have no percentage.
    let value = if reading.above_safe_limit {
        "100%+".to_string()
    } else {
        reading
            .percent
            .map_or_else(|| "--".into(), |percent| format!("{percent}%"))
    };
    let value_color = match reading.availability {
        Availability::Available if reading.above_safe_limit => t.warning,
        Availability::Available => t.accent,
        Availability::Denied => t.warning,
        Availability::Checking | Availability::Unavailable => t.muted,
    };
    font::text(canvas, 252, y + 3, &value, value_color, 6);
    let pending = s
        .controls_request
        .filter(|request| match request.operation {
            None => true,
            Some(crate::controls::Operation::Adjust {
                control: target, ..
            }) => target == control,
            Some(crate::controls::Operation::ToggleMute) => control == Control::Volume,
        });
    let (hint, color) = if let Some(request) = pending {
        (
            s.locale.controls_pending(request.operation.is_none()),
            t.muted,
        )
    } else if let Some(failure) = s.control_failure(control) {
        (s.locale.controls_failure(failure), t.warning)
    } else {
        (s.locale.control_hint(control, reading), value_color)
    };
    font::text(canvas, 16, y + 15, &abbreviated(hint, 35), color, 35);
}

pub(super) fn radio_request_applies(s: &Shell, kind: radios::Kind) -> Option<radios::Operation> {
    s.radios_request
        .map(|request| request.operation)
        .filter(|operation| match operation {
            radios::Operation::Refresh => true,
            radios::Operation::SetPowered { kind: target, .. }
            | radios::Operation::List { kind: target } => *target == kind,
        })
}

pub(super) fn quick_radio(canvas: &mut Pixmap, s: &Shell, kind: radios::Kind, y: i32) {
    let t = &s.theme;
    let state = s.radio_state(kind);
    font::text(canvas, 16, y + 3, s.locale.radio_title(kind), t.text, 24);
    let color = match state.availability {
        radios::Availability::Available if state.powered == Some(true) => t.success,
        radios::Availability::Blocked => t.warning,
        _ => t.muted,
    };
    font::text(canvas, 252, y + 3, s.locale.radio_value(state), color, 6);
    let (hint, color) = if let Some(operation) = radio_request_applies(s, kind) {
        (s.locale.radio_pending(operation).to_string(), t.muted)
    } else if let Some(failure) = s.radio_failure(kind) {
        (s.locale.radio_failure(failure).to_string(), t.warning)
    } else if state.availability == radios::Availability::Available
        && state.powered == Some(true)
        && !state.detail.trim().is_empty()
    {
        (state.detail.clone(), color)
    } else {
        (s.locale.radio_state(state).to_string(), color)
    };
    font::text(canvas, 16, y + 15, &abbreviated(&hint, 35), color, 35);
}

pub(super) fn radio_list(canvas: &mut Pixmap, s: &Shell, kind: radios::Kind, selected: usize) {
    let t = &s.theme;
    rect(canvas, 0, 18, WIDTH as i32, SOFTKEY_TOP - 18, t.background);
    font::text(
        canvas,
        16,
        22,
        s.locale.radio_list_title(kind),
        t.accent,
        36,
    );
    let pending = radio_request_applies(s, kind);
    if s.radio_entries.is_empty() {
        let state = s.radio_state(kind);
        let message = if let Some(operation) = pending {
            s.locale.radio_pending(operation)
        } else if let Some(failure) = s.radio_failure(kind) {
            s.locale.radio_failure(failure)
        } else if state.availability != radios::Availability::Available
            || state.powered != Some(true)
        {
            s.locale.radio_state(state)
        } else {
            s.locale.radio_empty(kind)
        };
        font::text(canvas, 16, 56, message, t.muted, 35);
    } else {
        let selected = selected.min(s.radio_entries.len() - 1);
        let first = selected.saturating_sub(3);
        for (index, entry) in s.radio_entries.iter().enumerate().skip(first).take(4) {
            let y = 36 + (index - first) as i32 * 27;
            if index == selected {
                chrome::selection(canvas, (8, y, 300, 25), t);
            } else {
                chrome::rounded(canvas, (8, y, 300, 25), 2, t.surface);
            }
            let title = if entry.name.trim().is_empty() {
                &entry.id
            } else {
                &entry.name
            };
            font::text(canvas, 16, y + 3, &abbreviated(title, 35), t.text, 35);
            font::text(
                canvas,
                16,
                y + 15,
                &abbreviated(&entry.detail, 35),
                t.muted,
                35,
            );
        }
        scroll_indicator(canvas, 36, 106, first, 4, s.radio_entries.len(), t);
    }
    if !s.radio_entries.is_empty() {
        let (hint, color) = if let Some(operation) = pending {
            (s.locale.radio_pending(operation), t.muted)
        } else if let Some(failure) = s.radio_failure(kind) {
            (s.locale.radio_failure(failure), t.warning)
        } else {
            (s.locale.radio_list_hint(), t.muted)
        };
        font::text(canvas, 16, 145, &abbreviated(hint, 36), color, 36);
    }
}
