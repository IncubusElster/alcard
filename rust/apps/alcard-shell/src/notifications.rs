//! Freedesktop Notifications with bounded session history and event-driven updates.
//! The UI receives a coalesced wake, not one full snapshot per incoming message.
use futures_lite::StreamExt;
use std::{
    collections::HashMap,
    num::NonZeroUsize,
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, SyncSender},
        Arc, Mutex,
    },
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use zbus::{connection::Builder, interface, object_server::SignalEmitter, OwnedVariant};

use crate::model::{Notice, NoticeAction};
use smithay_client_toolkit::reexports::calloop::channel::Sender as UpdateSender;

const BUS_NAME: &str = "org.freedesktop.Notifications";
const OBJECT_PATH: &str = "/org/freedesktop/Notifications";
const INTERFACE: &str = "org.freedesktop.Notifications";
const HISTORY_LIMIT: usize = 64;
const COMMAND_LIMIT: usize = 32;
const ACTION_LIMIT: usize = 64;
const TITLE_LIMIT: usize = 128;
const BODY_LIMIT: usize = 4096;
const DEFAULT_TIMEOUT_MS: u64 = 5000;
const RETRY_MIN: Duration = Duration::from_secs(2);
const RETRY_MAX: Duration = Duration::from_secs(30);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NotificationEvent {
    pub id: u32,
    pub generation: u64,
}
#[derive(Clone, Debug)]
pub struct Snapshot {
    pub notices: Vec<Notice>,
    /// Changes only on Notify, including replacement, never on read/delete/expiry.
    pub event_generation: u64,
    pub latest_event: Option<NotificationEvent>,
}

#[derive(Clone, Debug)]
struct Entry {
    notice: Notice,
    internal: bool,
    resident: bool,
    transient: bool,
    deadline: Option<Instant>,
}
#[derive(Default)]
struct Store {
    next_id: u32,
    event_generation: u64,
    latest_event: Option<NotificationEvent>,
    entries: Vec<Entry>, // newest first; unread is independent of active/expiry
    battery_id: Option<u32>,
    battery_dismissed: bool,
}

#[derive(Debug)]
struct Hints {
    desktop_entry: Option<String>,
    urgency: u8,
    resident: bool,
    transient: bool,
}
impl Default for Hints {
    fn default() -> Self {
        Self {
            desktop_entry: None,
            urgency: 1,
            resident: false,
            transient: false,
        }
    }
}
impl Hints {
    fn parse(hints: &HashMap<String, OwnedVariant>) -> Self {
        let boolean = |key: &str| {
            hints
                .get(key)
                .and_then(|value| bool::try_from(value).ok())
                .unwrap_or(false)
        };
        let desktop_entry = hints
            .get("desktop-entry")
            .and_then(|value| <&str>::try_from(value).ok())
            .filter(|value| {
                !value.is_empty()
                    && value.chars().count() <= 255
                    && !value.chars().any(|c| c.is_control() || c == '/')
            })
            .map(str::to_owned);
        let urgency = hints
            .get("urgency")
            .and_then(|value| u8::try_from(value).ok())
            .filter(|value| *value <= 2)
            .unwrap_or(1);
        Self {
            desktop_entry,
            urgency,
            resident: boolean("resident"),
            transient: boolean("transient"),
        }
    }
}
struct Incoming<'a> {
    internal: bool,
    replaces_id: u32,
    app_name: &'a str,
    title: &'a str,
    body: &'a str,
    actions: &'a [String],
    hints: Hints,
    expire_timeout: i32,
}
#[derive(Default)]
struct Expired {
    changed: bool,
    /// Only external notification IDs have a D-Bus protocol lifetime.
    closed: Vec<u32>,
}
impl Store {
    /// One owned, persistent battery notice. External senders cannot replace
    /// it, and a critical transition upgrades it without growing history.
    fn battery_notice(
        &mut self,
        notice: Option<BatteryNotice>,
        now: Instant,
        created_at: i64,
    ) -> Result<Vec<u32>, String> {
        let Some(notice) = notice else {
            if let Some(id) = self.battery_id.take() {
                self.entries.retain(|entry| entry.notice.id != id);
            }
            return Ok(Vec::new());
        };
        if !notice.announce && self.battery_dismissed {
            return Ok(Vec::new());
        }
        if notice.announce {
            self.battery_dismissed = false;
        } else if let Some(entry) = self.battery_id.and_then(|id| {
            self.entries
                .iter_mut()
                .find(|entry| entry.notice.id == id && entry.internal && entry.notice.active)
        }) {
            // A quiet refresh changes presentation only. Keeping this exact
            // generation preserves an unseen announcement and a live banner,
            // without replaying it or moving the warning through history.
            entry.notice.title = plain(&notice.title, TITLE_LIMIT, false);
            entry.notice.description = plain(&notice.body, BODY_LIMIT, true);
            entry.notice.urgency = if notice.critical { 2 } else { 1 };
            return Ok(Vec::new());
        }
        let previous_event = self.latest_event;
        let (id, evicted) = self.notify(
            Incoming {
                internal: true,
                replaces_id: self.battery_id.unwrap_or(0),
                app_name: "Alcard Shell",
                title: &notice.title,
                body: &notice.body,
                actions: &[],
                hints: Hints {
                    urgency: if notice.critical { 2 } else { 1 },
                    ..Hints::default()
                },
                expire_timeout: 0,
            },
            now,
            created_at,
        )?;
        self.battery_id = Some(id);
        if !notice.announce {
            // Restore a warning removed during unknown telemetry without a new
            // announcement. An unrelated event may still await its first wake.
            self.latest_event = previous_event;
        }
        Ok(evicted)
    }

