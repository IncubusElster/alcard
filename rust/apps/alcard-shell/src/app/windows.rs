//! Application launch tracking and stable compositor-window operations.

use super::{
    desktop, launch, launch_reason, localized, Application, Instant, LaunchPhase, LaunchStatus,
    PendingWindowOperation, Request, WindowInfo, WindowOperation, WindowOperationStatus,
    WINDOW_RESPONSE_TIMEOUT,
};

impl Application {
    pub(super) fn resolved_windows(&self) -> Vec<(u64, String)> {
        self.state
            .live_windows
            .iter()
            .map(|window| {
                (
                    window.id,
                    self.state
                        .resolve_app(&window.app_id)
                        .map(|app| app.desktop_id.clone())
                        .unwrap_or_default(),
                )
            })
            .collect()
    }

    pub(super) fn launch(&mut self, desktop_id: &str, retry: Option<u64>) -> Request {
        let old_name = self
            .state
            .applications
            .iter()
            .find(|app| app.desktop_id == desktop_id)
            .map(|app| app.name.clone())
            .unwrap_or_else(|| desktop_id.to_owned());
        self.refresh_applications();
        let windows = self.resolved_windows();
        let app = self.desktop_apps.iter().find(|app| app.id == desktop_id);
        let name = app.map(|app| app.name.as_str()).unwrap_or(&old_name);
        let spawn = || {
            app.ok_or(desktop::LaunchError::MissingDesktopEntry)
                .and_then(desktop::DesktopApp::launch)
        };
        let outcome = match retry {
            Some(attempt) => {
                self.launches
                    .retry(desktop_id, name, attempt, &windows, Instant::now(), spawn)
            }
            None => self
                .launches
                .start(desktop_id, name, &windows, Instant::now(), spawn),
        };
        match outcome {
            launch::Outcome::Started(attempt) | launch::Outcome::Suppressed(attempt) => {
                self.launch_intent = Some((desktop_id.into(), attempt, self.input_generation));
            }
            launch::Outcome::ExistingWindow(id) => {
                self.sync_launches();
                return self.begin_window_operation(Request::ActivateWindow(id), Instant::now());
            }
            launch::Outcome::Rejected(
                launch::Rejection::StaleAttempt | launch::Rejection::RetryUnavailable,
            ) => {}
            launch::Outcome::Rejected(reason) => {
                eprintln!("Launch request rejected: {reason:?}");
                let body = localized(self.state.locale,
                    "Нельзя начать ещё один запуск. Дождитесь уже запущенных приложений и повторите.",
                    "Another launch cannot start. Wait for pending applications and try again.",
                    "No se puede iniciar otra aplicación. Espere los inicios pendientes y reintente.");
                self.internal_notice(name.to_owned(), body.to_owned());
                self.failed(Request::LaunchDesktop(desktop_id.to_owned()));
            }
        }
        self.sync_launches();
        Request::None
    }

    pub(super) fn sync_launches(&mut self) {
        let Some(statuses) = self.launches.take_update() else {
            return;
        };
        self.launch_snapshot = statuses;
        let statuses = self.localized_launches();
        let changed: Vec<_> = statuses
            .iter()
            .filter(|status| {
                status.phase != LaunchPhase::Starting
                    && !self.state.launches.iter().any(|old| old == *status)
            })
            .cloned()
            .collect();
        self.state.sync_launches(statuses);
        for status in changed {
            self.internal_notice(status.name.clone(), status.reason.clone());
            let same_intent =
                self.launch_intent
                    .as_ref()
                    .is_some_and(|(id, attempt, generation)| {
                        id == &status.desktop_id
                            && *attempt == status.attempt
                            && *generation == self.input_generation
                    });
            if self.focused
                && same_intent
                && !self.authentication_active
                && !self.lock_inhibits_shell()
                && !self.state.modal_active()
            {
                self.state
                    .show_launch_error(&status.desktop_id, status.attempt);
                self.banner = None;
            }
        }
        self.dirty = true;
    }

