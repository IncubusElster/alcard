//! Owned login1 suspend intent and an independent delay-inhibitor monitor.

use crate::system::{Capability, Failure};
use futures_lite::future;
use smithay_client_toolkit::reexports::calloop::channel::Sender;
use std::{
    future::Future,
    os::fd::OwnedFd,
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc::{self, Receiver, SyncSender},
        Arc, Mutex, MutexGuard,
    },
    task::{Poll, Waker},
    thread,
    time::{Duration, Instant},
};
mod login1;
mod monitor;

use login1::{bus_failure, preflight, SystemBus};
use monitor::{lose_epoch, monitor_forever};

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

const READ_TIMEOUT: Duration = Duration::from_secs(6);
const MUTATION_TIMEOUT: Duration = Duration::from_secs(120);
const CANCEL_INTERVAL: Duration = Duration::from_millis(50);

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

fn send(updates: &Sender<Message>, message: Message) -> Result<(), Failure> {
    updates.send(message).map_err(|_| Failure::Cancelled)
}

#[cfg(test)]
#[path = "unit_tests/sleep.rs"]
mod tests;