    fn snapshot(&self) -> Snapshot {
        Snapshot {
            notices: self
                .entries
                .iter()
                .map(|entry| entry.notice.clone())
                .collect(),
            event_generation: self.event_generation,
            latest_event: self.latest_event,
        }
    }
    fn notify(
        &mut self,
        incoming: Incoming<'_>,
        now: Instant,
        created_at: i64,
    ) -> Result<(u32, Vec<u32>), String> {
        // Reject unsupported input before touching an existing replacement.
        let actions = bounded_actions(incoming.actions)?;
        let replace = incoming.replaces_id != 0
            && self.entries.iter().any(|entry| {
                entry.notice.id == incoming.replaces_id
                    && entry.notice.active
                    && entry.internal == incoming.internal
            });
        let id = if replace {
            incoming.replaces_id
        } else {
            self.allocate_id()
        };
        if replace {
            self.entries.retain(|entry| entry.notice.id != id);
        }
        // At one billion notifications per second this counter lasts centuries.
        self.event_generation = self.event_generation.saturating_add(1);
        let generation = self.event_generation;
        let deadline = if incoming.hints.urgency == 2
            || incoming.hints.resident
            || incoming.expire_timeout == 0
        {
            None
        } else {
            let timeout = if incoming.expire_timeout > 0 {
                incoming.expire_timeout as u64
            } else {
                DEFAULT_TIMEOUT_MS
            };
            Some(now + Duration::from_millis(timeout))
        };
        self.entries.insert(
            0,
            Entry {
                internal: incoming.internal,
                notice: Notice {
                    id,
                    generation,
                    title: plain(incoming.title, TITLE_LIMIT, false),
                    description: plain(incoming.body, BODY_LIMIT, true),
                    app_name: plain(incoming.app_name, 64, false),
                    desktop_entry: incoming.hints.desktop_entry,
                    created_at,
                    urgency: incoming.hints.urgency,
                    unread: true,
                    actions,
                    active: true,
                },
                resident: incoming.hints.resident,
                transient: incoming.hints.transient,
                deadline,
            },
        );
        self.latest_event = Some(NotificationEvent { id, generation });
        let mut evicted = Vec::new();
        while self.entries.len() > HISTORY_LIMIT {
            if let Some(entry) = self.entries.pop() {
                if entry.notice.active && !entry.internal {
                    evicted.push(entry.notice.id);
                }
            }
        }
        Ok((id, evicted))
    }
    fn allocate_id(&mut self) -> u32 {
        loop {
            self.next_id = self.next_id.wrapping_add(1);
            if self.next_id != 0
                && !self
                    .entries
                    .iter()
                    .any(|entry| entry.notice.id == self.next_id)
            {
                return self.next_id;
            }
        }
    }
    fn mark_read(&mut self, id: u32, generation: u64) -> bool {
        let Some(entry) = self
            .entries
            .iter_mut()
            .find(|entry| entry.notice.id == id && entry.notice.generation == generation)
        else {
            return false;
        };
        let changed = entry.notice.unread;
        entry.notice.unread = false;
        changed
    }
    /// End the protocol lifetime while preserving ordinary history and unread.
    fn close(&mut self, id: u32) -> Option<bool> {
        let index = self
            .entries
            .iter()
            .position(|entry| entry.notice.id == id)?;
        if !self.entries[index].notice.active {
            return Some(false);
        }
        if self.entries[index].transient {
            self.entries.remove(index);
        } else {
            let entry = &mut self.entries[index];
            entry.notice.active = false;
            entry.deadline = None;
        }
        Some(true)
    }
    fn dismiss(&mut self, id: u32) -> Option<bool> {
        let index = self
            .entries
            .iter()
            .position(|entry| entry.notice.id == id)?;
        let entry = self.entries.remove(index);
        if self.battery_id == Some(id) {
            self.battery_id = None;
            self.battery_dismissed = true;
        }
        Some(entry.notice.active && !entry.internal)
    }
    fn withdraw(&mut self, id: u32) -> Option<bool> {
        let index = self
            .entries
            .iter()
            .position(|entry| entry.notice.id == id && !entry.internal)?;
        if !self.entries[index].notice.active {
            return Some(false);
        }
        // An explicit sender recall removes the message from the user's view,
        // including history, rather than keeping a now-withdrawn unread item.
        self.entries.remove(index);
        Some(true)
    }
    fn clear(&mut self) -> Vec<u32> {
        self.battery_id = None;
        self.battery_dismissed = true;
        self.entries
            .drain(..)
            .filter(|entry| entry.notice.active && !entry.internal)
            .map(|entry| entry.notice.id)
            .collect()
    }
    fn invoke(&mut self, id: u32, generation: u64, action: &str) -> Option<Vec<Signal>> {
        let entry = self.entries.iter_mut().find(|entry| {
            entry.notice.id == id
                && entry.notice.generation == generation
                && entry.notice.active
                && entry
                    .notice
                    .actions
                    .iter()
                    .any(|candidate| candidate.key == action)
        })?;
        entry.notice.unread = false;
        let resident = entry.resident;
        let mut signals = vec![Signal::Action(id, action.to_owned())];
        if !resident && self.close(id) == Some(true) {
            signals.push(Signal::Closed(id, 2));
        }
        Some(signals)
    }
    fn expire(&mut self, now: Instant) -> Expired {
        let expired: Vec<(u32, bool)> = self
            .entries
            .iter()
            .filter(|entry| {
                entry.notice.active && entry.deadline.is_some_and(|deadline| now >= deadline)
            })
            .map(|entry| (entry.notice.id, entry.internal))
            .collect();
        for (id, _) in &expired {
            self.close(*id);
        }
        Expired {
            changed: !expired.is_empty(),
            closed: expired
                .into_iter()
                .filter_map(|(id, internal)| (!internal).then_some(id))
                .collect(),
        }
    }
    fn next_deadline(&self) -> Option<Instant> {
        self.entries.iter().filter_map(|entry| entry.deadline).min()
    }
    fn disconnect(&mut self) -> bool {
        let active: Vec<u32> = self
            .entries
            .iter()
            .filter(|entry| entry.notice.active && !entry.internal)
            .map(|entry| entry.notice.id)
            .collect();
        for id in &active {
            self.close(*id);
        }
        !active.is_empty()
    }
}
fn plain(input: &str, limit: usize, multiline: bool) -> String {
    let mut chars = input
        .chars()
        .filter(|c| !c.is_control() || matches!(c, '\n' | '\t'))
        .map(|c| match c {
            '\n' if !multiline => ' ',
            '\t' => ' ',
            _ => c,
        });
    let mut text: String = chars.by_ref().take(limit).collect();
    if chars.next().is_some() && limit != 0 {
        text.pop();
        text.push('…');
    }
    text
}
fn bounded_actions(actions: &[String]) -> Result<Vec<NoticeAction>, String> {
    if !actions.len().is_multiple_of(2) || actions.len() / 2 > ACTION_LIMIT {
        return Err(format!(
            "actions must contain at most {ACTION_LIMIT} key/label pairs"
        ));
    }
    let mut result: Vec<NoticeAction> = Vec::new();
    for pair in actions.as_chunks::<2>().0 {
        let key = &pair[0];
        if key.is_empty()
            || key.chars().count() > 128
            || key.chars().any(char::is_control)
            || result.iter().any(|action| &action.key == key)
        {
            return Err("action keys must be unique, nonempty and at most 128 characters".into());
        }
        let label = if pair[1].is_empty() { key } else { &pair[1] };
        result.push(NoticeAction {
            key: key.clone(),
            label: plain(label, 64, false),
        });
    }
    Ok(result)
}
fn unix_time() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|time| time.as_secs().min(i64::MAX as u64) as i64)
        .unwrap_or(0)
}

