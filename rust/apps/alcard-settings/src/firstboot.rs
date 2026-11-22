//! Explicit firstboot apply path for a dedicated Alpine/QEMU guest.
//! No privilege escalation: must already be root, and backend must verify
//! Alpine + root-owned firstboot marker before consuming the secret frame.
use alcard_setup_core::Wizard;
use std::{
    io::{self, Write},
    path::Path,
    process::{Command, Stdio},
};

pub(crate) fn apply(wizard: &Wizard) -> io::Result<()> {
    if unsafe { libc::geteuid() } != 0 {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "firstboot_requires_root",
        ));
    }
    if !Path::new("/var/lib/alcard/hello.pending").is_file() {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "firstboot_not_enabled",
        ));
    }
    let request = wizard
        .firstboot_request()
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "firstboot_not_ready"))?;
    let mut child = Command::new("/usr/sbin/alcard-setupd")
        .arg("--apply")
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;
    let write = child
        .stdin
        .take()
        .ok_or_else(|| io::Error::other("missing_apply_pipe"))?
        .write_all(&request);
    if write.is_err() {
        let _ = child.kill();
        let _ = child.wait();
        return Err(io::Error::other("apply_pipe_failed"));
    }
    let result = child.wait()?;
    if !result.success() {
        return Err(io::Error::other("apply_failed"));
    }
    Ok(())
}
