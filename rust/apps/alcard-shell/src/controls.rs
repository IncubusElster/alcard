//! On-demand device controls. Discovery and mutations never run on the Wayland
//! thread; one bounded worker owns one request at a time. Production paths and
//! commands are fixed, and unavailable hardware is never emulated.

mod audio;
mod brightness;

use smithay_client_toolkit::reexports::calloop::channel::Sender;
use std::{
    fs::OpenOptions,
    future::Future,
    io::{self, Read},
    os::{fd::AsRawFd, unix::fs::OpenOptionsExt},
    path::Path,
    process::{Child, Command, Stdio},
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, SyncSender},
        Arc,
    },
    thread,
    time::{Duration, Instant},
};
use zbus::{wire::DynamicDeserialize, DBusError};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Control {
    Brightness,
    Volume,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Direction {
    Decrease,
    Increase,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Operation {
    Adjust {
        control: Control,
        direction: Direction,
    },
    ToggleMute,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Availability {
    #[default]
    Checking,
    Available,
    Unavailable,
    Denied,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Reading {
    pub availability: Availability,
    pub percent: Option<u8>,
    /// None means that backend mute is unavailable. PipeWire provides a real
    /// software mute independently of hardware mixer switches. Zero volume
    /// is not evidence that the output is muted.
    pub muted: Option<bool>,
    /// An externally set level above the backend ceiling (PipeWire unity) is
    /// displayed as 100%+; increasing or unmuting such a level is refused.
    pub above_safe_limit: bool,
}

impl Reading {
    fn failed(error: Failure) -> Self {
        Self {
            availability: availability(error),
            ..Self::default()
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Snapshot {
    pub brightness: Reading,
    pub volume: Reading,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Failure {
    Unavailable,
    Denied,
    Timeout,
    /// A write may have occurred, or read-back did not confirm the request.
    Unknown,
    /// The device/service/control identity no longer matches discovery.
    Changed,
    Invalid,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Message {
    Refreshed {
        request: u64,
        snapshot: Snapshot,
    },
    Finished {
        request: u64,
        operation: Operation,
        snapshot: Snapshot,
        result: Result<(), Failure>,
    },
}

const READ_TIMEOUT: Duration = Duration::from_secs(6);
const WRITE_TIMEOUT: Duration = Duration::from_secs(6);
const CANCEL_INTERVAL: Duration = Duration::from_millis(20);
const MAX_REPLY_BYTES: usize = 64 * 1024;

#[derive(Default)]
struct Latches {
    stopped: AtomicBool,
    busy: AtomicBool,
}

enum Work {
    Refresh(u64),
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
            .name("alcard-controls".into())
            .spawn(move || run(incoming, updates, worker_latches))
        {
            eprintln!("Device control worker unavailable: {error}");
        }
        Self { commands, latches }
    }

    pub fn refresh(&self, request: u64) -> bool {
        self.submit(Work::Refresh(request))
    }

    pub fn execute(&self, request: u64, operation: Operation) -> bool {
        self.submit(Work::Execute(request, operation))
    }

    fn submit(&self, work: Work) -> bool {
        if self.latches.stopped.load(Ordering::Acquire)
            || self
                .latches
                .busy
                .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
                .is_err()
        {
            return false;
        }
        if self.commands.try_send(work).is_err() {
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
        // No UI-thread join. D-Bus waits and command polling observe stop;
        // subprocess RAII kills and reaps a still-running control helper.
    }
}

fn run(incoming: Receiver<Work>, updates: Sender<Message>, latches: Arc<Latches>) {
    let mut brightness = brightness::Backend::system();
    let mut audio = audio::Backend::system();
    let mut snapshot = Snapshot::default();
    while let Ok(work) = incoming.recv() {
        if latches.stopped.load(Ordering::Acquire) {
            break;
        }
        let message = match work {
            Work::Stop => break,
            Work::Refresh(request) => {
                snapshot.brightness =
                    brightness.refresh(&Budget::new(&latches.stopped, READ_TIMEOUT));
                snapshot.volume = audio.refresh(&Budget::new(&latches.stopped, READ_TIMEOUT));
                Message::Refreshed { request, snapshot }
            }
            Work::Execute(request, operation) => {
                let budget = Budget::new(&latches.stopped, WRITE_TIMEOUT);
                let result = match operation {
                    Operation::Adjust {
                        control: Control::Brightness,
                        direction,
                    } => {
                        let (reading, result) = brightness.adjust(direction, &budget);
                        snapshot.brightness = reading;
                        result
                    }
                    Operation::Adjust {
                        control: Control::Volume,
                        ..
                    }
                    | Operation::ToggleMute => {
                        let (reading, result) = audio.execute(operation, &budget);
                        snapshot.volume = reading;
                        result
                    }
                };
                Message::Finished {
                    request,
                    operation,
                    snapshot,
                    result,
                }
            }
        };
        latches.busy.store(false, Ordering::Release);
        if latches.stopped.load(Ordering::Acquire) || updates.send(message).is_err() {
            break;
        }
    }
}

fn availability(error: Failure) -> Availability {
    if error == Failure::Denied {
        Availability::Denied
    } else {
        Availability::Unavailable
    }
}

fn io_failure(error: &io::Error) -> Failure {
    if error.kind() == io::ErrorKind::PermissionDenied {
        Failure::Denied
    } else {
        Failure::Unavailable
    }
}

/// Only regular pseudo-files; O_NONBLOCK prevents a replaced FIFO from hanging
/// before the file-type check. Production callers use fixed /sys paths.
fn read_small(path: &Path) -> Result<String, Failure> {
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(nix::libc::O_NONBLOCK | nix::libc::O_NOFOLLOW)
        .open(path)
        .map_err(|error| io_failure(&error))?;
    if !file
        .metadata()
        .map_err(|error| io_failure(&error))?
        .is_file()
    {
        return Err(Failure::Invalid);
    }
    let mut text = String::new();
    file.take(513)
        .read_to_string(&mut text)
        .map_err(|error| io_failure(&error))?;
    if text.len() > 512 {
        return Err(Failure::Invalid);
    }
    Ok(text.trim().into())
}

fn safe_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 128
        && !matches!(name, "." | "..")
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"_-.+:".contains(&byte))
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

    fn check(&self, mutating: bool) -> Result<(), Failure> {
        if self.stopped.load(Ordering::Acquire) || Instant::now() >= self.deadline {
            Err(if mutating {
                Failure::Unknown
            } else {
                Failure::Timeout
            })
        } else {
            Ok(())
        }
    }

    fn dbus<T>(
        &self,
        future: impl Future<Output = zbus::Result<T>>,
        mutating: bool,
    ) -> Result<T, Failure> {
        self.check(false)?;
        let runtime = zruntime::SharedRuntime::current().map_err(|_| Failure::Unavailable)?;
        runtime.block_on(futures_lite::future::or(
            async {
                loop {
                    self.check(mutating)?;
                    runtime.sleep(CANCEL_INTERVAL).await;
                }
            },
            async { future.await.map_err(|error| bus_failure(&error, mutating)) },
        ))
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
        | "org.freedesktop.login1.NotYourDevice" => Failure::Denied,
        "org.freedesktop.DBus.Error.UnknownMethod"
        | "org.freedesktop.DBus.Error.UnknownInterface"
        | "org.freedesktop.DBus.Error.UnknownObject"
        | "org.freedesktop.DBus.Error.InvalidArgs"
        | "org.freedesktop.DBus.Error.NotSupported"
        | "org.freedesktop.DBus.Error.ServiceUnknown"
        | "org.freedesktop.DBus.Error.NameHasNoOwner"
        | "org.freedesktop.login1.NoSessionForPID"
        | "org.freedesktop.login1.NoSuchSession" => Failure::Unavailable,
        _ if mutating => Failure::Unknown,
        _ => Failure::Unavailable,
    }
}

fn decode<T: for<'d> DynamicDeserialize<'d>>(message: zbus::Message) -> Result<T, Failure> {
    if message.body().len() > MAX_REPLY_BYTES {
        return Err(Failure::Invalid);
    }
    message.body().deserialize().map_err(|_| Failure::Invalid)
}

struct Running(Child);

impl Drop for Running {
    fn drop(&mut self) {
        if !matches!(self.0.try_wait(), Ok(Some(_))) {
            let _ = self.0.kill();
        }
        let _ = self.0.wait();
    }
}

fn nonblocking(pipe: &impl AsRawFd) -> io::Result<()> {
    let fd = pipe.as_raw_fd();
    // SAFETY: fcntl only changes flags of this live, owned child pipe. No
    // pointers or ownership transfers are involved.
    let flags = unsafe { nix::libc::fcntl(fd, nix::libc::F_GETFL) };
    if flags < 0
        || unsafe { nix::libc::fcntl(fd, nix::libc::F_SETFL, flags | nix::libc::O_NONBLOCK) } < 0
    {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

fn drain(pipe: &mut impl Read, bytes: &mut Vec<u8>, limit: usize) -> Result<(), Failure> {
    let mut buffer = [0_u8; 4096];
    loop {
        match pipe.read(&mut buffer) {
            Ok(0) => return Ok(()),
            Ok(count) => {
                if bytes.len() + count > limit {
                    return Err(Failure::Invalid);
                }
                bytes.extend_from_slice(&buffer[..count]);
            }
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => return Ok(()),
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(_) => return Err(Failure::Unavailable),
        }
    }
}

fn run_command(
    command: &mut Command,
    budget: &Budget<'_>,
    mutating: bool,
) -> Result<String, Failure> {
    run_command_with_output_limit(command, budget, mutating, MAX_REPLY_BYTES)
}

/// PipeWire's complete graph is read through one connection, avoiding repeated
/// full graph enumeration by filtered helpers. Its caller may allow up to
/// 1 MiB of stdout; diagnostics remain bounded by the ordinary 64 KiB limit.
fn run_command_with_output_limit(
    command: &mut Command,
    budget: &Budget<'_>,
    mutating: bool,
    stdout_limit: usize,
) -> Result<String, Failure> {
    if stdout_limit == 0 || stdout_limit > 1024 * 1024 {
        return Err(Failure::Invalid);
    }
    budget.check(false)?;
    let mut child = Running(
        command
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|error| io_failure(&error))?,
    );
    let mut stdout = child.0.stdout.take().ok_or(Failure::Unknown)?;
    let mut stderr = child.0.stderr.take().ok_or(Failure::Unknown)?;
    let attempted_error = |error| if mutating { Failure::Unknown } else { error };
    nonblocking(&stdout)
        .and_then(|()| nonblocking(&stderr))
        .map_err(|error| attempted_error(io_failure(&error)))?;
    let mut output = Vec::new();
    let mut errors = Vec::new();
    loop {
        budget.check(mutating)?;
        drain(&mut stdout, &mut output, stdout_limit).map_err(attempted_error)?;
        drain(&mut stderr, &mut errors, MAX_REPLY_BYTES).map_err(attempted_error)?;
        if let Some(status) = child
            .0
            .try_wait()
            .map_err(|error| attempted_error(io_failure(&error)))?
        {
            drain(&mut stdout, &mut output, stdout_limit).map_err(attempted_error)?;
            drain(&mut stderr, &mut errors, MAX_REPLY_BYTES).map_err(attempted_error)?;
            if !status.success() {
                return Err(if mutating {
                    Failure::Unknown
                } else if String::from_utf8_lossy(&errors).contains("Permission denied") {
                    Failure::Denied
                } else {
                    Failure::Unavailable
                });
            }
            return String::from_utf8(output).map_err(|_| attempted_error(Failure::Invalid));
        }
        thread::sleep(CANCEL_INTERVAL);
    }
}

#[cfg(test)]
#[path = "unit_tests/controls.rs"]
mod tests;
