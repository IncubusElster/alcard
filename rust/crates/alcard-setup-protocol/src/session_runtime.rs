//! Read-only installed-session preflight, shared by Hello and privileged apply.
//! Each caller checks the filesystem/package database independently. A plan or
//! presentation flag never authorizes a session or substitutes for these checks.
use crate::Session;
use std::{
    fs, io,
    os::unix::fs::MetadataExt,
    path::Path,
    process::{Command, Stdio},
};

fn unavailable() -> io::Error {
    io::Error::new(
        io::ErrorKind::NotFound,
        "selected_session_components_unavailable",
    )
}

fn trusted_parents(path: &Path) -> io::Result<()> {
    for parent in path.ancestors().skip(1) {
        // Following a system directory symlink supports usr-merged Alpine;
        // its parent and its resolved destination must both remain trusted.
        let metadata = fs::metadata(parent)?;
        if !metadata.is_dir()
            || metadata.uid() != 0
            || metadata.mode() & 0o022 != 0
            || metadata.mode() & 0o001 == 0
        {
            return Err(unavailable());
        }
    }
    Ok(())
}

fn trusted_file(path: &Path, executable: bool) -> io::Result<()> {
    trusted_file_access(path, executable, if executable { 0o005 } else { 0o004 })
}

fn trusted_file_access(path: &Path, executable: bool, user_access: u32) -> io::Result<()> {
    if !path.is_absolute() {
        return Err(unavailable());
    }
    trusted_parents(path)?;
    let link = fs::symlink_metadata(path)?;
    if link.uid() != 0 || (!executable && link.file_type().is_symlink()) {
        return Err(unavailable());
    }
    // BusyBox getty and distribution binaries may be root-owned symlinks.
    // Inspect the resolved executable and every destination directory too.
    let resolved = fs::canonicalize(path)?;
    trusted_parents(&resolved)?;
    let metadata = fs::symlink_metadata(resolved)?;
    if !metadata.is_file()
        || metadata.uid() != 0
        || metadata.mode() & 0o022 != 0
        || metadata.len() == 0
        // The eventual session is an ordinary user, not this root firstboot
        // process. Owner-only access must not advertise a usable user session.
        || metadata.mode() & user_access != user_access
    {
        return Err(unavailable());
    }
    Ok(())
}

fn executable(path: &str) -> io::Result<()> {
    trusted_file(Path::new(path), true)
}

fn configuration(path: &str) -> io::Result<()> {
    trusted_file(Path::new(path), false)
}

fn helper_privilege(mode: u32, uid: u32, gid: u32, shadow_gid: Option<u32>) -> bool {
    uid == 0
        && mode & 0o022 == 0
        && mode & 0o001 != 0
        && (mode & 0o4000 != 0
            || (mode & 0o2000 != 0 && shadow_gid.is_some_and(|shadow| shadow == gid)))
}

fn privileged_helper(path: &str, shadow_gid: Option<u32>) -> io::Result<()> {
    // Execute-only helpers are legitimate: Alpine unix_chkpwd may be 2751.
    trusted_file_access(Path::new(path), true, 0o001)?;
    let metadata = fs::metadata(path)?;
    if !helper_privilege(metadata.mode(), metadata.uid(), metadata.gid(), shadow_gid) {
        return Err(unavailable());
    }
    Ok(())
}

fn shadow_group(contents: &str) -> Option<u32> {
    let mut entries = contents.lines().filter(|line| line.starts_with("shadow:"));
    let entry = entries.next()?;
    if entries.next().is_some() {
        return None;
    }
    let fields: Vec<_> = entry.split(':').collect();
    if fields.len() != 4 {
        return None;
    }
    fields[2].parse().ok()
}

