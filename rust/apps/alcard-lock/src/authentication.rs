//! Cancellable, unprivileged PAM conversation. The GUI never enters libpam:
//! one private child can block in a module without blocking the lock surfaces.
use crate::{
    protocol::{self, Command, Event, Failure},
    secret::Secret,
};
use nix::{
    fcntl::{fcntl, FcntlArg, OFlag},
    libc,
    sys::signal::{killpg, Signal},
    unistd::{Gid, Pid, Uid, User},
};
use smithay_client_toolkit::reexports::calloop::channel::Sender;
use std::{
    ffi::{c_char, c_int, c_void, CStr, CString},
    fs::{File, OpenOptions},
    io::{self, Read, Write},
    os::unix::{
        fs::{MetadataExt, OpenOptionsExt},
        io::AsFd,
        process::CommandExt,
    },
    path::{Path, PathBuf},
    process::{Child, ChildStdin, ChildStdout, Command as ProcessCommand, Stdio},
    ptr,
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, SyncSender},
        Arc,
    },
    thread,
    time::{Duration, Instant},
};
use zeroize::{Zeroize, Zeroizing};

const SERVICE: &CStr = c"alcard-lock";
const POLICY: &str = "/etc/pam.d/alcard-lock";
const STARTUP_TIMEOUT: Duration = Duration::from_secs(8);
const ATTEMPT_TIMEOUT: Duration = Duration::from_secs(120);
const TICK: Duration = Duration::from_millis(20);
const MAX_PAM_MESSAGES: usize = 32;

pub struct Worker {
    commands: SyncSender<Command>,
    stopped: Arc<AtomicBool>,
}
impl Worker {
    pub fn start(events: Sender<Event>) -> io::Result<Self> {
        let (commands, incoming) = mpsc::sync_channel(4);
        let stopped = Arc::new(AtomicBool::new(false));
        let worker_stop = stopped.clone();
        let executable = helper_executable()?;
        thread::Builder::new()
            .name("alcard-lock-pam".into())
            .spawn(move || {
                controller(incoming, events, worker_stop, executable);
            })?;
        Ok(Self { commands, stopped })
    }
    pub fn send(&self, command: Command) -> bool {
        !self.stopped.load(Ordering::Acquire) && self.commands.try_send(command).is_ok()
    }
}
impl Drop for Worker {
    fn drop(&mut self) {
        self.stopped.store(true, Ordering::Release);
        let _ = self.commands.try_send(Command::Shutdown);
        // No join on the Wayland thread. The controller kills/reaps only its
        // own auth subprocess. It never terminates the locker or session.
    }
}

