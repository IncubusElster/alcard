//! Public, non-secret recovery intent for one compositor lifetime.
//!
//! This journal cannot authenticate or unlock anything. A live locker holds an
//! exclusive advisory file lock. Its intent survives an abrupt exit, allowing
//! the Shell to start a replacement which must acquire its own compositor lock
//! and perform the usual current-user PAM conversation. Only the locker may
//! clear intent, after its authenticated unlock has been acknowledged.
use std::{
    ffi::CString,
    fs::{File, OpenOptions, TryLockError},
    io::{self, Read},
    os::{
        fd::{AsFd, AsRawFd, BorrowedFd, FromRawFd, OwnedFd},
        unix::fs::{FileExt, MetadataExt, OpenOptionsExt},
    },
    path::Path,
    sync::Arc,
};

const PENDING: u8 = b'1';
const CLEAR: u8 = b'0';

fn invalid(message: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Identity {
    uid: u32,
    pid: u32,
    start: u64,
}

fn start_time(pid: u32) -> io::Result<u64> {
    let mut stat = String::new();
    File::open(format!("/proc/{pid}/stat"))?
        .take(4097)
        .read_to_string(&mut stat)?;
    if stat.len() > 4096 || !stat.starts_with(&format!("{pid} (")) {
        return Err(invalid("Invalid compositor process identity"));
    }
    parse_start_time(&stat)
}

fn parse_start_time(stat: &str) -> io::Result<u64> {
    // comm may itself contain spaces and ')'; the remaining fields are
    // kernel-generated words. Field 22 is index 19 after the final ') '.
    let (_, fields) = stat
        .rsplit_once(") ")
        .ok_or_else(|| invalid("Invalid compositor process stat"))?;
    let mut fields = fields.split_whitespace();
    if matches!(fields.next(), None | Some("Z" | "X" | "x")) {
        return Err(invalid("Compositor is no longer alive"));
    }
    fields
        .nth(18)
        .and_then(|value| value.parse().ok())
        .filter(|value| *value > 0)
        .ok_or_else(|| invalid("Missing compositor process start time"))
}

fn peer(fd: BorrowedFd<'_>) -> io::Result<Identity> {
    let mut credentials = libc::ucred {
        pid: 0,
        uid: 0,
        gid: 0,
    };
    let mut length = std::mem::size_of_val(&credentials) as libc::socklen_t;
    // SAFETY: the borrowed socket outlives this call; both output pointers
    // describe initialized, correctly sized objects owned by this stack.
    if unsafe {
        libc::getsockopt(
            fd.as_raw_fd(),
            libc::SOL_SOCKET,
            libc::SO_PEERCRED,
            (&mut credentials as *mut libc::ucred).cast(),
            &mut length,
        )
    } != 0
    {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: these libc identity getters have no pointer preconditions.
    let uid = unsafe { libc::getuid() };
    if length as usize != std::mem::size_of_val(&credentials)
        || credentials.pid <= 0
        || credentials.uid != uid
        || unsafe { libc::geteuid() } != uid
    {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "Compositor must belong to the current ordinary session identity",
        ));
    }
    let pid = credentials.pid as u32;
    Ok(Identity {
        uid,
        pid,
        start: start_time(pid)?,
    })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Inode {
    device: u64,
    number: u64,
}

fn inode(file: &File, uid: u32) -> io::Result<Inode> {
    let metadata = file.metadata()?;
    if !metadata.is_file()
        || metadata.uid() != uid
        || metadata.mode() & 0o7777 != 0o600
        || metadata.nlink() != 1
        || metadata.len() > 1
    {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "Lock journal must be one private, regular, single-link file",
        ));
    }
    Ok(Inode {
        device: metadata.dev(),
        number: metadata.ino(),
    })
}

