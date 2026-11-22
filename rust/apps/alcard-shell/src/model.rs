//! Pure Shell state machine; real desktop entries and compositor windows only.
//! Tests construct controlled state directly, without a runtime demo mode.
use crate::{
    controls::{self, Control, Direction},
    i18n::Locale,
    locking,
    modal::{View as ModalView, MODAL_ROWS},
    radios, sleep,
    status::StatusSnapshot,
    system::{Capability, Failure, Operation, Snapshot as SystemSnapshot},
    theme::Theme,
};
use alcard_ui::dialog::{ChoiceList, TextInput};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Screen {
    Home,
    Recent,
    Notifications,
    Menu,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    None,
    Recent,
    Menu,
    Notifications,
    Open,
    Launch,
    Back,
    Confirm,
    Cancel,
    Decline,
    Read,
    Actions,
    Select,
    Acknowledge,
    Search,
    Clear,
    Retry,
    ApplyText,
    CancelText,
    Quick,
    Mute,
    Unmute,
    TurnOn,
    TurnOff,
}

/// Session locking has its own compositor/PAM lifecycle, not login1 power policy.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum QuickAction {
    System(Operation),
    Lock,
    Control(Control),
    Radio(radios::Kind),
    Suspend,
}
impl QuickAction {
    pub const ALL: [Self; 9] = [
        Self::Control(Control::Brightness),
        Self::Control(Control::Volume),
        Self::Radio(radios::Kind::Wifi),
        Self::Radio(radios::Kind::Bluetooth),
        Self::System(Operation::ALL[0]),
        Self::System(Operation::ALL[1]),
        Self::System(Operation::ALL[2]),
        Self::Lock,
        Self::Suspend,
    ];
    pub const BRIGHTNESS_INDEX: usize = 0;
    pub const VOLUME_INDEX: usize = 1;
    pub const WIFI_INDEX: usize = 2;
    pub const BLUETOOTH_INDEX: usize = 3;
    pub const LOCK_INDEX: usize = 7;
    pub const SUSPEND_INDEX: usize = 8;

    pub fn system_index(operation: Operation) -> usize {
        4 + operation.index()
    }

    pub fn radio_index(kind: radios::Kind) -> usize {
        match kind {
            radios::Kind::Wifi => Self::WIFI_INDEX,
            radios::Kind::Bluetooth => Self::BLUETOOTH_INDEX,
        }
    }
}

/// Quick actions are nested Home states; they never become another screen.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SystemView {
    Menu {
        selected: usize,
    },
    Confirm {
        operation: Operation,
        scroll: usize,
    },
    Unavailable {
        operation: Operation,
        capability: Capability,
        scroll: usize,
    },
    Status {
        operation: Operation,
        scroll: usize,
    },
    LockStatus {
        scroll: usize,
    },
    RadioList {
        kind: radios::Kind,
        selected: usize,
    },
    RadioDetails {
        kind: radios::Kind,
        entry: radios::Entry,
        scroll: usize,
    },
    SuspendConfirm {
        scroll: usize,
    },
    SuspendUnavailable {
        scroll: usize,
    },
    SuspendStatus {
        scroll: usize,
    },
}

impl SystemView {
    pub fn control(&self) -> Option<Control> {
        match self {
            Self::Menu {
                selected: QuickAction::BRIGHTNESS_INDEX,
            } => Some(Control::Brightness),
            Self::Menu {
                selected: QuickAction::VOLUME_INDEX,
            } => Some(Control::Volume),
            _ => None,
        }
    }

