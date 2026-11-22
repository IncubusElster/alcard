//! Validated operations and the terminal authorization adapter.
use super::{
    services::{bt, network_admin, nm, output, sinks},
    Operation, SINK, WP,
};
use crate::{
    network,
    preferences::{ConfigSource, Overrides},
};
use std::{
    fs, io,
    net::{IpAddr, Ipv4Addr},
    path::Path,
    process::{Command, Stdio},
};

fn admin_command(program: &str) -> Command {
    if unsafe { libc::geteuid() } == 0 {
        return Command::new(program);
    }
    // Alpine restricts traversal of /etc/doas.d. Probe the effective account
    // policy with a harmless command, as Hello does when provisioning it.
    let has_doas = Command::new("/usr/bin/doas")
        .args(["-n", "-u", "root", "/bin/ash", "-c", "exit 0"])
        .env("LC_ALL", "C")
        .stdin(Stdio::null())
        .output()
        .is_ok_and(|r| r.status.success() || r.stderr == b"doas: Authentication required\n");
    if has_doas {
        let mut cmd = Command::new("/usr/bin/doas");
        cmd.arg(program);
        cmd
    } else {
        // User values are passed as separate positional parameters to the fixed command.
        let mut cmd = Command::new("/bin/su");
        cmd.args(["-c", r#"exec "$@""#, "--", "root", "alcard", program]);
        cmd
    }
}

impl Operation {
    fn network_args<'a>(&'a self, value: &'a str) -> io::Result<Vec<&'a str>> {
        let invalid = || io::Error::new(io::ErrorKind::InvalidInput, "Invalid network setting");
        Ok(match self {
            Self::Radio if matches!(value, "on" | "off") => vec!["radio", "wifi", value],
            Self::ActivateDevice(iface) => vec!["device", "connect", iface],
            Self::Disconnect(iface) => vec!["device", "disconnect", iface],
            Self::Wifi { iface, ssid } => vec!["device", "wifi", "connect", ssid, "ifname", iface],
            Self::Profile { uuid, field } => {
                let modify = |props: &[&'a str]| {
                    let mut args = vec!["connection", "modify", "uuid", uuid.as_str()];
                    args.extend_from_slice(props);
                    args
                };
                match *field {
                    "auto" => modify(&[
                        "connection.autoconnect",
                        if value == "on" { "yes" } else { "no" },
                    ]),
                    "dhcp" => modify(&[
                        "ipv4.method",
                        "auto",
                        "ipv4.addresses",
                        "",
                        "ipv4.gateway",
                        "",
                        "ipv4.dns",
                        "",
                        "ipv4.ignore-auto-dns",
                        "no",
                    ]),
                    "address" => {
                        if !value.split_once('/').is_some_and(|(ip, prefix)| {
                            ip.parse::<Ipv4Addr>().is_ok()
                                && prefix.parse::<u8>().is_ok_and(|n| n <= 32)
                        }) {
                            return Err(invalid());
                        }
                        modify(&["ipv4.addresses", value, "ipv4.method", "manual"])
                    }
                    "gateway" => {
                        if !value.is_empty() && value.parse::<Ipv4Addr>().is_err() {
                            return Err(invalid());
                        }
                        modify(&["ipv4.gateway", value])
                    }
                    "dns" => {
                        if value
                            .split([',', ' '])
                            .filter(|s| !s.is_empty())
                            .any(|s| !matches!(s.parse::<IpAddr>(), Ok(IpAddr::V4(_))))
                        {
                            return Err(invalid());
                        }
                        modify(&[
                            "ipv4.dns",
                            value,
                            "ipv4.ignore-auto-dns",
                            if value.trim().is_empty() { "no" } else { "yes" },
                        ])
                    }
                    "up" => vec!["connection", "up", "uuid", uuid],
                    "forget" => vec!["connection", "delete", "uuid", uuid],
                    _ => return Err(invalid()),
                }
            }
            _ => return Err(invalid()),
        })
    }
    pub fn terminal_command(&self, value: &str) -> io::Result<Option<Command>> {
        if let Self::System(key) = self {
            let mut command = admin_command("/usr/sbin/alcard-setupd");
            command.args(["--setting", key, value]);
            return Ok(Some(command));
        }
        let permissions: &[&str] = match self {
            Self::Radio => &["enable-disable-wifi"],
            Self::Wifi { .. } => &["network-control", "settings.modify.system"],
            Self::Disconnect(_) | Self::ActivateDevice(_) | Self::Profile { field: "up", .. } => {
                &["network-control"]
            }
            Self::Profile { .. } => &["settings.modify.system"],
            _ => return Ok(None),
        };
        if network_admin(permissions) {
            let mut command = admin_command(network::NMCLI);
            command
                .args(["--wait", "30", "--ask"])
                .args(self.network_args(value)?);
            return Ok(Some(command));
        }
        Ok(None)
    }
    pub fn apply(&self, value: &str) -> io::Result<()> {
        let invalid = || io::Error::new(io::ErrorKind::InvalidInput, "Invalid setting value");
        match self {
            Self::Preference(group, key) => {
                let v = match *key {
                    "theme" | "icon_style" | "accent" => toml_edit::Value::from(value),
                    "suspend_on_critical" => toml_edit::Value::from(value == "on"),
                    _ => toml_edit::Value::from(value.parse::<i64>().map_err(|_| invalid())?),
                };
                ConfigSource::from_environment(Overrides::default()).save(*group, key, v)?;
                reload_shell();
            }
            Self::Wifi { iface, ssid } => network::connect(iface, ssid, value)?,
            Self::Radio | Self::Disconnect(_) | Self::ActivateDevice(_) | Self::Profile { .. } => {
                nm(&self.network_args(value)?)?;
            }
            Self::Bluetooth(cmd, addr) => {
                match *cmd {
                    "power" => bt(&["power", value])?,
                    "scan" => bt(&["--timeout", "8", "scan", "on"])?,
                    "connect" | "disconnect" | "remove" => bt(&[cmd, addr])?,
                    _ => return Err(invalid()),
                };
            }
            Self::Volume => {
                if !value.parse::<u8>().is_ok_and(|n| n <= 100) {
                    return Err(invalid());
                }
                output(WP, &["set-volume", SINK, &format!("{value}%")])?;
            }
            Self::Mute => {
                output(
                    WP,
                    &["set-mute", SINK, if value == "on" { "1" } else { "0" }],
                )?;
            }
            Self::Output => {
                if !sinks()?.iter().any(|(id, _)| id == value) {
                    return Err(invalid());
                }
                output(WP, &["set-default", value])?;
            }
            Self::System(_) => return Err(invalid()),
        }
        Ok(())
    }
}
pub fn reload_shell() {
    // Limit reload to real Shell executables belonging to the invoking user.
    use std::os::unix::fs::MetadataExt;
    let Ok(entries) = fs::read_dir("/proc") else {
        return;
    };
    for entry in entries.flatten() {
        let Ok(pid) = entry.file_name().to_string_lossy().parse::<i32>() else {
            continue;
        };
        use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
        let fd = unsafe { libc::syscall(libc::SYS_pidfd_open, pid, 0) };
        if fd < 0 {
            continue;
        }
        // Pin this process across the checks and signal; its numeric PID can be reused.
        let handle = unsafe { OwnedFd::from_raw_fd(fd as i32) };
        if entry
            .metadata()
            .is_ok_and(|m| m.uid() == unsafe { libc::getuid() })
            && fs::read_link(entry.path().join("exe"))
                .is_ok_and(|p| p == Path::new("/usr/bin/alcard-shell"))
        {
            unsafe {
                libc::syscall(
                    libc::SYS_pidfd_send_signal,
                    handle.as_raw_fd(),
                    libc::SIGHUP,
                    std::ptr::null::<libc::siginfo_t>(),
                    0,
                );
            }
        }
    }
}

#[cfg(test)]
#[path = "../unit_tests/settings.rs"]
mod tests;
