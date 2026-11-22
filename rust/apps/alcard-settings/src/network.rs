//! Only an armed local firstboot may activate a connection from Hello.
use alcard_settings_core::network;
pub(crate) use network::scan;
use std::{
    fs, io,
    os::unix::fs::{MetadataExt, PermissionsExt},
    path::Path,
};

pub(crate) fn detect() -> (Option<(String, bool)>, Option<String>) {
    let mut wired = None;
    let mut wifi = None;
    for row in network::devices().unwrap_or_default() {
        if row[2] == "unmanaged" {
            continue;
        }
        match row[1].as_str() {
            "ethernet" if wired.is_none() => wired = Some((row[0].clone(), row[2] == "connected")),
            "wifi" if wifi.is_none() => wifi = Some(row[0].clone()),
            _ => {}
        }
    }
    (wired, wifi)
}

pub(crate) fn connect(
    iface: &str,
    ssid: &str,
    password: &str,
    cancel: &std::sync::atomic::AtomicBool,
) -> io::Result<()> {
    let path = Path::new("/var/lib/alcard/hello.pending");
    let meta = path.symlink_metadata()?;
    if unsafe { libc::geteuid() } != 0
        || !meta.is_file()
        || meta.uid() != 0
        || meta.permissions().mode() & 0o777 != 0o600
        || fs::read_to_string(path)? != "ALCARD_FIRSTBOOT_V1\n"
        || Path::new("/var/lib/alcard/hello.complete").exists()
    {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "firstboot_not_armed",
        ));
    }
    network::connect_until(iface, ssid, password, cancel)
}
