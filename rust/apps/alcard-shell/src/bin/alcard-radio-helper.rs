//! Fixed, on-demand privileged boundary for Shell's Quick radio controls.
//! It accepts no device, path, address, credential or arbitrary command.

#[path = "../radio_protocol.rs"]
mod radio_protocol;
#[path = "../radio_system.rs"]
mod radio_system;

use radio_protocol::{Failure, Operation};
use std::{ffi::OsString, io::Write, time::Duration};

fn argument(args: impl IntoIterator<Item = OsString>) -> Result<Operation, Failure> {
    let mut args = args.into_iter();
    let value = args.next().ok_or(Failure::InvalidResponse)?;
    if args.next().is_some() {
        return Err(Failure::InvalidResponse);
    }
    Operation::parse(value.to_str().ok_or(Failure::InvalidResponse)?)
}

fn main() {
    let operation = match argument(std::env::args_os().skip(1)) {
        Ok(operation) => operation,
        Err(_) => std::process::exit(2),
    };
    if operation != Operation::Refresh && !nix::unistd::geteuid().is_root() {
        std::process::exit(126);
    }
    // The UI cannot kill a root process after pkexec. This deadline is in the
    // helper itself, covers syscalls/serialization/stdio, and never waits for
    // an authentication prompt or an external daemon to finish. iw also gets
    // PR_SET_PDEATHSIG, so the watchdog cannot leave a root scan child behind.
    if std::thread::Builder::new()
        .name("radio-deadline".into())
        .spawn(|| {
            std::thread::sleep(Duration::from_secs(10));
            // SAFETY: process-wide emergency exit; no untrusted pointers.
            unsafe { nix::libc::_exit(124) };
        })
        .is_err()
    {
        std::process::exit(1);
    }
    let reply = radio_system::execute(operation).encode();
    if reply.len() > radio_protocol::MAX_REPLY {
        std::process::exit(1);
    }
    if std::io::stdout()
        .lock()
        .write_all(reply.as_bytes())
        .is_err()
    {
        std::process::exit(1);
    }
}

#[cfg(test)]
#[path = "unit_tests/alcard-radio-helper.rs"]
mod tests;