    pub(super) fn localized_launches(&self) -> Vec<LaunchStatus> {
        self.launch_snapshot
            .iter()
            .map(|status| LaunchStatus {
                desktop_id: status.desktop_id.clone(),
                name: status.name.clone(),
                attempt: status.attempt,
                phase: match status.phase {
                    launch::Phase::Starting => LaunchPhase::Starting,
                    launch::Phase::Unconfirmed => LaunchPhase::Unconfirmed,
                    launch::Phase::Failed(_) => LaunchPhase::Failed,
                },
                reason: launch_reason(self.state.locale, status.phase),
            })
            .collect()
    }

    pub(super) fn begin_window_operation(&mut self, request: Request, now: Instant) -> Request {
        let (id, kind) = match &request {
            Request::ActivateWindow(id) => (*id, WindowOperation::Activating),
            Request::CloseWindow(id) => (*id, WindowOperation::Closing),
            _ => return Request::None,
        };
        if self
            .window_operations
            .iter()
            .any(|operation| operation.id == id && operation.deadline.is_some())
        {
            return Request::None;
        }
        if !self.state.live_windows.iter().any(|window| window.id == id) {
            self.failed(request);
            return Request::None;
        }
        self.window_operations
            .retain(|operation| operation.id != id);
        self.window_operations.push(PendingWindowOperation {
            id,
            kind,
            deadline: Some(now + WINDOW_RESPONSE_TIMEOUT),
        });
        self.sync_window_operations();
        request
    }

    pub(super) fn sync_window_operations(&mut self) {
        self.state.sync_window_operations(
            self.window_operations
                .iter()
                .map(|operation| WindowOperationStatus {
                    id: operation.id,
                    kind: operation.kind,
                })
                .collect(),
        );
        self.dirty = true;
    }

    pub(super) fn tick_window_operations(&mut self, now: Instant) {
        let mut expired = Vec::new();
        for operation in &mut self.window_operations {
            if operation.deadline.is_some_and(|deadline| now >= deadline) {
                operation.deadline = None;
                operation.kind = match operation.kind {
                    WindowOperation::Activating => WindowOperation::ActivationUnconfirmed,
                    _ => WindowOperation::CloseUnconfirmed,
                };
                expired.push((operation.id, operation.kind));
            }
        }
        if !expired.is_empty() {
            for (id, kind) in expired {
                if let Some(window) = self
                    .state
                    .live_windows
                    .iter()
                    .find(|window| window.id == id)
                {
                    let body = if kind == WindowOperation::ActivationUnconfirmed {
                        localized(
                            self.state.locale,
                            "Переключение пока не подтверждено.",
                            "Switching to the window has not been confirmed yet.",
                            "El cambio a la ventana aún no está confirmado.",
                        )
                    } else {
                        localized(self.state.locale, "Окно ещё открыто. Возможно, приложение ожидает сохранения. Переключитесь к нему для проверки.", "The window is still open. The application may be waiting for you to save. Switch to it to check.", "La ventana sigue abierta. Quizá la aplicación espere que guarde los cambios. Ábrala para comprobarlo.")
                    };
                    self.internal_notice(window.title.clone(), body.to_owned());
                }
            }
            self.sync_window_operations();
        }
    }

    pub fn windows(&mut self, windows: Vec<WindowInfo>) {
        if self.state.live_windows != windows {
            self.state.sync_windows(windows);
            self.launches.windows(&self.resolved_windows());
            self.sync_launches();
            let count = self.window_operations.len();
            self.window_operations.retain(|operation| {
                self.state
                    .live_windows
                    .iter()
                    .find(|window| window.id == operation.id)
                    .is_some_and(|window| {
                        !(window.active
                            && matches!(
                                operation.kind,
                                WindowOperation::Activating
                                    | WindowOperation::ActivationUnconfirmed
                            ))
                    })
            });
            if count != self.window_operations.len() {
                self.sync_window_operations();
            }
            self.dirty = true;
        }
    }

    pub fn windows_unavailable(&mut self) {
        self.window_operations.clear();
        self.sync_window_operations();
        self.state.lose_window_service();
        self.dirty = true;
    }
}
