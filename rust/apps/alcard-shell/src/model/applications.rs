//! Application discovery, search, window identity and nested application views.

use super::{
    modal_scroll_position, scroll_position, wrap_text, Action, AppTarget, ApplicationOption,
    ApplicationView, ChoiceList, Dialog, InfoKind, Key, LaunchPhase, LaunchStatus, MenuApp,
    Request, Screen, SelectionMemory, Shell, WindowInfo, WindowOperation, WindowOperationStatus,
    MODAL_ROWS, NOTICE_COLUMNS, NOTICE_ROWS,
};
use crate::i18n::ShellLabels;

impl Shell {
    pub(super) fn remember_selection(&mut self) {
        match self.screen {
            Screen::Menu => {
                self.menu_memory.id = self
                    .menu_app(self.selected)
                    .map(|app| app.desktop_id.clone());
                self.menu_memory.index = self.selected;
            }
            Screen::Recent => {
                self.recent_memory.id = self.window_at(self.selected).map(|(window, _)| window.id);
                self.recent_memory.index = self.selected;
            }
            _ => {}
        }
    }

    pub(super) fn restore_menu_selection(&mut self) {
        let index = self
            .menu_memory
            .id
            .as_ref()
            .and_then(|id| {
                self.filtered_applications()
                    .position(|app| &app.desktop_id == id)
            })
            .unwrap_or_else(|| {
                self.menu_memory
                    .index
                    .min(self.filtered_applications().count().saturating_sub(1))
            });
        let id = self.menu_app(index).map(|app| app.desktop_id.clone());
        self.menu_memory.index = index;
        self.menu_memory.id = id;
        if self.screen == Screen::Menu {
            self.selected = self.menu_memory.index;
            self.page = self.selected / 8;
        }
    }

    pub(super) fn restore_recent_selection(&mut self) {
        let total = self.live_windows.len() + self.closed_windows.len();
        self.recent_memory.index = self
            .recent_memory
            .id
            .and_then(|id| {
                (0..total).find(|index| {
                    self.window_at(*index)
                        .is_some_and(|(window, _)| window.id == id)
                })
            })
            .unwrap_or_else(|| self.recent_memory.index.min(total.saturating_sub(1)));
        self.recent_memory.id = self
            .window_at(self.recent_memory.index)
            .map(|(window, _)| window.id);
        if self.screen == Screen::Recent {
            self.selected = self.recent_memory.index;
        }
    }

    pub fn set_applications(&mut self, applications: Vec<MenuApp>) {
        self.remember_selection();
        self.applications = applications;
        self.restore_menu_selection();
        self.reconcile_application_view();
    }

    pub(super) fn filtered_applications(&self) -> impl Iterator<Item = &MenuApp> {
        self.matching_applications(&self.search_query)
    }

    pub(super) fn matching_applications(&self, query: &str) -> impl Iterator<Item = &MenuApp> {
        let query = query.trim().to_lowercase();
        self.applications.iter().filter(move |app| {
            query.is_empty()
                || app.name.to_lowercase().contains(&query)
                || app.desktop_id.to_lowercase().contains(&query)
        })
    }

    /// Indices in Menu always address this filtered view, never the desktop
    /// catalog. Resolution and window identity still use the full catalog.
    pub fn menu_apps(&self) -> Vec<&MenuApp> {
        self.filtered_applications().collect()
    }

    pub fn menu_app(&self, index: usize) -> Option<&MenuApp> {
        self.filtered_applications().nth(index)
    }

    /// Preview only the number of matches. The actual grid and navigation
    /// remain on the committed query until the editor accepts it.
    pub fn search_match_count(&self) -> usize {
        let query = self
            .search_input
            .as_ref()
            .map_or(self.search_query.as_str(), |input| input.value());
        self.matching_applications(query).count()
    }

    pub fn is_text_editing(&self) -> bool {
        self.search_input.is_some()
    }

