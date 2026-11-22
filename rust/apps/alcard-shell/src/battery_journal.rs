//! A session-local automatic-sleep latch which survives a Shell restart. A
//! malformed or inaccessible marker fails closed; it never authorizes sleep.
use crate::battery::Inhibit;
use nix::libc;
use std::{
    env,
    fs::{self, File, Metadata, OpenOptions},
    io::{self, Write},
    os::{
        fd::{AsRawFd, FromRawFd},
        unix::fs::{FileExt, MetadataExt, OpenOptionsExt},
    },
    path::{Path, PathBuf},
};

const NAME: &std::ffi::CStr = c"alcard-battery-inhibit-v1";
const CONTENT: &[u8] = b"alcard-battery-inhibit-v1:attempt\n";
const RESUME_CONTENT: &[u8] = b"alcard-battery-inhibit-v1:resume\n";

pub struct Journal {
    path: PathBuf,
    directory: File,
    device: u64,
    inode: u64,
    uid: u32,
}

impl Journal {
    pub fn from_environment() -> io::Result<Self> {
        let path = env::var_os("XDG_RUNTIME_DIR")
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "XDG_RUNTIME_DIR is unset"))?;
        Self::open(Path::new(&path))
    }

    fn open(path: &Path) -> io::Result<Self> {
        if !path.is_absolute() {
            return Err(invalid(
                "battery journal requires an absolute runtime directory",
            ));
        }
        let directory = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(path)?;
        let metadata = directory.metadata()?;
        // SAFETY: geteuid has no arguments, pointers, or side effects.
        let uid = unsafe { libc::geteuid() };
        trusted_directory(&metadata, uid)?;
        let journal = Self {
            path: path.to_owned(),
            directory,
            device: metadata.dev(),
            inode: metadata.ino(),
            uid,
        };
        journal.read_inhibit()?;
        Ok(journal)
    }

    #[cfg(test)]
    fn at(path: &Path) -> io::Result<Self> {
        Self::open(path)
    }

    /// A later replacement or permission failure must retain the latch, not
    /// turn an unreadable journal into permission for another automatic sleep.
    pub fn inhibit(&self) -> Option<Inhibit> {
        self.read_inhibit().unwrap_or(Some(Inhibit::Attempt))
    }

    /// True authorizes exactly one newly recorded attempt. Existing valid
    /// content returns false; invalid content or a concurrent operation errs.
    pub fn latch(&self) -> io::Result<bool> {
        self.record(Inhibit::Attempt)
    }

    /// Waking at critical charge survives a Shell restart too. It must never
    /// weaken an existing automatic-attempt inhibition to a resume inhibition.
    pub fn resume(&self) -> io::Result<()> {
        self.record(Inhibit::Resume).map(|_| ())
    }

    fn record(&self, inhibit: Inhibit) -> io::Result<bool> {
        let _guard = self.guard()?;
        match self.open_marker(true) {
            Ok(mut file) => {
                // Leave a partial marker on an I/O failure: no subsequent
                // restart may interpret an uncertain attempt as unused.
                trusted_marker(&file.metadata()?, self.uid, false)?;
                self.write_inhibit(&mut file, inhibit)?;
                self.match_path(&file)?;
                self.verify_directory()?;
                Ok(true)
            }
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                let mut file = self.open_marker(false)?;
                let existing = self.validate_marker(&file)?;
                self.match_path(&file)?;
                if inhibit == Inhibit::Attempt && existing == Inhibit::Resume {
                    // Do not authorize a sleep while another inhibition still
                    // exists. Retain the stronger consumed-attempt latch even
                    // if this conservative refusal is followed by a restart.
                    self.write_inhibit(&mut file, Inhibit::Attempt)?;
                    self.match_path(&file)?;
                }
                Ok(false)
            }
            Err(error) => Err(error),
        }
    }

    /// Compare and clear under one directory lock. A caller which observed a
    /// Resume marker cannot erase an Attempt recorded by another instance.
    /// None only matches an already absent marker; it never removes a file.
    pub fn clear_if(&self, expected: Option<Inhibit>) -> io::Result<bool> {
        let _guard = self.guard()?;
        let file = match self.open_marker(false) {
            Ok(file) => file,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                return Ok(expected.is_none());
            }
            Err(error) => return Err(error),
        };
        let actual = self.validate_marker(&file)?;
        if Some(actual) != expected {
            return Ok(false);
        }
        self.remove_matching(&file)?;
        Ok(true)
    }

    #[cfg(test)]
    fn clear(&self) -> io::Result<()> {
        if self.clear_if(self.read_inhibit()?)? {
            Ok(())
        } else {
            Err(invalid("battery journal inhibition changed before clear"))
        }
    }

    fn read_inhibit(&self) -> io::Result<Option<Inhibit>> {
        let _guard = self.guard()?;
        let file = match self.open_marker(false) {
            Ok(file) => file,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error),
        };
        let inhibit = self.validate_marker(&file)?;
        self.match_path(&file)?;
        Ok(Some(inhibit))
    }

    fn guard(&self) -> io::Result<Guard<'_>> {
        self.verify_directory()?;
        // Nonblocking directory locking serializes publication and removal by
        // our instances, including the interval before a new marker is fully
        // written. There is no second lock file or UI-thread blocking wait.
        // SAFETY: directory is a live owned descriptor; flock uses no pointers.
        if unsafe { libc::flock(self.directory.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
            return Err(io::Error::last_os_error());
        }
        let guard = Guard(&self.directory);
        self.verify_directory()?;
        Ok(guard)
    }

    fn verify_directory(&self) -> io::Result<()> {
        let opened = self.directory.metadata()?;
        let named = fs::symlink_metadata(&self.path)?;
        trusted_directory(&opened, self.uid)?;
        trusted_directory(&named, self.uid)?;
        if opened.dev() != self.device
            || opened.ino() != self.inode
            || named.dev() != self.device
            || named.ino() != self.inode
        {
            return Err(invalid("battery runtime directory identity changed"));
        }
        Ok(())
    }

    fn open_marker(&self, create: bool) -> io::Result<File> {
        let mut flags = libc::O_RDWR | libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC;
        if create {
            flags |= libc::O_CREAT | libc::O_EXCL;
        }
        // SAFETY: the directory descriptor remains live, NAME is a fixed
        // NUL-terminated filename, and the mode argument matches O_CREAT.
        let descriptor = unsafe {
            libc::openat(
                self.directory.as_raw_fd(),
                NAME.as_ptr(),
                flags,
                0o600 as libc::mode_t,
            )
        };
        if descriptor < 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: openat returned a new descriptor, whose ownership moves to File.
        Ok(unsafe { File::from_raw_fd(descriptor) })
    }

    fn write_inhibit(&self, file: &mut File, inhibit: Inhibit) -> io::Result<()> {
        let content = match inhibit {
            Inhibit::Attempt => CONTENT,
            Inhibit::Resume => RESUME_CONTENT,
        };
        file.set_len(0)?;
        file.write_all(content)?;
        file.sync_data()?;
        self.validate_marker(file)?;
        Ok(())
    }

    fn validate_marker(&self, file: &File) -> io::Result<Inhibit> {
        let metadata = file.metadata()?;
        trusted_marker(&metadata, self.uid, true)?;
        let mut content = vec![0; metadata.len() as usize];
        file.read_exact_at(&mut content, 0)?;
        match content.as_slice() {
            CONTENT => Ok(Inhibit::Attempt),
            RESUME_CONTENT => Ok(Inhibit::Resume),
            _ => Err(invalid("battery journal has invalid version or content")),
        }
    }

    fn match_path(&self, file: &File) -> io::Result<()> {
        let named = self.open_marker(false)?;
        let named = named.metadata()?;
        let opened = file.metadata()?;
        trusted_marker(&named, self.uid, true)?;
        trusted_marker(&opened, self.uid, true)?;
        if named.dev() != opened.dev() || named.ino() != opened.ino() {
            return Err(invalid("battery journal identity changed"));
        }
        Ok(())
    }

    fn remove_matching(&self, file: &File) -> io::Result<()> {
        self.validate_marker(file)?;
        self.match_path(file)?;
        self.verify_directory()?;
        // SAFETY: the pinned directory descriptor is live, NAME is a fixed
        // NUL-terminated filename, and flags=0 only unlinks a file.
        if unsafe { libc::unlinkat(self.directory.as_raw_fd(), NAME.as_ptr(), 0) } != 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }
}

