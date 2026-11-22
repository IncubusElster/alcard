//! Starts the independent, unprivileged session locker. This channel carries
//! public lifecycle only; the Shell never receives or validates a password.
use alcard_lock_state::{Observation, Session, Watch};
use smithay_client_toolkit::reexports::calloop::channel::Sender;
use std::{
    io::{self, Read},
    os::fd::{AsFd, AsRawFd, FromRawFd, OwnedFd},
    process::{Command, Stdio},
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc::{self, SyncSender},
        Arc,
    },
    thread,
    time::{Duration, Instant},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    Starting,
    Locked,
    Failed,
    Uncertain,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct State {
    pub id: u64,
    pub phase: Phase,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Message {
    /// Public presentation/focus suppression. Never an authentication result.
    Presence(bool),
    /// Advisory journal presence survives Shell/session-bus restart. It is
    /// deliberately separate from both public D-Bus presence and UNLOCKED.
    Journal(bool),
    Changed(State),
    Unlocked {
        id: u64,
    },
}

enum Work {
    Lock { id: u64, locale: &'static str },
    Observe,
    MonitorFailed,
    Stop,
}

pub struct Client {
    commands: SyncSender<Work>,
    busy: Arc<AtomicBool>,
    stopped: Arc<AtomicBool>,
    present: Arc<AtomicBool>,
    wake: Arc<Wake>,
}

impl Client {
    pub fn start(updates: Sender<Message>, socket: &impl AsFd) -> io::Result<Self> {
        let session = Session::from_wayland_socket(socket)?;
        // Subscribe before the initial snapshot. A crash or intent write
        // between these operations must remain observable.
        let watch = session.watch()?;
        let initial = session.observe()?;
        let (commands, incoming) = mpsc::sync_channel(4);
        let busy = Arc::new(AtomicBool::new(initial != Observation::Clear));
        let stopped = Arc::new(AtomicBool::new(false));
        let present = Arc::new(AtomicBool::new(initial != Observation::Clear));
        let wake = Arc::new(Wake::new()?);
        let observing = Arc::new(AtomicBool::new(false));
        let watch_commands = commands.clone();
        let watch_stop = stopped.clone();
        let watch_wake = wake.clone();
        let watch_observing = observing.clone();
        thread::Builder::new()
            .name("alcard-lock-journal".into())
            .spawn(move || {
                observe_events(
                    watch,
                    watch_commands,
                    watch_stop,
                    watch_wake,
                    watch_observing,
                );
            })?;
        if let Err(error) = crate::lock_presence::start(updates.clone(), stopped.clone()) {
            eprintln!("Lock presentation monitor unavailable: {error}");
        }
        let worker_busy = busy.clone();
        let worker_stop = stopped.clone();
        let worker_present = present.clone();
        if let Err(error) = thread::Builder::new()
            .name("alcard-lock-lifecycle".into())
            .spawn(move || {
                worker(
                    incoming,
                    updates,
                    session,
                    worker_busy,
                    worker_stop,
                    worker_present,
                    observing,
                );
            })
        {
            stopped.store(true, Ordering::Release);
            wake.notify();
            return Err(error);
        }
        Ok(Self {
            commands,
            busy,
            stopped,
            present,
            wake,
        })
    }

    pub fn present(&self) -> bool {
        self.present.load(Ordering::Acquire)
    }

    pub fn lock(&self, id: u64, locale: &'static str) -> bool {
        if self.busy.swap(true, Ordering::AcqRel) {
            return false;
        }
        if self.commands.try_send(Work::Lock { id, locale }).is_err() {
            self.busy.store(false, Ordering::Release);
            return false;
        }
        true
    }
}

impl Drop for Client {
    fn drop(&mut self) {
        self.stopped.store(true, Ordering::Release);
        self.wake.notify();
        // Never kill a locker when Shell exits. Its compositor lock and PAM
        // conversation must survive the parent; broken status pipes are benign.
        let _ = self.commands.try_send(Work::Stop);
    }
}

struct Wake(OwnedFd);
impl Wake {
    fn new() -> io::Result<Self> {
        // SAFETY: eventfd has no pointer parameters; this descriptor is owned
        // only by the cancellation handle and is never inherited across exec.
        let fd = unsafe { nix::libc::eventfd(0, nix::libc::EFD_CLOEXEC | nix::libc::EFD_NONBLOCK) };
        if fd < 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: successful eventfd returned a new owned descriptor.
        Ok(Self(unsafe { OwnedFd::from_raw_fd(fd) }))
    }
    fn notify(&self) {
        let value = 1_u64;
        // SAFETY: value is an initialized eight-byte eventfd counter, and the
        // descriptor remains open through this nonblocking cancellation write.
        let _ = unsafe { nix::libc::write(self.0.as_raw_fd(), (&value as *const u64).cast(), 8) };
    }
}

fn observe_events(
    watch: Watch,
    commands: SyncSender<Work>,
    stopped: Arc<AtomicBool>,
    wake: Arc<Wake>,
    observing: Arc<AtomicBool>,
) {
    while !stopped.load(Ordering::Acquire) {
        let mut fds = [
            nix::libc::pollfd {
                fd: watch.as_fd().as_raw_fd(),
                events: nix::libc::POLLIN,
                revents: 0,
            },
            nix::libc::pollfd {
                fd: wake.0.as_raw_fd(),
                events: nix::libc::POLLIN,
                revents: 0,
            },
        ];
        // SAFETY: fds is an initialized, writable two-entry array. Both
        // descriptors stay owned for the entire blocking poll.
        let result = unsafe { nix::libc::poll(fds.as_mut_ptr(), fds.len() as _, -1) };
        if stopped.load(Ordering::Acquire) {
            return;
        }
        if result < 0 && io::Error::last_os_error().kind() == io::ErrorKind::Interrupted {
            continue;
        }
        if result < 0
            || fds[0].revents & (nix::libc::POLLERR | nix::libc::POLLHUP | nix::libc::POLLNVAL) != 0
            || watch.drain().is_err()
        {
            let _ = commands.send(Work::MonitorFailed);
            return;
        }
        if !observing.swap(true, Ordering::AcqRel) && commands.try_send(Work::Observe).is_err() {
            observing.store(false, Ordering::Release);
        }
    }
}

#[derive(Clone, Copy)]
struct Attempt {
    id: u64,
    locale: &'static str,
    recovering: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Outcome {
    Unlocked,
    FailedBeforeRequest,
    Uncertain,
    Detached,
}

const FIRST_RETRY: Duration = Duration::from_secs(1);
const MAX_RETRY: Duration = Duration::from_secs(30);

fn recovery_needed(attempt: Option<Attempt>, observation: Option<Observation>) -> bool {
    observation == Some(Observation::Abandoned)
        || (observation == Some(Observation::Clear)
            && attempt.is_some_and(|attempt| attempt.recovering))
}

fn publish_journal(
    session: &Session,
    present: &AtomicBool,
    updates: &Sender<Message>,
    last_error: &mut Option<String>,
) -> Option<Observation> {
    let observation = match session.observe() {
        Ok(observation) => {
            *last_error = None;
            Some(observation)
        }
        Err(error) => {
            let error = error.to_string();
            if last_error.as_ref() != Some(&error) {
                eprintln!("Lock recovery journal unavailable: {error}");
                *last_error = Some(error);
            }
            None
        }
    };
    let active = observation != Some(Observation::Clear);
    if present.swap(active, Ordering::AcqRel) != active {
        let _ = updates.send(Message::Journal(active));
    }
    observation
}

fn worker(
    incoming: mpsc::Receiver<Work>,
    updates: Sender<Message>,
    session: Session,
    busy: Arc<AtomicBool>,
    stopped: Arc<AtomicBool>,
    present: Arc<AtomicBool>,
    observing: Arc<AtomicBool>,
) {
    let mut attempt: Option<Attempt> = None;
    let mut retry_at: Option<Instant> = None;
    let mut delay = FIRST_RETRY;
    let mut last_error = None;
    let mut work = Some(Work::Observe);
    while !stopped.load(Ordering::Acquire) && session.is_live() {
        match work.take() {
            Some(Work::Stop) => return,
            Some(Work::MonitorFailed) => {
                eprintln!("Lock recovery journal monitor failed; automatic recovery stopped");
                present.store(true, Ordering::Release);
                busy.store(true, Ordering::Release);
                let _ = updates.send(Message::Journal(true));
                return;
            }
            Some(Work::Lock { id, locale }) => {
                attempt = Some(Attempt {
                    id,
                    locale,
                    recovering: false,
                });
                retry_at = Some(Instant::now());
                delay = FIRST_RETRY;
            }
            Some(Work::Observe) => {
                observing.store(false, Ordering::Release);
            }
            None => {}
        }
        let observation = publish_journal(&session, &present, &updates, &mut last_error);
        if recovery_needed(attempt, observation) {
            if attempt.is_none() {
                // Zero is reserved for adopting an external/previous Shell's
                // abandoned locker. It is not a new UI authentication result.
                attempt = Some(Attempt {
                    id: 0,
                    locale: "auto",
                    recovering: true,
                });
            }
            if retry_at.is_none() {
                retry_at = Some(Instant::now() + delay);
            }
            busy.store(true, Ordering::Release);
        } else if attempt.is_none() {
            busy.store(observation != Some(Observation::Clear), Ordering::Release);
            retry_at = None;
            delay = FIRST_RETRY;
        }
        if let Some(current) = attempt.filter(|_| retry_at.is_some_and(|at| Instant::now() >= at)) {
            retry_at = None;
            let may_start = matches!(
                observation,
                Some(Observation::Abandoned | Observation::Clear)
            );
            if may_start {
                let started = Instant::now();
                let outcome = run(current.id, current.locale, &updates, &stopped);
                if outcome == Outcome::Detached {
                    return;
                }
                let observation = publish_journal(&session, &present, &updates, &mut last_error);
                if outcome == Outcome::Unlocked {
                    attempt = None;
                    delay = FIRST_RETRY;
                    // Publish completion only after releasing our request
                    // gate. Journal absence alone never emits UNLOCKED.
                    busy.store(observation != Some(Observation::Clear), Ordering::Release);
                    let _ = updates.send(Message::Unlocked { id: current.id });
                } else if outcome == Outcome::FailedBeforeRequest
                    && observation == Some(Observation::Clear)
                    && !current.recovering
                {
                    attempt = None;
                    busy.store(false, Ordering::Release);
                    let _ = updates.send(Message::Changed(State {
                        id: current.id,
                        phase: Phase::Failed,
                    }));
                } else {
                    attempt = Some(Attempt {
                        recovering: true,
                        ..current
                    });
                    busy.store(true, Ordering::Release);
                    let _ = updates.send(Message::Changed(State {
                        id: current.id,
                        phase: Phase::Uncertain,
                    }));
                    if started.elapsed() >= MAX_RETRY {
                        delay = FIRST_RETRY;
                    }
                    if recovery_needed(attempt, observation) {
                        retry_at = Some(Instant::now() + delay);
                        delay = (delay * 2).min(MAX_RETRY);
                    }
                }
            } else if observation == Some(Observation::Held) {
                // Another live locker won ownership. Never disturb it or
                // launch duplicates; close-write will wake us if it exits.
                attempt = Some(Attempt {
                    recovering: true,
                    ..current
                });
            }
        }
        let received = if let Some(at) = retry_at {
            incoming.recv_timeout(at.saturating_duration_since(Instant::now()))
        } else {
            incoming
                .recv()
                .map_err(|_| mpsc::RecvTimeoutError::Disconnected)
        };
        match received {
            Ok(next) => work = Some(next),
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => return,
        }
    }
}

#[derive(Default)]
struct Protocol {
    pending: Vec<u8>,
    locked: bool,
    unlocked: bool,
    failed_before_lock: bool,
    invalid: bool,
}

impl Protocol {
    fn feed(&mut self, bytes: &[u8]) {
        for &byte in bytes {
            if self.invalid {
                return;
            }
            if byte == b'\n' {
                match self.pending.as_slice() {
                    b"FAILED" if !self.locked && !self.failed_before_lock => {
                        self.failed_before_lock = true
                    }
                    b"LOCKED" if !self.locked && !self.failed_before_lock => self.locked = true,
                    b"UNLOCKED" if self.locked && !self.unlocked => self.unlocked = true,
                    _ => self.invalid = true,
                }
                self.pending.clear();
            } else if self.pending.len() < 16 {
                self.pending.push(byte);
            } else {
                self.invalid = true;
                self.pending.clear();
            }
        }
    }

    fn complete(&self) -> bool {
        self.locked && self.unlocked && !self.invalid && self.pending.is_empty()
    }

    fn failed(&self) -> bool {
        self.failed_before_lock && !self.locked && !self.invalid && self.pending.is_empty()
    }
}

fn run(id: u64, locale: &'static str, updates: &Sender<Message>, stopped: &AtomicBool) -> Outcome {
    let changed = |phase| {
        let _ = updates.send(Message::Changed(State { id, phase }));
    };
    let result = (|| -> io::Result<_> {
        // Installed alongside Shell; no PATH lookup, shell interpolation,
        // user-selected helper, or authentication arguments/environment.
        let binary = std::env::current_exe()?.with_file_name("alcard-lock");
        Command::new(binary)
            .args(["--notify", "--locale", locale])
            // Protocol tracing in a diagnostic Shell must not record the
            // separate credential client's Wayland keyboard events.
            .env_remove("WAYLAND_DEBUG")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
    })();
    let mut child = match result {
        Ok(child) => child,
        Err(error) => {
            eprintln!("Could not start the session locker: {error}");
            return Outcome::FailedBeforeRequest;
        }
    };
    let mut output = child.stdout.take().expect("piped child stdout");
    let fd = output.as_raw_fd();
    // SAFETY: fd is owned by output for the full lifetime of these calls.
    let nonblocking = unsafe {
        let flags = nix::libc::fcntl(fd, nix::libc::F_GETFL);
        flags >= 0 && nix::libc::fcntl(fd, nix::libc::F_SETFL, flags | nix::libc::O_NONBLOCK) == 0
    };
    let mut protocol = Protocol::default();
    if !nonblocking {
        // Child may already have locked. Continue watching this exact process
        // without reading a potentially blocking pipe; never kill it here.
        protocol.invalid = true;
        changed(Phase::Uncertain);
    }
    let mut reported_locked = false;
    let mut reported_timeout = false;
    let mut eof = false;
    let started = Instant::now();
    loop {
        if stopped.load(Ordering::Acquire) {
            // Dropping Child does not terminate it. The next Shell adopts its
            // lease; a parent exit is never a lock cancellation.
            return Outcome::Detached;
        }
        let mut bytes = [0_u8; 64];
        while nonblocking && !eof {
            match output.read(&mut bytes) {
                Ok(0) => {
                    eof = true;
                    break;
                }
                Ok(n) => protocol.feed(&bytes[..n]),
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => break,
                Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                Err(_) => {
                    protocol.invalid = true;
                    eof = true;
                    break;
                }
            }
        }
        if protocol.locked && !protocol.invalid && !reported_locked {
            reported_locked = true;
            changed(Phase::Locked);
        }
        match child.try_wait() {
            Ok(Some(status)) if eof || !nonblocking => {
                if status.success() && protocol.complete() {
                    return Outcome::Unlocked;
                }
                // The client can die after the compositor accepts lock but
                // before it writes LOCKED. Only its explicit pre-request
                // FAILED record proves that no lock request was sent.
                return if !status.success() && protocol.failed() {
                    Outcome::FailedBeforeRequest
                } else {
                    Outcome::Uncertain
                };
            }
            Err(_) => {
                changed(Phase::Uncertain);
                // An observation failure never permits killing this process.
                // A retry still requires an abandoned journal and a new
                // exclusive lease, so a live locker cannot be replaced.
                return Outcome::Uncertain;
            }
            _ => {}
        }
        if !reported_locked && !reported_timeout && started.elapsed() > Duration::from_secs(10) {
            reported_timeout = true;
            changed(Phase::Uncertain);
            // Keep observing this exact child; a deadline never unlocks or
            // kills it. Recovery requires its exit and an abandoned journal.
        }
        thread::sleep(Duration::from_millis(50));
    }
}

#[cfg(test)]
#[path = "unit_tests/locking.rs"]
mod tests;