    pub(super) fn edit_search(&mut self, key: Key) -> Request {
        let Some(input) = self.search_input.as_mut() else {
            return Request::None;
        };
        match key {
            Key::Enter => {
                let query = input.value().trim().to_owned();
                self.search_input = None;
                self.remember_selection();
                self.search_query = query;
                // Preserve the selected desktop when it matches the new
                // query; otherwise choose the first result, not an unrelated
                // old grid index. Cancel never changes query or selection.
                self.menu_memory.index = 0;
                self.restore_menu_selection();
            }
            Key::Escape => self.search_input = None,
            Key::Text(ch) => {
                let mut bytes = [0; 4];
                input.insert(ch.encode_utf8(&mut bytes));
            }
            Key::Left => {
                input.left();
            }
            Key::Right => {
                input.right();
            }
            Key::Home => {
                input.home();
            }
            Key::End => {
                input.end();
            }
            Key::Backspace => {
                input.backspace();
            }
            Key::Delete => {
                input.delete();
            }
            Key::ClearText => {
                input.clear();
            }
            // Physical softkeys are printable characters in this context.
            // A stale semantic action can never accept or dismiss the field.
            _ => {}
        }
        Request::None
    }

    pub fn sync_launches(&mut self, launches: Vec<LaunchStatus>) {
        self.launches = launches;
        self.reconcile_application_view();
    }

    pub fn sync_window_operations(&mut self, operations: Vec<WindowOperationStatus>) {
        self.window_operations = operations;
        self.reconcile_application_view();
    }

    pub fn launch_status(&self, desktop_id: &str) -> Option<&LaunchStatus> {
        self.launches
            .iter()
            .find(|status| status.desktop_id == desktop_id)
    }

    pub fn window_operation(&self, id: u64) -> Option<WindowOperation> {
        self.window_operations
            .iter()
            .find(|status| status.id == id)
            .map(|status| status.kind)
    }

    pub(super) fn window_busy(&self, id: u64) -> bool {
        matches!(
            self.window_operation(id),
            Some(WindowOperation::Activating | WindowOperation::Closing)
        )
    }

    /// Called only when the owner still has the user's launch intent and focus.
    /// A late failure from an older attempt cannot replace the current view.
    pub fn show_launch_error(&mut self, desktop_id: &str, attempt: u64) {
        if !matches!(self.screen, Screen::Menu | Screen::Recent)
            || !self.launch_status(desktop_id).is_some_and(|status| {
                status.attempt == attempt && status.phase != LaunchPhase::Starting
            })
        {
            return;
        }
        let target = AppTarget::Desktop(desktop_id.to_string());
        if self.valid_target(&target) {
            self.push_application_view(ApplicationView::Details { target, scroll: 0 });
        }
    }