fn helper_executable() -> io::Result<PathBuf> {
    // A Rust test executable cannot dispatch production main's private worker
    // argument. Integration uses the separately built production binary.
    // This override is entirely absent from an installed locker.
    #[cfg(test)]
    if let Some(path) = std::env::var_os("ALCARD_LOCK_TEST_HELPER") {
        return Ok(PathBuf::from(path));
    }
    std::env::current_exe()
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Identity {
    uid: u32,
    user: String,
}
impl Identity {
    fn current() -> Result<Self, Failure> {
        let uid = Uid::current();
        if uid.is_root() || uid != Uid::effective() || Gid::current() != Gid::effective() {
            return Err(Failure::IdentityChanged);
        }
        let user = User::from_uid(uid)
            .map_err(|_| Failure::Unavailable)?
            .ok_or(Failure::Unavailable)?;
        if user.uid != uid || !valid_username(&user.name) {
            return Err(Failure::IdentityChanged);
        }
        Ok(Self {
            uid: uid.as_raw(),
            user: user.name,
        })
    }
}

fn valid_username(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 256
        && !name.chars().any(char::is_control)
        && protocol::plain(name, 256) == name
}

/// A child may speak only for one monotonic attempt and one pending prompt.
/// Responses from cancelled/previous attempts cannot enter a new PAM handle.
#[derive(Default)]
struct Gate {
    ready: bool,
    attempt: Option<u64>,
    serial: u64,
    waiting: Option<u64>,
    finished: Option<Result<(), Failure>>,
}
impl Gate {
    fn challenge(&mut self, attempt: u64, serial: u64) -> bool {
        if !self.ready
            || self.attempt != Some(attempt)
            || serial == 0
            || serial <= self.serial
            || self.waiting.is_some()
            || self.finished.is_some()
        {
            return false;
        }
        self.serial = serial;
        self.waiting = Some(serial);
        true
    }
    fn response(&mut self, attempt: u64, serial: u64) -> bool {
        if self.ready
            && self.attempt == Some(attempt)
            && self.waiting == Some(serial)
            && self.finished.is_none()
        {
            self.waiting = None;
            true
        } else {
            false
        }
    }
    fn finish(&mut self, attempt: u64, result: Result<(), Failure>) -> bool {
        if !self.ready
            || self.attempt != Some(attempt)
            || self.finished.is_some()
            || self.waiting.is_some()
        {
            return false;
        }
        self.finished = Some(result);
        true
    }
}

struct Process {
    child: Child,
    input: ChildStdin,
    output: ChildStdout,
    incoming: Vec<u8>,
    outgoing: Zeroizing<Vec<u8>>,
    written: usize,
    gate: Gate,
    deadline: Option<Instant>,
    eof: bool,
    reaped: bool,
}
impl Process {
    fn spawn(executable: &Path, attempt: Option<u64>) -> io::Result<Self> {
        let mut child = ProcessCommand::new(executable)
            .arg("--pam-worker")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .env_clear()
            .env("PATH", "/usr/sbin:/usr/bin:/sbin:/bin")
            .process_group(0)
            .spawn()?;
        let result = (|| {
            let input = child.stdin.take().ok_or_else(unavailable_io)?;
            let output = child.stdout.take().ok_or_else(unavailable_io)?;
            for fd in [input.as_fd(), output.as_fd()] {
                let flags = OFlag::from_bits_truncate(fcntl(fd, FcntlArg::F_GETFL)?);
                fcntl(fd, FcntlArg::F_SETFL(flags | OFlag::O_NONBLOCK))?;
            }
            Ok((input, output))
        })();
        let (input, output) = match result {
            Ok(pipes) => pipes,
            Err(error) => {
                let _ = killpg(Pid::from_raw(child.id() as i32), Signal::SIGKILL);
                let _ = child.wait();
                return Err(error);
            }
        };
        Ok(Self {
            child,
            input,
            output,
            incoming: Vec::with_capacity(protocol::MAX_PACKET * 2),
            outgoing: Zeroizing::new(Vec::new()),
            written: 0,
            gate: Gate {
                attempt,
                ..Gate::default()
            },
            deadline: Some(Instant::now() + STARTUP_TIMEOUT),
            eof: false,
            reaped: false,
        })
    }
    fn queue(&mut self, command: &Command) -> io::Result<()> {
        if !self.outgoing.is_empty() {
            return Err(unavailable_io());
        }
        self.outgoing = protocol::encode_command(command)?;
        self.written = 0;
        Ok(())
    }
    fn flush(&mut self) -> io::Result<()> {
        while self.written < self.outgoing.len() {
            match self.input.write(&self.outgoing[self.written..]) {
                Ok(0) => return Err(unavailable_io()),
                Ok(length) => self.written += length,
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => return Ok(()),
                Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                Err(error) => return Err(error),
            }
        }
        self.outgoing.zeroize();
        self.written = 0;
        Ok(())
    }
    fn receive(&mut self) -> io::Result<Vec<Event>> {
        let mut bytes = [0; protocol::MAX_PACKET];
        let mut events = Vec::new();
        // Drain through EOF before inspecting a child's exit status. PAM can
        // emit several informational messages followed immediately by its
        // result; an exited child must not hide a result still in the pipe.
        for _ in 0..MAX_PAM_MESSAGES + 4 {
            let length = match self.output.read(&mut bytes) {
                Ok(0) => {
                    self.eof = true;
                    break;
                }
                Ok(length) => length,
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => break,
                Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                Err(error) => return Err(error),
            };
            if self.incoming.len() + length > protocol::MAX_PACKET * 2 + 4 {
                return Err(unavailable_io());
            }
            self.incoming.extend_from_slice(&bytes[..length]);
            while let Some(event) = protocol::next_event(&mut self.incoming)? {
                if events.len() >= MAX_PAM_MESSAGES + 4 {
                    return Err(unavailable_io());
                }
                events.push(event);
            }
        }
        Ok(events)
    }
}
impl Drop for Process {
    fn drop(&mut self) {
        self.outgoing.zeroize();
        if !self.reaped {
            // The unreaped child still reserves its PID/process-group ID.
            // A group signal cannot target a later unrelated process here.
            let _ = killpg(Pid::from_raw(self.child.id() as i32), Signal::SIGKILL);
            let _ = self.child.wait();
        }
    }
}

fn unavailable_io() -> io::Error {
    io::Error::other("Private PAM authentication is unavailable")
}

fn stopped_attempt(events: &Sender<Event>, process: &Process, failure: Failure) {
    if let Some(attempt) = process.gate.attempt {
        let _ = events.send(Event::Finished {
            attempt,
            result: Err(failure),
        });
    } else {
        let _ = events.send(Event::Fatal(failure));
    }
}

fn exited_event(
    initialized: bool,
    attempt: Option<u64>,
    result: Result<(), Failure>,
) -> Option<Event> {
    match attempt {
        Some(attempt) => Some(Event::Finished { attempt, result }),
        None if !initialized => Some(Event::Fatal(Failure::Unavailable)),
        // The initialized idle helper holds no authentication result. Keep
        // the pinned identity and let explicit Begin create a fresh child.
        None => None,
    }
}

fn controller(
    commands: Receiver<Command>,
    events: Sender<Event>,
    stopped: Arc<AtomicBool>,
    executable: PathBuf,
) {
    let mut process = match Process::spawn(&executable, None) {
        Ok(process) => Some(process),
        Err(_) => {
            let _ = events.send(Event::Fatal(Failure::Unavailable));
            return;
        }
    };
    let mut identity: Option<Identity> = None;
    let mut last_attempt = 0;
    while !stopped.load(Ordering::Acquire) {
        // Never allow a hostile flood to starve subprocess/deadline handling.
        for _ in 0..4 {
            let command = match commands.try_recv() {
                Ok(command) => command,
                Err(mpsc::TryRecvError::Empty) => break,
                Err(mpsc::TryRecvError::Disconnected) => return,
            };
            match command {
                Command::Shutdown => return,
                Command::Begin { attempt } if attempt > last_attempt && identity.is_some() => {
                    if process
                        .as_ref()
                        .is_some_and(|child| child.gate.attempt.is_some())
                    {
                        continue;
                    }
                    last_attempt = attempt;
                    if process.is_none() {
                        match Process::spawn(&executable, Some(attempt)) {
                            Ok(child) => process = Some(child),
                            Err(_) => {
                                let _ = events.send(Event::Finished {
                                    attempt,
                                    result: Err(Failure::Unavailable),
                                });
                            }
                        }
                    } else if let Some(child) = process.as_mut() {
                        child.gate.attempt = Some(attempt);
                        child.deadline = Some(Instant::now() + ATTEMPT_TIMEOUT);
                        if child.queue(&Command::Begin { attempt }).is_err() {
                            stopped_attempt(&events, child, Failure::Unavailable);
                            process = None;
                        }
                    }
                }
                Command::Respond {
                    attempt,
                    serial,
                    secret,
                } => {
                    if let Some(child) = process.as_mut() {
                        if child.gate.response(attempt, serial)
                            && child
                                .queue(&Command::Respond {
                                    attempt,
                                    serial,
                                    secret,
                                })
                                .is_err()
                        {
                            stopped_attempt(&events, child, Failure::Unavailable);
                            process = None;
                        }
                    }
                }
                Command::Cancel { attempt }
                    if process
                        .as_ref()
                        .is_some_and(|child| child.gate.attempt == Some(attempt)) =>
                {
                    process = None; // Kill/reap before permitting another attempt.
                    let _ = events.send(Event::Finished {
                        attempt,
                        result: Err(Failure::Cancelled),
                    });
                }
                _ => {}
            }
        }
        let Some(child) = process.as_mut() else {
            thread::sleep(TICK);
            continue;
        };
        if child
            .deadline
            .is_some_and(|deadline| Instant::now() >= deadline)
        {
            stopped_attempt(&events, child, Failure::Timeout);
            process = None;
            continue;
        }
        let received = child.flush().and_then(|()| child.receive());
        let mut failed = None;
        match received {
            Err(_) => failed = Some(Failure::Unavailable),
            Ok(received) => {
                for event in received {
                    match event {
                        Event::Ready { uid, user } => {
                            let incoming = Identity { uid, user };
                            if child.gate.ready
                                || incoming.uid != Uid::current().as_raw()
                                || !valid_username(&incoming.user)
                                || identity
                                    .as_ref()
                                    .is_some_and(|expected| expected != &incoming)
                            {
                                failed = Some(Failure::IdentityChanged);
                                break;
                            }
                            child.gate.ready = true;
                            if identity.is_none() {
                                if events
                                    .send(Event::Ready {
                                        uid,
                                        user: incoming.user.clone(),
                                    })
                                    .is_err()
                                {
                                    return;
                                }
                                identity = Some(incoming);
                            }
                            if let Some(attempt) = child.gate.attempt {
                                child.deadline = Some(Instant::now() + ATTEMPT_TIMEOUT);
                                if child.queue(&Command::Begin { attempt }).is_err() {
                                    failed = Some(Failure::Unavailable);
                                    break;
                                }
                            } else {
                                child.deadline = None;
                            }
                        }
                        Event::Challenge {
                            attempt,
                            serial,
                            prompt,
                            echo,
                        } => {
                            if !child.gate.ready || !child.gate.challenge(attempt, serial) {
                                failed = Some(Failure::Unavailable);
                                break;
                            }
                            if events
                                .send(Event::Challenge {
                                    attempt,
                                    serial,
                                    prompt,
                                    echo,
                                })
                                .is_err()
                            {
                                return;
                            }
                        }
                        Event::Notice {
                            attempt,
                            text,
                            error,
                        } => {
                            if child.gate.attempt != Some(attempt) || child.gate.finished.is_some()
                            {
                                failed = Some(Failure::Unavailable);
                                break;
                            }
                            if events
                                .send(Event::Notice {
                                    attempt,
                                    text,
                                    error,
                                })
                                .is_err()
                            {
                                return;
                            }
                        }
                        Event::Finished { attempt, result } => {
                            if !child.gate.ready || !child.gate.finish(attempt, result) {
                                failed = Some(Failure::Unavailable);
                                break;
                            }
                            // The child must also exit cleanly after pam_end.
                            child.deadline = Some(Instant::now() + STARTUP_TIMEOUT);
                        }
                        Event::Fatal(failure) => {
                            failed = Some(failure);
                            break;
                        }
                    }
                }
            }
        }
        if let Some(failure) = failed {
            stopped_attempt(&events, child, failure);
            let startup = identity.is_none();
            process = None;
            if startup {
                return;
            }
            continue;
        }
        // An exit can race the final nonblocking read. Do not reap/decide
        // until EOF proves all result bytes have been consumed.
        if !child.eof {
            thread::sleep(TICK);
            continue;
        }
        match child.child.try_wait() {
            Ok(Some(status)) => {
                child.reaped = true;
                let result = if status.success() && child.incoming.is_empty() {
                    child.gate.finished.unwrap_or(Err(Failure::Unavailable))
                } else {
                    Err(Failure::Unavailable)
                };
                if let Some(event) = exited_event(identity.is_some(), child.gate.attempt, result) {
                    if events.send(event).is_err() {
                        return;
                    }
                }
                let startup = identity.is_none();
                process = None;
                if startup {
                    return;
                }
            }
            Err(_) => {
                stopped_attempt(&events, child, Failure::Unavailable);
                process = None;
            }
            Ok(None) => {}
        }
        thread::sleep(TICK);
    }
}

// Minimal Linux-PAM ABI from <security/pam_appl.h> and <security/_pam_types.h>.
// No binding to a polkit administrator identity and no direct shadow access.
const PAM_SUCCESS: c_int = 0;
const PAM_AUTH_ERR: c_int = 7;
const PAM_CONV_ERR: c_int = 19;
const PAM_ABORT: c_int = 26;
const PAM_USER: c_int = 2;
const PAM_DISALLOW_NULL_AUTHTOK: c_int = 1;

#[repr(C)]
struct PamMessage {
    style: c_int,
    text: *const c_char,
}
#[repr(C)]
struct PamResponse {
    response: *mut c_char,
    code: c_int,
}
#[repr(C)]
struct PamConversation {
    callback: unsafe extern "C" fn(
        c_int,
        *const *const PamMessage,
        *mut *mut PamResponse,
        *mut c_void,
    ) -> c_int,
    context: *mut c_void,
}
#[link(name = "pam")]
unsafe extern "C" {
    fn pam_start(
        service: *const c_char,
        user: *const c_char,
        conversation: *const PamConversation,
        handle: *mut *mut c_void,
    ) -> c_int;
    fn pam_authenticate(handle: *mut c_void, flags: c_int) -> c_int;
    fn pam_acct_mgmt(handle: *mut c_void, flags: c_int) -> c_int;
    fn pam_get_item(handle: *const c_void, item: c_int, value: *mut *const c_void) -> c_int;
    fn pam_end(handle: *mut c_void, status: c_int) -> c_int;
}

struct PamHandle {
    handle: *mut c_void,
    status: c_int,
}
impl PamHandle {
    fn close(&mut self) -> c_int {
        if self.handle.is_null() {
            return PAM_ABORT;
        }
        let handle = std::mem::replace(&mut self.handle, ptr::null_mut());
        // SAFETY: this uniquely owned live handle came from pam_start.
        unsafe { pam_end(handle, self.status) }
    }
}
impl Drop for PamHandle {
    fn drop(&mut self) {
        if !self.handle.is_null() {
            self.close();
        }
    }
}

struct Conversation {
    input: io::Stdin,
    output: io::Stdout,
    attempt: u64,
    serial: u64,
    messages: usize,
}

/// C-allocated response ownership transfers to PAM only on full success.
/// A cancelled or malformed multi-message exchange wipes partial responses.
struct Responses {
    pointer: *mut PamResponse,
    lengths: [usize; MAX_PAM_MESSAGES],
    count: usize,
}
impl Responses {
    fn new(count: usize) -> Option<Self> {
        // SAFETY: bounded allocation, correct alignment supplied by calloc.
        let pointer = unsafe { libc::calloc(count, std::mem::size_of::<PamResponse>()) }
            .cast::<PamResponse>();
        if pointer.is_null() {
            return None;
        }
        Some(Self {
            pointer,
            lengths: [0; MAX_PAM_MESSAGES],
            count,
        })
    }
    fn set(&mut self, index: usize, secret: &Secret) -> bool {
        let bytes = secret.as_str().as_bytes();
        // SAFETY: calloc supplies room for the exact response and trailing NUL.
        let response = unsafe { libc::calloc(bytes.len() + 1, 1) }.cast::<c_char>();
        if response.is_null() {
            return false;
        }
        // SAFETY: the bounded source does not overlap this new allocation;
        // index is from the checked PAM message loop.
        unsafe {
            ptr::copy_nonoverlapping(bytes.as_ptr(), response.cast(), bytes.len());
            (*self.pointer.add(index)).response = response;
        }
        self.lengths[index] = bytes.len() + 1;
        true
    }
    fn transfer(mut self) -> *mut PamResponse {
        let pointer = self.pointer;
        self.pointer = ptr::null_mut();
        pointer
    }
}
impl Drop for Responses {
    fn drop(&mut self) {
        if self.pointer.is_null() {
            return;
        }
        for index in 0..self.count {
            // SAFETY: this guard owns every non-null allocation until transfer.
            unsafe {
                let response = (*self.pointer.add(index)).response;
                if !response.is_null() {
                    std::slice::from_raw_parts_mut(response.cast::<u8>(), self.lengths[index])
                        .zeroize();
                    libc::free(response.cast());
                }
            }
        }
        // SAFETY: the response array itself was allocated by calloc.
        unsafe { libc::free(self.pointer.cast()) };
    }
}

unsafe fn bounded_c_text<'a>(value: *const c_char, maximum: usize) -> Option<&'a str> {
    if value.is_null() {
        return None;
    }
    // SAFETY: callers provide a live PAM/NSS NUL-terminated string. A bound
    // prevents unbounded scanning/copying of an oversized trusted-module item.
    let length = unsafe { libc::strnlen(value, maximum + 1) };
    if length > maximum {
        return None;
    }
    // SAFETY: strnlen established the valid prefix; its lifetime stays inside
    // the owning PAM callback/handle call.
    std::str::from_utf8(unsafe { std::slice::from_raw_parts(value.cast(), length) }).ok()
}

unsafe extern "C" fn conversation_callback(
    count: c_int,
    messages: *const *const PamMessage,
    output: *mut *mut PamResponse,
    data: *mut c_void,
) -> c_int {
    if count <= 0
        || count as usize > MAX_PAM_MESSAGES
        || messages.is_null()
        || output.is_null()
        || data.is_null()
    {
        return PAM_CONV_ERR;
    }
    // SAFETY: PAM supplies a writable response pointer; start it empty even
    // when a later prompt is unsupported or the private pipe disappears.
    unsafe { *output = ptr::null_mut() };
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        // SAFETY: worker_main keeps this unique context alive through pam_end.
        let state = unsafe { &mut *data.cast::<Conversation>() };
        if state.attempt == 0 {
            return Err(());
        }
        let mut replies = Responses::new(count as usize).ok_or(())?;
        for index in 0..count as usize {
            state.messages += 1;
            if state.messages > MAX_PAM_MESSAGES {
                return Err(());
            }
            // SAFETY: Linux-PAM passes an array of count message pointers.
            let message = unsafe { *messages.add(index) };
            if message.is_null() {
                return Err(());
            }
            // SAFETY: checked PAM message pointer is valid for this callback.
            let message = unsafe { &*message };
            // SAFETY: PAM owns the prompt throughout this callback.
            let text = protocol::plain(
                unsafe { bounded_c_text(message.text, 1024) }.ok_or(())?,
                256,
            );
            match message.style {
                1 | 2 => {
                    state.serial = state.serial.checked_add(1).ok_or(())?;
                    protocol::write_event(
                        &mut state.output,
                        &Event::Challenge {
                            attempt: state.attempt,
                            serial: state.serial,
                            prompt: text,
                            echo: message.style == 2,
                        },
                    )
                    .map_err(|_| ())?;
                    let Command::Respond {
                        attempt,
                        serial,
                        secret,
                    } = protocol::read_command(&mut state.input).map_err(|_| ())?
                    else {
                        return Err(());
                    };
                    if attempt != state.attempt
                        || serial != state.serial
                        || !replies.set(index, &secret)
                    {
                        return Err(());
                    }
                    // Secret is wiped here. PAM owns the C copy only after all
                    // responses succeed; Linux-PAM clears/frees it afterwards.
                }
                3 | 4 => protocol::write_event(
                    &mut state.output,
                    &Event::Notice {
                        attempt: state.attempt,
                        text,
                        error: message.style == 3,
                    },
                )
                .map_err(|_| ())?,
                _ => return Err(()),
            }
        }
        // SAFETY: output is the live writable pointer checked at entry.
        unsafe { *output = replies.transfer() };
        Ok(())
    }));
    if matches!(result, Ok(Ok(()))) {
        PAM_SUCCESS
    } else {
        PAM_CONV_ERR
    }
}

