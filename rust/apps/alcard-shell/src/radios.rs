//! On-demand Quick-menu radio requests. The UI supplies a typed operation only;
//! fixed helper argv, a single worker and bounded public replies keep device I/O
//! outside the Wayland event loop. No Settings application or polling daemon.

#[path = "radio_protocol.rs"]
mod protocol;
pub use protocol::{Availability, Entry, Failure, Kind, Message, Operation, Snapshot, State};

use smithay_client_toolkit::reexports::calloop::channel::Sender;
use std::{
    io::{self, Read},
    os::fd::AsRawFd,
    process::{Child, Command, Stdio},
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, SyncSender},
        Arc,
    },
    thread,
    time::{Duration, Instant},
};

const HELPER: &str = "/usr/libexec/alcard/alcard-radio-helper";
const PKEXEC: &str = "/usr/bin/pkexec";
const DEADLINE: Duration = Duration::from_secs(12);
const POLL: Duration = Duration::from_millis(20);

#[derive(Default)]
struct Latches {
    stopped: AtomicBool,
    busy: AtomicBool,
    disabled: AtomicBool,
}
enum Work {
    Execute(u64, Operation),
    Stop,
}

pub struct Client {
    commands: SyncSender<Work>,
    latches: Arc<Latches>,
}

impl Client {
    pub fn start(updates: Sender<Message>) -> Self {
        let (commands, incoming) = mpsc::sync_channel(1);
        let latches = Arc::new(Latches::default());
        let worker_latches = latches.clone();
        if let Err(error) = thread::Builder::new()
            .name("alcard-radios".into())
            .spawn(move || run(incoming, updates, worker_latches))
        {
            eprintln!("Radio control worker unavailable: {error}");
            latches.stopped.store(true, Ordering::Release);
        }
        Self { commands, latches }
    }

    pub fn refresh(&self, request: u64) -> bool {
        self.execute(request, Operation::Refresh)
    }

    pub fn execute(&self, request: u64, operation: Operation) -> bool {
        if self.latches.stopped.load(Ordering::Acquire)
            || self.latches.disabled.load(Ordering::Acquire)
            || self
                .latches
                .busy
                .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
                .is_err()
        {
            return false;
        }
        if self
            .commands
            .try_send(Work::Execute(request, operation))
            .is_err()
        {
            self.latches.busy.store(false, Ordering::Release);
            return false;
        }
        true
    }
}

impl Drop for Client {
    fn drop(&mut self) {
        self.latches.stopped.store(true, Ordering::Release);
        let _ = self.commands.try_send(Work::Stop);
        // No Wayland-thread join. The helper also has its own root-side hard
        // deadline: after pkexec, an ordinary user cannot kill its root child.
    }
}

fn run(incoming: Receiver<Work>, updates: Sender<Message>, latches: Arc<Latches>) {
    let mut snapshot = Snapshot::default();
    while let Ok(work) = incoming.recv() {
        if latches.stopped.load(Ordering::Acquire) {
            break;
        }
        let Work::Execute(request, operation) = work else {
            break;
        };
        let reply =
            invoke(operation, &latches).and_then(|text| protocol::Reply::parse(&text, operation));
        let (entries, result) = match reply {
            Ok(reply) => {
                snapshot = reply.snapshot;
                (reply.entries, reply.result)
            }
            Err(error) => {
                snapshot.failed(operation);
                (Vec::new(), Err(error))
            }
        };
        let message = Message {
            request,
            operation,
            snapshot: snapshot.clone(),
            entries,
            result,
        };
        latches.busy.store(false, Ordering::Release);
        if latches.stopped.load(Ordering::Acquire)
            || updates.send(message).is_err()
            || latches.disabled.load(Ordering::Acquire)
        {
            break;
        }
    }
}

