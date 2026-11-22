//! Quick settings, bounded service requests and lock/suspend status views.

use super::{
    controls, locking, modal_scroll_position, radios, sleep, Action, Capability, Control,
    ControlsRequest, Direction, Failure, Key, ModalView, Operation, QuickAction, RadioRequest,
    Request, Shell, SystemPhase, SystemRequest, SystemSnapshot, SystemView, MODAL_ROWS,
};
use crate::i18n::ShellLabels;

impl Shell {
    pub fn system_busy(&self) -> bool {
        self.lock_busy()
            || self.system_operation_busy()
            || self.sleep_progress.is_some_and(|progress| progress.busy())
            || self.sleep_snapshot.monitor == sleep::MonitorState::Preparing
    }

    pub(super) fn system_operation_busy(&self) -> bool {
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

    pub(super) fn allocate_suspend(&mut self, show_status: bool) -> Request {
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

    pub(super) fn allocate_lock(&mut self, show_status: bool) -> Request {
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

    pub(super) fn sleep_allowed(&self) -> bool {
        self.sleep_snapshot.capability == Capability::Allowed
            && self.sleep_snapshot.monitor == sleep::MonitorState::Ready
    }

    pub(super) fn sleep_cancellation(&self) -> Request {
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

    pub(super) fn dismiss_sleep_status(&mut self) {
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

    pub(super) fn control_operation_allowed(&self, operation: controls::Operation) -> bool {
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

    pub(super) fn adjust_control(&mut self, operation: controls::Operation) -> Request {
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

    pub(super) fn controls_actions(&self, selected: Control) -> [Action; 5] {
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

    pub(super) fn controls_key(&mut self, selected: Control, key: Key, action: Action) -> Request {
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

    pub(super) fn begin_radio_operation(&mut self, operation: radios::Operation) -> Request {
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

    pub(super) fn radio_actions(&self, kind: radios::Kind) -> [Action; 5] {
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

    pub(super) fn radio_key(&mut self, kind: radios::Kind, action: Action) -> Request {
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

    pub(super) fn clamp_system_scroll(&mut self) {
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

    pub(super) fn refresh_system_menu(&mut self, selected: usize) -> Request {
        self.system_selected = selected.min(QuickAction::ALL.len() - 1);
        self.system_view = Some(SystemView::Menu {
            selected: self.system_selected,
        });
        self.system_snapshot = SystemSnapshot::default();
        Request::RefreshSystem
    }

    pub(super) fn open_system_menu(&mut self) -> Request {
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

    pub(super) fn system_actions(&self, view: &SystemView) -> [Action; 5] {
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

    pub(super) fn system_key(&mut self, key: Key) -> Request {
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
}