    pub fn radio(&self) -> Option<radios::Kind> {
        match self {
            Self::Menu { selected } => match QuickAction::ALL.get(*selected) {
                Some(QuickAction::Radio(kind)) => Some(*kind),
                _ => None,
            },
            Self::RadioList { kind, .. } | Self::RadioDetails { kind, .. } => Some(*kind),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ControlsRequest {
    pub id: u64,
    /// None is capability discovery; a mutation keeps its exact operation.
    pub operation: Option<controls::Operation>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RadioRequest {
    pub id: u64,
    pub operation: radios::Operation,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SystemPhase {
    Pending,
    Accepted,
    Failed(Failure),
    Uncertain,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SystemRequest {
    pub id: u64,
    pub operation: Operation,
    pub phase: SystemPhase,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InfoKind {
    Unsupported,
    Activation,
    Close,
    LaunchError,
    NotificationError,
    TargetUnavailable,
}

#[derive(Clone, Debug)]
pub struct MenuApp {
    /// Stable XDG desktop file ID (including .desktop), never a list index.
    pub desktop_id: String,
    pub name: String,
    pub icon: Option<String>,
    /// Optional extra hint, not a process name or window title.
    pub startup_wm_class: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WindowInfo {
    pub id: u64,
    pub title: String,
    pub app_id: String,
    pub active: bool,
    pub activity: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LaunchPhase {
    Starting,
    Unconfirmed,
    Failed,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LaunchStatus {
    pub desktop_id: String,
    pub name: String,
    pub attempt: u64,
    pub phase: LaunchPhase,
    pub reason: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WindowOperation {
    Activating,
    Closing,
    ActivationUnconfirmed,
    CloseUnconfirmed,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WindowOperationStatus {
    pub id: u64,
    pub kind: WindowOperation,
}

/// A nested view never follows the list position after an asynchronous update.
/// The app_id additionally prevents an identity change on an existing handle
/// from silently turning a window command into a different app's command.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AppTarget {
    Desktop(String),
    Window { id: u64, app_id: String },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ApplicationOption {
    Close,
    Windows,
    Retry { attempt: u64 },
    Details,
}

#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub enum ApplicationView {
    #[default]
    List,
    Options {
        target: AppTarget,
        entries: Vec<ApplicationOption>,
        selected: usize,
    },
    Windows {
        desktop_id: String,
        choice: ChoiceList<u64>,
    },
    Details {
        target: AppTarget,
        scroll: usize,
    },
    ConfirmRetry {
        desktop_id: String,
        attempt: u64,
        scroll: usize,
    },
}

#[derive(Default)]
struct SelectionMemory<T> {
    id: Option<T>,
    index: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NoticeAction {
    pub key: String,
    pub label: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Notice {
    pub id: u32,
    pub generation: u64,
    pub title: String,
    pub description: String,
    pub app_name: String,
    pub desktop_entry: Option<String>,
    pub created_at: i64,
    pub urgency: u8,
    pub unread: bool,
    pub actions: Vec<NoticeAction>,
    /// True while the notification is live; historical entries must not
    /// create a new popup after they expire or are closed.
    pub active: bool,
}

pub const NOTICE_COLUMNS: usize = 36;
pub const NOTICE_ROWS: usize = 10;

/// Nested views of Notifications, never a fifth top-level Shell screen.
/// Each view pins a content generation so an asynchronous replacement cannot
/// silently change the target of an action or mark the new content as read.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub enum NotificationView {
    #[default]
    List,
    Detail {
        id: u32,
        generation: u64,
        scroll: usize,
    },
    Options {
        id: Option<u32>,
        generation: u64,
        scroll: usize,
        from_detail: bool,
        entries: Vec<NotificationOption>,
        selected: usize,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NotificationOption {
    Invoke { key: String, label: String },
    Delete,
    Clear,
    Retry,
}

pub fn wrap_text(value: &str, columns: usize) -> Vec<String> {
    let columns = columns.clamp(1, 128);
    let mut lines = Vec::new();
    for paragraph in value.split('\n') {
        let mut line = String::new();
        let mut used = 0;
        for word in paragraph.split_whitespace() {
            let length = word.chars().count();
            if used > 0 {
                if used + 1 + length <= columns {
                    line.push(' ');
                    used += 1;
                } else {
                    lines.push(std::mem::take(&mut line));
                    used = 0;
                }
            }
            for ch in word.chars() {
                if used == columns {
                    lines.push(std::mem::take(&mut line));
                    used = 0;
                }
                line.push(ch);
                used += 1;
            }
        }
        // Preserve paragraph boundaries, including deliberate blank lines.
        lines.push(line);
    }
    lines
}

impl Notice {
    pub fn text_lines(&self, locale: Locale) -> Vec<String> {
        let title = if self.title.is_empty() {
            locale.notification_title()
        } else {
            &self.title
        };
        let source = if self.app_name.is_empty() {
            self.desktop_entry
                .as_deref()
                .filter(|name| !name.is_empty())
                .unwrap_or_else(|| locale.unknown_application())
        } else {
            &self.app_name
        };
        let timestamp = chrono::DateTime::from_timestamp(self.created_at, 0)
            .map(|time| {
                time.with_timezone(&chrono::Local)
                    .format("%Y-%m-%d %H:%M")
                    .to_string()
            })
            .unwrap_or_else(|| "?".into());
        let mut lines = wrap_text(title, NOTICE_COLUMNS);
        lines.extend(wrap_text(
            &format!("{}: {source}", locale.notification_source()),
            NOTICE_COLUMNS,
        ));
        lines.push(timestamp);
        if !self.active && !self.actions.is_empty() {
            lines.extend(wrap_text(locale.notification_expired(), NOTICE_COLUMNS));
        }
        lines.push(String::new());
        lines.extend(wrap_text(&self.description, NOTICE_COLUMNS));
        lines
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Dialog {
    // Store stable IDs, never mutable list indexes.
    ConfirmClose(u64),
    ConfirmClear,
    Info(InfoKind),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Key {
    Soft(usize),
    Up,
    Down,
    Left,
    Right,
    Enter,
    Escape,
    Home,
    Backspace,
    Delete,
    End,
    ClearText,
    Text(char),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Request {
    None,
    LaunchDesktop(String),
    RetryLaunch {
        desktop_id: String,
        attempt: u64,
    },
    ActivateWindow(u64),
    CloseWindow(u64),
    DismissNotification(u32),
    ClearNotifications,
    MarkNotificationRead {
        id: u32,
        generation: u64,
    },
    InvokeNotification {
        id: u32,
        generation: u64,
        action: String,
    },
    RetryNotifications,
    RefreshSystem,
    RefreshSleep,
    PrepareSuspend {
        request: u64,
    },
    CancelSuspend {
        request: u64,
    },
    RefreshControls {
        request: u64,
    },
    AdjustControl {
        request: u64,
        operation: controls::Operation,
    },
    RadioOperation {
        request: u64,
        operation: radios::Operation,
    },
    LockSession {
        request: u64,
    },
    SystemOperation {
        request: u64,
        operation: Operation,
    },
}

pub struct Shell {
    pub screen: Screen,
    pub locale: Locale,
    pub theme: Theme,
    pub selected: usize,
    pub page: usize,
    pub dialog: Option<Dialog>,
    pub dialog_scroll: usize,
    dialog_window_app_id: Option<String>,
    pub live_windows: Vec<WindowInfo>,
    pub closed_windows: Vec<WindowInfo>,
    pub toplevel_unavailable: bool,
    pub applications: Vec<MenuApp>,
    pub search_query: String,
    pub search_input: Option<TextInput>,
    pub system_view: Option<SystemView>,
    pub system_snapshot: SystemSnapshot,
    pub system_request: Option<SystemRequest>,
    pub sleep_snapshot: sleep::Snapshot,
    pub sleep_progress: Option<sleep::Progress>,
    pub controls_snapshot: controls::Snapshot,
    pub controls_request: Option<ControlsRequest>,
    pub controls_failure: Option<controls::Failure>,
    controls_failure_target: Option<Control>,
    pub radios_snapshot: radios::Snapshot,
    pub radios_request: Option<RadioRequest>,
    pub radios_failure: Option<radios::Failure>,
    radios_failure_target: Option<radios::Kind>,
    pub radio_entries: Vec<radios::Entry>,
    pub lock_state: Option<locking::State>,
    system_selected: usize,
    next_system_request: u64,
    next_sleep_request: u64,
    sleep_dismissed: Option<u64>,
    next_controls_request: u64,
    next_radio_request: u64,
    next_lock_request: u64,
    pub launches: Vec<LaunchStatus>,
    pub window_operations: Vec<WindowOperationStatus>,
    pub application_view: ApplicationView,
    application_back: Vec<ApplicationView>,
    menu_memory: SelectionMemory<String>,
    recent_memory: SelectionMemory<u64>,
    pub notices: Vec<Notice>,
    pub notification_view: NotificationView,
    pub notifications_unavailable: bool,
    pub status: StatusSnapshot,
    pub battery_preferences: alcard_ui::preferences::BatteryPreferences,
}

impl Shell {
    pub fn new(locale: Locale, theme: Theme) -> Self {
        Self {
            screen: Screen::Home,
            locale,
            theme,
            selected: 0,
            page: 0,
            dialog: None,
            dialog_scroll: 0,
            dialog_window_app_id: None,
            live_windows: Vec::new(),
            closed_windows: Vec::new(),
            toplevel_unavailable: false,
            applications: Vec::new(),
            search_query: String::new(),
            search_input: None,
            system_view: None,
            system_snapshot: SystemSnapshot::default(),
            system_request: None,
            sleep_snapshot: sleep::Snapshot::default(),
            sleep_progress: None,
            controls_snapshot: controls::Snapshot::default(),
            controls_request: None,
            controls_failure: None,
            controls_failure_target: None,
            radios_snapshot: radios::Snapshot::default(),
            radios_request: None,
            radios_failure: None,
            radios_failure_target: None,
            radio_entries: Vec::new(),
            lock_state: None,
            system_selected: QuickAction::BRIGHTNESS_INDEX,
            next_system_request: 0,
            next_sleep_request: 0,
            sleep_dismissed: None,
            next_controls_request: 0,
            next_radio_request: 0,
            next_lock_request: 0,
            launches: Vec::new(),
            window_operations: Vec::new(),
            application_view: ApplicationView::List,
            application_back: Vec::new(),
            menu_memory: SelectionMemory::default(),
            recent_memory: SelectionMemory::default(),
            notices: Vec::new(),
            notification_view: NotificationView::List,
            notifications_unavailable: false,
            status: StatusSnapshot::default(),
            battery_preferences: alcard_ui::preferences::BatteryPreferences::default(),
        }
    }

    /// Commit validated presentation values without replacing navigation or
    /// service state. Locale changes may only clamp a now-shorter text scroll.
    pub fn apply_preferences(&mut self, locale: Locale, theme: Theme) {
        self.locale = locale;
        self.theme = theme;
        self.reconcile_notification_view();
        self.reconcile_application_view();
        self.dialog_scroll = self
            .dialog_scroll
            .min(self.dialog_lines().len().saturating_sub(MODAL_ROWS));
        self.clamp_system_scroll();
    }

    pub fn sync_notices(&mut self, notices: Vec<Notice>) {
        let selected = if self.screen == Screen::Notifications {
            self.notices.get(self.selected).map(|notice| notice.id)
        } else {
            None
        };
        self.notices = notices;
        if self.screen == Screen::Notifications {
            self.selected = selected
                .and_then(|id| self.notices.iter().position(|notice| notice.id == id))
                .unwrap_or_else(|| self.selected.min(self.notices.len().saturating_sub(1)));
        }
        self.reconcile_notification_view();
    }

    pub fn unread_count(&self) -> usize {
        self.notices.iter().filter(|notice| notice.unread).count()
    }

    pub fn viewed_notice(&self) -> Option<&Notice> {
        let (id, generation) = match &self.notification_view {
            NotificationView::List => return None,
            NotificationView::Detail { id, generation, .. } => (*id, *generation),
            NotificationView::Options {
                id: Some(id),
                generation,
                ..
            } => (*id, *generation),
            NotificationView::Options { id: None, .. } => return None,
        };
        self.notices
            .iter()
            .find(|notice| notice.id == id && notice.generation == generation)
    }

    fn notice_options(&self, id: Option<u32>, generation: u64) -> Vec<NotificationOption> {
        if self.notifications_unavailable {
            return vec![NotificationOption::Retry];
        }
        let mut entries = Vec::new();
        if let Some(notice) = self
            .notices
            .iter()
            .find(|n| Some(n.id) == id && n.generation == generation)
        {
            if notice.active {
                entries.extend(
                    notice
                        .actions
                        .iter()
                        .map(|action| NotificationOption::Invoke {
                            key: action.key.clone(),
                            label: action.label.clone(),
                        }),
                );
            }
            entries.push(NotificationOption::Delete);
        }
        if !self.notices.is_empty() {
            entries.push(NotificationOption::Clear);
        }
        entries
    }

    fn notification_options_back(
        &mut self,
        id: Option<u32>,
        generation: u64,
        scroll: usize,
        from_detail: bool,
    ) {
        self.notification_view = if from_detail
            && self
                .notices
                .iter()
                .any(|n| Some(n.id) == id && n.generation == generation)
        {
            NotificationView::Detail {
                id: id.unwrap(),
                generation,
                scroll,
            }
        } else {
            NotificationView::List
        };
    }

    fn reconcile_notification_view(&mut self) {
        match self.notification_view.clone() {
            NotificationView::List => {}
            NotificationView::Detail {
                id,
                generation,
                scroll,
            } => {
                let Some(notice) = self.viewed_notice() else {
                    self.notification_view = NotificationView::List;
                    return;
                };
                let scroll = scroll.min(
                    notice
                        .text_lines(self.locale)
                        .len()
                        .saturating_sub(NOTICE_ROWS),
                );
                self.notification_view = NotificationView::Detail {
                    id,
                    generation,
                    scroll,
                };
            }
            NotificationView::Options {
                id,
                generation,
                scroll,
                from_detail,
                entries,
                selected,
            } => {
                if id.is_some() && self.viewed_notice().is_none() {
                    self.notification_view = NotificationView::List;
                    return;
                }
                let next = self.notice_options(id, generation);
                // A removed action never transfers focus to Delete/Clear or to
                // another named action. Return to its originating view instead.
                if let Some(index) = entries
                    .get(selected)
                    .and_then(|old| next.iter().position(|item| item == old))
                {
                    self.notification_view = NotificationView::Options {
                        id,
                        generation,
                        scroll,
                        from_detail,
                        entries: next,
                        selected: index,
                    };
                } else {
                    self.notification_options_back(id, generation, scroll, from_detail);
                }
            }
        }
    }

    pub fn notification_service(&mut self, available: bool) {
        self.notifications_unavailable = !available;
        self.reconcile_notification_view();
    }

    pub fn notification_failed(&mut self) {
        self.open_dialog(Dialog::Info(InfoKind::NotificationError));
    }

    fn remember_selection(&mut self) {
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

    fn restore_menu_selection(&mut self) {
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

    fn restore_recent_selection(&mut self) {
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

    fn filtered_applications(&self) -> impl Iterator<Item = &MenuApp> {
        self.matching_applications(&self.search_query)
    }

    fn matching_applications(&self, query: &str) -> impl Iterator<Item = &MenuApp> {
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

    fn edit_search(&mut self, key: Key) -> Request {
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

    fn window_busy(&self, id: u64) -> bool {
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

    fn app_window(&self, desktop_id: &str) -> Option<&WindowInfo> {
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

    pub fn modal_active(&self) -> bool {
        self.dialog.is_some()
            || self.search_input.is_some()
            || self.system_view.is_some()
            || matches!(self.application_view, ApplicationView::ConfirmRetry { .. })
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

    fn valid_target(&self, target: &AppTarget) -> bool {
        match target {
            AppTarget::Desktop(_) => self.target_app(target).is_some(),
            AppTarget::Window { .. } => self.target_window(target).is_some(),
        }
    }

    fn selected_target(&self) -> Option<AppTarget> {
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

    fn target_primary(&self, target: &AppTarget) -> Action {
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

    fn activate_target(&self, target: &AppTarget) -> Request {
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

    fn app_options(&self, target: &AppTarget) -> Vec<ApplicationOption> {
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

    fn push_application_view(&mut self, view: ApplicationView) {
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

    fn back_application_view(&mut self) {
        self.application_view = self.application_back.pop().unwrap_or_default();
        self.reconcile_application_view();
    }

    fn reset_application_view(&mut self) {
        self.application_view = ApplicationView::List;
        self.application_back.clear();
    }

    fn reconcile_application_view(&mut self) {
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

    pub fn system_busy(&self) -> bool {
        self.lock_busy()
            || self.system_operation_busy()
            || self.sleep_progress.is_some_and(|progress| progress.busy())
            || self.sleep_snapshot.monitor == sleep::MonitorState::Preparing
    }

    fn system_operation_busy(&self) -> bool {
        self.system_request.is_some_and(|request| {
            matches!(
                request.phase,
                SystemPhase::Pending | SystemPhase::Accepted | SystemPhase::Uncertain
            )
        })
    }

    pub fn lock_busy(&self) -> bool {
        self.lock_state
            .is_some_and(|state| state.phase != locking::Phase::Failed)
    }

    /// Both manual and automatic requests use the same monotonic identity and
    /// lifecycle. An automatic request preserves the user's underlying view.
    pub fn begin_lock(&mut self, show_status: bool) -> Request {
        if self.system_busy() {
            return Request::None;
        }
        self.allocate_lock(show_status)
    }

    /// Sleep preparation may acquire its own lock while its request is busy.
    /// It never bypasses another lock or an actual session/power operation,
    /// and it never replaces the view underneath the separate lock surface.
    pub fn begin_sleep_lock(&mut self, request: Option<u64>) -> Request {
        if self.lock_busy() || self.system_operation_busy() {
            return Request::None;
        }
        let permitted = match request {
            Some(request) => self.sleep_progress.is_some_and(|progress| {
                progress.request == request && progress.stage == sleep::Stage::Locking
            }),
            None => self.sleep_snapshot.monitor == sleep::MonitorState::Preparing,
        };
        if !permitted {
            return Request::None;
        }
        self.allocate_lock(false)
    }

    /// Critical battery requests share the manual request counter and backend,
    /// but may reuse an existing lock and never replace the underlying view.
    pub fn begin_battery_suspend(&mut self) -> Request {
        if self.system_operation_busy()
            || self.sleep_progress.is_some_and(|progress| progress.busy())
            || !self.sleep_allowed()
        {
            return Request::None;
        }
        self.allocate_suspend(false)
    }

    fn allocate_suspend(&mut self, show_status: bool) -> Request {
        let Some(request) = self.next_sleep_request.checked_add(1) else {
            return Request::None;
        };
        self.next_sleep_request = request;
        self.sleep_dismissed = None;
        self.sleep_progress = Some(sleep::Progress {
            request,
            stage: sleep::Stage::Preparing,
            failure: None,
        });
        if show_status {
            self.system_view = Some(SystemView::SuspendStatus { scroll: 0 });
        }
        Request::PrepareSuspend { request }
    }

    fn allocate_lock(&mut self, show_status: bool) -> Request {
        let Some(id) = self.next_lock_request.checked_add(1) else {
            return Request::None;
        };
        self.next_lock_request = id;
        self.lock_state = Some(locking::State {
            id,
            phase: locking::Phase::Starting,
        });
        if show_status {
            self.system_view = Some(SystemView::LockStatus { scroll: 0 });
        }
        Request::LockSession { request: id }
    }

    /// The coordinator owns ordering and recovery. Observations only update
    /// data; neither an external sleep nor a late reply opens a Shell card.
    pub fn set_sleep_snapshot(&mut self, snapshot: sleep::Snapshot) -> bool {
        if self.sleep_snapshot == snapshot {
            return false;
        }
        self.sleep_snapshot = snapshot;
        self.clamp_system_scroll();
        true
    }

    pub fn set_sleep_progress(&mut self, progress: Option<sleep::Progress>) -> bool {
        if self.sleep_progress == progress {
            return false;
        }
        if let Some(progress) = progress {
            self.next_sleep_request = self.next_sleep_request.max(progress.request);
        }
        self.sleep_progress = progress;
        self.clamp_system_scroll();
        true
    }

    fn sleep_allowed(&self) -> bool {
        self.sleep_snapshot.capability == Capability::Allowed
            && self.sleep_snapshot.monitor == sleep::MonitorState::Ready
    }

    fn sleep_cancellation(&self) -> Request {
        self.sleep_progress
            .filter(|progress| {
                matches!(
                    progress.stage,
                    sleep::Stage::Preparing | sleep::Stage::Locking
                )
            })
            .map_or(Request::None, |progress| Request::CancelSuspend {
                request: progress.request,
            })
    }

    fn dismiss_sleep_status(&mut self) {
        self.sleep_dismissed = self.sleep_progress.map(|progress| progress.request);
    }

    pub fn lock_changed(&mut self, state: locking::State) -> bool {
        let Some(current) = self.lock_state.as_mut() else {
            return false;
        };
        if current.id != state.id
            || current.phase == locking::Phase::Failed
            || state.phase == locking::Phase::Starting
        {
            return false;
        }
        *current = state;
        self.clamp_system_scroll();
        true
    }

    pub fn lock_unlocked(&mut self, id: u64) -> bool {
        if !self.lock_state.is_some_and(|state| {
            state.id == id
                && matches!(
                    state.phase,
                    locking::Phase::Locked | locking::Phase::Uncertain
                )
        }) {
            return false;
        }
        self.lock_state = None;
        if matches!(self.system_view, Some(SystemView::LockStatus { .. })) {
            self.system_view = None;
        }
        true
    }

    pub fn sync_system(&mut self, snapshot: SystemSnapshot) {
        self.system_snapshot = snapshot;
        self.clamp_system_scroll();
    }

    /// A late result updates its original request, without taking focus or
    /// reopening a card after Home/Esc. An unknown outcome remains latched.
    pub fn system_finished(
        &mut self,
        id: u64,
        operation: Operation,
        result: Result<(), Failure>,
    ) -> bool {
        let Some(request) = self.system_request.as_mut() else {
            return false;
        };
        if request.id != id
            || request.operation != operation
            || request.phase != SystemPhase::Pending
        {
            return false;
        }
        request.phase = match result {
            Ok(()) => SystemPhase::Accepted,
            Err(Failure::Unknown) => SystemPhase::Uncertain,
            Err(error) => SystemPhase::Failed(error),
        };
        self.clamp_system_scroll();
        true
    }

    pub fn control_reading(&self, control: Control) -> controls::Reading {
        match control {
            Control::Brightness => self.controls_snapshot.brightness,
            Control::Volume => self.controls_snapshot.volume,
        }
    }

    pub fn control_failure(&self, control: Control) -> Option<controls::Failure> {
        self.controls_failure.filter(|_| {
            self.controls_failure_target
                .is_none_or(|target| target == control)
        })
    }

    /// Late, duplicated or differently targeted replies never replace a newer
    /// observation. Completing a request does not reopen its view or move focus.
    pub fn controls_message(&mut self, message: controls::Message) -> bool {
        let Some(pending) = self.controls_request else {
            return false;
        };
        let (snapshot, failure) = match message {
            controls::Message::Refreshed { request, snapshot }
                if pending.id == request && pending.operation.is_none() =>
            {
                (snapshot, None)
            }
            controls::Message::Finished {
                request,
                operation,
                snapshot,
                result,
            } if pending.id == request && pending.operation == Some(operation) => {
                (snapshot, result.err())
            }
            _ => return false,
        };
        self.controls_snapshot = snapshot;
        self.controls_request = None;
        self.controls_failure = failure;
        self.controls_failure_target = pending.operation.map(|operation| match operation {
            controls::Operation::Adjust { control, .. } => control,
            controls::Operation::ToggleMute => Control::Volume,
        });
        true
    }

    pub(crate) fn refresh_controls(&mut self) -> Request {
        if self.controls_request.is_some() || self.system_busy() {
            return Request::None;
        }
        let Some(id) = self.next_controls_request.checked_add(1) else {
            return Request::None;
        };
        self.next_controls_request = id;
        self.controls_request = Some(ControlsRequest {
            id,
            operation: None,
        });
        self.controls_snapshot = controls::Snapshot::default();
        self.controls_failure = None;
        self.controls_failure_target = None;
        Request::RefreshControls { request: id }
    }

    fn control_operation_allowed(&self, operation: controls::Operation) -> bool {
        if self.controls_request.is_some() || self.system_busy() {
            return false;
        }
        match operation {
            controls::Operation::ToggleMute => {
                let reading = self.control_reading(Control::Volume);
                reading.availability == controls::Availability::Available
                    && reading.muted.is_some()
                    && !(reading.above_safe_limit && reading.muted == Some(true))
            }
            controls::Operation::Adjust { control, direction } => {
                let reading = self.control_reading(control);
                if reading.availability != controls::Availability::Available {
                    return false;
                }
                reading.percent.is_some_and(|percent| match direction {
                    Direction::Increase => percent < 100 && !reading.above_safe_limit,
                    Direction::Decrease => {
                        reading.above_safe_limit
                            || percent
                                > match control {
                                    Control::Brightness => 1,
                                    Control::Volume => 0,
                                }
                    }
                })
            }
        }
    }

    fn adjust_control(&mut self, operation: controls::Operation) -> Request {
        if !self.control_operation_allowed(operation) {
            return Request::None;
        }
        let Some(id) = self.next_controls_request.checked_add(1) else {
            return Request::None;
        };
        self.next_controls_request = id;
        self.controls_request = Some(ControlsRequest {
            id,
            operation: Some(operation),
        });
        self.controls_failure = None;
        // Keep the last observed reading until the backend returns read-back.
        self.controls_failure_target = None;
        Request::AdjustControl {
            request: id,
            operation,
        }
    }

    fn controls_actions(&self, selected: Control) -> [Action; 5] {
        let mute = if selected == Control::Volume
            && self.control_operation_allowed(controls::Operation::ToggleMute)
        {
            if self.controls_snapshot.volume.muted == Some(true) {
                Action::Unmute
            } else {
                Action::Mute
            }
        } else {
            Action::None
        };
        [
            Action::None,
            mute,
            Action::None,
            if self.controls_request.is_none() && !self.system_busy() {
                Action::Retry
            } else {
                Action::None
            },
            Action::Back,
        ]
    }

    fn controls_key(&mut self, selected: Control, key: Key, action: Action) -> Request {
        match key {
            Key::Left | Key::Right => self.adjust_control(controls::Operation::Adjust {
                control: selected,
                direction: if key == Key::Left {
                    Direction::Decrease
                } else {
                    Direction::Increase
                },
            }),
            _ => match action {
                Action::Mute | Action::Unmute => {
                    self.adjust_control(controls::Operation::ToggleMute)
                }
                Action::Retry => self.refresh_controls(),
                _ => Request::None,
            },
        }
    }

    pub fn radio_state(&self, kind: radios::Kind) -> &radios::State {
        match kind {
            radios::Kind::Wifi => &self.radios_snapshot.wifi,
            radios::Kind::Bluetooth => &self.radios_snapshot.bluetooth,
        }
    }

    pub fn radio_failure(&self, kind: radios::Kind) -> Option<radios::Failure> {
        self.radios_failure.filter(|_| {
            self.radios_failure_target
                .is_none_or(|target| target == kind)
        })
    }

    pub fn radio_busy(&self) -> bool {
        self.radios_request.is_some()
    }

    pub(crate) fn refresh_radios(&mut self) -> Request {
        self.begin_radio_operation(radios::Operation::Refresh)
    }

    fn begin_radio_operation(&mut self, operation: radios::Operation) -> Request {
        if self.radio_busy() || self.system_busy() {
            return Request::None;
        }
        let Some(id) = self.next_radio_request.checked_add(1) else {
            return Request::None;
        };
        self.next_radio_request = id;
        self.radios_request = Some(RadioRequest { id, operation });
        self.radios_failure = None;
        self.radios_failure_target = None;
        // Preserve the last observed state until an exact, matching read-back.
        Request::RadioOperation {
            request: id,
            operation,
        }
    }

    pub fn radios_message(&mut self, message: radios::Message) -> bool {
        let Some(pending) = self.radios_request else {
            return false;
        };
        if pending.id != message.request || pending.operation != message.operation {
            return false;
        }
        self.radios_snapshot = message.snapshot;
        self.radios_failure = message.result.err();
        self.radios_failure_target = pending.operation.kind();
        self.radios_request = None;
        if let radios::Operation::List { kind } = pending.operation {
            if let Some(SystemView::RadioList {
                kind: visible,
                selected,
            }) = &mut self.system_view
            {
                if *visible == kind {
                    let selected_id = self
                        .radio_entries
                        .get(*selected)
                        .map(|entry| entry.id.clone());
                    self.radio_entries = message.entries;
                    *selected = selected_id
                        .and_then(|id| self.radio_entries.iter().position(|entry| entry.id == id))
                        .unwrap_or_else(|| {
                            (*selected).min(self.radio_entries.len().saturating_sub(1))
                        });
                }
            }
        }
        // Completion must never reopen a dismissed list or move Shell focus.
        true
    }

    fn radio_actions(&self, kind: radios::Kind) -> [Action; 5] {
        let ready = !self.radio_busy() && !self.system_busy();
        let state = self.radio_state(kind);
        let toggle = if ready && state.availability == radios::Availability::Available {
            match state.powered {
                Some(true) => Action::TurnOff,
                Some(false) => Action::TurnOn,
                None => Action::None,
            }
        } else {
            Action::None
        };
        [
            if ready { Action::Open } else { Action::None },
            toggle,
            Action::None,
            if ready { Action::Retry } else { Action::None },
            Action::Back,
        ]
    }

    fn radio_key(&mut self, kind: radios::Kind, action: Action) -> Request {
        if self.radio_busy() || self.system_busy() {
            return Request::None;
        }
        match action {
            Action::Open => {
                self.radio_entries.clear();
                self.system_view = Some(SystemView::RadioList { kind, selected: 0 });
                self.begin_radio_operation(radios::Operation::List { kind })
            }
            Action::TurnOn | Action::TurnOff => {
                self.begin_radio_operation(radios::Operation::SetPowered {
                    kind,
                    powered: action == Action::TurnOn,
                })
            }
            Action::Retry => self.refresh_radios(),
            _ => Request::None,
        }
    }

    pub fn system_lines(&self) -> Vec<String> {
        match &self.system_view {
            Some(SystemView::Confirm { operation, .. }) => {
                ModalView::for_system_confirm(self.locale, *operation).lines()
            }
            Some(SystemView::Unavailable {
                operation,
                capability,
                ..
            }) => ModalView::for_system_unavailable(self.locale, *operation, *capability).lines(),
            Some(SystemView::Status { operation, .. }) => self
                .system_request
                .as_ref()
                .filter(|request| request.operation == *operation)
                .map_or_else(Vec::new, |request| {
                    ModalView::for_system_status(self.locale, request).lines()
                }),
            Some(SystemView::LockStatus { .. }) => self
                .lock_state
                .map(|state| ModalView::for_lock_status(self.locale, state.phase).lines())
                .unwrap_or_default(),
            Some(SystemView::RadioDetails { entry, .. }) => self.locale.radio_details_lines(entry),
            Some(SystemView::SuspendConfirm { .. }) => {
                ModalView::for_suspend_confirm(self.locale).lines()
            }
            Some(SystemView::SuspendUnavailable { .. }) => {
                ModalView::for_suspend_unavailable(self.locale, self.sleep_snapshot).lines()
            }
            Some(SystemView::SuspendStatus { .. }) => {
                ModalView::for_suspend_status(self.locale, self.sleep_snapshot, self.sleep_progress)
                    .lines()
            }
            _ => Vec::new(),
        }
    }

    fn clamp_system_scroll(&mut self) {
        let maximum = self.system_lines().len().saturating_sub(MODAL_ROWS);
        if let Some(
            SystemView::Confirm { scroll, .. }
            | SystemView::Unavailable { scroll, .. }
            | SystemView::Status { scroll, .. }
            | SystemView::LockStatus { scroll }
            | SystemView::RadioDetails { scroll, .. }
            | SystemView::SuspendConfirm { scroll }
            | SystemView::SuspendUnavailable { scroll }
            | SystemView::SuspendStatus { scroll },
        ) = &mut self.system_view
        {
            *scroll = (*scroll).min(maximum);
        }
    }

    fn refresh_system_menu(&mut self, selected: usize) -> Request {
        self.system_selected = selected.min(QuickAction::ALL.len() - 1);
        self.system_view = Some(SystemView::Menu {
            selected: self.system_selected,
        });
        self.system_snapshot = SystemSnapshot::default();
        Request::RefreshSystem
    }

    fn open_system_menu(&mut self) -> Request {
        if self.sleep_progress.is_some_and(|progress| {
            progress.busy() || self.sleep_dismissed != Some(progress.request)
        }) || self.sleep_snapshot.monitor == sleep::MonitorState::Preparing
        {
            self.system_view = Some(SystemView::SuspendStatus { scroll: 0 });
            Request::None
        } else if self.lock_state.is_some() {
            self.system_view = Some(SystemView::LockStatus { scroll: 0 });
            Request::None
        } else if let Some(request) = self.system_request {
            self.system_view = Some(SystemView::Status {
                operation: request.operation,
                scroll: 0,
            });
            Request::None
        } else {
            self.refresh_system_menu(self.system_selected)
        }
    }

    fn system_actions(&self, view: &SystemView) -> [Action; 5] {
        let mut actions = [
            Action::None,
            Action::None,
            Action::None,
            Action::None,
            Action::Back,
        ];
        match view {
            SystemView::Menu { selected } => match QuickAction::ALL.get(*selected) {
                Some(QuickAction::Control(control)) => return self.controls_actions(*control),
                Some(QuickAction::Radio(kind)) => return self.radio_actions(*kind),
                Some(QuickAction::Suspend) if !self.system_busy() => {
                    actions[0] = if self.sleep_allowed() {
                        Action::Select
                    } else {
                        Action::Read
                    };
                }
                Some(QuickAction::Lock) if !self.system_busy() => actions[0] = Action::Select,
                Some(QuickAction::System(operation))
                    if !self.system_busy()
                        && self.system_snapshot.capabilities[operation.index()]
                            != Capability::Checking =>
                {
                    actions[0] = Action::Select;
                }
                _ => {}
            },
            SystemView::RadioList { selected, .. } => {
                if !self.radio_busy() && !self.system_busy() {
                    if self.radio_entries.get(*selected).is_some() {
                        actions[0] = Action::Read;
                    }
                    actions[3] = Action::Retry;
                }
            }
            SystemView::RadioDetails { .. } => {}
            SystemView::Confirm { operation, .. } => {
                if !self.system_busy()
                    && matches!(
                        self.system_snapshot.capabilities[operation.index()],
                        Capability::Allowed | Capability::Authentication
                    )
                {
                    actions[0] = Action::Confirm;
                }
                actions[4] = Action::Decline;
            }
            SystemView::Unavailable { .. } => {
                actions[0] = Action::Acknowledge;
                actions[1] = Action::Retry;
            }
            SystemView::Status { .. } => {
                if !self.system_busy() {
                    actions[0] = Action::Acknowledge;
                }
            }
            SystemView::LockStatus { .. } => {
                if !self.lock_busy() {
                    actions[0] = Action::Acknowledge;
                }
            }
            SystemView::SuspendConfirm { .. } => {
                if !self.system_busy() && self.sleep_allowed() {
                    actions[0] = Action::Confirm;
                }
                actions[4] = Action::Decline;
            }
            SystemView::SuspendUnavailable { .. } => {
                actions[0] = Action::Acknowledge;
                if !self.system_busy() {
                    actions[1] = Action::Retry;
                }
            }
            SystemView::SuspendStatus { .. } => {
                if self.sleep_cancellation() != Request::None {
                    actions[4] = Action::Cancel;
                } else if !self.sleep_progress.is_some_and(|progress| progress.busy())
                    && self.sleep_snapshot.monitor != sleep::MonitorState::Preparing
                {
                    actions[0] = Action::Acknowledge;
                }
            }
        }
        actions
    }

    fn system_key(&mut self, key: Key) -> Request {
        let Some(view) = self.system_view.clone() else {
            return Request::None;
        };
        if key == Key::Home {
            let cancellation = self.sleep_cancellation();
            self.go_home();
            return cancellation;
        }
        let action = self.key_action(key);
        if matches!(
            action,
            Action::Back | Action::Decline | Action::Acknowledge | Action::Cancel
        ) {
            match view {
                SystemView::Menu { selected } => {
                    self.system_selected = selected;
                    self.system_view = None;
                }
                SystemView::Confirm { operation, .. }
                | SystemView::Unavailable { operation, .. } => {
                    self.system_selected = QuickAction::system_index(operation);
                    self.system_view = Some(SystemView::Menu {
                        selected: self.system_selected,
                    });
                }
                SystemView::Status { operation, .. } if !self.system_busy() => {
                    self.system_request = None;
                    return self.refresh_system_menu(QuickAction::system_index(operation));
                }
                SystemView::Status { .. } => self.system_view = None,
                SystemView::LockStatus { .. } if !self.lock_busy() => {
                    self.lock_state = None;
                    return self.refresh_system_menu(QuickAction::LOCK_INDEX);
                }
                SystemView::LockStatus { .. } => self.system_view = None,
                SystemView::RadioList { kind, .. } => {
                    self.system_selected = QuickAction::radio_index(kind);
                    self.system_view = Some(SystemView::Menu {
                        selected: self.system_selected,
                    });
                }
                SystemView::RadioDetails { kind, entry, .. } => {
                    let selected = self
                        .radio_entries
                        .iter()
                        .position(|current| current.id == entry.id)
                        .unwrap_or(0);
                    self.system_view = Some(SystemView::RadioList { kind, selected });
                }
                SystemView::SuspendConfirm { .. } | SystemView::SuspendUnavailable { .. } => {
                    self.system_selected = QuickAction::SUSPEND_INDEX;
                    self.system_view = Some(SystemView::Menu {
                        selected: self.system_selected,
                    });
                }
                SystemView::SuspendStatus { .. } => {
                    let cancellation = self.sleep_cancellation();
                    self.dismiss_sleep_status();
                    self.system_selected = QuickAction::SUSPEND_INDEX;
                    if cancellation != Request::None {
                        self.system_view = Some(SystemView::Menu {
                            selected: self.system_selected,
                        });
                        return cancellation;
                    }
                    if self.system_busy() {
                        self.system_view = None;
                    } else {
                        return self.refresh_system_menu(QuickAction::SUSPEND_INDEX);
                    }
                }
            }
            return Request::None;
        }
        if let SystemView::RadioList { kind, selected } = view {
            match key {
                Key::Up | Key::Down => {
                    let selected = if key == Key::Up {
                        selected.saturating_sub(1)
                    } else {
                        (selected + 1).min(self.radio_entries.len().saturating_sub(1))
                    };
                    self.system_view = Some(SystemView::RadioList { kind, selected });
                }
                _ if action == Action::Read => {
                    if let Some(entry) = self.radio_entries.get(selected).cloned() {
                        self.system_view = Some(SystemView::RadioDetails {
                            kind,
                            entry,
                            scroll: 0,
                        });
                    }
                }
                _ if action == Action::Retry => {
                    return self.begin_radio_operation(radios::Operation::List { kind });
                }
                _ => {}
            }
            return Request::None;
        }
        if let SystemView::Menu { selected } = view {
            let selected = selected.min(QuickAction::ALL.len() - 1);
            if !matches!(key, Key::Up | Key::Down) {
                match QuickAction::ALL[selected] {
                    QuickAction::Control(control) => {
                        return self.controls_key(control, key, action)
                    }
                    QuickAction::Radio(kind) => return self.radio_key(kind, action),
                    _ => {}
                }
            }
            match key {
                Key::Up | Key::Down => {
                    self.system_selected = if key == Key::Up {
                        selected.saturating_sub(1)
                    } else {
                        (selected + 1).min(QuickAction::ALL.len() - 1)
                    };
                    self.system_view = Some(SystemView::Menu {
                        selected: self.system_selected,
                    });
                }
                _ if matches!(action, Action::Select | Action::Read) && !self.system_busy() => {
                    let operation = match QuickAction::ALL[selected] {
                        QuickAction::System(operation) => operation,
                        QuickAction::Lock => return self.begin_lock(true),
                        QuickAction::Control(_) | QuickAction::Radio(_) => return Request::None,
                        QuickAction::Suspend => {
                            self.system_view = Some(if self.sleep_allowed() {
                                SystemView::SuspendConfirm { scroll: 0 }
                            } else {
                                SystemView::SuspendUnavailable { scroll: 0 }
                            });
                            return Request::None;
                        }
                    };
                    let capability = self.system_snapshot.capabilities[operation.index()];
                    self.system_view = Some(
                        if matches!(capability, Capability::Allowed | Capability::Authentication) {
                            SystemView::Confirm {
                                operation,
                                scroll: 0,
                            }
                        } else {
                            SystemView::Unavailable {
                                operation,
                                capability,
                                scroll: 0,
                            }
                        },
                    );
                }
                _ => {}
            }
            return Request::None;
        }
        if matches!(key, Key::Up | Key::Down | Key::Left | Key::Right) {
            let maximum = self.system_lines().len().saturating_sub(MODAL_ROWS);
            if let Some(
                SystemView::Confirm { scroll, .. }
                | SystemView::Unavailable { scroll, .. }
                | SystemView::Status { scroll, .. }
                | SystemView::LockStatus { scroll }
                | SystemView::RadioDetails { scroll, .. }
                | SystemView::SuspendConfirm { scroll }
                | SystemView::SuspendUnavailable { scroll }
                | SystemView::SuspendStatus { scroll },
            ) = &mut self.system_view
            {
                *scroll = modal_scroll_position(*scroll, maximum, key);
            }
            return Request::None;
        }
        match (view, action) {
            (SystemView::Confirm { operation, .. }, Action::Confirm) if !self.system_busy() => {
                let Some(id) = self.next_system_request.checked_add(1) else {
                    return Request::None;
                };
                self.next_system_request = id;
                self.system_request = Some(SystemRequest {
                    id,
                    operation,
                    phase: SystemPhase::Pending,
                });
                self.system_view = Some(SystemView::Status {
                    operation,
                    scroll: 0,
                });
                Request::SystemOperation {
                    request: id,
                    operation,
                }
            }
            (SystemView::Unavailable { operation, .. }, Action::Retry) => {
                self.refresh_system_menu(QuickAction::system_index(operation))
            }
            (SystemView::SuspendConfirm { .. }, Action::Confirm)
                if !self.system_busy() && self.sleep_allowed() =>
            {
                self.allocate_suspend(true)
            }
            (SystemView::SuspendUnavailable { .. }, Action::Retry) => {
                self.sleep_snapshot.capability = Capability::Checking;
                self.sleep_snapshot.monitor = sleep::MonitorState::Connecting;
                self.sleep_snapshot.delay_ms = 0;
                self.system_selected = QuickAction::SUSPEND_INDEX;
                self.system_view = Some(SystemView::Menu {
                    selected: self.system_selected,
                });
                Request::RefreshSleep
            }
            _ => Request::None,
        }
    }

    pub fn actions(&self) -> [Action; 5] {
        if self.is_text_editing() {
            return [
                Action::ApplyText,
                Action::None,
                Action::None,
                Action::None,
                Action::CancelText,
            ];
        }
        if let Some(dialog) = &self.dialog {
            if matches!(dialog, Dialog::ConfirmClose(_)) {
                return [
                    Action::Confirm,
                    Action::None,
                    Action::None,
                    Action::None,
                    Action::Decline,
                ];
            }
            return [
                if matches!(dialog, Dialog::Info(_)) {
                    Action::Acknowledge
                } else {
                    Action::Clear
                },
                Action::None,
                Action::None,
                Action::None,
                Action::Cancel,
            ];
        }
        if let Some(view) = &self.system_view {
            return self.system_actions(view);
        }
        if matches!(self.screen, Screen::Menu | Screen::Recent) {
            match &self.application_view {
                ApplicationView::List => {}
                ApplicationView::Options {
                    target,
                    entries,
                    selected,
                } => {
                    return [
                        Action::None,
                        if entries
                            .get(*selected)
                            .is_some_and(|option| self.application_option_enabled(target, option))
                        {
                            Action::Select
                        } else {
                            Action::None
                        },
                        Action::None,
                        Action::None,
                        Action::Back,
                    ]
                }
                ApplicationView::Windows { desktop_id, choice } => {
                    return [
                        if choice.selected().is_some() {
                            Action::Read
                        } else {
                            Action::None
                        },
                        if self.app_windows(desktop_id).iter().any(|window| {
                            Some(&window.id) == choice.selected() && !self.window_busy(window.id)
                        }) {
                            Action::Select
                        } else {
                            Action::None
                        },
                        Action::None,
                        Action::None,
                        Action::Back,
                    ]
                }
                ApplicationView::Details { target, .. } => {
                    return [
                        self.target_primary(target),
                        Action::Actions,
                        Action::None,
                        Action::None,
                        Action::Back,
                    ]
                }
                ApplicationView::ConfirmRetry { .. } => {
                    return [
                        Action::Retry,
                        Action::None,
                        Action::None,
                        Action::None,
                        Action::Cancel,
                    ]
                }
            }
        }
        if self.screen == Screen::Notifications {
            return match &self.notification_view {
                NotificationView::List => [
                    if self.notices.is_empty() {
                        Action::None
                    } else {
                        Action::Read
                    },
                    if self.notices.is_empty() && !self.notifications_unavailable {
                        Action::None
                    } else {
                        Action::Actions
                    },
                    Action::None,
                    Action::None,
                    Action::Back,
                ],
                NotificationView::Detail { .. } => [
                    Action::None,
                    Action::Actions,
                    Action::None,
                    Action::None,
                    Action::Back,
                ],
                NotificationView::Options {
                    entries, selected, ..
                } => [
                    Action::None,
                    if entries.get(*selected).is_some() {
                        Action::Select
                    } else {
                        Action::None
                    },
                    Action::None,
                    Action::None,
                    Action::Back,
                ],
            };
        }
        match self.screen {
            Screen::Home => [
                Action::Recent,
                Action::Quick,
                Action::Menu,
                Action::None,
                Action::Notifications,
            ],
            Screen::Menu | Screen::Recent => [
                self.selected_target()
                    .map_or(Action::None, |target| self.target_primary(&target)),
                if self.selected_target().is_some() {
                    Action::Actions
                } else {
                    Action::None
                },
                Action::None,
                if self.screen == Screen::Menu {
                    Action::Search
                } else {
                    Action::None
                },
                Action::Back,
            ],
            Screen::Notifications => unreachable!("handled above"),
        }
    }

    pub fn enter(&mut self, screen: Screen) {
        self.remember_selection();
        self.screen = screen;
        self.selected = 0;
        self.page = 0;
        self.dialog = None;
        self.dialog_scroll = 0;
        self.search_input = None;
        self.system_view = None;
        self.notification_view = NotificationView::List;
        self.reset_application_view();
        match screen {
            Screen::Menu => self.restore_menu_selection(),
            Screen::Recent => self.restore_recent_selection(),
            _ => {}
        }
    }

    pub fn go_home(&mut self) {
        // Home also arrives through IPC. Closing a status card must remember
        // its dismissal even when the coordinator cancelled it before here.
        if matches!(self.system_view, Some(SystemView::SuspendStatus { .. })) {
            self.dismiss_sleep_status();
        }
        self.enter(Screen::Home);
    }

    fn open_dialog(&mut self, dialog: Dialog) {
        self.dialog_window_app_id = match &dialog {
            Dialog::ConfirmClose(id) => self
                .live_windows
                .iter()
                .find(|window| window.id == *id)
                .map(|window| window.app_id.clone()),
            _ => None,
        };
        self.search_input = None;
        self.system_view = None;
        self.dialog_scroll = 0;
        self.dialog = Some(dialog);
    }

    pub fn dialog_lines(&self) -> Vec<String> {
        self.dialog.as_ref().map_or_else(Vec::new, |dialog| {
            ModalView::for_dialog(dialog, self.locale, &self.live_windows, self.notices.len())
                .lines()
        })
    }

    pub fn retry_lines(&self, desktop_id: &str) -> Vec<String> {
        ModalView::for_retry(
            self.locale,
            &self.target_name(&AppTarget::Desktop(desktop_id.into())),
        )
        .lines()
    }

    fn key_action(&self, key: Key) -> Action {
        match key {
            Key::Enter => self.actions()[0],
            Key::Soft(index) => self.actions().get(index).copied().unwrap_or(Action::None),
            Key::Escape | Key::Backspace => Action::Back,
            _ => Action::None,
        }
    }

    pub fn on_key(&mut self, key: Key) -> Request {
        if key == Key::Home {
            let cancellation = self.sleep_cancellation();
            if cancellation != Request::None {
                self.dismiss_sleep_status();
                self.go_home();
                return cancellation;
            }
        }
        if self.is_text_editing() {
            return self.edit_search(key);
        }
        if self.dialog.is_some() {
            return self.dialog_key(key);
        }
        if self.system_view.is_some() {
            return self.system_key(key);
        }
        if matches!(self.screen, Screen::Menu | Screen::Recent)
            && self.application_view != ApplicationView::List
        {
            return self.application_key(key);
        }
        if self.screen == Screen::Notifications && self.notification_view != NotificationView::List
        {
            return self.notification_key(key);
        }
        match key {
            Key::Home => self.go_home(),
            Key::Up | Key::Down | Key::Left | Key::Right => {
                let count = self.count();
                if count == 0 {
                    return Request::None;
                }
                let pos = self.selected;
                self.selected = match (self.screen, key) {
                    (Screen::Menu, Key::Left) => pos.saturating_sub(1),
                    (Screen::Menu, Key::Right) => (pos + 1).min(count - 1),
                    (Screen::Menu, Key::Up) => pos.saturating_sub(4),
                    (Screen::Menu, Key::Down) => (pos + 4).min(count - 1),
                    (_, Key::Up) => pos.saturating_sub(1),
                    (_, Key::Down) => (pos + 1).min(count - 1),
                    _ => pos,
                };
                if self.screen == Screen::Menu {
                    self.page = self.selected / 8;
                }
                self.remember_selection();
            }
            _ => return self.perform(self.key_action(key)),
        }
        Request::None
    }

    pub fn count(&self) -> usize {
        match self.screen {
            Screen::Home => 0,
            Screen::Menu => self.filtered_applications().count(),
            Screen::Recent => self.live_windows.len() + self.closed_windows.len(),
            Screen::Notifications => self.notices.len(),
        }
    }

    fn open_notification_options(&mut self) {
        let (id, generation, scroll, from_detail) = match &self.notification_view {
            NotificationView::List => (
                self.notices.get(self.selected).map(|n| n.id),
                self.notices.get(self.selected).map_or(0, |n| n.generation),
                0,
                false,
            ),
            NotificationView::Detail {
                id,
                generation,
                scroll,
            } => (Some(*id), *generation, *scroll, true),
            NotificationView::Options { .. } => return,
        };
        let entries = self.notice_options(id, generation);
        if !entries.is_empty() {
            self.notification_view = NotificationView::Options {
                id,
                generation,
                scroll,
                from_detail,
                entries,
                selected: 0,
            };
        }
    }

    fn open_application_options(&mut self, target: AppTarget) {
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

    fn perform(&mut self, action: Action) -> Request {
        match action {
            Action::Quick if self.screen == Screen::Home => return self.open_system_menu(),
            Action::Recent => self.enter(Screen::Recent),
            Action::Menu => self.enter(Screen::Menu),
            Action::Notifications => self.enter(Screen::Notifications),
            Action::Search if self.screen == Screen::Menu => {
                self.search_input = Some(TextInput::new(&self.search_query, 96));
            }
            Action::Back | Action::Cancel => self.go_home(),
            Action::Read => {
                let Some(notice) = self.notices.get(self.selected) else {
                    return Request::None;
                };
                self.notification_view = NotificationView::Detail {
                    id: notice.id,
                    generation: notice.generation,
                    scroll: 0,
                };
                if !self.notifications_unavailable {
                    return Request::MarkNotificationRead {
                        id: notice.id,
                        generation: notice.generation,
                    };
                }
            }
            Action::Actions => {
                if self.screen == Screen::Notifications {
                    self.open_notification_options();
                } else if let Some(target) = self.selected_target() {
                    self.open_application_options(target);
                }
            }
            Action::Open | Action::Launch => {
                return self
                    .selected_target()
                    .map_or(Request::None, |target| self.activate_target(&target));
            }
            _ => {}
        }
        Request::None
    }

    fn choose_application_option(
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

    fn application_key(&mut self, key: Key) -> Request {
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

    fn notification_key(&mut self, key: Key) -> Request {
        let before = self.notification_view.clone();
        self.reconcile_notification_view();
        if before != self.notification_view {
            return Request::None;
        }
        if key == Key::Home {
            self.go_home();
            return Request::None;
        }
        let view = self.notification_view.clone();
        if matches!(key, Key::Up | Key::Down | Key::Left | Key::Right) {
            match &view {
                NotificationView::Detail { scroll, .. } => {
                    let maximum = self.viewed_notice().map_or(0, |notice| {
                        notice
                            .text_lines(self.locale)
                            .len()
                            .saturating_sub(NOTICE_ROWS)
                    });
                    let next = scroll_position(*scroll, maximum, key);
                    if let NotificationView::Detail { scroll, .. } = &mut self.notification_view {
                        *scroll = next;
                    }
                }
                NotificationView::Options {
                    entries, selected, ..
                } => {
                    let next = match key {
                        Key::Up => selected.saturating_sub(1),
                        Key::Down => (*selected + 1).min(entries.len().saturating_sub(1)),
                        _ => *selected,
                    };
                    if let NotificationView::Options { selected, .. } = &mut self.notification_view
                    {
                        *selected = next;
                    }
                }
                NotificationView::List => {}
            }
            return Request::None;
        }
        match (view, self.key_action(key)) {
            (NotificationView::Detail { id, .. }, Action::Back) => {
                self.notification_view = NotificationView::List;
                if let Some(index) = self.notices.iter().position(|notice| notice.id == id) {
                    self.selected = index;
                }
            }
            (
                NotificationView::Options {
                    id,
                    generation,
                    scroll,
                    from_detail,
                    ..
                },
                Action::Back,
            ) => self.notification_options_back(id, generation, scroll, from_detail),
            (NotificationView::Detail { .. }, Action::Actions) => self.open_notification_options(),
            (
                NotificationView::Options {
                    id,
                    generation,
                    entries,
                    selected,
                    ..
                },
                Action::Select,
            ) => match entries.get(selected) {
                Some(NotificationOption::Invoke { key, .. }) => {
                    if !self.notifications_unavailable
                        && self.viewed_notice().is_some_and(|notice| {
                            notice.active && notice.actions.iter().any(|action| action.key == *key)
                        })
                    {
                        return Request::InvokeNotification {
                            id: id.unwrap(),
                            generation,
                            action: key.clone(),
                        };
                    }
                }
                Some(NotificationOption::Delete)
                    if !self.notifications_unavailable && self.viewed_notice().is_some() =>
                {
                    return Request::DismissNotification(id.unwrap())
                }
                Some(NotificationOption::Clear)
                    if !self.notifications_unavailable && !self.notices.is_empty() =>
                {
                    self.open_dialog(Dialog::ConfirmClear)
                }
                Some(NotificationOption::Retry) if self.notifications_unavailable => {
                    return Request::RetryNotifications
                }
                _ => {}
            },
            _ => {}
        }
        Request::None
    }

    fn dialog_key(&mut self, key: Key) -> Request {
        match key {
            Key::Left | Key::Up | Key::Right | Key::Down => {
                let maximum = self.dialog_lines().len().saturating_sub(MODAL_ROWS);
                self.dialog_scroll = modal_scroll_position(self.dialog_scroll, maximum, key);
                return Request::None;
            }
            Key::Home | Key::Escape | Key::Backspace | Key::Soft(4) => {
                self.dialog = None;
                return Request::None;
            }
            Key::Enter | Key::Soft(0) => {}
            _ => return Request::None,
        }
        match self.dialog.take() {
            Some(Dialog::ConfirmClear)
                if !self.notifications_unavailable && !self.notices.is_empty() =>
            {
                Request::ClearNotifications
            }
            Some(Dialog::ConfirmClose(id))
                if self.live_windows.iter().any(|window| {
                    window.id == id
                        && self
                            .dialog_window_app_id
                            .as_ref()
                            .is_some_and(|app_id| &window.app_id == app_id)
                }) && !self.window_busy(id) =>
            {
                Request::CloseWindow(id)
            }
            _ => Request::None,
        }
    }

    pub fn request_failed(&mut self, request: Request) {
        match request {
            Request::LockSession { request } => {
                self.lock_changed(locking::State {
                    id: request,
                    phase: locking::Phase::Failed,
                });
                return;
            }
            Request::RefreshSystem => {
                self.sync_system(SystemSnapshot {
                    capabilities: [Capability::Unavailable; 3],
                });
                return;
            }
            Request::RefreshSleep => {
                self.set_sleep_snapshot(sleep::Snapshot {
                    epoch: self.sleep_snapshot.epoch,
                    capability: Capability::Unavailable,
                    monitor: sleep::MonitorState::Unavailable,
                    delay_ms: 0,
                });
                return;
            }
            Request::PrepareSuspend { request } => {
                if self.sleep_progress.is_some_and(|progress| {
                    progress.request == request && progress.stage == sleep::Stage::Preparing
                }) {
                    self.set_sleep_progress(Some(sleep::Progress {
                        request,
                        stage: sleep::Stage::Failed,
                        failure: Some(Failure::Unavailable),
                    }));
                }
                return;
            }
            Request::CancelSuspend { request } => {
                if self
                    .sleep_progress
                    .is_some_and(|progress| progress.request == request && progress.busy())
                {
                    self.set_sleep_progress(Some(sleep::Progress {
                        request,
                        stage: sleep::Stage::Unknown,
                        failure: Some(Failure::Unknown),
                    }));
                }
                return;
            }
            Request::RefreshControls { request: id }
            | Request::AdjustControl { request: id, .. } => {
                let operation = match request {
                    Request::AdjustControl { operation, .. } => Some(operation),
                    _ => None,
                };
                if self.controls_request == Some(ControlsRequest { id, operation }) {
                    let target = operation.map(|operation| match operation {
                        controls::Operation::Adjust { control, .. } => control,
                        controls::Operation::ToggleMute => Control::Volume,
                    });
                    let reading = controls::Reading {
                        availability: controls::Availability::Unavailable,
                        percent: None,
                        muted: None,
                        above_safe_limit: false,
                    };
                    if target.is_none_or(|control| control == Control::Brightness) {
                        self.controls_snapshot.brightness = reading;
                    }
                    if target.is_none_or(|control| control == Control::Volume) {
                        self.controls_snapshot.volume = reading;
                    }
                    self.controls_request = None;
                    self.controls_failure = Some(controls::Failure::Unavailable);
                    self.controls_failure_target = target;
                }
                return;
            }
            Request::RadioOperation { request, operation } => {
                if self.radios_request
                    == Some(RadioRequest {
                        id: request,
                        operation,
                    })
                {
                    self.radios_request = None;
                    self.radios_failure = Some(radios::Failure::Unavailable);
                    self.radios_failure_target = operation.kind();
                    let unavailable = radios::State {
                        availability: radios::Availability::Unavailable,
                        powered: None,
                        detail: String::new(),
                    };
                    match operation {
                        radios::Operation::Refresh => {
                            self.radios_snapshot.wifi = unavailable.clone();
                            self.radios_snapshot.bluetooth = unavailable;
                        }
                        radios::Operation::SetPowered { kind, .. }
                        | radios::Operation::List { kind } => match kind {
                            radios::Kind::Wifi => self.radios_snapshot.wifi = unavailable,
                            radios::Kind::Bluetooth => self.radios_snapshot.bluetooth = unavailable,
                        },
                    }
                }
                return;
            }
            Request::SystemOperation { request, operation } => {
                self.system_finished(request, operation, Err(Failure::Unavailable));
                return;
            }
            _ => {}
        }
        let info = match request {
            Request::LaunchDesktop(_) | Request::RetryLaunch { .. } => InfoKind::LaunchError,
            Request::ActivateWindow(_) => InfoKind::Activation,
            Request::CloseWindow(_) => InfoKind::Close,
            Request::DismissNotification(_)
            | Request::ClearNotifications
            | Request::MarkNotificationRead { .. }
            | Request::InvokeNotification { .. }
            | Request::RetryNotifications => InfoKind::NotificationError,
            Request::None => InfoKind::Unsupported,
            Request::RefreshSystem
            | Request::RefreshSleep
            | Request::PrepareSuspend { .. }
            | Request::CancelSuspend { .. }
            | Request::RefreshControls { .. }
            | Request::AdjustControl { .. }
            | Request::RadioOperation { .. }
            | Request::SystemOperation { .. }
            | Request::LockSession { .. } => {
                unreachable!("handled above")
            }
        };
        self.open_dialog(Dialog::Info(info));
    }
}

fn scroll_position(scroll: usize, maximum: usize, key: Key) -> usize {
    match key {
        Key::Up => scroll.saturating_sub(1),
        Key::Down => (scroll + 1).min(maximum),
        Key::Left => scroll.saturating_sub(NOTICE_ROWS),
        Key::Right => (scroll + NOTICE_ROWS).min(maximum),
        _ => scroll,
    }
}

fn modal_scroll_position(scroll: usize, maximum: usize, key: Key) -> usize {
    match key {
        Key::Up => scroll.saturating_sub(1),
        Key::Down => (scroll + 1).min(maximum),
        Key::Left => scroll.saturating_sub(MODAL_ROWS),
        Key::Right => (scroll + MODAL_ROWS).min(maximum),
        _ => scroll,
    }
}

#[cfg(test)]
#[path = "unit_tests/model.rs"]
mod tests;
