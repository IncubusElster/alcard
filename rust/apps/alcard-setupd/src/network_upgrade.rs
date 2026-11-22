//! Import the single profile produced by previous Alcard Hello releases.
//! Keep the original files. OpenRC switches owners at the next boot.
use crate::apply::atomic_write;
use std::{
    fs, io,
    os::unix::fs::{MetadataExt, PermissionsExt},
    path::Path,
    process::Command,
};
use zeroize::Zeroizing;
fn bad() -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidData,
        "Unrecognized legacy Alcard Wi-Fi profile",
    )
}
fn profile(iface: &str, wpa: &str) -> io::Result<Zeroizing<String>> {
    let lines: Vec<_> = wpa.lines().map(str::trim).collect();
    let ["ctrl_interface=/run/wpa_supplicant", "update_config=0", "network={", name, auth, "}"] =
        lines.as_slice()
    else {
        return Err(bad());
    };
    let quoted = name
        .strip_prefix("ssid=\"")
        .and_then(|s| s.strip_suffix('"'))
        .ok_or_else(bad)?;
    let mut chars = quoted.chars();
    let mut ssid = String::new();
    while let Some(c) = chars.next() {
        ssid.push(match c {
            '\\' => match chars.next() {
                Some(c @ ('\\' | '"')) => c,
                _ => return Err(bad()),
            },
            '"' => return Err(bad()),
            c => c,
        });
    }
    if ssid.is_empty() || ssid.len() > 32 || ssid.chars().any(char::is_control) {
        return Err(bad());
    }
    // Byte lists preserve leading spaces, backslashes and non-ASCII SSIDs.
    let bytes = ssid.bytes().map(|b| format!("{b};")).collect::<String>();
    let mut out=Zeroizing::new(format!("[connection]\nid=Alcard {iface}\ntype=wifi\ninterface-name={iface}\nautoconnect=true\n\n[wifi]\nmode=infrastructure\nssid={bytes}\n"));
    if *auth != "key_mgmt=NONE" {
        let psk = auth
            .strip_prefix("psk=")
            .filter(|v| v.len() == 64 && v.bytes().all(|b| b.is_ascii_hexdigit()))
            .ok_or_else(bad)?;
        out.push_str("\n[wifi-security]\nkey-mgmt=wpa-psk\npsk=");
        out.push_str(psk);
        out.push('\n');
    }
    out.push_str("\n[ipv4]\nmethod=auto\n\n[ipv6]\nmethod=auto\n");
    Ok(out)
}
fn import() -> io::Result<()> {
    let legacy = Path::new("/etc/network/interfaces.d/alcard-wifi");
    let stanza = match fs::read_to_string(legacy) {
        Ok(s) => s,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(e),
    };
    let iface = stanza
        .lines()
        .next()
        .and_then(|l| l.strip_prefix("auto "))
        .filter(|s| {
            !s.is_empty()
                && s.len() <= 15
                && s.bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"_.-".contains(&b))
        })
        .ok_or_else(bad)?;
    let source = format!("/etc/wpa_supplicant/wpa_supplicant-{iface}.conf");
    if stanza != format!("auto {iface}\niface {iface} inet dhcp\n    wpa-conf {source}\n") {
        return Err(bad());
    }
    let meta = fs::symlink_metadata(&source)?;
    if !meta.is_file() || meta.uid() != 0 || meta.mode() & 0o077 != 0 || meta.len() > 4096 {
        return Err(bad());
    }
    let out = profile(iface, &Zeroizing::new(fs::read_to_string(source)?))?;
    let dir = Path::new("/etc/NetworkManager/system-connections");
    fs::create_dir_all(dir)?;
    let p = dir.join(format!("alcard-{iface}.nmconnection"));
    match fs::symlink_metadata(&p) {
        Ok(meta) if meta.is_file() && meta.uid() == 0 && meta.permissions().mode() & 0o077 == 0 => {
            Ok(())
        }
        Ok(_) => Err(bad()),
        Err(e) if e.kind() == io::ErrorKind::NotFound => atomic_write(&p, out.as_bytes(), 0o600),
        Err(e) => Err(e),
    }
}
pub fn run() -> io::Result<()> {
    if unsafe { libc::geteuid() } != 0 {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "root required",
        ));
    }
    import()?;
    let update = |op: &str, service: &str, level: &str| -> io::Result<()> {
        if !Command::new("/sbin/rc-update")
            .args([op, service, level])
            .status()?
            .success()
        {
            return Err(io::Error::other("OpenRC update failed"));
        }
        Ok(())
    };
    for service in ["dbus", "networkmanager"] {
        update("add", service, "default")?;
    }
    for level in ["sysinit", "boot", "default"] {
        for service in ["networking", "wpa_supplicant"] {
            if Path::new("/etc/runlevels")
                .join(level)
                .join(service)
                .is_symlink()
            {
                update("del", service, level)?;
            }
        }
    }
    Ok(())
}

#[cfg(test)]
#[path = "unit_tests/network_upgrade.rs"]
mod tests;