struct Running<'a> {
    child: Option<Child>,
    disabled: &'a AtomicBool,
}
impl Running<'_> {
    fn child(&mut self) -> &mut Child {
        self.child.as_mut().expect("owned child")
    }
}
impl Drop for Running<'_> {
    fn drop(&mut self) {
        let Some(mut child) = self.child.take() else {
            return;
        };
        if matches!(child.try_wait(), Ok(Some(_))) {
            return;
        }
        let _ = child.kill();
        let until = Instant::now() + Duration::from_millis(100);
        loop {
            if matches!(child.try_wait(), Ok(Some(_))) {
                return;
            }
            if Instant::now() >= until {
                break;
            }
            thread::sleep(POLL);
        }
        // pkexec may be waiting for polkit with a root euid before our helper's
        // watchdog even starts. Never wait here: emit the bounded failure and
        // disable further requests instead of accumulating privileged children.
        // At most this one detached reaper is possible for the client lifetime.
        self.disabled.store(true, Ordering::Release);
        let _ = thread::Builder::new()
            .name("radio-reaper".into())
            .spawn(move || {
                let _ = child.wait();
            });
    }
}

fn nonblocking(pipe: &impl AsRawFd) -> Result<(), Failure> {
    // SAFETY: this changes flags on our live child pipe without pointers.
    let flags = unsafe { nix::libc::fcntl(pipe.as_raw_fd(), nix::libc::F_GETFL) };
    if flags < 0
        || unsafe {
            nix::libc::fcntl(
                pipe.as_raw_fd(),
                nix::libc::F_SETFL,
                flags | nix::libc::O_NONBLOCK,
            )
        } < 0
    {
        return Err(Failure::Unavailable);
    }
    Ok(())
}

fn drain(pipe: &mut impl Read, output: &mut Vec<u8>) -> Result<(), Failure> {
    let mut buffer = [0; 4096];
    loop {
        match pipe.read(&mut buffer) {
            Ok(0) => return Ok(()),
            Ok(size) => {
                if output.len() + size > protocol::MAX_REPLY {
                    return Err(Failure::InvalidResponse);
                }
                output.extend_from_slice(&buffer[..size]);
            }
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => return Ok(()),
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(_) => return Err(Failure::Unavailable),
        }
    }
}

fn invoke(operation: Operation, latches: &Latches) -> Result<String, Failure> {
    let mut command = if operation == Operation::Refresh {
        Command::new(HELPER)
    } else {
        let mut command = Command::new(PKEXEC);
        command.args(["--disable-internal-agent", HELPER]);
        command
    };
    command
        .arg(operation.argument())
        .env_clear()
        .env("PATH", "/usr/sbin:/usr/bin:/sbin:/bin")
        .env("LC_ALL", "C")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut running = Running {
        child: Some(command.spawn().map_err(|_| Failure::Unavailable)?),
        disabled: &latches.disabled,
    };
    let mut stdout = running.child().stdout.take().ok_or(Failure::Unavailable)?;
    let mut stderr = running.child().stderr.take().ok_or(Failure::Unavailable)?;
    nonblocking(&stdout)?;
    nonblocking(&stderr)?;
    let deadline = Instant::now() + DEADLINE;
    let mut output = Vec::new();
    let mut errors = Vec::new();
    loop {
        if latches.stopped.load(Ordering::Acquire) || Instant::now() >= deadline {
            return Err(Failure::Timeout);
        }
        drain(&mut stdout, &mut output)?;
        drain(&mut stderr, &mut errors)?;
        if let Some(status) = running
            .child()
            .try_wait()
            .map_err(|_| Failure::Unavailable)?
        {
            drain(&mut stdout, &mut output)?;
            drain(&mut stderr, &mut errors)?;
            if !status.success() {
                return Err(match status.code() {
                    Some(126 | 127) => Failure::Permission,
                    Some(124) => Failure::Timeout,
                    _ => Failure::Failed,
                });
            }
            return String::from_utf8(output).map_err(|_| Failure::InvalidResponse);
        }
        thread::sleep(POLL);
    }
}
