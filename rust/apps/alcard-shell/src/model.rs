//! Pure Shell state machine; real desktop entries and compositor windows only.
//! Tests construct controlled state directly, without a runtime demo mode.
use crate::{
    controls::{self, Control, Direction},
    i18n::{Locale, ShellLabels},
    locking,
    modal::{View as ModalView, MODAL_ROWS},
    radios, sleep,
    status::StatusSnapshot,
    system::{Capability, Failure, Operation, Snapshot as SystemSnapshot},
    theme::Theme,
};
use alcard_ui::dialog::{ChoiceList, TextInput};

mod applications;
mod notifications;
mod quick_settings;

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

    pub fn modal_active(&self) -> bool {
        self.dialog.is_some()
            || self.search_input.is_some()
            || self.system_view.is_some()
            || matches!(self.application_view, ApplicationView::ConfirmRetry { .. })
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
