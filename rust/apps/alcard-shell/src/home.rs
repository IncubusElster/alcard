//! Per-session S2.4 Home command channel. Linux abstract AF_UNIX sockets do not
//! leave stale filesystem nodes after a crash or require a separate daemon.
//! The compositor runs `alcard-shell --home`; only the already-running Shell
//! owns this socket and can request compositor focus.
use std::{
    env, fs,
    io::{self, Read, Write},
    os::{
        linux::net::SocketAddrExt,
        unix::{
            fs::MetadataExt,
            net::{SocketAddr, UnixListener, UnixStream},
        },
    },
    path::{Path, PathBuf},
    thread,
    time::Duration,
};

use nix::{
    sys::socket::{getsockopt, sockopt::PeerCredentials},
    unistd::Uid,
};
use smithay_client_toolkit::reexports::calloop::channel::Sender;

const REQUEST: &[u8; 5] = b"HOME\n";
const ACK: &[u8; 3] = b"OK\n";
const IO_TIMEOUT: Duration = Duration::from_secs(1);

fn session_dir() -> io::Result<PathBuf> {
    let path = PathBuf::from(
        env::var_os("XDG_RUNTIME_DIR")
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "XDG_RUNTIME_DIR is unset"))?,
    );
    validate_runtime_dir(&path, Uid::current().as_raw())?;
    Ok(path)
}

fn validate_runtime_dir(path: &Path, uid: u32) -> io::Result<()> {
    // Never use arbitrary /tmp paths or follow symlinks supplied as the
    // supposed user session root.
    if !path.is_absolute() || fs::symlink_metadata(path)?.file_type().is_symlink() {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "XDG_RUNTIME_DIR must be an absolute, non-symlink directory",
        ));
    }
    let metadata = fs::metadata(path)?;
    if !metadata.is_dir() || metadata.uid() != uid || metadata.mode() & 0o777 != 0o700 {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "XDG_RUNTIME_DIR must be owned by this user and have mode 0700",
        ));
    }
    Ok(())
}

fn name(uid: u32, runtime: &Path, wayland_display: &str) -> Vec<u8> {
    // Stable, short and session-scoped. The hash is an identifier, not a
    // security credential: SO_PEERCRED checks actual uid on every request.
    use std::os::unix::ffi::OsStrExt;
    let mut h: u64 = 0xcbf29ce484222325;
    for byte in runtime
        .as_os_str()
        .as_bytes()
        .iter()
        .copied()
        .chain([0xff])
        .chain(wayland_display.as_bytes().iter().copied())
    {
        h ^= u64::from(byte);
        h = h.wrapping_mul(0x100000001b3);
    }
    format!("alcard-shell-home-v1-{uid}-{h:016x}").into_bytes()
}

fn address() -> io::Result<SocketAddr> {
    let runtime = session_dir()?;
    let display = env::var("WAYLAND_DISPLAY")
        .map_err(|_| io::Error::new(io::ErrorKind::NotFound, "WAYLAND_DISPLAY is unset"))?;
    if display.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "Empty WAYLAND_DISPLAY",
        ));
    }
    SocketAddr::from_abstract_name(name(Uid::current().as_raw(), &runtime, &display))
}

fn exchange(mut stream: UnixStream) -> io::Result<()> {
    stream.set_read_timeout(Some(IO_TIMEOUT))?;
    stream.set_write_timeout(Some(IO_TIMEOUT))?;
    stream.write_all(REQUEST)?;
    let mut reply = [0_u8; 3];
    stream.read_exact(&mut reply)?;
    if &reply != ACK {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "Alcard Shell refused the Home command",
        ));
    }
    Ok(())
}

/// Submit to the *existing* Shell. Never starts a second GUI instance.
pub fn request_home() -> io::Result<()> {
    exchange(UnixStream::connect_addr(&address()?)?)
}

fn handle_request(
    mut stream: UnixStream,
    expected_uid: u32,
    mut dispatch: impl FnMut() -> io::Result<()>,
) -> io::Result<()> {
    stream.set_read_timeout(Some(IO_TIMEOUT))?;
    stream.set_write_timeout(Some(IO_TIMEOUT))?;
    let peer = getsockopt(&stream, PeerCredentials).map_err(io::Error::other)?;
    if peer.uid() != expected_uid {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "Home request came from a different user",
        ));
    }
    let mut command = [0_u8; REQUEST.len()];
    stream.read_exact(&mut command)?;
    if &command != REQUEST {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "Unknown Home command",
        ));
    }
    // ACK means "queued in the running Shell", not that the compositor
    // has already granted focus (that must be checked on real hardware).
    dispatch()?;
    stream.write_all(ACK)
}

/// Starts only once per user Wayland session: a second Shell cannot bind the
/// same abstract socket. No stale socket cleanup, lockfiles or user daemon.
pub fn start(proxy: Sender<()>) -> io::Result<()> {
    let listener = UnixListener::bind_addr(&address()?)?;
    let uid = Uid::current().as_raw();
    thread::Builder::new()
        .name("alcard-home-ipc".into())
        .spawn(move || {
            for incoming in listener.incoming() {
                match incoming {
                    Ok(stream) => {
                        if let Err(error) = handle_request(stream, uid, || {
                            proxy.send(()).map_err(|_| {
                                io::Error::new(
                                    io::ErrorKind::BrokenPipe,
                                    "Alcard Shell event loop is closed",
                                )
                            })
                        }) {
                            eprintln!("Rejected Home request: {error}");
                        }
                    }
                    Err(error) => {
                        eprintln!("Home command listener failed: {error}");
                        break;
                    }
                }
            }
        })?;
    Ok(())
}

#[cfg(test)]
#[path = "unit_tests/home.rs"]
mod tests;