    /// Match only identities advertised by XDG/Wayland. Desktop ID is the
    /// authoritative match; StartupWMClass is a fallback when unambiguous.
    /// No title/Exec/PID guesses and no cross-application fuzzy matches.
    pub fn resolve_app(&self, app_id: &str) -> Option<&MenuApp> {
        if app_id.is_empty() {
            return None;
        }
        let key = app_id.strip_suffix(".desktop").unwrap_or(app_id);
        fn unique<'a>(mut apps: impl Iterator<Item = &'a MenuApp>) -> Option<&'a MenuApp> {
            let app = apps.next()?;
            apps.next().is_none().then_some(app)
        }
        let by_id = unique(
            self.applications
                .iter()
                .filter(|app| app.desktop_id.strip_suffix(".desktop") == Some(key)),
        );
        if by_id.is_some() {
            return by_id;
        }
        unique(self.applications.iter().filter(|app| {
            app.startup_wm_class
                .as_deref()
                .is_some_and(|class| class.eq_ignore_ascii_case(key))
        }))
    }

    pub(super) fn app_window(&self, desktop_id: &str) -> Option<&WindowInfo> {
        self.live_windows
            .iter()
            .filter(|window| {
                self.resolve_app(&window.app_id)
                    .is_some_and(|app| app.desktop_id == desktop_id)
            })
            .max_by_key(|window| (window.activity, window.id))
    }

    /// Multiple windows remain individually visible in Recent. Menu activates
    /// only the most recently active matching window, not a random process.
    pub fn running_count(&self, index: usize) -> usize {
        let Some(app) = self.menu_app(index) else {
            return 0;
        };
        self.live_windows
            .iter()
            .filter(|window| {
                self.resolve_app(&window.app_id)
                    .is_some_and(|match_app| match_app.desktop_id == app.desktop_id)
            })
            .count()
    }

    pub fn window_at(&self, index: usize) -> Option<(&WindowInfo, bool)> {
        if index < self.live_windows.len() {
            self.live_windows.get(index).map(|w| (w, true))
        } else {
            self.closed_windows
                .get(index - self.live_windows.len())
                .map(|w| (w, false))
        }
    }

    pub fn sync_windows(&mut self, mut incoming: Vec<WindowInfo>) {
        self.remember_selection();
        incoming.sort_by(|a, b| b.activity.cmp(&a.activity).then_with(|| b.id.cmp(&a.id)));
        incoming.truncate(128);
        // Prepend the whole disappearing batch in its existing MRU order.
        // Inserting each item at zero would reverse simultaneous closures.
        let mut closed: Vec<_> = self
            .live_windows
            .iter()
            .filter(|previous| !incoming.iter().any(|current| current.id == previous.id))
            .cloned()
            .collect();
        for previous in &self.closed_windows {
            if !incoming.iter().any(|current| current.id == previous.id)
                && !closed.iter().any(|current| current.id == previous.id)
            {
                closed.push(previous.clone());
            }
        }
        closed.sort_by(|a, b| b.activity.cmp(&a.activity).then_with(|| b.id.cmp(&a.id)));
        closed.truncate(12);
        self.closed_windows = closed;
        self.live_windows = incoming;
        self.restore_recent_selection();
        self.reconcile_application_view();
        if let Some(Dialog::ConfirmClose(id)) = &self.dialog {
            let available = self.live_windows.iter().any(|window| {
                window.id == *id && self.dialog_window_app_id.as_ref() == Some(&window.app_id)
            });
            if !available {
                self.open_dialog(Dialog::Info(InfoKind::TargetUnavailable));
            }
        }
    }

    pub fn lose_window_service(&mut self) {
        self.toplevel_unavailable = true;
        self.live_windows.clear();
        self.closed_windows.clear();
        self.window_operations.clear();
        self.recent_memory = SelectionMemory::default();
        if self.screen == Screen::Recent {
            self.selected = 0;
        }
        self.dialog = None;
        self.reconcile_application_view();
    }

    pub fn app_windows(&self, desktop_id: &str) -> Vec<&WindowInfo> {
        self.live_windows
            .iter()
            .filter(|window| {
                self.resolve_app(&window.app_id)
                    .is_some_and(|app| app.desktop_id == desktop_id)
            })
            .collect()
    }

    pub fn target_window(&self, target: &AppTarget) -> Option<(&WindowInfo, bool)> {
        let AppTarget::Window { id, app_id } = target else {
            return None;
        };
        self.live_windows
            .iter()
            .find(|window| window.id == *id && window.app_id == *app_id)
            .map(|window| (window, true))
            .or_else(|| {
                self.closed_windows
                    .iter()
                    .find(|window| window.id == *id && window.app_id == *app_id)
                    .map(|window| (window, false))
            })
    }

    pub fn target_app(&self, target: &AppTarget) -> Option<&MenuApp> {
        match target {
            AppTarget::Desktop(id) => self.applications.iter().find(|app| &app.desktop_id == id),
            AppTarget::Window { .. } => self
                .target_window(target)
                .and_then(|(window, _)| self.resolve_app(&window.app_id)),
        }
    }

    pub fn target_name(&self, target: &AppTarget) -> String {
        match target {
            AppTarget::Desktop(_) => self
                .target_app(target)
                .map(|app| app.name.clone())
                .unwrap_or_default(),
            AppTarget::Window { .. } => self
                .target_window(target)
                .map(|(window, _)| {
                    if window.title.is_empty() {
                        window.app_id.clone()
                    } else {
                        window.title.clone()
                    }
                })
                .unwrap_or_default(),
        }
    }

    pub(super) fn valid_target(&self, target: &AppTarget) -> bool {
        match target {
            AppTarget::Desktop(_) => self.target_app(target).is_some(),
            AppTarget::Window { .. } => self.target_window(target).is_some(),
        }
    }

    pub(super) fn selected_target(&self) -> Option<AppTarget> {
        match self.screen {
            Screen::Menu => self
                .menu_app(self.selected)
                .map(|app| AppTarget::Desktop(app.desktop_id.clone())),
            Screen::Recent => self
                .window_at(self.selected)
                .map(|(window, _)| AppTarget::Window {
                    id: window.id,
                    app_id: window.app_id.clone(),
                }),
            _ => None,
        }
    }

    pub(super) fn target_primary(&self, target: &AppTarget) -> Action {
        if let Some((window, true)) = self.target_window(target) {
            return if self.window_busy(window.id) {
                Action::None
            } else {
                Action::Open
            };
        }
        let Some(app) = self.target_app(target) else {
            return Action::None;
        };
        if let Some(window) = self.app_window(&app.desktop_id) {
            return if self.window_busy(window.id) {
                Action::None
            } else {
                Action::Open
            };
        }
        // A failed attempt also requires an explicit, attempt-bound retry.
        // Merely pressing the main action repeatedly never creates a process.
        if self.launch_status(&app.desktop_id).is_some() {
            Action::None
        } else if matches!(target, AppTarget::Window { .. }) {
            Action::Launch
        } else {
            Action::Open
        }
    }

    pub(super) fn activate_target(&self, target: &AppTarget) -> Request {
        if self.target_primary(target) == Action::None {
            return Request::None;
        }
        if let Some((window, true)) = self.target_window(target) {
            return Request::ActivateWindow(window.id);
        }
        self.target_app(target).map_or(Request::None, |app| {
            self.app_window(&app.desktop_id).map_or_else(
                || Request::LaunchDesktop(app.desktop_id.clone()),
                |window| Request::ActivateWindow(window.id),
            )
        })
    }

    pub(super) fn app_options(&self, target: &AppTarget) -> Vec<ApplicationOption> {
        let mut entries = Vec::new();
        if self.target_window(target).is_some_and(|(_, live)| live) {
            entries.push(ApplicationOption::Close);
        }
        if let Some(app) = self.target_app(target) {
            if !self.app_windows(&app.desktop_id).is_empty() {
                entries.push(ApplicationOption::Windows);
            }
            if let Some(status) = self.launch_status(&app.desktop_id) {
                entries.push(ApplicationOption::Retry {
                    attempt: status.attempt,
                });
            }
        }
        entries.push(ApplicationOption::Details);
        entries
    }

    pub fn application_option_enabled(
        &self,
        target: &AppTarget,
        option: &ApplicationOption,
    ) -> bool {
        if !self.valid_target(target) {
            return false;
        }
        match option {
            ApplicationOption::Close => self
                .target_window(target)
                .is_some_and(|(window, live)| live && !self.window_busy(window.id)),
            ApplicationOption::Windows => self
                .target_app(target)
                .is_some_and(|app| !self.app_windows(&app.desktop_id).is_empty()),
            ApplicationOption::Retry { attempt } => self
                .target_app(target)
                .and_then(|app| self.launch_status(&app.desktop_id))
                .is_some_and(|status| {
                    status.attempt == *attempt && status.phase != LaunchPhase::Starting
                }),
            ApplicationOption::Details => true,
        }
    }

    pub(super) fn push_application_view(&mut self, view: ApplicationView) {
        if self.application_view == view {
            return;
        }
        // A Details -> Options shortcut returns to the originating Options
        // instead of recursively growing a navigation stack.
        if let ApplicationView::Options { target, .. } = &view {
            if matches!(self.application_back.last(), Some(ApplicationView::Options { target: old, .. }) if old == target)
            {
                self.application_view = self.application_back.pop().unwrap();
                self.reconcile_application_view();
                return;
            }
        }
        if self.application_back.len() >= 8 {
            self.application_back.remove(0);
        }
        self.application_back
            .push(std::mem::replace(&mut self.application_view, view));
    }

    pub(super) fn back_application_view(&mut self) {
        self.application_view = self.application_back.pop().unwrap_or_default();
        self.reconcile_application_view();
    }

    pub(super) fn reset_application_view(&mut self) {
        self.application_view = ApplicationView::List;
        self.application_back.clear();
    }

    pub(super) fn reconcile_application_view(&mut self) {
        match self.application_view.clone() {
            ApplicationView::List => {}
            ApplicationView::Options {
                target,
                entries,
                selected,
            } => {
                if !self.valid_target(&target) {
                    self.reset_application_view();
                    return;
                }
                let next = self.app_options(&target);
                if let Some(index) = entries
                    .get(selected)
                    .and_then(|old| next.iter().position(|entry| entry == old))
                {
                    self.application_view = ApplicationView::Options {
                        target,
                        entries: next,
                        selected: index,
                    };
                } else {
                    // For example, the selected Close action disappeared when
                    // the window closed. A harmless full card replaces it.
                    self.application_view = ApplicationView::Details { target, scroll: 0 };
                }
            }
            ApplicationView::Windows {
                desktop_id,
                mut choice,
            } => {
                let keys: Vec<_> = self.app_windows(&desktop_id).iter().map(|w| w.id).collect();
                if !choice.selected().is_some_and(|id| keys.contains(id)) {
                    // A disappearing selection must not turn an already
                    // queued Select into activation of a neighbouring window.
                    self.back_application_view();
                } else {
                    choice.reconcile(keys);
                    self.application_view = ApplicationView::Windows { desktop_id, choice };
                }
            }
            ApplicationView::Details { target, scroll } => {
                if !self.valid_target(&target) {
                    self.reset_application_view();
                } else {
                    let scroll = scroll.min(
                        self.application_lines(&target)
                            .len()
                            .saturating_sub(NOTICE_ROWS),
                    );
                    self.application_view = ApplicationView::Details { target, scroll };
                }
            }
            ApplicationView::ConfirmRetry {
                desktop_id,
                attempt,
                scroll,
            } => {
                if !self
                    .applications
                    .iter()
                    .any(|app| app.desktop_id == desktop_id)
                    || !self.launch_status(&desktop_id).is_some_and(|status| {
                        status.attempt == attempt && status.phase == LaunchPhase::Unconfirmed
                    })
                {
                    self.back_application_view();
                } else {
                    let scroll = scroll.min(
                        self.retry_lines(&desktop_id)
                            .len()
                            .saturating_sub(MODAL_ROWS),
                    );
                    self.application_view = ApplicationView::ConfirmRetry {
                        desktop_id,
                        attempt,
                        scroll,
                    };
                }
            }
        }
    }

    pub fn application_lines(&self, target: &AppTarget) -> Vec<String> {
        let mut lines = wrap_text(&self.target_name(target), NOTICE_COLUMNS);
        if let Some((window, live)) = self.target_window(target) {
            lines.push(
                if live {
                    self.locale.running()
                } else {
                    self.locale.closed()
                }
                .into(),
            );
            if let Some(operation) = self.window_operation(window.id) {
                lines.extend(wrap_text(
                    self.locale.window_operation(operation),
                    NOTICE_COLUMNS,
                ));
            }
            lines.push(format!(
                "{}: {}",
                self.locale.window_identifier(),
                window.id
            ));
            lines.extend(wrap_text(
                &format!("app_id: {}", window.app_id),
                NOTICE_COLUMNS,
            ));
            if !live {
                lines.extend(wrap_text(self.locale.history_explanation(), NOTICE_COLUMNS));
            }
        }
        if let Some(app) = self.target_app(target) {
            if matches!(target, AppTarget::Window { .. }) {
                lines.extend(wrap_text(&app.name, NOTICE_COLUMNS));
            }
            lines.extend(wrap_text(&app.desktop_id, NOTICE_COLUMNS));
            lines.push(format!(
                "{}: {}",
                self.locale.application_windows(),
                self.app_windows(&app.desktop_id).len()
            ));
            if let Some(status) = self.launch_status(&app.desktop_id) {
                lines.push(String::new());
                lines.extend(wrap_text(
                    self.locale.launch_phase(status.phase),
                    NOTICE_COLUMNS,
                ));
                lines.push(format!(
                    "{}: {}",
                    self.locale.launch_attempt(),
                    status.attempt
                ));
                if !status.reason.is_empty() {
                    lines.extend(wrap_text(&status.reason, NOTICE_COLUMNS));
                }
            }
        }
        lines
    }

    pub(super) fn open_application_options(&mut self, target: AppTarget) {
        if !self.valid_target(&target) {
            return;
        }
        let entries = self.app_options(&target);
        self.push_application_view(ApplicationView::Options {
            target,
            entries,
            selected: 0,
        });
    }

    pub(super) fn choose_application_option(
        &mut self,
        target: AppTarget,
        option: ApplicationOption,
    ) -> Request {
        if !self.application_option_enabled(&target, &option) {
            return Request::None;
        }
        match option {
            ApplicationOption::Details => {
                self.push_application_view(ApplicationView::Details { target, scroll: 0 })
            }
            ApplicationOption::Close => {
                if let Some((window, true)) = self.target_window(&target) {
                    self.open_dialog(Dialog::ConfirmClose(window.id));
                }
            }
            ApplicationOption::Windows => {
                if let Some(app) = self.target_app(&target) {
                    if let Some(window) = self.app_window(&app.desktop_id) {
                        self.push_application_view(ApplicationView::Windows {
                            desktop_id: app.desktop_id.clone(),
                            choice: {
                                let mut choice = ChoiceList::new(
                                    self.app_windows(&app.desktop_id).iter().map(|w| w.id),
                                );
                                choice.select(&window.id);
                                choice
                            },
                        });
                    }
                }
            }
            ApplicationOption::Retry { attempt } => {
                if let Some(app) = self.target_app(&target) {
                    let desktop_id = app.desktop_id.clone();
                    if self
                        .launch_status(&desktop_id)
                        .is_some_and(|status| status.phase == LaunchPhase::Unconfirmed)
                    {
                        self.push_application_view(ApplicationView::ConfirmRetry {
                            desktop_id,
                            attempt,
                            scroll: 0,
                        });
                    } else {
                        return Request::RetryLaunch {
                            desktop_id,
                            attempt,
                        };
                    }
                }
            }
        }
        Request::None
    }

    pub(super) fn application_key(&mut self, key: Key) -> Request {
        let before = self.application_view.clone();
        self.reconcile_application_view();
        if before != self.application_view {
            return Request::None;
        }
        if key == Key::Home {
            self.go_home();
            return Request::None;
        }
        let view = self.application_view.clone();
        if let ApplicationView::ConfirmRetry {
            desktop_id,
            attempt,
            scroll,
        } = view
        {
            match key {
                Key::Left | Key::Up | Key::Right | Key::Down => {
                    let maximum = self
                        .retry_lines(&desktop_id)
                        .len()
                        .saturating_sub(MODAL_ROWS);
                    self.application_view = ApplicationView::ConfirmRetry {
                        desktop_id,
                        attempt,
                        scroll: modal_scroll_position(scroll, maximum, key),
                    }
                }
                Key::Escape | Key::Backspace | Key::Soft(4) => self.back_application_view(),
                Key::Enter | Key::Soft(0) => {
                    self.back_application_view();
                    if self.launch_status(&desktop_id).is_some_and(|status| {
                        status.attempt == attempt && status.phase == LaunchPhase::Unconfirmed
                    }) {
                        return Request::RetryLaunch {
                            desktop_id,
                            attempt,
                        };
                    }
                }
                _ => {}
            }
            return Request::None;
        }
        if matches!(key, Key::Up | Key::Down | Key::Left | Key::Right) {
            match &view {
                ApplicationView::Options {
                    entries, selected, ..
                } => {
                    let next = match key {
                        Key::Up => selected.saturating_sub(1),
                        Key::Down => (*selected + 1).min(entries.len().saturating_sub(1)),
                        _ => *selected,
                    };
                    if let ApplicationView::Options { selected, .. } = &mut self.application_view {
                        *selected = next;
                    }
                }
                ApplicationView::Windows { .. } => {
                    if let ApplicationView::Windows { choice, .. } = &mut self.application_view {
                        match key {
                            Key::Up => {
                                choice.up();
                            }
                            Key::Down => {
                                choice.down();
                            }
                            _ => {}
                        }
                    }
                }
                ApplicationView::Details { target, scroll } => {
                    let maximum = self
                        .application_lines(target)
                        .len()
                        .saturating_sub(NOTICE_ROWS);
                    let next = scroll_position(*scroll, maximum, key);
                    if let ApplicationView::Details { scroll, .. } = &mut self.application_view {
                        *scroll = next;
                    }
                }
                _ => {}
            }
            return Request::None;
        }
        match (view, self.key_action(key)) {
            (_, Action::Back) => self.back_application_view(),
            (
                ApplicationView::Options {
                    target,
                    entries,
                    selected,
                },
                Action::Select,
            ) => {
                if let Some(option) = entries.get(selected) {
                    return self.choose_application_option(target, option.clone());
                }
            }
            (ApplicationView::Windows { desktop_id, choice }, Action::Read) => {
                if let Some(window) = self
                    .app_windows(&desktop_id)
                    .into_iter()
                    .find(|window| Some(&window.id) == choice.selected())
                {
                    let target = AppTarget::Window {
                        id: window.id,
                        app_id: window.app_id.clone(),
                    };
                    self.push_application_view(ApplicationView::Details { target, scroll: 0 });
                }
            }
            (ApplicationView::Windows { desktop_id, choice }, Action::Select) => {
                if let Some(id) = choice.selected().copied() {
                    if self
                        .app_windows(&desktop_id)
                        .iter()
                        .any(|window| window.id == id && !self.window_busy(id))
                    {
                        return Request::ActivateWindow(id);
                    }
                }
            }
            (ApplicationView::Details { target, .. }, Action::Open | Action::Launch) => {
                return self.activate_target(&target)
            }
            (ApplicationView::Details { target, .. }, Action::Actions) => {
                self.open_application_options(target)
            }
            _ => {}
        }
        Request::None
    }
}
