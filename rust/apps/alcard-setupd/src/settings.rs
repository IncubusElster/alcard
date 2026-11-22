//! Root-only allowlist for post-install Settings operations.
//! This entry point cannot create accounts or consume firstboot credentials.
use crate::failure::invalid;
use crate::system_config::{
    atomic_write, check_alpine, command, ntp_service, set_language, set_layouts, set_timezone,
};
use std::{
    fs::{self, OpenOptions},
    io::{self, Write},
    os::unix::fs::OpenOptionsExt,
    path::Path,
    process::{Command, Stdio},
};

/// Authenticated post-install actions have no access to account provisioning.
pub(crate) fn apply(key: &str, value: &str) -> io::Result<()> {
    if unsafe { libc::geteuid() } != 0 {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "root required",
        ));
    }
    check_alpine()?;
    let lock = OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open("/run/alcard-settings.lock")?;
    lock.lock()?;
    match key {
        "hostname"
            if !value.is_empty()
                && value.len() <= 63
                && !value.starts_with('-')
                && !value.ends_with('-')
                && value
                    .bytes()
                    .all(|c| c.is_ascii_alphanumeric() || c == b'-') =>
        {
            command("/bin/hostname", &[value])?;
            atomic_write(
                Path::new("/etc/hostname"),
                format!("{value}\n").as_bytes(),
                0o644,
            )
        }
        "language" => set_language(match value {
            "en" => "en_US.UTF-8",
            "ru" => "ru_RU.UTF-8",
            "es" => "es_ES.UTF-8",
            _ => return Err(invalid("invalid_language")),
        }),
        "layouts" => {
            let layouts = value.split(',').collect::<Vec<_>>();
            if layouts.len() > 4
                || layouts.iter().enumerate().any(|(i, x)| {
                    !matches!(*x, "us" | "ru" | "es" | "de") || layouts[..i].contains(x)
                })
            {
                return Err(invalid("invalid_layouts"));
            }
            set_layouts(&layouts)
        }
        "active-layout" if matches!(value, "us" | "ru" | "de" | "es") => {
            if !fs::read_to_string("/etc/alcard-keyboards")?
                .trim()
                .split(',')
                .any(|l| l == value)
            {
                return Err(invalid("layout_not_selected"));
            }
            let out = Command::new("/bin/gzip")
                .args([
                    "-dc",
                    &format!("/usr/share/alcard/keyboard/console/{value}.bmap.gz"),
                ])
                .output()?;
            if !out.status.success() {
                return Err(invalid("keymap_unavailable"));
            }
            let mut child = Command::new("/sbin/loadkmap")
                .stdin(Stdio::piped())
                .spawn()?;
            let wrote = child.stdin.take().unwrap().write_all(&out.stdout);
            let status = child.wait()?;
            wrote?;
            if !status.success() {
                return Err(invalid("keymap_failed"));
            }
            atomic_write(
                Path::new("/run/alcard-active-layout"),
                value.as_bytes(),
                0o644,
            )
        }
        "timezone" if alcard_setup_protocol::is_valid_timezone(value) => {
            let data = fs::read(Path::new("/usr/share/zoneinfo").join(value))?;
            if !data.starts_with(b"TZif") {
                return Err(invalid("invalid_timezone"));
            }
            set_timezone(value)
        }
        "ntp" if matches!(value, "on" | "off") => {
            let service = ntp_service()?;
            let enabled = value == "on";
            let started = Path::new("/run/openrc/started").join(service).exists();
            if enabled != started {
                command(
                    "/sbin/rc-service",
                    &[service, if enabled { "start" } else { "stop" }],
                )?;
            }
            command(
                "/sbin/rc-update",
                &[if enabled { "add" } else { "del" }, service, "default"],
            )
        }
        "datetime" => {
            if ["chronyd", "ntpd"].iter().any(|s| {
                Path::new("/etc/runlevels/default").join(s).exists()
                    || Path::new("/run/openrc/started").join(s).exists()
            }) {
                return Err(invalid("disable_ntp_first"));
            }
            if value.len() != 19
                || !value.bytes().enumerate().all(|(i, b)| match i {
                    4 | 7 => b == b'-',
                    10 => b == b' ',
                    13 | 16 => b == b':',
                    _ => b.is_ascii_digit(),
                })
            {
                return Err(invalid("invalid_local_datetime"));
            }
            let parsed = Command::new("/bin/date")
                .args(["-d", value, "+%Y-%m-%d %H:%M:%S"])
                .output()?;
            if !parsed.status.success() || parsed.stdout != format!("{value}\n").as_bytes() {
                return Err(invalid("invalid_local_datetime"));
            }
            command("/bin/date", &["-s", value])?;
            if Path::new("/dev/rtc0").exists() {
                command("/sbin/hwclock", &["--systohc", "--utc"])?;
            }
            Ok(())
        }
        "brightness" if value.parse::<u8>().is_ok_and(|n| n <= 100) => command(
            "/usr/bin/brightnessctl",
            &["-c", "backlight", "set", &format!("{value}%")],
        ),
        _ => Err(invalid("invalid_setting")),
    }
}
