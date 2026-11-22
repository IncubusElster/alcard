//! Shared Alpine configuration writes for validated firstboot and Settings requests.
//! Callers own authorization and validation; no account provisioning belongs here.
use crate::failure::invalid;
use std::{
    fs::{self, File, OpenOptions},
    io::{self, Write},
    os::unix::fs::{OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

pub(crate) fn check_alpine() -> io::Result<()> {
    let os_release = fs::read_to_string("/etc/os-release")?;
    if !os_release.lines().any(|line| line.trim() == "ID=alpine") {
        return Err(invalid("alpine_guest_required"));
    }
    Ok(())
}

pub(crate) fn command(binary: &str, arguments: &[&str]) -> io::Result<()> {
    let status = Command::new(binary)
        .args(arguments)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()?;
    if !status.success() {
        return Err(invalid("system_command_failed"));
    }
    Ok(())
}
pub(crate) fn atomic_write(path: &Path, value: &[u8], mode: u32) -> io::Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| invalid("invalid_output_path"))?;
    // Firstboot and separate Settings frontends can write concurrently.
    let temp = path.with_extension(format!("alcard-new-{}", std::process::id()));
    if temp.exists() {
        fs::remove_file(&temp)?;
    }
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(mode)
        .open(&temp)?;
    // Preserve the requested policy/config mode even under a root umask of 077.
    // The descriptor is a newly created file inside a trusted parent directory.
    file.set_permissions(fs::Permissions::from_mode(mode))?;
    file.write_all(value)?;
    file.sync_all()?;
    drop(file);
    fs::rename(temp, path)?;
    File::open(parent)?.sync_all()
}

pub(crate) fn set_language(lang: &str) -> io::Result<()> {
    fs::create_dir_all("/etc/profile.d")?;
    atomic_write(
        Path::new("/etc/profile.d/alcard-locale.sh"),
        format!("export LANG='{lang}'\n").as_bytes(),
        0o644,
    )?;
    Ok(())
}

pub(crate) fn set_layouts(layouts: &[&str]) -> io::Result<()> {
    // US is mandatory, first, and remains the console's boot layout.
    if layouts.first() != Some(&"us") {
        return Err(invalid("original_us_required"));
    }
    let installed = Path::new("/usr/share/alcard/keyboard/console");
    for layout in layouts {
        if !installed.join(format!("{layout}.bmap.gz")).is_file() {
            return Err(invalid("selected_alcard_keymap_unavailable"));
        }
    }
    let keymap = fs::read(installed.join("us.bmap.gz"))?;
    fs::create_dir_all("/etc/keymap")?;
    atomic_write(Path::new("/etc/keymap/alcard.bmap.gz"), &keymap, 0o644)?;
    if !Path::new("/etc/init.d/loadkmap").is_file() {
        return Err(invalid("loadkmap_service_unavailable"));
    }
    atomic_write(
        Path::new("/etc/conf.d/loadkmap"),
        b"KEYMAP=/etc/keymap/alcard.bmap.gz\n",
        0o644,
    )?;
    command("/sbin/rc-update", &["add", "loadkmap", "boot"])?;
    // Keep US first for all sessions; additional layouts can be selected
    // later by the runtime keyboard switcher.
    atomic_write(
        Path::new("/etc/alcard-keyboards"),
        format!("{}\n", layouts.join(",")).as_bytes(),
        0o644,
    )?;
    // Generated env contains only strictly validated enum-derived identifiers.
    // The session script must opt in to reading this after graphical packaging.
    fs::create_dir_all("/etc/alcard")?;
    let groups = std::iter::repeat_n("alcard", layouts.len())
        .collect::<Vec<_>>()
        .join(",");
    let variants = layouts.join(",");
    let options = if layouts.len() > 1 {
        "grp:alt_shift_toggle"
    } else {
        ""
    };
    atomic_write(Path::new("/etc/alcard/keyboard-xkb.env"),
        format!("export XKB_DEFAULT_LAYOUT='{groups}'\nexport XKB_DEFAULT_VARIANT='{variants}'\nexport XKB_DEFAULT_OPTIONS='{options}'\n").as_bytes(), 0o644)?;

    Ok(())
}

pub(crate) fn set_timezone(tz: &str) -> io::Result<()> {
    let zone_path = PathBuf::from("/usr/share/zoneinfo").join(tz);
    if !zone_path.is_file() {
        return Err(invalid("timezone_not_installed"));
    }
    // Callers validate the zone identifier before requesting this write.
    let data = fs::read(zone_path)?;
    atomic_write(Path::new("/etc/localtime"), &data, 0o644)?;
    atomic_write(
        Path::new("/etc/timezone"),
        format!("{tz}\n").as_bytes(),
        0o644,
    )?;
    Ok(())
}

pub(crate) fn ntp_service() -> io::Result<&'static str> {
    ["chronyd", "ntpd"]
        .into_iter()
        .find(|s| Path::new("/etc/init.d").join(s).is_file())
        .ok_or_else(|| invalid("ntp_service_required"))
}
