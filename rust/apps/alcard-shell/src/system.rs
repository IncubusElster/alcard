//! Bounded login1 quick actions. No shell commands, privilege helpers, or
//! credentials: the system service and existing polkit agent own policy.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Operation {
    Logout,
    Reboot,
    PowerOff,
}

impl Operation {
    pub const ALL: [Self; 3] = [Self::Logout, Self::Reboot, Self::PowerOff];

    pub const fn index(self) -> usize {
        match self {
            Self::Logout => 0,
            Self::Reboot => 1,
            Self::PowerOff => 2,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Capability {
    Checking,
    Allowed,
    Authentication,
    Denied,
    Unsupported,
    Blocked,
    Unavailable,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Snapshot {
    pub capabilities: [Capability; 3],
}

impl Default for Snapshot {
    fn default() -> Self {
        Self {
            capabilities: [Capability::Checking; 3],
        }
    }
}

impl Snapshot {
    pub fn capability(&self, operation: Operation) -> Capability {
        self.capabilities[operation.index()]
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Failure {
    Unavailable,
    Denied,
    Cancelled,
    Blocked,
    SessionUnavailable,
    /// A read-only preflight/connection deadline; no mutation was attempted.
    Timeout,
    /// A mutation may have reached login1, but its result is not known.
    Unknown,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Message {
    Snapshot(Snapshot),
    Finished {
        request: u64,
        operation: Operation,
        /// Ok means the service accepted the request, not that power/session
        /// transition has finished. Never automatically repeat a request.
        result: Result<(), Failure>,
    },
}

use nix::unistd::Uid;
use smithay_client_toolkit::reexports::calloop::channel::Sender;
use std::{
    collections::HashMap,
    future::Future,
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, SyncSender},
        Arc,
    },
    thread,
    time::{Duration, Instant},
};
use zbus::{
    proxy::{CacheProperties, MethodFlags},
    wire::DynamicDeserialize,
    DBusError, OwnedObjectPath, OwnedVariant,
};

const SERVICE: &str = "org.freedesktop.login1";
const MANAGER_PATH: &str = "/org/freedesktop/login1";
const MANAGER: &str = "org.freedesktop.login1.Manager";
const SESSION: &str = "org.freedesktop.login1.Session";
const PROPERTIES: &str = "org.freedesktop.DBus.Properties";
const READ_TIMEOUT: Duration = Duration::from_secs(6);
const MUTATION_TIMEOUT: Duration = Duration::from_secs(120);
const CANCEL_INTERVAL: Duration = Duration::from_millis(50);
const MAX_REPLY_BYTES: usize = 64 * 1024;
const MAX_INHIBITORS: usize = 512;
const ROOT_CHECK_INHIBITORS: u64 = 1;

#[derive(Default)]
struct Control {
    stopped: AtomicBool,
    busy: AtomicBool,
    refreshing: AtomicBool,
}

enum Work {
    Refresh,
    Execute { request: u64, operation: Operation },
    Stop,
}

pub struct Client {
    commands: SyncSender<Work>,
    control: Arc<Control>,
}

impl Client {
    /// Creates one idle worker. The system bus is contacted only after refresh.
    pub fn start(updates: Sender<Message>) -> Self {
        let (commands, incoming) = mpsc::sync_channel(2);
        let control = Arc::new(Control::default());
        let worker_control = control.clone();
        let worker_updates = updates.clone();
        if let Err(error) = thread::Builder::new()
            .name("alcard-system".into())
            .spawn(move || run(incoming, worker_updates, worker_control))
        {
            eprintln!("System action worker unavailable: {error}");
            let _ = updates.send(Message::Snapshot(unavailable()));
        }
        Self { commands, control }
    }

    /// Coalesce an already pending refresh. Never wait for D-Bus or queue space.
    pub fn refresh(&self) -> bool {
        if self.control.busy.load(Ordering::Acquire) || self.control.stopped.load(Ordering::Acquire)
        {
            return false;
        }
        if self.control.refreshing.swap(true, Ordering::AcqRel) {
            return true;
        }
        if self.commands.try_send(Work::Refresh).is_err() {
            self.control.refreshing.store(false, Ordering::Release);
            return false;
        }
        true
    }

    /// Exactly one operation may be pending. An accepted or uncertain mutation
    /// keeps this latch for the client's lifetime, even if the UI is reopened.
    pub fn execute(&self, request: u64, operation: Operation) -> bool {
        if self.control.stopped.load(Ordering::Acquire)
            || self
                .control
                .busy
                .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
                .is_err()
        {
            return false;
        }
        if self
            .commands
            .try_send(Work::Execute { request, operation })
            .is_err()
        {
            self.control.busy.store(false, Ordering::Release);
            return false;
        }
        true
    }
}

impl Drop for Client {
    fn drop(&mut self) {
        self.control.stopped.store(true, Ordering::Release);
        // No join on the UI thread. Pending asynchronous calls notice stop
        // within CANCEL_INTERVAL; an idle worker wakes on this bounded queue.
        let _ = self.commands.try_send(Work::Stop);
    }
}

fn unavailable() -> Snapshot {
    Snapshot {
        capabilities: [Capability::Unavailable; 3],
    }
}

fn publish(updates: &Sender<Message>, previous: &mut Snapshot, next: Snapshot) -> bool {
    if *previous == next {
        return true;
    }
    *previous = next;
    updates.send(Message::Snapshot(next)).is_ok()
}

fn settle(control: &Control, result: Result<(), Failure>) {
    if !matches!(result, Ok(()) | Err(Failure::Unknown)) {
        control.busy.store(false, Ordering::Release);
    }
}

fn run(incoming: Receiver<Work>, updates: Sender<Message>, control: Arc<Control>) {
    let mut bus = None;
    let mut context = None;
    let mut snapshot = Snapshot::default();
    while let Ok(work) = incoming.recv() {
        if control.stopped.load(Ordering::Acquire) {
            break;
        }
        match work {
            Work::Stop => break,
            Work::Refresh => {
                if !publish(&updates, &mut snapshot, Snapshot::default()) {
                    break;
                }
                let budget = Budget::new(&control.stopped, READ_TIMEOUT);
                if bus.is_none() {
                    bus = SystemBus::connect(&budget).ok();
                }
                let result = bus
                    .as_mut()
                    .ok_or(Failure::Unavailable)
                    .and_then(|bus| probe(bus, &budget));
                let next = match result {
                    Ok((next, state)) => {
                        context = Some(next);
                        state
                    }
                    Err(_) => {
                        context = None;
                        bus = None;
                        unavailable()
                    }
                };
                control.refreshing.store(false, Ordering::Release);
                if control.stopped.load(Ordering::Acquire)
                    || !publish(&updates, &mut snapshot, next)
                {
                    break;
                }
            }
            Work::Execute { request, operation } => {
                let execution = match (bus.as_mut(), context.as_ref()) {
                    (Some(bus), Some(context)) => {
                        execute_pinned(bus, context, operation, &control.stopped)
                    }
                    _ => Execution::failed(Failure::Unavailable),
                };
                let mut next = snapshot;
                next.capabilities[operation.index()] = execution.capability;
                if execution.invalidated {
                    // A failed owner/transport preflight cannot be followed
                    // by an implicit reconnect and mutation.
                    context = None;
                    bus = None;
                    next = unavailable();
                }
                if !publish(&updates, &mut snapshot, next) {
                    break;
                }
                let result = execution.result;
                settle(&control, result);
                if control.stopped.load(Ordering::Acquire)
                    || updates
                        .send(Message::Finished {
                            request,
                            operation,
                            result,
                        })
                        .is_err()
                {
                    break;
                }
            }
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
enum WaitFailure {
    Deadline,
    Stopped,
    RuntimeUnavailable,
}

struct Budget<'a> {
    stopped: &'a AtomicBool,
    deadline: Instant,
}
impl<'a> Budget<'a> {
    fn new(stopped: &'a AtomicBool, duration: Duration) -> Self {
        Self {
            stopped,
            deadline: Instant::now() + duration,
        }
    }

    /// Drive zbus's current runtime on this worker. Race each operation against
    /// its deadline and cancellation timer so stalled I/O remains cancellable.
    fn wait<F: Future>(&self, future: F) -> Result<F::Output, WaitFailure> {
        let runtime =
            zruntime::SharedRuntime::current().map_err(|_| WaitFailure::RuntimeUnavailable)?;
        runtime.block_on(futures_lite::future::or(
            async {
                loop {
                    if self.stopped.load(Ordering::Acquire) {
                        return Err(WaitFailure::Stopped);
                    }
                    let now = Instant::now();
                    if now >= self.deadline {
                        return Err(WaitFailure::Deadline);
                    }
                    runtime
                        .sleep((self.deadline - now).min(CANCEL_INTERVAL))
                        .await;
                }
            },
            async { Ok(future.await) },
        ))
    }

    fn dbus<T>(
        &self,
        future: impl Future<Output = zbus::Result<T>>,
        mutating: bool,
    ) -> Result<T, Failure> {
        match self.wait(future) {
            Ok(result) => result.map_err(|error| bus_failure(&error, mutating)),
            Err(WaitFailure::Deadline) => Err(if mutating {
                Failure::Unknown
            } else {
                Failure::Timeout
            }),
            Err(WaitFailure::Stopped) => Err(if mutating {
                Failure::Unknown
            } else {
                Failure::Cancelled
            }),
            Err(WaitFailure::RuntimeUnavailable) => Err(Failure::Unavailable),
        }
    }
}

fn bus_failure(error: &zbus::Error, mutating: bool) -> Failure {
    let name = match error {
        zbus::Error::MethodError(name, _, _) => name.as_str().to_owned(),
        zbus::Error::FDO(error) => error.name().as_str().to_owned(),
        zbus::Error::DBus(error) => error.name().as_str().to_owned(),
        _ => String::new(),
    };
    match name.as_str() {
        "org.freedesktop.DBus.Error.AccessDenied"
        | "org.freedesktop.DBus.Error.AuthFailed"
        | "org.freedesktop.DBus.Error.InteractiveAuthorizationRequired"
        | "org.freedesktop.PolicyKit1.Error.NotAuthorized" => Failure::Denied,
        "org.freedesktop.DBus.Error.Cancelled"
        | "org.freedesktop.DBus.Error.Canceled"
        | "org.freedesktop.PolicyKit1.Error.Cancelled"
        | "org.freedesktop.PolicyKit1.Error.Canceled"
        | "org.freedesktop.login1.Cancelled"
        | "org.freedesktop.login1.ActionCancelled" => Failure::Cancelled,
        "org.freedesktop.login1.OperationInProgress"
        | "org.freedesktop.login1.ShutdownInProgress"
        | "org.freedesktop.login1.Inhibited" => Failure::Blocked,
        "org.freedesktop.login1.NoSessionForPID" | "org.freedesktop.login1.NoSuchSession" => {
            Failure::SessionUnavailable
        }
        // These explicit method errors guarantee the requested method was not
        // executed. They must not create an uncertain/accepted latch.
        "org.freedesktop.DBus.Error.UnknownMethod"
        | "org.freedesktop.DBus.Error.UnknownInterface"
        | "org.freedesktop.DBus.Error.UnknownObject"
        | "org.freedesktop.DBus.Error.InvalidArgs"
        | "org.freedesktop.DBus.Error.ServiceUnknown"
        | "org.freedesktop.DBus.Error.NameHasNoOwner" => Failure::Unavailable,
        _ if mutating => Failure::Unknown,
        _ => Failure::Unavailable,
    }
}

fn decode<T>(message: zbus::Message) -> Result<T, Failure>
where
    T: for<'d> DynamicDeserialize<'d>,
{
    if message.body().len() > MAX_REPLY_BYTES {
        return Err(Failure::Unavailable);
    }
    message
        .body()
        .deserialize()
        .map_err(|_| Failure::Unavailable)
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Session {
    id: String,
    path: String,
    uid: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Methods([bool; 3]);

#[derive(Clone, Debug)]
struct Pinned {
    owner: String,
    methods: Methods,
    session: Option<Session>,
}

struct Prepared {
    session: Option<Session>,
    capability: Capability,
}

struct Execution {
    result: Result<(), Failure>,
    capability: Capability,
    invalidated: bool,
}
impl Execution {
    fn failed(error: Failure) -> Self {
        Self {
            result: Err(error),
            capability: match error {
                Failure::Denied => Capability::Denied,
                Failure::Blocked => Capability::Blocked,
                _ => Capability::Unavailable,
            },
            invalidated: matches!(error, Failure::Unavailable | Failure::Timeout),
        }
    }
}

/// The small transport seam permits safety tests without accessing the host
/// system bus. Production has exactly one implementation, SystemBus.
trait Login1 {
    fn owner(&mut self, budget: &Budget<'_>) -> Result<String, Failure>;
    fn methods(&mut self, owner: &str, budget: &Budget<'_>) -> Result<Methods, Failure>;
    fn session(&mut self, owner: &str, budget: &Budget<'_>) -> Result<Session, Failure>;
    fn capability(
        &mut self,
        owner: &str,
        operation: Operation,
        budget: &Budget<'_>,
    ) -> Result<Capability, Failure>;
    fn inhibited(&mut self, owner: &str, budget: &Budget<'_>) -> Result<bool, Failure>;
    fn mutate(
        &mut self,
        owner: &str,
        operation: Operation,
        session: Option<&Session>,
        budget: &Budget<'_>,
    ) -> Result<(), Failure>;
}

fn execute_pinned(
    service: &mut impl Login1,
    pinned: &Pinned,
    operation: Operation,
    stopped: &AtomicBool,
) -> Execution {
    let read_budget = Budget::new(stopped, READ_TIMEOUT);
    let prepared = match prepare(service, pinned, operation, &read_budget) {
        Ok(prepared) => prepared,
        Err(error) => return Execution::failed(error),
    };
    let mutation_budget = Budget::new(stopped, MUTATION_TIMEOUT);
    Execution {
        result: service.mutate(
            &pinned.owner,
            operation,
            prepared.session.as_ref(),
            &mutation_budget,
        ),
        capability: prepared.capability,
        invalidated: false,
    }
}

fn probe(service: &mut impl Login1, budget: &Budget<'_>) -> Result<(Pinned, Snapshot), Failure> {
    let owner = service.owner(budget)?;
    let methods = service.methods(&owner, budget)?;
    let mut snapshot = unavailable();
    let session = if methods.0[Operation::Logout.index()] {
        match service.session(&owner, budget) {
            Ok(session) => {
                snapshot.capabilities[Operation::Logout.index()] = Capability::Allowed;
                Some(session)
            }
            Err(_) => None,
        }
    } else {
        snapshot.capabilities[Operation::Logout.index()] = Capability::Unsupported;
        None
    };
    for operation in [Operation::Reboot, Operation::PowerOff] {
        snapshot.capabilities[operation.index()] = if methods.0[operation.index()] {
            service
                .capability(&owner, operation, budget)
                .unwrap_or(Capability::Unavailable)
        } else {
            Capability::Unsupported
        };
    }
    if [Operation::Reboot, Operation::PowerOff]
        .iter()
        .any(|operation| enabled(snapshot.capability(*operation)))
    {
        let inhibited = service.inhibited(&owner, budget);
        for operation in [Operation::Reboot, Operation::PowerOff] {
            if enabled(snapshot.capability(operation)) {
                match inhibited {
                    Ok(true) => {
                        snapshot.capabilities[operation.index()] = Capability::Blocked;
                    }
                    Err(_) => {
                        snapshot.capabilities[operation.index()] = Capability::Unavailable;
                    }
                    Ok(false) => {}
                }
            }
        }
    }
    if service.owner(budget)? != owner {
        return Err(Failure::Unavailable);
    }
    Ok((
        Pinned {
            owner,
            methods,
            session,
        },
        snapshot,
    ))
}

fn prepare(
    service: &mut impl Login1,
    pinned: &Pinned,
    operation: Operation,
    budget: &Budget<'_>,
) -> Result<Prepared, Failure> {
    if service.owner(budget)? != pinned.owner {
        return Err(Failure::Unavailable);
    }
    if !pinned.methods.0[operation.index()] {
        return Err(Failure::Unavailable);
    }
    let (session, capability) = if operation == Operation::Logout {
        let expected = pinned.session.as_ref().ok_or(Failure::SessionUnavailable)?;
        let current = service.session(&pinned.owner, budget)?;
        if &current != expected {
            return Err(Failure::SessionUnavailable);
        }
        (Some(current), Capability::Allowed)
    } else {
        let capability = service.capability(&pinned.owner, operation, budget)?;
        match capability {
            Capability::Allowed | Capability::Authentication => {}
            Capability::Denied => return Err(Failure::Denied),
            Capability::Blocked => return Err(Failure::Blocked),
            _ => return Err(Failure::Unavailable),
        }
        if service.inhibited(&pinned.owner, budget)? {
            return Err(Failure::Blocked);
        }
        (None, capability)
    };
    // Read-only policy queries can race a daemon restart. Even if ownership
    // changes after this check, the mutation still targets this unique owner.
    if service.owner(budget)? != pinned.owner {
        return Err(Failure::Unavailable);
    }
    Ok(Prepared {
        session,
        capability,
    })
}

fn enabled(capability: Capability) -> bool {
    matches!(capability, Capability::Allowed | Capability::Authentication)
}

fn permission(value: &str) -> Capability {
    match value {
        "yes" => Capability::Allowed,
        "challenge" => Capability::Authentication,
        "no" => Capability::Denied,
        "na" => Capability::Unsupported,
        _ => Capability::Unavailable,
    }
}

type Inhibitor = (String, String, String, String, u32, u32);

fn shutdown_inhibited(inhibitors: &[Inhibitor]) -> Result<bool, Failure> {
    if inhibitors.len() > MAX_INHIBITORS {
        return Err(Failure::Unavailable);
    }
    for (what, _, _, mode, _, _) in inhibitors {
        if what.split(':').any(|kind| kind == "shutdown") {
            match mode.as_str() {
                "block" | "block-weak" => return Ok(true),
                "delay" => {}
                _ => return Err(Failure::Unavailable),
            }
        }
    }
    Ok(false)
}

fn session_from_properties(
    path: &OwnedObjectPath,
    mut properties: HashMap<String, OwnedVariant>,
    uid: u32,
) -> Result<Session, Failure> {
    let invalid = || Failure::SessionUnavailable;
    if properties.len() > 64
        || path.as_str().len() > 512
        || !path
            .as_str()
            .starts_with("/org/freedesktop/login1/session/")
    {
        return Err(invalid());
    }
    let id =
        String::try_from(properties.remove("Id").ok_or_else(invalid)?).map_err(|_| invalid())?;
    let (owner, _): (u32, OwnedObjectPath) = properties
        .remove("User")
        .ok_or_else(invalid)?
        .try_into()
        .map_err(|_| invalid())?;
    let kind =
        String::try_from(properties.remove("Type").ok_or_else(invalid)?).map_err(|_| invalid())?;
    let remote =
        bool::try_from(properties.remove("Remote").ok_or_else(invalid)?).map_err(|_| invalid())?;
    if id.is_empty()
        || id.len() > 128
        || matches!(id.as_str(), "self" | "auto")
        || !id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"_.-".contains(&byte))
        || owner != uid
        || remote
        || !matches!(kind.as_str(), "wayland" | "x11")
    {
        return Err(invalid());
    }
    Ok(Session {
        id,
        path: path.as_str().into(),
        uid: owner,
    })
}

struct SystemBus {
    connection: zbus::Connection,
}
impl SystemBus {
    fn connect(budget: &Budget<'_>) -> Result<Self, Failure> {
        Ok(Self {
            connection: budget.dbus(zbus::Connection::system(), false)?,
        })
    }
}
impl Login1 for SystemBus {
    fn owner(&mut self, budget: &Budget<'_>) -> Result<String, Failure> {
        let owner: String = decode(budget.dbus(
            self.connection.call_method(
                Some("org.freedesktop.DBus"),
                "/org/freedesktop/DBus",
                Some("org.freedesktop.DBus"),
                "GetNameOwner",
                &(SERVICE,),
            ),
            false,
        )?)?;
        if owner.len() > 255 || zbus::names::UniqueName::try_from(owner.as_str()).is_err() {
            return Err(Failure::Unavailable);
        }
        Ok(owner)
    }

    fn methods(&mut self, owner: &str, budget: &Budget<'_>) -> Result<Methods, Failure> {
        let xml: String = decode(budget.dbus(
            self.connection.call_method(
                Some(owner),
                MANAGER_PATH,
                Some("org.freedesktop.DBus.Introspectable"),
                "Introspect",
                &(),
            ),
            false,
        )?)?;
        supported_methods(&xml)
    }

    fn session(&mut self, owner: &str, budget: &Budget<'_>) -> Result<Session, Failure> {
        let path: OwnedObjectPath = decode(budget.dbus(
            self.connection.call_method(
                Some(owner),
                MANAGER_PATH,
                Some(MANAGER),
                "GetSessionByPID",
                &(std::process::id(),),
            ),
            false,
        )?)?;
        let properties = decode(budget.dbus(
            self.connection.call_method(
                Some(owner),
                path.as_str(),
                Some(PROPERTIES),
                "GetAll",
                &(SESSION,),
            ),
            false,
        )?)?;
        session_from_properties(&path, properties, Uid::current().as_raw())
    }

    fn capability(
        &mut self,
        owner: &str,
        operation: Operation,
        budget: &Budget<'_>,
    ) -> Result<Capability, Failure> {
        let method = match operation {
            Operation::Reboot => "CanReboot",
            Operation::PowerOff => "CanPowerOff",
            Operation::Logout => return Err(Failure::Unavailable),
        };
        let value: String = decode(
            budget.dbus(
                self.connection
                    .call_method(Some(owner), MANAGER_PATH, Some(MANAGER), method, &()),
                false,
            )?,
        )?;
        Ok(permission(&value))
    }

    fn inhibited(&mut self, owner: &str, budget: &Budget<'_>) -> Result<bool, Failure> {
        let inhibitors: Vec<Inhibitor> = decode(budget.dbus(
            self.connection.call_method(
                Some(owner),
                MANAGER_PATH,
                Some(MANAGER),
                "ListInhibitors",
                &(),
            ),
            false,
        )?)?;
        shutdown_inhibited(&inhibitors)
    }

    fn mutate(
        &mut self,
        owner: &str,
        operation: Operation,
        session: Option<&Session>,
        budget: &Budget<'_>,
    ) -> Result<(), Failure> {
        budget.dbus(
            async {
                let proxy: zbus::Proxy<'_> = zbus::proxy::Builder::new(&self.connection)
                    .destination(owner)
                    .path(MANAGER_PATH)
                    .interface(MANAGER)
                    .cache_properties(CacheProperties::No)
                    .build()
                    .await?;
                let reply: Option<()> = match operation {
                    Operation::Logout => {
                        let session = session.ok_or(zbus::Error::InvalidReply)?;
                        proxy
                            .call_with_flags(
                                "TerminateSession",
                                MethodFlags::ALLOW_INTERACTIVE_AUTH,
                                &(session.id.as_str(),),
                            )
                            .await?
                    }
                    Operation::Reboot | Operation::PowerOff => {
                        let method = if operation == Operation::Reboot {
                            "RebootWithFlags"
                        } else {
                            "PowerOffWithFlags"
                        };
                        // SD_LOGIND_ROOT_CHECK_INHIBITORS is the only power
                        // flag: never SKIP_INHIBITORS, force, kexec, or soft reboot.
                        proxy
                            .call_with_flags(
                                method,
                                MethodFlags::ALLOW_INTERACTIVE_AUTH,
                                &(ROOT_CHECK_INHIBITORS,),
                            )
                            .await?
                    }
                };
                reply.ok_or(zbus::Error::InvalidReply)
            },
            true,
        )
    }
}

/// Read only the bounded interface/method/argument subset of introspection XML.
/// No entity expansion, DTD loading, filesystem access, or external XML parser.
fn supported_methods(xml: &str) -> Result<Methods, Failure> {
    if xml.len() > MAX_REPLY_BYTES {
        return Err(Failure::Unavailable);
    }
    let mut methods = Methods([false; 3]);
    let mut remaining = xml;
    let mut manager = false;
    let mut current: Option<(Operation, usize, bool)> = None;
    let mut tags = 0;
    while let Some(start) = remaining.find('<') {
        remaining = &remaining[start + 1..];
        if remaining.starts_with("!--") {
            let end = remaining.find("-->").ok_or(Failure::Unavailable)?;
            remaining = &remaining[end + 3..];
            continue;
        }
        let end = remaining.find('>').ok_or(Failure::Unavailable)?;
        let tag = remaining[..end].trim();
        remaining = &remaining[end + 1..];
        tags += 1;
        if tags > 4096 {
            return Err(Failure::Unavailable);
        }
        let name = tag
            .split_ascii_whitespace()
            .next()
            .unwrap_or_default()
            .trim_end_matches('/');
        match name {
            "interface" => {
                manager = attribute(tag, "name") == Some(MANAGER);
                current = None;
            }
            "/interface" => {
                manager = false;
                current = None;
            }
            "method" if manager => {
                let operation = match attribute(tag, "name") {
                    Some("TerminateSession") => Some(Operation::Logout),
                    Some("RebootWithFlags") => Some(Operation::Reboot),
                    Some("PowerOffWithFlags") => Some(Operation::PowerOff),
                    _ => None,
                };
                current = operation.map(|operation| (operation, 0, true));
            }
            "arg" if manager => {
                if let Some((operation, count, valid)) = current.as_mut() {
                    if attribute(tag, "direction").unwrap_or("in") == "in" {
                        *count += 1;
                        let signature = if *operation == Operation::Logout {
                            "s"
                        } else {
                            "t"
                        };
                        *valid &= attribute(tag, "type") == Some(signature);
                    } else {
                        *valid = false;
                    }
                }
            }
            "/method" if manager => {
                if let Some((operation, count, valid)) = current.take() {
                    methods.0[operation.index()] = count == 1 && valid;
                }
            }
            _ => {}
        }
    }
    if manager || current.is_some() {
        return Err(Failure::Unavailable);
    }
    Ok(methods)
}

fn attribute<'a>(tag: &'a str, wanted: &str) -> Option<&'a str> {
    let mut remaining = tag
        .find(char::is_whitespace)
        .map(|index| &tag[index..])?
        .trim_start();
    while !remaining.is_empty() && remaining != "/" {
        let equal = remaining.find('=')?;
        let key = remaining[..equal].trim();
        remaining = remaining[equal + 1..].trim_start();
        let quote = remaining.chars().next()?;
        if quote != '"' && quote != '\'' {
            return None;
        }
        remaining = &remaining[1..];
        let end = remaining.find(quote)?;
        let value = &remaining[..end];
        if key == wanted {
            return Some(value);
        }
        remaining = remaining[end + 1..].trim_start();
    }
    None
}

#[cfg(test)]
#[path = "unit_tests/system.rs"]
mod tests;