#[derive(Debug)]
pub enum Message {
    /// At most one wake is queued until Client::take_update consumes the state.
    Changed,
    AuthenticationActive(bool),
    Available,
    Unavailable(String),
    CommandFailed(String),
}
#[derive(Debug)]
pub struct BatteryNotice {
    pub title: String,
    pub body: String,
    pub critical: bool,
    pub announce: bool,
}

#[cfg(test)]
#[path = "unit_tests/notifications_battery.rs"]
mod battery_tests;

#[derive(Debug)]
pub enum Command {
    PostInternal {
        title: String,
        body: String,
    },
    Battery(Option<BatteryNotice>),
    Retry,
    Dismiss(u32),
    Clear,
    MarkRead {
        id: u32,
        generation: u64,
    },
    Invoke {
        id: u32,
        generation: u64,
        action: String,
    },
    Shutdown,
}
#[derive(Debug)]
enum Work {
    User(Command),
    Wake,
}
#[derive(Default)]
struct Shared {
    store: Mutex<Store>,
    dirty: AtomicBool,
}
#[derive(Clone)]
struct Publisher {
    shared: Arc<Shared>,
    updates: UpdateSender<Message>,
}
impl Publisher {
    /// Call while holding the store lock. No snapshot is copied into a queue.
    fn changed(&self) {
        if !self.shared.dirty.swap(true, Ordering::AcqRel) {
            let _ = self.updates.send(Message::Changed);
        }
    }
}
#[derive(Clone)]
struct Waker {
    commands: SyncSender<Work>,
    pending: Arc<AtomicBool>,
}
impl Waker {
    fn wake(&self) {
        if !self.pending.swap(true, Ordering::AcqRel) && self.commands.try_send(Work::Wake).is_err()
        {
            // A full queue already guarantees that the worker will wake.
            self.pending.store(false, Ordering::Release);
        }
    }
    fn consumed(&self) {
        self.pending.store(false, Ordering::Release);
    }
}