fn open_at(directory: &File, name: &CString, write: bool, create: bool) -> io::Result<File> {
    let flags = libc::O_CLOEXEC
        | libc::O_NOFOLLOW
        | libc::O_NONBLOCK
        | if write { libc::O_RDWR } else { libc::O_RDONLY }
        | if create { libc::O_CREAT } else { 0 };
    // SAFETY: directory remains open, name is a single NUL-terminated
    // component, and the returned owned descriptor is checked before use.
    let raw = unsafe { libc::openat(directory.as_raw_fd(), name.as_ptr(), flags, 0o600) };
    if raw < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: successful openat transfers a new descriptor to this File.
    Ok(unsafe { File::from_raw_fd(raw) })
}

fn intent(file: &File) -> io::Result<bool> {
    let mut byte = [0];
    match file.read_at(&mut byte, 0)? {
        0 => Ok(false),
        1 if byte[0] == CLEAR => Ok(false),
        1 if byte[0] == PENDING => Ok(true),
        _ => Err(invalid("Invalid lock journal record")),
    }
}

#[derive(Clone)]
pub struct Session {
    directory: Arc<File>,
    name: CString,
    identity: Identity,
    inode: Inode,
}

/// An advisory observation, never evidence of an authenticated unlock.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Observation {
    /// No recovery intent and no current writer. This does not query the
    /// compositor and does not grant any permission to unlock it.
    Clear,
    /// A locker holds the journal, possibly still preparing its lock request.
    Held,
    /// Intent remains but its owner exited. A replacement must lock and use PAM.
    Abandoned,
}

impl Session {
    /// Use the actual connected Wayland socket, not an environment-supplied
    /// PID, bus name or a separately reconnected display pathname.
    pub fn from_wayland_socket(socket: &impl AsFd) -> io::Result<Self> {
        let identity = peer(socket.as_fd())?;
        let runtime = std::env::var_os("XDG_RUNTIME_DIR")
            .ok_or_else(|| invalid("XDG_RUNTIME_DIR is unset"))?;
        Self::open(Path::new(&runtime), identity)
    }

    fn open(runtime: &Path, identity: Identity) -> io::Result<Self> {
        if !runtime.is_absolute() {
            return Err(invalid("XDG_RUNTIME_DIR must be absolute"));
        }
        let directory = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(runtime)?;
        let metadata = directory.metadata()?;
        if !metadata.is_dir() || metadata.uid() != identity.uid || metadata.mode() & 0o7777 != 0o700
        {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "XDG_RUNTIME_DIR must be private mode 0700 and owned by this user",
            ));
        }
        let name = CString::new(format!(
            "alcard-lock-v1-{}-{}.state",
            identity.pid, identity.start
        ))
        .expect("numeric session identity");
        let file = open_at(&directory, &name, true, true)?;
        let inode = inode(&file, identity.uid)?;
        let session = Self {
            directory: Arc::new(directory),
            name,
            identity,
            inode,
        };
        session.verify(&file)?;
        Ok(session)
    }

    pub fn is_live(&self) -> bool {
        start_time(self.identity.pid).is_ok_and(|start| start == self.identity.start)
    }

    fn verify(&self, file: &File) -> io::Result<()> {
        if inode(file, self.identity.uid)? != self.inode {
            return Err(invalid("Lock journal inode changed"));
        }
        // Reopen through the held private directory to reject replacement,
        // unlink and symlink races. We never unlink or rename a held inode.
        let current = open_at(&self.directory, &self.name, false, false)?;
        if inode(&current, self.identity.uid)? != self.inode {
            return Err(invalid("Lock journal pathname changed"));
        }
        Ok(())
    }

    fn file(&self, write: bool) -> io::Result<File> {
        let file = open_at(&self.directory, &self.name, write, false)?;
        self.verify(&file)?;
        Ok(file)
    }

    pub fn observe(&self) -> io::Result<Observation> {
        let file = self.file(false)?;
        match file.try_lock() {
            Ok(()) => Ok(if intent(&file)? {
                Observation::Abandoned
            } else {
                Observation::Clear
            }),
            Err(TryLockError::WouldBlock) => Ok(Observation::Held),
            Err(TryLockError::Error(error)) => Err(error),
        }
        // This read-only descriptor closes here, releasing only its own lock.
    }

    pub fn try_lease(&self) -> io::Result<Option<Lease>> {
        if !self.is_live() {
            return Err(invalid("Compositor lifetime ended"));
        }
        let file = self.file(true)?;
        match file.try_lock() {
            Ok(()) => {
                // A malformed journal cannot be silently overwritten or
                // reinterpreted as a safe, previously unlocked session.
                let _ = intent(&file)?;
                Ok(Some(Lease {
                    session: self.clone(),
                    file,
                }))
            }
            Err(TryLockError::WouldBlock) => Ok(None),
            Err(TryLockError::Error(error)) => Err(error),
        }
    }

    /// Listen for intent writes and owner descriptor closure. Inotify wakes
    /// even after SIGKILL; no polling of /proc or periodic idle retry is needed.
    pub fn watch(&self) -> io::Result<Watch> {
        let file = self.file(false)?;
        // SAFETY: inotify_init1 has no pointer parameters.
        let raw = unsafe { libc::inotify_init1(libc::IN_CLOEXEC | libc::IN_NONBLOCK) };
        if raw < 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: the successful syscall returned a new owned descriptor.
        let fd = unsafe { OwnedFd::from_raw_fd(raw) };
        let path = CString::new(format!("/proc/self/fd/{}", file.as_raw_fd()))
            .expect("numeric descriptor path");
        // SAFETY: fd is a live inotify instance and path is NUL-terminated.
        if unsafe {
            libc::inotify_add_watch(
                fd.as_raw_fd(),
                path.as_ptr(),
                libc::IN_MODIFY
                    | libc::IN_CLOSE_WRITE
                    | libc::IN_ATTRIB
                    | libc::IN_MOVE_SELF
                    | libc::IN_DELETE_SELF,
            )
        } < 0
        {
            return Err(io::Error::last_os_error());
        }
        self.verify(&file)?;
        Ok(Watch { fd, _file: file })
    }
}