fn policy_file(path: &Path) -> io::Result<File> {
    // Avoid a FIFO open blocking before the GUI can establish its lock.
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NONBLOCK | libc::O_NOFOLLOW)
        .open(path)?;
    let metadata = file.metadata()?;
    if !metadata.is_file()
        || metadata.uid() != 0
        || metadata.mode() & 0o022 != 0
        || metadata.len() == 0
        || metadata.len() > 16 * 1024
    {
        return Err(unavailable_io());
    }
    Ok(file)
}

fn pam_failure(code: c_int, account: bool) -> Failure {
    match code {
        6 | 7 | 10 | 11 if !account => Failure::Denied,
        6 | 7 | 10 | 11 | 12 | 13 | 16 | 27 if account => Failure::AccountDenied,
        _ => Failure::Unavailable,
    }
}

fn verified_pam_user(handle: *mut c_void, expected: &Identity) -> bool {
    let mut name = ptr::null();
    // SAFETY: handle is live and name receives a borrowed PAM_USER item.
    if unsafe { pam_get_item(handle, PAM_USER, &mut name) } != PAM_SUCCESS {
        return false;
    }
    // SAFETY: successful pam_get_item returns a PAM-owned string or null.
    let Some(name) = (unsafe { bounded_c_text(name.cast(), 256) }) else {
        return false;
    };
    if !valid_username(name) {
        return false;
    }
    User::from_name(name).ok().flatten().is_some_and(|user| {
        user.uid.as_raw() == expected.uid
            && Uid::current().as_raw() == expected.uid
            && Uid::current() == Uid::effective()
            && Gid::current() == Gid::effective()
    })
}

