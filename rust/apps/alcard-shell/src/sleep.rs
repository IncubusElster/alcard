//! Owned login1 suspend intent and an independent delay-inhibitor monitor.

use crate::system::{Capability, Failure};
use futures_lite::{future, StreamExt};
use nix::unistd::Uid;
use smithay_client_toolkit::reexports::calloop::channel::Sender;
use std::{
    collections::HashMap,
    future::Future,
    num::NonZeroUsize,
    os::fd::{AsRawFd, OwnedFd},
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc::{self, Receiver, SyncSender},
        Arc, Mutex, MutexGuard,
    },
    task::{Poll, Waker},
    thread,
    time::{Duration, Instant},
};
use zbus::{
    message::Sequence,
    proxy::{CacheProperties, MethodFlags},
    wire::DynamicDeserialize,
    DBusError, OwnedObjectPath, OwnedVariant,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MonitorState {
    Connecting,
    Ready,
    Preparing,
    Unavailable,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Snapshot {
    pub epoch: u64,
    pub capability: Capability,
    pub monitor: MonitorState,
    pub delay_ms: u64,
}
impl Default for Snapshot {
    fn default() -> Self {
        Self {
            epoch: 0,
            capability: Capability::Checking,
            monitor: MonitorState::Connecting,
            delay_ms: 0,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stage {
    Preparing,
    Locking,
    Queued,
    Requested,
    Ended,
    Failed,
    Unknown,
    Cancelled,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Progress {
    pub request: u64,
    pub stage: Stage,
    pub failure: Option<Failure>,
}
impl Progress {
    pub fn busy(&self) -> bool {
        matches!(
            self.stage,
            Stage::Preparing | Stage::Locking | Stage::Queued | Stage::Requested | Stage::Unknown
        )
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Message {
    Snapshot(Snapshot),
    Refreshed {
        request: u64,
        snapshot: Snapshot,
    },
    Prepared {
        request: u64,
        epoch: u64,
        result: Result<(), Failure>,
    },
    Preparing {
        epoch: u64,
        cycle: u64,
    },
    CycleEnded {
        epoch: u64,
        cycle: Option<u64>,
    },
    Finished {
        request: u64,
        epoch: u64,
        result: Result<(), Failure>,
    },
    Lost {
        epoch: u64,
    },
}

#[derive(Clone, Debug)]
pub struct Permit {
    valid: Arc<AtomicBool>,
    deadline: Option<Instant>,
}
impl Permit {
    pub fn new() -> Self {
        Self {
            valid: Arc::new(AtomicBool::new(true)),
            deadline: None,
        }
    }
    pub fn valid(&self) -> bool {
        self.valid.load(Ordering::Acquire)
            && self
                .deadline
                .is_none_or(|deadline| Instant::now() < deadline)
    }
    pub fn revoke(&self) {
        self.valid.store(false, Ordering::Release);
    }
    /// Tighten only this clone's authority while retaining shared revocation.
    /// A battery receipt therefore expires even if the UI stops dispatching
    /// during the worker's asynchronous preflight. Manual permits stay unbounded.
    pub fn until(mut self, deadline: Instant) -> Self {
        self.deadline = Some(self.deadline.map_or(deadline, |old| old.min(deadline)));
        self
    }
}
impl Default for Permit {
    fn default() -> Self {
        Self::new()
    }
}

const SERVICE: &str = "org.freedesktop.login1";
const MANAGER_PATH: &str = "/org/freedesktop/login1";
const MANAGER: &str = "org.freedesktop.login1.Manager";
const SESSION: &str = "org.freedesktop.login1.Session";
const SEAT: &str = "org.freedesktop.login1.Seat";
const PROPERTIES: &str = "org.freedesktop.DBus.Properties";
const READ_TIMEOUT: Duration = Duration::from_secs(6);
const MUTATION_TIMEOUT: Duration = Duration::from_secs(120);
const CANCEL_INTERVAL: Duration = Duration::from_millis(50);
const MAX_REPLY_BYTES: usize = 64 * 1024;
const MAX_INHIBITORS: usize = 512;
const ROOT_CHECK_INHIBITORS: u64 = 1;

#[derive(Clone, Debug, PartialEq, Eq)]
struct Session {
    id: String,
    path: String,
    uid: u32,
    kind: String,
    seat_id: String,
    seat_path: String,
}

#[derive(Clone)]
struct Context {
    epoch: u64,
    owner: String,
    session: Session,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PendingPhase {
    Preparing,
    Prepared,
    Queued,
    Dispatched,
}

struct Pending {
    request: u64,
    epoch: u64,
    cancel: Permit,
    lock: Option<Permit>,
    phase: PendingPhase,
    prepared: Option<Session>,
}
impl Pending {
    fn revoke(&self) {
        self.cancel.revoke();
        if let Some(permit) = &self.lock {
            permit.revoke();
        }
    }
}

#[derive(Default)]
struct State {
    snapshot: Snapshot,
    context: Option<Context>,
    connection: Option<zbus::Connection>,
    delay: Option<OwnedFd>,
    cycle: Option<u64>,
    next_cycle: u64,
    pending: Option<Pending>,
}

#[derive(Default)]
struct Notify {
    pending: AtomicBool,
    waker: Mutex<Option<Waker>>,
}
impl Notify {
    fn wake(&self) {
        self.pending.store(true, Ordering::Release);
        if let Some(waker) = self.waker.lock().unwrap_or_else(|e| e.into_inner()).take() {
            waker.wake();
        }
    }
    async fn wait(&self) {
        future::poll_fn(|cx| {
            let mut waker = self.waker.lock().unwrap_or_else(|e| e.into_inner());
            if self.pending.swap(false, Ordering::AcqRel) {
                Poll::Ready(())
            } else {
                *waker = Some(cx.waker().clone());
                Poll::Pending
            }
        })
        .await
    }
}

#[derive(Default)]
struct Shared {
    stopped: AtomicBool,
    refreshing: AtomicBool,
    refresh_request: AtomicU64,
    notify: Notify,
    state: Mutex<State>,
}
impl Shared {
    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }
    fn stop(&self) {
        self.stopped.store(true, Ordering::Release);
        let delay = {
            let mut state = self.lock();
            if let Some(pending) = state.pending.take() {
                pending.revoke();
            }
            state.context = None;
            state.connection = None;
            state.delay.take()
        };
        drop(delay);
        self.notify.wake();
    }
}

enum Work {
    Prepare {
        request: u64,
        epoch: u64,
        cancel: Permit,
    },
    Commit {
        request: u64,
        epoch: u64,
        permit: Permit,
        cancel: Permit,
    },
    Stop,
}

pub struct Client {
    commands: SyncSender<Work>,
    shared: Arc<Shared>,
}
impl Client {
    pub fn start(updates: Sender<Message>) -> Self {
        let (commands, incoming) = mpsc::sync_channel(2);
        let shared = Arc::new(Shared::default());
        let worker_shared = shared.clone();
        let worker_updates = updates.clone();
        if let Err(error) = thread::Builder::new()
            .name("alcard-sleep".into())
            .spawn(move || run(incoming, worker_updates, worker_shared))
        {
            shared.stop();
            eprintln!("Suspend worker unavailable: {error}");
            let _ = updates.send(Message::Snapshot(Snapshot {
                monitor: MonitorState::Unavailable,
                capability: Capability::Unavailable,
                ..Snapshot::default()
            }));
        }
        Self { commands, shared }
    }

    pub fn refresh(&self) -> bool {
        if self.shared.stopped.load(Ordering::Acquire) {
            return false;
        }
        self.shared.refreshing.store(true, Ordering::Release);
        self.shared.notify.wake();
        true
    }

    /// Correlate the battery's one read-only refresh with its discharge episode.
    /// A concurrent menu refresh may coalesce, but cannot acknowledge this token.
    pub fn refresh_for(&self, request: u64) -> bool {
        if request == 0
            || self.shared.stopped.load(Ordering::Acquire)
            || self
                .shared
                .refresh_request
                .compare_exchange(0, request, Ordering::AcqRel, Ordering::Acquire)
                .is_err()
        {
            return false;
        }
        self.shared.refreshing.store(true, Ordering::Release);
        self.shared.notify.wake();
        true
    }

    pub fn prepare(&self, request: u64) -> bool {
        let (epoch, cancel) = {
            let mut state = self.shared.lock();
            if self.shared.stopped.load(Ordering::Acquire)
                || state.pending.is_some()
                || state.snapshot.monitor != MonitorState::Ready
                || state.snapshot.capability != Capability::Allowed
                || state.delay.is_none()
                || state.context.is_none()
            {
                return false;
            }
            let epoch = state.snapshot.epoch;
            let cancel = Permit::new();
            state.pending = Some(Pending {
                request,
                epoch,
                cancel: cancel.clone(),
                lock: None,
                phase: PendingPhase::Preparing,
                prepared: None,
            });
            (epoch, cancel)
        };
        if self
            .commands
            .try_send(Work::Prepare {
                request,
                epoch,
                cancel,
            })
            .is_err()
        {
            clear_pending(&self.shared, request, epoch);
            return false;
        }
        true
    }

    pub fn commit(&self, request: u64, epoch: u64, permit: Permit) -> bool {
        let cancel = {
            let mut state = self.shared.lock();
            if self.shared.stopped.load(Ordering::Acquire)
                || state.snapshot.epoch != epoch
                || state.snapshot.monitor != MonitorState::Ready
                || state.delay.is_none()
                || !permit.valid()
            {
                return false;
            }
            let Some(pending) = state.pending.as_mut() else {
                return false;
            };
            if pending.request != request
                || pending.epoch != epoch
                || pending.phase != PendingPhase::Prepared
                || !pending.cancel.valid()
            {
                return false;
            }
            pending.lock = Some(permit.clone());
            pending.phase = PendingPhase::Queued;
            pending.cancel.clone()
        };
        if self
            .commands
            .try_send(Work::Commit {
                request,
                epoch,
                permit,
                cancel,
            })
            .is_err()
        {
            clear_pending(&self.shared, request, epoch);
            return false;
        }
        true
    }

    /// Revocation is synchronous even if the command worker is awaiting D-Bus.
    /// It cannot undo a method that has already been dispatched.
    pub fn cancel(&self, request: u64) {
        let mut state = self.shared.lock();
        if let Some(pending) = state
            .pending
            .as_ref()
            .filter(|pending| pending.request == request)
        {
            pending.revoke();
            if pending.phase == PendingPhase::Prepared {
                state.pending = None;
            }
        }
    }

    /// This path never queues behind SuspendWithFlags. Taking the owned FD out
    /// of the short shared-state lock releases this cycle's delay immediately.
    pub fn lock_ready(&self, epoch: u64, cycle: u64, permit: Permit) -> bool {
        release_delay(&self.shared, epoch, cycle, &permit)
    }
}
impl Drop for Client {
    fn drop(&mut self) {
        self.shared.stop();
        let _ = self.commands.try_send(Work::Stop);
    }
}

fn release_delay(shared: &Shared, epoch: u64, cycle: u64, permit: &Permit) -> bool {
    let delay = {
        let mut state = shared.lock();
        if shared.stopped.load(Ordering::Acquire)
            || state.snapshot.epoch != epoch
            || state.snapshot.monitor != MonitorState::Preparing
            || state.cycle != Some(cycle)
            || !permit.valid()
        {
            return false;
        }
        state.delay.take()
    };
    drop(delay);
    true
}

fn clear_pending(shared: &Shared, request: u64, epoch: u64) {
    let mut state = shared.lock();
    if state
        .pending
        .as_ref()
        .is_some_and(|pending| pending.request == request && pending.epoch == epoch)
    {
        if let Some(pending) = state.pending.take() {
            pending.revoke();
        }
    }
}

fn context_for(shared: &Shared, epoch: u64) -> Result<(Context, zbus::Connection), Failure> {
    let state = shared.lock();
    if state.snapshot.epoch != epoch
        || state.snapshot.monitor != MonitorState::Ready
        || state.delay.is_none()
    {
        return Err(Failure::Unavailable);
    }
    Ok((
        state.context.clone().ok_or(Failure::Unavailable)?,
        state.connection.clone().ok_or(Failure::Unavailable)?,
    ))
}

fn run(commands: Receiver<Work>, updates: Sender<Message>, shared: Arc<Shared>) {
    let runtime = match zruntime::SharedRuntime::current() {
        Ok(runtime) => runtime,
        Err(_) => {
            lose_epoch(&shared, &updates);
            shared.stop();
            return;
        }
    };
    let monitor_shared = shared.clone();
    let monitor_updates = updates.clone();
    // The runtime helper keeps this observer running while this thread blocks
    // on recv() or awaits a suspended/unknown mutation RPC.
    let _monitor = runtime.spawn("alcard-sleep-monitor", async move {
        monitor_forever(monitor_shared, monitor_updates).await;
    });
    while let Ok(work) = commands.recv() {
        if shared.stopped.load(Ordering::Acquire) {
            break;
        }
        match work {
            Work::Stop => break,
            Work::Prepare {
                request,
                epoch,
                cancel,
            } => {
                let result = runtime.block_on(prepare_request(&shared, request, epoch, &cancel));
                if result.is_err() {
                    clear_pending(&shared, request, epoch);
                }
                if updates
                    .send(Message::Prepared {
                        request,
                        epoch,
                        result,
                    })
                    .is_err()
                {
                    break;
                }
            }
            Work::Commit {
                request,
                epoch,
                permit,
                cancel,
            } => {
                let result =
                    runtime.block_on(commit_request(&shared, request, epoch, &permit, &cancel));
                if !matches!(result, Ok(()) | Err(Failure::Unknown)) {
                    clear_pending(&shared, request, epoch);
                }
                if updates
                    .send(Message::Finished {
                        request,
                        epoch,
                        result,
                    })
                    .is_err()
                {
                    break;
                }
            }
        }
    }
    shared.stop();
}

async fn prepare_request(
    shared: &Shared,
    request: u64,
    epoch: u64,
    cancel: &Permit,
) -> Result<(), Failure> {
    let (context, connection) = context_for(shared, epoch)?;
    let bus = SystemBus { connection };
    let budget = Budget::new(shared, Some(cancel), READ_TIMEOUT);
    preflight(&bus, &context.owner, &context.session, &budget).await?;
    budget.check(false)?;
    let mut state = shared.lock();
    if state.snapshot.epoch != epoch
        || state.snapshot.monitor != MonitorState::Ready
        || state.delay.is_none()
    {
        return Err(Failure::Unavailable);
    }
    let pending = state.pending.as_mut().ok_or(Failure::Cancelled)?;
    if pending.request != request
        || pending.epoch != epoch
        || pending.phase != PendingPhase::Preparing
        || !pending.cancel.valid()
    {
        return Err(Failure::Cancelled);
    }
    pending.prepared = Some(context.session);
    pending.phase = PendingPhase::Prepared;
    Ok(())
}

async fn commit_request(
    shared: &Shared,
    request: u64,
    epoch: u64,
    permit: &Permit,
    cancel: &Permit,
) -> Result<(), Failure> {
    let (context, connection) = context_for(shared, epoch)?;
    let expected = {
        let state = shared.lock();
        let pending = state.pending.as_ref().ok_or(Failure::Cancelled)?;
        if pending.request != request
            || pending.epoch != epoch
            || pending.phase != PendingPhase::Queued
            || !pending.cancel.valid()
            || !permit.valid()
        {
            return Err(Failure::Cancelled);
        }
        pending.prepared.clone().ok_or(Failure::Unavailable)?
    };
    let bus = SystemBus { connection };
    let budget = Budget::new(shared, Some(cancel), READ_TIMEOUT);
    preflight(&bus, &context.owner, &expected, &budget).await?;
    budget.check(false)?;
    if !permit.valid() {
        return Err(Failure::Cancelled);
    }
    let mutation_budget = Budget::new(shared, Some(cancel), MUTATION_TIMEOUT);
    let gate = Gate {
        shared,
        request,
        epoch,
        permit,
    };
    bus.suspend(&context.owner, &gate, &mutation_budget).await
}

struct Gate<'a> {
    shared: &'a Shared,
    request: u64,
    epoch: u64,
    permit: &'a Permit,
}
impl Gate<'_> {
    fn dispatch(&self) -> Result<(), Failure> {
        let mut state = self.shared.lock();
        if self.shared.stopped.load(Ordering::Acquire) {
            return Err(Failure::Cancelled);
        }
        if state.snapshot.epoch != self.epoch
            || state.snapshot.monitor != MonitorState::Ready
            || state.delay.is_none()
            || state.context.is_none()
        {
            return Err(Failure::Unavailable);
        }
        if state.snapshot.capability != Capability::Allowed {
            return Err(if state.snapshot.capability == Capability::Blocked {
                Failure::Blocked
            } else {
                Failure::Denied
            });
        }
        let pending = state.pending.as_mut().ok_or(Failure::Cancelled)?;
        if pending.request != self.request
            || pending.epoch != self.epoch
            || pending.phase != PendingPhase::Queued
            || !pending.cancel.valid()
            || !self.permit.valid()
        {
            return Err(Failure::Cancelled);
        }
        pending.phase = PendingPhase::Dispatched;
        Ok(())
    }
}

struct Budget<'a> {
    shared: &'a Shared,
    cancel: Option<&'a Permit>,
    deadline: Instant,
}
impl<'a> Budget<'a> {
    fn new(shared: &'a Shared, cancel: Option<&'a Permit>, duration: Duration) -> Self {
        Self {
            shared,
            cancel,
            deadline: Instant::now() + duration,
        }
    }
    fn check(&self, mutating: bool) -> Result<(), Failure> {
        let failure = if self.shared.stopped.load(Ordering::Acquire)
            || self.cancel.is_some_and(|permit| !permit.valid())
        {
            Some(Failure::Cancelled)
        } else if Instant::now() >= self.deadline {
            Some(Failure::Timeout)
        } else {
            None
        };
        match failure {
            Some(_) if mutating => Err(Failure::Unknown),
            Some(error) => Err(error),
            None => Ok(()),
        }
    }
    async fn dbus<T>(
        &self,
        future: impl Future<Output = zbus::Result<T>>,
        mutating: bool,
    ) -> Result<T, Failure> {
        self.check(false)?;
        let runtime = zruntime::SharedRuntime::current().map_err(|_| Failure::Unavailable)?;
        future::or(
            async {
                loop {
                    self.check(mutating)?;
                    runtime
                        .sleep(
                            CANCEL_INTERVAL
                                .min(self.deadline.saturating_duration_since(Instant::now())),
                        )
                        .await;
                }
            },
            async { future.await.map_err(|error| bus_failure(&error, mutating)) },
        )
        .await
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

fn permission(value: &str) -> Capability {
    match value {
        "yes" => Capability::Allowed,
        "challenge" => Capability::Authentication,
        "no" => Capability::Denied,
        "na" => Capability::Unsupported,
        "inhibited" | "inhibitor-blocked" | "challenge-inhibitor-blocked" => Capability::Blocked,
        _ => Capability::Unavailable,
    }
}

type Inhibitor = (String, String, String, String, u32, u32);
fn sleep_inhibited(inhibitors: &[Inhibitor]) -> Result<bool, Failure> {
    if inhibitors.len() > MAX_INHIBITORS {
        return Err(Failure::Unavailable);
    }
    for (what, _, _, mode, _, _) in inhibitors {
        if what.split(':').any(|kind| kind == "sleep") {
            match mode.as_str() {
                "block" | "block-weak" => return Ok(true),
                "delay" => {}
                _ => return Err(Failure::Unavailable),
            }
        }
    }
    Ok(false)
}

trait Login1: Sync {
    fn owner(&self, budget: &Budget<'_>) -> impl Future<Output = Result<String, Failure>> + Send;
    fn session(
        &self,
        owner: &str,
        budget: &Budget<'_>,
    ) -> impl Future<Output = Result<Session, Failure>> + Send;
    fn capability(
        &self,
        owner: &str,
        budget: &Budget<'_>,
    ) -> impl Future<Output = Result<Capability, Failure>> + Send;
    fn inhibited(
        &self,
        owner: &str,
        budget: &Budget<'_>,
    ) -> impl Future<Output = Result<bool, Failure>> + Send;
    fn preparing(
        &self,
        owner: &str,
        budget: &Budget<'_>,
    ) -> impl Future<Output = Result<(bool, u64, Sequence), Failure>> + Send;
}

async fn preflight(
    bus: &impl Login1,
    owner: &str,
    session: &Session,
    budget: &Budget<'_>,
) -> Result<(), Failure> {
    if bus.owner(budget).await? != owner {
        return Err(Failure::Unavailable);
    }
    if &bus.session(owner, budget).await? != session {
        return Err(Failure::SessionUnavailable);
    }
    match bus.capability(owner, budget).await? {
        Capability::Allowed => {}
        Capability::Denied | Capability::Authentication => return Err(Failure::Denied),
        Capability::Blocked => return Err(Failure::Blocked),
        _ => return Err(Failure::Unavailable),
    }
    if bus.inhibited(owner, budget).await? || bus.preparing(owner, budget).await?.0 {
        return Err(Failure::Blocked);
    }
    if bus.owner(budget).await? != owner {
        return Err(Failure::Unavailable);
    }
    budget.check(false)
}

struct SystemBus {
    connection: zbus::Connection,
}

fn decode<T: for<'d> DynamicDeserialize<'d>>(message: zbus::Message) -> Result<T, Failure> {
    if message.body().len() > MAX_REPLY_BYTES || !message.data().fds().is_empty() {
        return Err(Failure::Unavailable);
    }
    message
        .body()
        .deserialize()
        .map_err(|_| Failure::Unavailable)
}

fn decode_delay(message: zbus::Message) -> Result<OwnedFd, Failure> {
    if message.body().len() > MAX_REPLY_BYTES || message.data().fds().len() != 1 {
        return Err(Failure::Unavailable);
    }
    let owned: zbus::OwnedFd = message
        .body()
        .deserialize()
        .map_err(|_| Failure::Unavailable)?;
    // OwnedFd deserialization duplicates the received FD. Drop the complete
    // reply now; otherwise that hidden duplicate would prolong inhibition.
    drop(message);
    let fd: OwnedFd = owned.into();
    validate_delay_fd(fd)
}

fn validate_delay_fd(fd: OwnedFd) -> Result<OwnedFd, Failure> {
    // SAFETY: only flags/metadata of this owned, live descriptor are accessed.
    let flags = unsafe { nix::libc::fcntl(fd.as_raw_fd(), nix::libc::F_GETFD) };
    if flags < 0
        || unsafe {
            nix::libc::fcntl(
                fd.as_raw_fd(),
                nix::libc::F_SETFD,
                flags | nix::libc::FD_CLOEXEC,
            )
        } < 0
    {
        return Err(Failure::Unavailable);
    }
    let mut metadata = std::mem::MaybeUninit::<nix::libc::stat>::uninit();
    if unsafe { nix::libc::fstat(fd.as_raw_fd(), metadata.as_mut_ptr()) } < 0 {
        return Err(Failure::Unavailable);
    }
    let metadata = unsafe { metadata.assume_init() };
    if metadata.st_mode & nix::libc::S_IFMT != nix::libc::S_IFIFO {
        return Err(Failure::Unavailable);
    }
    Ok(fd)
}

impl SystemBus {
    async fn properties(
        &self,
        owner: &str,
        path: &str,
        interface: &str,
        budget: &Budget<'_>,
    ) -> Result<(HashMap<String, OwnedVariant>, Sequence), Failure> {
        let message = budget
            .dbus(
                self.connection.call_method(
                    Some(owner),
                    path,
                    Some(PROPERTIES),
                    "GetAll",
                    &(interface,),
                ),
                false,
            )
            .await?;
        let position = message.recv_position();
        let properties: HashMap<String, OwnedVariant> = decode(message)?;
        if properties.len() > 128 {
            return Err(Failure::Unavailable);
        }
        Ok((properties, position))
    }

    async fn methods(&self, owner: &str, budget: &Budget<'_>) -> Result<(), Failure> {
        let xml: String = decode(
            budget
                .dbus(
                    self.connection.call_method(
                        Some(owner),
                        MANAGER_PATH,
                        Some("org.freedesktop.DBus.Introspectable"),
                        "Introspect",
                        &(),
                    ),
                    false,
                )
                .await?,
        )?;
        supported_api(&xml)
    }

    async fn inhibit(&self, owner: &str, budget: &Budget<'_>) -> Result<OwnedFd, Failure> {
        // No interactive-authorization header and no global block/force flag.
        let message = budget
            .dbus(
                self.connection.call_method(
                    Some(owner),
                    MANAGER_PATH,
                    Some(MANAGER),
                    "Inhibit",
                    &(
                        "sleep",
                        "Alcard Shell",
                        "Lock the session before sleep",
                        "delay",
                    ),
                ),
                false,
            )
            .await?;
        decode_delay(message)
    }

    async fn suspend(
        &self,
        owner: &str,
        gate: &Gate<'_>,
        budget: &Budget<'_>,
    ) -> Result<(), Failure> {
        let proxy: zbus::Proxy<'_> = budget
            .dbus(
                async {
                    zbus::proxy::Builder::new(&self.connection)
                        .destination(owner)
                        .path(MANAGER_PATH)
                        .interface(MANAGER)
                        .cache_properties(CacheProperties::No)
                        .build()
                        .await
                },
                false,
            )
            .await?;
        budget.check(false)?;
        gate.dispatch()?;
        let reply: Option<()> = budget
            .dbus(
                proxy.call_with_flags(
                    "SuspendWithFlags",
                    MethodFlags::empty(),
                    &(ROOT_CHECK_INHIBITORS,),
                ),
                true,
            )
            .await?;
        reply.ok_or(Failure::Unknown)
    }
}

impl Login1 for SystemBus {
    async fn owner(&self, budget: &Budget<'_>) -> Result<String, Failure> {
        let owner: String = decode(
            budget
                .dbus(
                    self.connection.call_method(
                        Some("org.freedesktop.DBus"),
                        "/org/freedesktop/DBus",
                        Some("org.freedesktop.DBus"),
                        "GetNameOwner",
                        &(SERVICE,),
                    ),
                    false,
                )
                .await?,
        )?;
        if owner.len() > 255 || zbus::names::UniqueName::try_from(owner.as_str()).is_err() {
            return Err(Failure::Unavailable);
        }
        Ok(owner)
    }
    async fn session(&self, owner: &str, budget: &Budget<'_>) -> Result<Session, Failure> {
        let path: OwnedObjectPath = decode(
            budget
                .dbus(
                    self.connection.call_method(
                        Some(owner),
                        MANAGER_PATH,
                        Some(MANAGER),
                        "GetSessionByPID",
                        &(std::process::id(),),
                    ),
                    false,
                )
                .await?,
        )?;
        let (properties, _) = self
            .properties(owner, path.as_str(), SESSION, budget)
            .await?;
        let session = session_from_properties(&path, properties, Uid::current().as_raw())?;
        let (seat, _) = self
            .properties(owner, &session.seat_path, SEAT, budget)
            .await?;
        validate_seat(&session, seat)?;
        Ok(session)
    }
    async fn capability(&self, owner: &str, budget: &Budget<'_>) -> Result<Capability, Failure> {
        let value: String = decode(
            budget
                .dbus(
                    self.connection.call_method(
                        Some(owner),
                        MANAGER_PATH,
                        Some(MANAGER),
                        "CanSuspend",
                        &(),
                    ),
                    false,
                )
                .await?,
        )?;
        Ok(permission(&value))
    }
    async fn inhibited(&self, owner: &str, budget: &Budget<'_>) -> Result<bool, Failure> {
        let values: Vec<Inhibitor> = decode(
            budget
                .dbus(
                    self.connection.call_method(
                        Some(owner),
                        MANAGER_PATH,
                        Some(MANAGER),
                        "ListInhibitors",
                        &(),
                    ),
                    false,
                )
                .await?,
        )?;
        sleep_inhibited(&values)
    }
    async fn preparing(
        &self,
        owner: &str,
        budget: &Budget<'_>,
    ) -> Result<(bool, u64, Sequence), Failure> {
        let (mut props, position) = self
            .properties(owner, MANAGER_PATH, MANAGER, budget)
            .await?;
        let preparing = bool::try_from(
            props
                .remove("PreparingForSleep")
                .ok_or(Failure::Unavailable)?,
        )
        .map_err(|_| Failure::Unavailable)?;
        let delay_us = u64::try_from(
            props
                .remove("InhibitDelayMaxUSec")
                .ok_or(Failure::Unavailable)?,
        )
        .map_err(|_| Failure::Unavailable)?;
        Ok((preparing, delay_us / 1000, position))
    }
}

fn safe_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 128
        && !matches!(id, "self" | "auto")
        && id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"_.-".contains(&byte))
}
fn session_from_properties(
    path: &OwnedObjectPath,
    mut props: HashMap<String, OwnedVariant>,
    uid: u32,
) -> Result<Session, Failure> {
    let invalid = || Failure::SessionUnavailable;
    if props.len() > 64
        || path.as_str().len() > 512
        || !path
            .as_str()
            .starts_with("/org/freedesktop/login1/session/")
    {
        return Err(invalid());
    }
    let id = String::try_from(props.remove("Id").ok_or_else(invalid)?).map_err(|_| invalid())?;
    let (owner, _): (u32, OwnedObjectPath) = props
        .remove("User")
        .ok_or_else(invalid)?
        .try_into()
        .map_err(|_| invalid())?;
    let kind =
        String::try_from(props.remove("Type").ok_or_else(invalid)?).map_err(|_| invalid())?;
    let class =
        String::try_from(props.remove("Class").ok_or_else(invalid)?).map_err(|_| invalid())?;
    let remote =
        bool::try_from(props.remove("Remote").ok_or_else(invalid)?).map_err(|_| invalid())?;
    let active =
        bool::try_from(props.remove("Active").ok_or_else(invalid)?).map_err(|_| invalid())?;
    let (seat_id, seat_path): (String, OwnedObjectPath) = props
        .remove("Seat")
        .ok_or_else(invalid)?
        .try_into()
        .map_err(|_| invalid())?;
    if !safe_id(&id)
        || owner != uid
        || remote
        || !active
        || !matches!(kind.as_str(), "wayland" | "x11")
        || !matches!(class.as_str(), "user" | "user-early")
        || !safe_id(&seat_id)
        || seat_path.as_str().len() > 512
        || !seat_path
            .as_str()
            .starts_with("/org/freedesktop/login1/seat/")
    {
        return Err(invalid());
    }
    Ok(Session {
        id,
        path: path.as_str().into(),
        uid: owner,
        kind,
        seat_id,
        seat_path: seat_path.as_str().into(),
    })
}
fn validate_seat(
    session: &Session,
    mut props: HashMap<String, OwnedVariant>,
) -> Result<(), Failure> {
    let invalid = || Failure::SessionUnavailable;
    if props.len() > 64 {
        return Err(invalid());
    }
    let id = String::try_from(props.remove("Id").ok_or_else(invalid)?).map_err(|_| invalid())?;
    let graphical =
        bool::try_from(props.remove("CanGraphical").ok_or_else(invalid)?).map_err(|_| invalid())?;
    let (active, path): (String, OwnedObjectPath) = props
        .remove("ActiveSession")
        .ok_or_else(invalid)?
        .try_into()
        .map_err(|_| invalid())?;
    if !graphical || id != session.seat_id || active != session.id || path.as_str() != session.path
    {
        return Err(invalid());
    }
    Ok(())
}

/// Only the fixed introspection subset is read; no XML entities or DTDs are
/// expanded and no network/filesystem parser is involved.
fn supported_api(xml: &str) -> Result<(), Failure> {
    const REQUIRED: [(&str, &str, &str, &str); 8] = [
        ("method", "GetSessionByPID", "u", "o"),
        ("method", "CanSuspend", "", "s"),
        ("method", "SuspendWithFlags", "t", ""),
        ("method", "ListInhibitors", "", "a(ssssuu)"),
        ("method", "Inhibit", "ssss", "h"),
        ("signal", "PrepareForSleep", "", "b"),
        ("property", "PreparingForSleep", "", "b"),
        ("property", "InhibitDelayMaxUSec", "", "t"),
    ];
    if xml.len() > MAX_REPLY_BYTES {
        return Err(Failure::Unavailable);
    }
    let mut valid = [false; 8];
    let mut seen = [false; 8];
    let mut remaining = xml;
    let mut manager = false;
    let mut manager_seen = false;
    let mut current: Option<(usize, String, String)> = None;
    let mut count = 0;
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
        count += 1;
        if count > 4096 {
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
                if manager && std::mem::replace(&mut manager_seen, true) {
                    return Err(Failure::Unavailable);
                }
            }
            "/interface" => {
                if current.is_some() {
                    return Err(Failure::Unavailable);
                }
                manager = false;
            }
            "method" | "signal" | "property" if manager => {
                if current.is_some() {
                    return Err(Failure::Unavailable);
                }
                let index = REQUIRED.iter().position(|(kind, wanted, _, _)| {
                    *kind == name && attribute(tag, "name") == Some(*wanted)
                });
                if let Some(index) = index {
                    if std::mem::replace(&mut seen[index], true) {
                        return Err(Failure::Unavailable);
                    }
                    if name == "property" {
                        valid[index] = attribute(tag, "type") == Some(REQUIRED[index].3)
                            && matches!(attribute(tag, "access"), Some("read" | "readwrite"));
                    } else if tag.ends_with('/') {
                        valid[index] = REQUIRED[index].2.is_empty() && REQUIRED[index].3.is_empty();
                    } else {
                        current = Some((index, String::new(), String::new()));
                    }
                }
            }
            "arg" if manager => {
                if let Some((index, inputs, outputs)) = current.as_mut() {
                    let signature = attribute(tag, "type").ok_or(Failure::Unavailable)?;
                    if signature.len() > 32 {
                        return Err(Failure::Unavailable);
                    }
                    let direction =
                        attribute(tag, "direction").unwrap_or(if REQUIRED[*index].0 == "signal" {
                            "out"
                        } else {
                            "in"
                        });
                    match direction {
                        "in" if REQUIRED[*index].0 == "method" => inputs.push_str(signature),
                        "out" => outputs.push_str(signature),
                        _ => return Err(Failure::Unavailable),
                    }
                    if inputs.len() > 64 || outputs.len() > 64 {
                        return Err(Failure::Unavailable);
                    }
                }
            }
            "/method" | "/signal" if manager => {
                if let Some((index, inputs, outputs)) = current.take() {
                    valid[index] = inputs == REQUIRED[index].2
                        && outputs == REQUIRED[index].3
                        && name.trim_start_matches('/') == REQUIRED[index].0;
                }
            }
            _ => {}
        }
    }
    if manager || current.is_some() || valid.iter().any(|valid| !valid) {
        Err(Failure::Unavailable)
    } else {
        Ok(())
    }
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

fn send(updates: &Sender<Message>, message: Message) -> Result<(), Failure> {
    updates.send(message).map_err(|_| Failure::Cancelled)
}

fn lose_epoch(shared: &Shared, updates: &Sender<Message>) {
    let (epoch, snapshot, delay) = {
        let mut state = shared.lock();
        let epoch = state.snapshot.epoch;
        if let Some(pending) = state.pending.take() {
            pending.revoke();
        }
        state.context = None;
        state.connection = None;
        state.cycle = None;
        let delay = state.delay.take();
        state.snapshot.monitor = MonitorState::Unavailable;
        state.snapshot.capability = Capability::Unavailable;
        (epoch, state.snapshot, delay)
    };
    drop(delay);
    let _ = send(updates, Message::Lost { epoch });
    let _ = send(updates, Message::Snapshot(snapshot));
}

fn begin_epoch(
    shared: &Shared,
    updates: &Sender<Message>,
    mut context: Context,
    connection: zbus::Connection,
) -> Result<Context, Failure> {
    let snapshot = {
        let mut state = shared.lock();
        if shared.stopped.load(Ordering::Acquire) {
            return Err(Failure::Cancelled);
        }
        context.epoch = state
            .snapshot
            .epoch
            .checked_add(1)
            .ok_or(Failure::Unavailable)?;
        state.snapshot = Snapshot {
            epoch: context.epoch,
            ..Snapshot::default()
        };
        state.cycle = None;
        state.next_cycle = 0;
        state.delay = None;
        state.context = Some(context.clone());
        state.connection = Some(connection);
        state.snapshot
    };
    send(updates, Message::Snapshot(snapshot))?;
    Ok(context)
}

fn update_policy(
    shared: &Shared,
    epoch: u64,
    capability: Capability,
    delay_ms: u64,
) -> Result<(), Failure> {
    let mut state = shared.lock();
    if state.snapshot.epoch != epoch || state.context.is_none() {
        return Err(Failure::Unavailable);
    }
    state.snapshot.capability = capability;
    state.snapshot.delay_ms = delay_ms;
    Ok(())
}

fn begin_cycle(shared: &Shared, updates: &Sender<Message>, epoch: u64) -> Result<(), Failure> {
    let event = {
        let mut state = shared.lock();
        if state.snapshot.epoch != epoch || state.context.is_none() {
            return Err(Failure::Unavailable);
        }
        if state.cycle.is_some() {
            None
        } else {
            let cycle = state
                .next_cycle
                .checked_add(1)
                .ok_or(Failure::Unavailable)?;
            state.next_cycle = cycle;
            state.cycle = Some(cycle);
            state.snapshot.monitor = MonitorState::Preparing;
            Some((cycle, state.snapshot))
        }
    };
    if let Some((cycle, snapshot)) = event {
        send(updates, Message::Snapshot(snapshot))?;
        send(updates, Message::Preparing { epoch, cycle })?;
    }
    Ok(())
}

fn end_cycle(shared: &Shared, updates: &Sender<Message>, epoch: u64) -> Result<bool, Failure> {
    let ended = {
        let mut state = shared.lock();
        if state.snapshot.epoch != epoch || state.context.is_none() {
            return Err(Failure::Unavailable);
        }
        state.cycle.take().map(|cycle| {
            let delay = state.delay.take();
            if let Some(pending) = state.pending.take() {
                pending.revoke();
            }
            state.snapshot.monitor = MonitorState::Connecting;
            (cycle, state.snapshot, delay)
        })
    };
    let Some((cycle, snapshot, delay)) = ended else {
        return Ok(false);
    };
    drop(delay);
    send(
        updates,
        Message::CycleEnded {
            epoch,
            cycle: Some(cycle),
        },
    )?;
    send(updates, Message::Snapshot(snapshot))?;
    Ok(true)
}

fn install_delay(shared: &Shared, epoch: u64, delay: OwnedFd) -> Result<(), Failure> {
    let mut state = shared.lock();
    if shared.stopped.load(Ordering::Acquire)
        || state.snapshot.epoch != epoch
        || state.context.is_none()
        || state.delay.is_some()
    {
        return Err(Failure::Unavailable);
    }
    state.delay = Some(delay);
    Ok(())
}

fn ready(shared: &Shared, updates: &Sender<Message>, epoch: u64) -> Result<(), Failure> {
    let snapshot = {
        let mut state = shared.lock();
        if shared.stopped.load(Ordering::Acquire)
            || state.snapshot.epoch != epoch
            || state.context.is_none()
            || state.delay.is_none()
            || state.cycle.is_some()
        {
            return Err(Failure::Unavailable);
        }
        state.snapshot.monitor = MonitorState::Ready;
        state.snapshot
    };
    send(updates, Message::Snapshot(snapshot))
}

async fn refresh_epoch(
    bus: &SystemBus,
    context: &Context,
    shared: &Shared,
    updates: &Sender<Message>,
) -> Result<Sequence, Failure> {
    let budget = Budget::new(shared, None, READ_TIMEOUT);
    if bus.owner(&budget).await? != context.owner {
        return Err(Failure::Unavailable);
    }
    if bus.session(&context.owner, &budget).await? != context.session {
        return Err(Failure::SessionUnavailable);
    }
    let mut capability = bus.capability(&context.owner, &budget).await?;
    if bus.inhibited(&context.owner, &budget).await? && capability == Capability::Allowed {
        capability = Capability::Blocked;
    }
    let (preparing, delay_ms, position) = bus.preparing(&context.owner, &budget).await?;
    update_policy(shared, context.epoch, capability, delay_ms)?;
    if preparing {
        begin_cycle(shared, updates, context.epoch)?;
        return Ok(position);
    }
    // A queried false can also settle a cycle whose false signal is queued.
    end_cycle(shared, updates, context.epoch)?;
    let have_delay = shared.lock().delay.is_some();
    let mut final_position = position;
    if !have_delay {
        let delay = match bus.inhibit(&context.owner, &budget).await {
            Ok(delay) => delay,
            Err(Failure::Blocked) => {
                // elogind refuses a new delay inhibitor once sleep has begun.
                // Observe that state instead of announcing unprotected Ready.
                let (preparing, delay_ms, position) =
                    bus.preparing(&context.owner, &budget).await?;
                update_policy(shared, context.epoch, capability, delay_ms)?;
                if preparing {
                    begin_cycle(shared, updates, context.epoch)?;
                    return Ok(position);
                }
                return Err(Failure::Blocked);
            }
            Err(error) => return Err(error),
        };
        let (preparing, delay_ms, position) = bus.preparing(&context.owner, &budget).await?;
        if bus.owner(&budget).await? != context.owner {
            return Err(Failure::Unavailable);
        }
        update_policy(shared, context.epoch, capability, delay_ms)?;
        install_delay(shared, context.epoch, delay)?;
        if preparing {
            begin_cycle(shared, updates, context.epoch)?;
            return Ok(position);
        }
        final_position = position;
    } else if bus.owner(&budget).await? != context.owner {
        return Err(Failure::Unavailable);
    }
    ready(shared, updates, context.epoch)?;
    Ok(final_position)
}

enum MonitorEvent {
    Sleep(Option<zbus::Result<zbus::Message>>),
    Owner(Option<zbus::Result<zbus::Message>>),
    Closed,
    Wake,
}

async fn refresh_capability(
    bus: &impl Login1,
    context: &Context,
    shared: &Shared,
    updates: &Sender<Message>,
) -> Result<(), Failure> {
    let budget = Budget::new(shared, None, READ_TIMEOUT);
    if bus.owner(&budget).await? != context.owner {
        return Err(Failure::Unavailable);
    }
    if bus.session(&context.owner, &budget).await? != context.session {
        return Err(Failure::SessionUnavailable);
    }
    let mut capability = bus.capability(&context.owner, &budget).await?;
    if bus.inhibited(&context.owner, &budget).await? && capability == Capability::Allowed {
        capability = Capability::Blocked;
    }
    if bus.owner(&budget).await? != context.owner {
        return Err(Failure::Unavailable);
    }
    let snapshot = {
        let mut state = shared.lock();
        if state.snapshot.epoch != context.epoch || state.context.is_none() {
            return Err(Failure::Unavailable);
        }
        state.snapshot.capability = capability;
        state.snapshot
    };
    // On an established subscription only signals advance sleep state. A
    // property snapshot here could mask a queued true/false pair belonging to
    // our dispatched request and leave the intent pending forever.
    send(updates, Message::Snapshot(snapshot))
}

async fn monitor_once(shared: &Shared, updates: &Sender<Message>) -> Result<(), Failure> {
    let budget = Budget::new(shared, None, READ_TIMEOUT);
    let connection = budget
        .dbus(
            zbus::connection::Builder::system()
                .max_queued(NonZeroUsize::new(32).unwrap())
                .build(),
            false,
        )
        .await?;
    let bus = SystemBus {
        connection: connection.clone(),
    };
    // Only signal-specific queues are created. An unfiltered MessageStream
    // would also retain Inhibit method replies and their hidden FD duplicates.
    let owner_rule = zbus::MatchRule::builder()
        .msg_type(zbus::message::Type::Signal)
        .sender("org.freedesktop.DBus")
        .path("/org/freedesktop/DBus")
        .interface("org.freedesktop.DBus")
        .member("NameOwnerChanged")
        .add_arg(SERVICE)
        .build()
        .map_err(|_| Failure::Unavailable)?;
    let mut owner_changes = budget
        .dbus(
            zbus::MessageStream::for_match_rule(
                owner_rule,
                &connection,
                Some(NonZeroUsize::new(16).unwrap()),
            ),
            false,
        )
        .await?;
    let owner = bus.owner(&budget).await?;
    let sleep_rule = zbus::MatchRule::builder()
        .msg_type(zbus::message::Type::Signal)
        .sender(owner.as_str())
        .path(MANAGER_PATH)
        .interface(MANAGER)
        .member("PrepareForSleep")
        .build()
        .map_err(|_| Failure::Unavailable)?;
    let mut sleep_changes = budget
        .dbus(
            zbus::MessageStream::for_match_rule(
                sleep_rule,
                &connection,
                Some(NonZeroUsize::new(16).unwrap()),
            ),
            false,
        )
        .await?;
    bus.methods(&owner, &budget).await?;
    let session = bus.session(&owner, &budget).await?;
    if bus.owner(&budget).await? != owner {
        return Err(Failure::Unavailable);
    }
    let context = begin_epoch(
        shared,
        updates,
        Context {
            epoch: 0,
            owner,
            session,
        },
        connection.clone(),
    )?;
    let mut floor = refresh_epoch(&bus, &context, shared, updates).await?;
    loop {
        if shared.stopped.load(Ordering::Acquire) {
            return Ok(());
        }
        if shared.refreshing.swap(false, Ordering::AcqRel) {
            let request = shared.refresh_request.swap(0, Ordering::AcqRel);
            refresh_capability(&bus, &context, shared, updates).await?;
            if request != 0 {
                let snapshot = shared.lock().snapshot;
                send(updates, Message::Refreshed { request, snapshot })?;
            }
            continue;
        }
        let event = future::or(
            async {
                connection.closed().await;
                MonitorEvent::Closed
            },
            future::or(
                async { MonitorEvent::Owner(owner_changes.next().await) },
                future::or(
                    async { MonitorEvent::Sleep(sleep_changes.next().await) },
                    async {
                        shared.notify.wait().await;
                        MonitorEvent::Wake
                    },
                ),
            ),
        )
        .await;
        match event {
            MonitorEvent::Wake => {}
            MonitorEvent::Closed | MonitorEvent::Owner(None) | MonitorEvent::Sleep(None) => {
                return Err(Failure::Unavailable);
            }
            MonitorEvent::Owner(Some(message)) => {
                let message = message.map_err(|_| Failure::Unavailable)?;
                let (name, old, new): (String, String, String) = decode(message)?;
                if name != SERVICE || new != context.owner || old == context.owner {
                    return Err(Failure::Unavailable);
                }
            }
            MonitorEvent::Sleep(Some(message)) => {
                let message = message.map_err(|_| Failure::Unavailable)?;
                if message.header().sender().map(|sender| sender.as_str())
                    != Some(context.owner.as_str())
                {
                    return Err(Failure::Unavailable);
                }
                let position = message.recv_position();
                let preparing: bool = decode(message)?;
                // A fresh PreparingForSleep property reply supersedes queued
                // older signals from this SAME connection. Serial numbers are
                // intentionally not used as ordering evidence.
                if position <= floor {
                    continue;
                }
                floor = position;
                if preparing {
                    begin_cycle(shared, updates, context.epoch)?;
                } else if end_cycle(shared, updates, context.epoch)? {
                    floor = refresh_epoch(&bus, &context, shared, updates).await?;
                }
            }
        }
        future::yield_now().await;
    }
}

async fn monitor_forever(shared: Arc<Shared>, updates: Sender<Message>) {
    let runtime = match zruntime::SharedRuntime::current() {
        Ok(runtime) => runtime,
        Err(_) => {
            lose_epoch(&shared, &updates);
            shared.stop();
            return;
        }
    };
    let mut retry = Duration::from_secs(1);
    while !shared.stopped.load(Ordering::Acquire) {
        let result = monitor_once(&shared, &updates).await;
        if shared.stopped.load(Ordering::Acquire) {
            break;
        }
        lose_epoch(&shared, &updates);
        if result == Err(Failure::Cancelled) {
            shared.stop();
            break;
        }
        // The only recurring timer is failure recovery. A healthy monitor
        // waits exclusively for signals, explicit refresh, and shutdown.
        future::or(
            async {
                runtime.sleep(retry).await;
            },
            shared.notify.wait(),
        )
        .await;
        if finish_recovery_refresh(&shared, &updates) {
            retry = Duration::from_secs(1);
        } else {
            retry = (retry * 2).min(Duration::from_secs(30));
        }
    }
}

/// A refresh accepted during recovery cannot wait for a healthy-monitor loop
/// whose wake flag this backoff just consumed. Settle its exact token as
/// unavailable and free the bounded slot, without turning reconnection into an
/// automatic retry of a battery action. A later explicit episode may try once.
fn finish_recovery_refresh(shared: &Shared, updates: &Sender<Message>) -> bool {
    if !shared.refreshing.swap(false, Ordering::AcqRel) {
        return false;
    }
    let request = shared.refresh_request.swap(0, Ordering::AcqRel);
    if request != 0 {
        let snapshot = Snapshot {
            monitor: MonitorState::Unavailable,
            capability: Capability::Unavailable,
            ..shared.lock().snapshot
        };
        let _ = send(updates, Message::Refreshed { request, snapshot });
    }
    true
}

#[cfg(test)]
#[path = "unit_tests/sleep.rs"]
mod tests;