fn password_helper() -> io::Result<()> {
    configuration("/etc/group")?;
    let shadow_gid = shadow_group(&fs::read_to_string("/etc/group")?);
    for path in ["/usr/sbin/unix_chkpwd", "/sbin/unix_chkpwd"] {
        match fs::symlink_metadata(path) {
            Ok(_) => return privileged_helper(path, shadow_gid),
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
    }
    Err(unavailable())
}

fn first_executable(paths: &[&str]) -> io::Result<()> {
    for path in paths {
        match fs::symlink_metadata(path) {
            Ok(_) => return executable(path),
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
    }
    Err(unavailable())
}

fn pam_policy(name: &str) -> io::Result<()> {
    // Linux-PAM prefers the administrator's /etc policy over vendor defaults.
    // An unsafe /etc entry must fail, never silently fall through to /usr/lib.
    for directory in ["/etc/pam.d", "/usr/lib/pam.d"] {
        let path = Path::new(directory).join(name);
        match fs::symlink_metadata(&path) {
            Ok(_) => return trusted_file(&path, false),
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
    }
    Err(unavailable())
}

fn elogind_pam_module() -> io::Result<()> {
    for path in [
        "/usr/lib/security/pam_elogind.so",
        "/lib/security/pam_elogind.so",
    ] {
        match fs::symlink_metadata(path) {
            // dlopen needs readability, not the executable bit. Distribution
            // module symlinks are allowed after checking their resolved target.
            Ok(_) => return trusted_file_access(Path::new(path), true, 0o004),
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
    }
    Err(unavailable())
}

fn packages(names: &[&str]) -> io::Result<()> {
    executable("/sbin/apk")?;
    for name in names {
        let status = Command::new("/sbin/apk")
            .args(["info", "-e", name])
            .env_clear()
            .env("PATH", "/usr/sbin:/usr/bin:/sbin:/bin")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()?;
        if !status.success() {
            return Err(unavailable());
        }
    }
    Ok(())
}

/// No writes, authentication, service start or privilege escalation. This may
/// be called as an ordinary user; privileged apply must call it again itself.
pub fn check(session: Session) -> io::Result<()> {
    if session == Session::Console {
        first_executable(&["/sbin/getty", "/usr/sbin/getty"])?;
        executable("/usr/libexec/alcard/tty1-dispatch")?;
        return executable("/sbin/apk");
    }

    for path in [
        "/usr/bin/labwc",
        "/usr/bin/foot",
        "/usr/bin/dbus-run-session",
        "/usr/libexec/alcard/start-session",
        "/usr/libexec/alcard/wayland-session",
    ] {
        executable(path)?;
    }
    first_executable(&["/usr/sbin/greetd", "/usr/bin/greetd"])?;
    first_executable(&["/usr/bin/agreety", "/usr/sbin/agreety"])?;
    configuration("/etc/greetd/alcard.toml")?;
    configuration("/etc/alcard/foot.ini")?;
    for name in [
        "alcard-greetd",
        "greetd",
        "base-auth",
        "base-account",
        "base-password",
        "base-session",
        "base-session-noninteractive",
    ] {
        pam_policy(name)?;
    }
    elogind_pam_module()?;
    packages(&[
        "alcard-desktop-minimal",
        "greetd",
        "greetd-agreety",
        "dbus",
        "elogind",
        "eudev",
        "eudev-openrc",
        "udev-init-scripts-openrc",
    ])?;

    if session == Session::Graphics {
        for path in [
            "/bin/setpriv",
            "/usr/bin/alcard-shell",
            "/usr/bin/alcard-polkit-agent",
            "/usr/bin/alcard-lock",
            "/usr/libexec/alcard/shell-session",
            "/usr/libexec/alcard/session-client",
            "/etc/alcard/labwc-shell/autostart",
            "/usr/lib/polkit-1/polkitd",
        ] {
            executable(path)?;
        }
        for path in [
            "/etc/alcard/labwc-shell/rc.xml",
            "/etc/alcard/labwc-shell/environment",
            "/etc/pam.d/alcard-lock",
        ] {
            configuration(path)?;
        }
        pam_policy("polkit-1")?;
        password_helper()?;
        // Unlike unix_chkpwd, the polkit helper must run as root, not shadow.
        privileged_helper("/usr/lib/polkit-1/polkit-agent-helper-1", None)?;
        packages(&[
            "setpriv",
            "alcard-desktop-shell",
            "alcard-shell",
            "alcard-polkit-agent",
            "alcard-lock",
            "polkit-elogind",
            "linux-pam",
        ])?;
    }
    Ok(())
}

#[cfg(test)]
#[path = "unit_tests/session_runtime.rs"]
mod tests;