pub struct Client {
    commands: SyncSender<Work>,
    shared: Arc<Shared>,
    stopped: Arc<AtomicBool>,
    wake: Waker,
}
impl Client {
    pub fn start(updates: UpdateSender<Message>) -> Self {
        Self::start_at(updates, None)
    }
    // An explicit private bus is used only by isolated integration tests.
    // There is no command-line test mode and no process environment mutation.
    fn start_at(updates: UpdateSender<Message>, address: Option<String>) -> Self {
        let (commands, requests) = mpsc::sync_channel(COMMAND_LIMIT);
        let wake = Waker {
            commands: commands.clone(),
            pending: Arc::new(AtomicBool::new(false)),
        };
        let shared = Arc::new(Shared::default());
        let stopped = Arc::new(AtomicBool::new(false));
        let publisher = Publisher {
            shared: shared.clone(),
            updates: updates.clone(),
        };
        let worker_wake = wake.clone();
        let worker_stopped = stopped.clone();
        if let Err(error) = thread::Builder::new()
            .name("alcard-notifications".into())
            .spawn(move || run(requests, publisher, worker_wake, worker_stopped, address))
        {
            let _ = updates.send(Message::Unavailable(format!(
                "Notification worker: {error}"
            )));
        }
        Self {
            commands,
            shared,
            stopped,
            wake,
        }
    }
    pub fn take_update(&self) -> Option<Snapshot> {
        let store = self.shared.store.lock().unwrap_or_else(|e| e.into_inner());
        self.shared
            .dirty
            .swap(false, Ordering::AcqRel)
            .then(|| store.snapshot())
    }
    pub fn request(&self, command: Command) -> Result<(), String> {
        // Bound payloads before placing them in the queue; the Wayland thread
        // only copies a bounded amount and never waits for D-Bus or the worker.
        let command = match command {
            Command::PostInternal { title, body } => Command::PostInternal {
                title: plain(&title, TITLE_LIMIT, false),
                body: plain(&body, BODY_LIMIT, true),
            },
            command => command,
        };
        self.commands
            .try_send(Work::User(command))
            .map_err(|error| match error {
                mpsc::TrySendError::Full(_) => "Notification service is busy; try again".into(),
                mpsc::TrySendError::Disconnected(_) => "Notification service disconnected".into(),
            })
    }
}
impl Drop for Client {
    fn drop(&mut self) {
        self.stopped.store(true, Ordering::Release);
        // Never block the UI while shutting down. A full queue already wakes
        // the worker, which checks stopped before processing another command.
        let _ = self.commands.try_send(Work::User(Command::Shutdown));
        self.wake.wake();
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Signal {
    Closed(u32, u32),
    Action(u32, String),
}
struct Server {
    publisher: Publisher,
    wake: Waker,
}
#[interface(name = "org.freedesktop.Notifications")]
impl Server {
    fn get_capabilities(&self) -> Vec<&str> {
        vec!["body", "actions"]
    }
    #[allow(clippy::too_many_arguments)]
    async fn notify(
        &self,
        app_name: String,
        replaces_id: u32,
        _app_icon: String,
        summary: String,
        body: String,
        actions: Vec<String>,
        hints: HashMap<String, OwnedVariant>,
        expire_timeout: i32,
        #[zbus(signal_emitter)] context: SignalEmitter<'_>,
    ) -> zbus::fdo::Result<u32> {
        let (id, evicted) = {
            let mut store = self
                .publisher
                .shared
                .store
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            let result = store
                .notify(
                    Incoming {
                        internal: false,
                        replaces_id,
                        app_name: &app_name,
                        title: &summary,
                        body: &body,
                        actions: &actions,
                        hints: Hints::parse(&hints),
                        expire_timeout,
                    },
                    Instant::now(),
                    unix_time(),
                )
                .map_err(zbus::fdo::Error::InvalidArgs)?;
            self.publisher.changed();
            result
        };
        self.wake.wake();
        for id in evicted {
            context
                .connection()
                .emit_signal(
                    None::<&str>,
                    OBJECT_PATH,
                    INTERFACE,
                    "NotificationClosed",
                    &(id, 4_u32),
                )
                .await
                .map_err(|error| zbus::fdo::Error::Failed(error.to_string()))?;
        }
        Ok(id)
    }
    async fn close_notification(
        &self,
        id: u32,
        #[zbus(signal_emitter)] context: SignalEmitter<'_>,
    ) -> zbus::fdo::Result<()> {
        {
            let mut store = self
                .publisher
                .shared
                .store
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            match store.withdraw(id) {
                Some(true) => self.publisher.changed(),
                Some(false) => {
                    return Err(zbus::fdo::Error::InvalidArgs(
                        "notification already closed".into(),
                    ))
                }
                None => {
                    return Err(zbus::fdo::Error::InvalidArgs(
                        "unknown notification ID".into(),
                    ))
                }
            }
        }
        self.wake.wake();
        context
            .connection()
            .emit_signal(
                None::<&str>,
                OBJECT_PATH,
                INTERFACE,
                "NotificationClosed",
                &(id, 3_u32),
            )
            .await
            .map_err(|error| zbus::fdo::Error::Failed(error.to_string()))
    }
    fn get_server_information(&self) -> (&str, &str, &str, &str) {
        ("Alcard Shell", "Alcard", env!("CARGO_PKG_VERSION"), "1.2")
    }
}
fn emit(connection: &zbus::Connection, signal: Signal) -> zbus::Result<()> {
    zbus::block_on(async {
        match signal {
            Signal::Closed(id, reason) => {
                connection
                    .emit_signal(
                        None::<&str>,
                        OBJECT_PATH,
                        INTERFACE,
                        "NotificationClosed",
                        &(id, reason),
                    )
                    .await
            }
            Signal::Action(id, action) => {
                connection
                    .emit_signal(
                        None::<&str>,
                        OBJECT_PATH,
                        INTERFACE,
                        "ActionInvoked",
                        &(id, action),
                    )
                    .await
            }
        }
    })
}
fn connect(
    publisher: &Publisher,
    wake: &Waker,
    address: Option<&str>,
) -> Result<zbus::Connection, String> {
    let runtime =
        zruntime::SharedRuntime::current().map_err(|error| format!("D-Bus runtime: {error}"))?;
    runtime.block_on(async {
        let builder = match address {
            Some(address) => Builder::address(address),
            None => Builder::session(),
        };
        let connection = builder
            .max_queued(NonZeroUsize::new(COMMAND_LIMIT).unwrap())
            .serve_at(
                OBJECT_PATH,
                Server {
                    publisher: publisher.clone(),
                    wake: wake.clone(),
                },
            )
            .build()
            .await
            .map_err(|error| format!("D-Bus connection: {error}"))?;
        // Request explicitly without replacement: a retry must never displace
        // the notification server of another desktop.
        let reply = connection
            .request_name_with_flags(BUS_NAME, zbus::fdo::RequestNameFlags::DO_NOT_QUEUE)
            .await
            .map_err(|error| format!("Cannot own {BUS_NAME}: {error}"))?;
        if !matches!(
            reply,
            zbus::fdo::RequestNameReply::PrimaryOwner | zbus::fdo::RequestNameReply::AlreadyOwner
        ) {
            return Err("Another notification service already owns the session name".into());
        }
        Ok(connection)
    })
}
fn handle_command(
    command: Command,
    publisher: &Publisher,
    connection: Option<&zbus::Connection>,
) -> Result<bool, String> {
    if matches!(command, Command::Shutdown) {
        return Ok(true);
    }
    let signals = {
        let mut store = publisher
            .shared
            .store
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let (signals, changed) = match command {
            Command::Battery(notice) => {
                let evicted = store.battery_notice(notice, Instant::now(), unix_time())?;
                (
                    evicted
                        .into_iter()
                        .map(|id| Signal::Closed(id, 4))
                        .collect(),
                    true,
                )
            }
            Command::PostInternal { title, body } => {
                let (_, evicted) = store.notify(
                    Incoming {
                        internal: true,
                        replaces_id: 0,
                        app_name: "Alcard Shell",
                        title: &title,
                        body: &body,
                        actions: &[],
                        hints: Hints::default(),
                        expire_timeout: -1,
                    },
                    Instant::now(),
                    unix_time(),
                )?;
                (
                    evicted
                        .into_iter()
                        .map(|id| Signal::Closed(id, 4))
                        .collect(),
                    true,
                )
            }
            Command::Retry => (Vec::new(), false),
            Command::Dismiss(id) => {
                let removed = store.dismiss(id);
                let signals = if removed == Some(true) {
                    vec![Signal::Closed(id, 2)]
                } else {
                    Vec::new()
                };
                (signals, removed.is_some())
            }
            Command::Clear => {
                let changed = !store.entries.is_empty();
                (
                    store
                        .clear()
                        .into_iter()
                        .map(|id| Signal::Closed(id, 2))
                        .collect(),
                    changed,
                )
            }
            Command::MarkRead { id, generation } => (Vec::new(), store.mark_read(id, generation)),
            Command::Invoke {
                id,
                generation,
                action,
            } => {
                if connection.is_none() {
                    let _ = publisher.updates.send(Message::CommandFailed(
                        "Notification service is unavailable".into(),
                    ));
                    return Ok(false);
                }
                match store.invoke(id, generation, &action) {
                    Some(signals) => (signals, true),
                    None => {
                        let _ = publisher.updates.send(Message::CommandFailed(
                            "Notification changed or its action is no longer available".into(),
                        ));
                        return Ok(false);
                    }
                }
            }
            Command::Shutdown => unreachable!(),
        };
        if changed {
            publisher.changed();
        }
        signals
    };
    if let Some(connection) = connection {
        for signal in signals {
            emit(connection, signal).map_err(|error| error.to_string())?;
        }
    }
    Ok(false)
}
fn expire_notifications(
    publisher: &Publisher,
    connection: Option<&zbus::Connection>,
    now: Instant,
) -> Result<(), String> {
    let expired = {
        let mut store = publisher
            .shared
            .store
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let expired = store.expire(now);
        if expired.changed {
            publisher.changed();
        }
        expired
    };
    if let Some(connection) = connection {
        for id in expired.closed {
            emit(connection, Signal::Closed(id, 1)).map_err(|error| error.to_string())?;
        }
    }
    Ok(())
}
fn serve(
    connection: &zbus::Connection,
    commands: &Receiver<Work>,
    publisher: &Publisher,
    wake: &Waker,
    stopped: &AtomicBool,
) -> Result<(), String> {
    let disconnected = Arc::new(AtomicBool::new(false));
    let monitor_disconnected = disconnected.clone();
    let monitor_wake = wake.clone();
    let monitor_updates = publisher.updates.clone();
    // zbus shares this thread's runtime. Its helper drives bus I/O and these
    // owned tasks while the command loop blocks; dropping the task guards
    // cancels the observers before reconnecting.
    let runtime = zruntime::SharedRuntime::current().map_err(|error| error.to_string())?;
    let mut messages = zbus::MessageStream::from(connection);
    let _monitor = runtime.spawn("alcard-notifications-connection", async move {
        while let Some(message) = messages.next().await {
            if message.is_err() {
                break;
            }
        }
        monitor_disconnected.store(true, Ordering::Release);
        let _ = monitor_updates.send(Message::AuthenticationActive(true));
        monitor_wake.wake();
    });
    let auth_connection = connection.clone();
    let auth_updates = publisher.updates.clone();
    let _authentication = runtime.spawn("alcard-authentication-presence", async move {
        if let Err(error) =
            crate::authentication::watch(auth_connection, auth_updates.clone()).await
        {
            eprintln!("Authentication presentation monitor unavailable: {error}");
            let _ = auth_updates.send(Message::AuthenticationActive(true));
        }
    });
    loop {
        if stopped.load(Ordering::Acquire) {
            return Ok(());
        }
        if disconnected.load(Ordering::Acquire) {
            return Err("D-Bus session disconnected; reconnecting".into());
        }
        let deadline = publisher
            .shared
            .store
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .next_deadline();
        let command = match deadline {
            Some(deadline) => {
                commands.recv_timeout(deadline.saturating_duration_since(Instant::now()))
            }
            None => commands
                .recv()
                .map_err(|_| mpsc::RecvTimeoutError::Disconnected),
        };
        match command {
            Ok(Work::Wake) => wake.consumed(),
            Ok(Work::User(command)) => {
                if handle_command(command, publisher, Some(connection))? {
                    return Ok(());
                }
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => return Ok(()),
        }
        expire_notifications(publisher, Some(connection), Instant::now())?;
    }
}
fn run(
    commands: Receiver<Work>,
    publisher: Publisher,
    wake: Waker,
    stopped: Arc<AtomicBool>,
    address: Option<String>,
) {
    let mut retry = RETRY_MIN;
    let mut last_error = None;
    loop {
        if stopped.load(Ordering::Acquire) {
            return;
        }
        let error = match connect(&publisher, &wake, address.as_deref()) {
            Ok(connection) => {
                retry = RETRY_MIN;
                last_error = None;
                let _ = publisher.updates.send(Message::Available);
                {
                    let _store = publisher
                        .shared
                        .store
                        .lock()
                        .unwrap_or_else(|e| e.into_inner());
                    publisher.changed();
                }
                match serve(&connection, &commands, &publisher, &wake, &stopped) {
                    Ok(()) => return,
                    Err(error) => error,
                }
            }
            Err(error) => error,
        };
        {
            let mut store = publisher
                .shared
                .store
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            if store.disconnect() {
                publisher.changed();
            }
        }
        let _ = publisher.updates.send(Message::AuthenticationActive(true));
        if last_error.as_ref() != Some(&error) {
            let _ = publisher.updates.send(Message::Unavailable(error.clone()));
            last_error = Some(error);
        }
        let deadline = Instant::now() + retry;
        retry = (retry * 2).min(RETRY_MAX);
        loop {
            if stopped.load(Ordering::Acquire) {
                return;
            }
            let wake_at = publisher
                .shared
                .store
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .next_deadline()
                .map_or(deadline, |expiry| expiry.min(deadline));
            match commands.recv_timeout(wake_at.saturating_duration_since(Instant::now())) {
                Ok(Work::Wake) => wake.consumed(),
                Ok(Work::User(Command::Retry)) => {
                    retry = RETRY_MIN;
                    break;
                }
                Ok(Work::User(command)) => {
                    if handle_command(command, &publisher, None).unwrap_or(false) {
                        return;
                    }
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => return,
            }
            // Internal notices have their normal lifetime even while the
            // session bus is unavailable. No D-Bus traffic is needed here.
            let _ = expire_notifications(&publisher, None, Instant::now());
            if Instant::now() >= deadline {
                break;
            }
        }
    }
}

#[cfg(test)]
#[path = "unit_tests/notifications.rs"]
mod tests;