struct Guard<'a>(&'a File);
impl Drop for Guard<'_> {
    fn drop(&mut self) {
        // SAFETY: the borrowed directory outlives the guard. Dropping a guard
        // releases only the advisory lock held by its own open descriptor.
        unsafe { libc::flock(self.0.as_raw_fd(), libc::LOCK_UN) };
    }
}

fn invalid(message: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}

fn trusted_directory(metadata: &Metadata, uid: u32) -> io::Result<()> {
    if !metadata.is_dir() || metadata.uid() != uid || metadata.mode() & 0o7777 != 0o700 {
        return Err(invalid(
            "battery runtime directory must be owned by this user with mode 0700",
        ));
    }
    Ok(())
}

fn trusted_marker(metadata: &Metadata, uid: u32, complete: bool) -> io::Result<()> {
    if !metadata.is_file()
        || metadata.uid() != uid
        || metadata.mode() & 0o7777 != 0o600
        || metadata.nlink() != 1
        || (complete
            && metadata.len() != CONTENT.len() as u64
            && metadata.len() != RESUME_CONTENT.len() as u64)
    {
        return Err(invalid(
            "battery journal must be an owned private regular file with exact content size",
        ));
    }
    Ok(())
}

#[cfg(test)]
#[path = "unit_tests/battery_journal.rs"]
mod tests;