/// Private subprocess entry, never an alternate unlock implementation. Only
/// the GUI holding ext_session_lock_v1 can issue unlock_and_destroy.
pub fn worker_main() -> io::Result<()> {
    let input = io::stdin();
    let output = io::stdout();
    for fd in [input.as_fd(), output.as_fd()] {
        let metadata = nix::sys::stat::fstat(fd).map_err(io::Error::from)?;
        if metadata.st_mode & libc::S_IFMT != libc::S_IFIFO {
            return Err(unavailable_io());
        }
    }
    let mut context = Box::new(Conversation {
        input: io::stdin(),
        output: io::stdout(),
        attempt: 0,
        serial: 0,
        messages: 0,
    });
    let identity = match Identity::current() {
        Ok(identity) => identity,
        Err(error) => {
            protocol::write_event(&mut context.output, &Event::Fatal(error))?;
            return Err(unavailable_io());
        }
    };
    if policy_file(Path::new(POLICY)).is_err() {
        protocol::write_event(&mut context.output, &Event::Fatal(Failure::Unavailable))?;
        return Err(unavailable_io());
    }
    let username = CString::new(identity.user.as_str()).map_err(|_| unavailable_io())?;
    let conversation = PamConversation {
        callback: conversation_callback,
        context: (&mut *context as *mut Conversation).cast(),
    };
    let mut handle = PamHandle {
        handle: ptr::null_mut(),
        status: PAM_ABORT,
    };
    // SAFETY: service and username are NUL terminated; the callback/context
    // and handle outlive every PAM call, including pam_end.
    let start = unsafe {
        pam_start(
            SERVICE.as_ptr(),
            username.as_ptr(),
            &conversation,
            &mut handle.handle,
        )
    };
    if start != PAM_SUCCESS || handle.handle.is_null() {
        protocol::write_event(&mut context.output, &Event::Fatal(Failure::Unavailable))?;
        return Err(unavailable_io());
    }
    protocol::write_event(
        &mut context.output,
        &Event::Ready {
            uid: identity.uid,
            user: identity.user.clone(),
        },
    )?;
    let Command::Begin { attempt } = protocol::read_command(&mut context.input)? else {
        return Err(unavailable_io());
    };
    context.attempt = attempt;
    // SAFETY: this subprocess uniquely owns the valid PAM handle. Modules may
    // block; the parent's deadline can terminate this process group safely.
    handle.status = unsafe { pam_authenticate(handle.handle, PAM_DISALLOW_NULL_AUTHTOK) };
    let mut result = if handle.status == PAM_SUCCESS {
        Ok(())
    } else {
        Err(pam_failure(handle.status, false))
    };
    if result.is_ok() {
        // SAFETY: same live authenticated PAM handle; no session is opened and
        // no credentials are changed or administrative identity accepted.
        handle.status = unsafe { pam_acct_mgmt(handle.handle, PAM_DISALLOW_NULL_AUTHTOK) };
        if handle.status != PAM_SUCCESS {
            result = Err(pam_failure(handle.status, true));
        }
    }
    if result.is_ok() && !verified_pam_user(handle.handle, &identity) {
        handle.status = PAM_AUTH_ERR;
        result = Err(Failure::IdentityChanged);
    }
    if handle.close() != PAM_SUCCESS {
        result = Err(Failure::Unavailable);
    }
    protocol::write_event(&mut context.output, &Event::Finished { attempt, result })
}

#[cfg(test)]
#[path = "unit_tests/authentication.rs"]
mod tests;
