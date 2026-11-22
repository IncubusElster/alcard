use super::*;
use crate::model::InfoKind;

fn win(id: u64, title: &str) -> WindowInfo {
    WindowInfo {
        id,
        title: title.into(),
        app_id: "org.example.App".into(),
        active: true,
        activity: 1,
    }
}

#[test]
fn close_dialog_keeps_the_exact_target_and_full_wrapped_suffix() {
    let title = format!("{} UNIQUE-TARGET-END", "Длинное имя aplicación ".repeat(20));
    let view = View::for_dialog(
        &Dialog::ConfirmClose(22),
        Locale::En,
        &[win(11, "Do not close"), win(22, &title)],
        0,
    );
    assert!(view.title.contains("Close"));
    assert_eq!(view.message, title);
    assert!(view.destructive);
    assert!(!view.information);
    let lines = view.lines();
    assert!(lines.len() > MODAL_ROWS);
    assert!(lines.last().unwrap().ends_with("UNIQUE-TARGET-END"));
    assert!(lines
        .iter()
        .all(|line| line.chars().count() <= MODAL_COLUMNS));
}

#[test]
fn controls_are_plain_text_and_length_limit_is_explicit() {
    let view = View::for_dialog(
        &Dialog::ConfirmClose(1),
        Locale::En,
        &[win(1, "Target\n\u{1b}[31m")],
        0,
    );
    assert_eq!(view.message, "Target[31m");
    let value = sanitize(&"ñ".repeat(MAX_CONTEXT_CHARS + 100), MAX_CONTEXT_CHARS);
    assert_eq!(value.chars().count(), MAX_CONTEXT_CHARS);
    assert!(value.ends_with('…'));
    let info = View::for_dialog(&Dialog::Info(InfoKind::Unsupported), Locale::En, &[], 0);
    assert!(info.information);
    assert!(!info.destructive);
}

#[test]
fn clear_counts_records_and_retry_identifies_app_and_risk() {
    let view = View::for_dialog(&Dialog::ConfirmClear, Locale::Ru, &[], 64);
    assert_eq!(view.message, "Записей: 64");
    for locale in [Locale::En, Locale::Ru, Locale::Es] {
        let view = View::for_retry(locale, "Exact application name");
        assert!(view.message.starts_with("Exact application name\n\n"));
        assert!(view.message.ends_with(locale.retry_explanation()));
        assert!(view
            .lines()
            .iter()
            .all(|line| line.chars().count() <= MODAL_COLUMNS));
    }
}

#[test]
fn system_confirmations_keep_the_complete_unsaved_work_warning_visible() {
    for locale in [Locale::En, Locale::Ru, Locale::Es] {
        for operation in Operation::ALL {
            let view = View::for_system_confirm(locale, operation);
            let lines = view.lines();
            assert!(view.destructive && !view.information);
            assert!(lines.len() <= MODAL_ROWS);
            assert!(lines
                .iter()
                .all(|line| line.chars().count() <= MODAL_COLUMNS));
            let warning = match locale {
                Locale::En => "unsaved changes",
                Locale::Ru => "несохранённые данные",
                Locale::Es => "cambios sin guardar",
            };
            assert!(view.message.contains(warning));
        }
    }
}

#[test]
fn system_status_distinguishes_acceptance_and_unknown_result_from_failure_before_send() {
    let mut request = SystemRequest {
        id: 9,
        operation: Operation::Reboot,
        phase: SystemPhase::Pending,
    };
    let pending = View::for_system_status(Locale::En, &request);
    request.phase = SystemPhase::Accepted;
    let accepted = View::for_system_status(Locale::En, &request);
    request.phase = SystemPhase::Failed(Failure::Timeout);
    let before_send = View::for_system_status(Locale::En, &request);
    request.phase = SystemPhase::Uncertain;
    let unknown = View::for_system_status(Locale::En, &request);
    assert!(pending.message.contains("Waiting"));
    assert!(accepted.message.contains("accepted"));
    assert!(accepted.message.contains("not yet been confirmed"));
    assert!(before_send.message.contains("was not sent"));
    assert!(unknown.message.contains("unknown"));
    assert_ne!(before_send.message, unknown.message);
    assert!(pending.information && accepted.information);
    assert!(before_send.destructive && unknown.destructive);
}