/// Ownership of one existing inode. Drop releases the lease but deliberately
/// retains any pending intent; error, cancellation and panic are not unlocks.
pub struct Lease {
    session: Session,
    file: File,
}

impl Lease {
    /// Must complete before sending ext_session_lock_manager_v1.lock.
    pub fn mark_requested(&self) -> io::Result<()> {
        if !self.session.is_live() {
            return Err(invalid("Compositor lifetime ended"));
        }
        self.session.verify(&self.file)?;
        self.file.write_all_at(&[PENDING], 0)
    }

    /// Call only after current-user PAM success AND the Wayland sync following
    /// unlock_and_destroy. This public receipt itself has no unlock capability.
    pub fn authenticated_unlock_acknowledged(&self) -> io::Result<()> {
        self.session.verify(&self.file)?;
        self.file.write_all_at(&[CLEAR], 0)
    }
}

pub struct Watch {
    fd: OwnedFd,
    _file: File,
}

impl AsFd for Watch {
    fn as_fd(&self) -> BorrowedFd<'_> {
        self.fd.as_fd()
    }
}

impl Watch {
    pub fn drain(&self) -> io::Result<()> {
        // We need only a wakeup, not event pathnames or an interpretation of
        // event contents. The caller always revalidates the journal inode.
        let mut bytes = [0_u8; 4096];
        loop {
            // SAFETY: fd is a live nonblocking inotify descriptor and bytes
            // describes its complete writable output buffer.
            let count =
                unsafe { libc::read(self.fd.as_raw_fd(), bytes.as_mut_ptr().cast(), bytes.len()) };
            if count > 0 {
                continue;
            }
            if count == 0 {
                return Err(invalid("Lock journal watch closed"));
            }
            let error = io::Error::last_os_error();
            match error.kind() {
                io::ErrorKind::WouldBlock => return Ok(()),
                io::ErrorKind::Interrupted => continue,
                _ => return Err(error),
            }
        }
    }
}

#[cfg(test)]
#[path = "unit_tests/lib.rs"]
mod tests;