#[test]
fn suspend_explanations_fit_the_native_modal_without_hiding_the_last_line() {
    for locale in [Locale::En, Locale::Ru, Locale::Es] {
        let mut views = vec![View::for_suspend_confirm(locale)];
        for monitor in [
            sleep::MonitorState::Connecting,
            sleep::MonitorState::Ready,
            sleep::MonitorState::Preparing,
            sleep::MonitorState::Unavailable,
        ] {
            for capability in [
                Capability::Checking,
                Capability::Allowed,
                Capability::Authentication,
                Capability::Denied,
                Capability::Unsupported,
                Capability::Blocked,
                Capability::Unavailable,
            ] {
                views.push(View::for_suspend_unavailable(
                    locale,
                    sleep::Snapshot {
                        epoch: 1,
                        capability,
                        monitor,
                        delay_ms: 5_000,
                    },
                ));
            }
        }
        for stage in [
            sleep::Stage::Preparing,
            sleep::Stage::Locking,
            sleep::Stage::Queued,
            sleep::Stage::Requested,
            sleep::Stage::Ended,
            sleep::Stage::Failed,
            sleep::Stage::Unknown,
            sleep::Stage::Cancelled,
        ] {
            views.push(View::for_suspend_status(
                locale,
                sleep::Snapshot::default(),
                Some(sleep::Progress {
                    request: 1,
                    stage,
                    failure: None,
                }),
            ));
        }
        for failure in [
            Failure::Unavailable,
            Failure::Denied,
            Failure::Cancelled,
            Failure::Blocked,
            Failure::SessionUnavailable,
            Failure::Timeout,
            Failure::Unknown,
        ] {
            views.push(View::for_suspend_status(
                locale,
                sleep::Snapshot::default(),
                Some(sleep::Progress {
                    request: 1,
                    stage: sleep::Stage::Failed,
                    failure: Some(failure),
                }),
            ));
        }
        for view in views {
            assert!(view.title.chars().count() <= 31);
            let lines = view.lines();
            assert!(
                !lines.is_empty() && lines.len() <= MODAL_ROWS,
                "{}: {:?}",
                locale.code(),
                view.message
            );
            assert!(lines
                .iter()
                .all(|line| line.chars().count() <= MODAL_COLUMNS));
        }
    }
}

#[test]
fn suspend_cycle_end_and_recovered_unknown_do_not_claim_sleep_succeeded() {
    let view = |stage, failure| {
        View::for_suspend_status(
            Locale::En,
            sleep::Snapshot::default(),
            Some(sleep::Progress {
                request: 2,
                stage,
                failure,
            }),
        )
    };
    let requested = view(sleep::Stage::Requested, None);
    let ended = view(sleep::Stage::Ended, None);
    let timeout = view(sleep::Stage::Failed, Some(Failure::Timeout));
    let uncertain = view(sleep::Stage::Unknown, Some(Failure::Unknown));
    let recovered = view(sleep::Stage::Failed, Some(Failure::Unknown));
    assert!(requested.message.contains("has not yet ended"));
    assert!(ended.message.contains("does not confirm the device slept"));
    assert!(ended.message.contains("not unlocked automatically"));
    assert!(timeout.message.contains("No sleep request was sent"));
    assert!(uncertain.message.contains("unknown"));
    assert_eq!(recovered.message, uncertain.message);
    assert!(!recovered.message.contains("No sleep request was sent"));
    assert!(recovered.destructive && uncertain.destructive);
    assert_ne!(ended.message, requested.message);
    let challenge = View::for_suspend_unavailable(
        Locale::En,
        sleep::Snapshot {
            epoch: 1,
            capability: Capability::Authentication,
            monitor: sleep::MonitorState::Ready,
            delay_ms: 5_000,
        },
    );
    assert!(challenge
        .message
        .contains("cannot ask for a password after locking"));
}

#[test]
fn external_sleep_in_progress_is_not_hidden_by_a_terminal_own_result() {
    for locale in [Locale::En, Locale::Ru, Locale::Es] {
        let snapshot = sleep::Snapshot {
            epoch: 1,
            capability: Capability::Allowed,
            monitor: sleep::MonitorState::Preparing,
            delay_ms: 5_000,
        };
        for stage in [
            None,
            Some(sleep::Stage::Cancelled),
            Some(sleep::Stage::Failed),
            Some(sleep::Stage::Ended),
        ] {
            let progress = stage.map(|stage| sleep::Progress {
                request: 1,
                stage,
                failure: (stage == sleep::Stage::Failed).then_some(Failure::Unknown),
            });
            let view = View::for_suspend_status(locale, snapshot, progress);
            assert_eq!(view.message, locale.suspend_external());
            assert!(view.lines().len() <= MODAL_ROWS);
            if let Some(progress) = progress {
                let restored = View::for_suspend_status(
                    locale,
                    sleep::Snapshot {
                        monitor: sleep::MonitorState::Ready,
                        ..snapshot
                    },
                    Some(progress),
                );
                assert_ne!(restored.message, view.message);
            }
        }
        let requested = View::for_suspend_status(
            locale,
            snapshot,
            Some(sleep::Progress {
                request: 1,
                stage: sleep::Stage::Requested,
                failure: None,
            }),
        );
        assert_eq!(
            requested.message,
            locale.suspend_status(sleep::Stage::Requested)
        );
    }
}
