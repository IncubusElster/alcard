//! Root-only local firstboot apply for Alpine; no sockets or remote API.
//! Never accept credentials via argv, environment, persisted files or logs.
use crate::admin_policy::PreparedAdmin;
use crate::failure::{at, invalid, Stage};
use alcard_setup_protocol::{session_runtime, validate_for_firstboot, SetupPlan};
use serde::Deserialize;
use std::{
    fs::{self, File, OpenOptions},
    io::{self, Read, Write},
    os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
    process::{Command, Stdio},
};
use zeroize::{Zeroize, Zeroizing};

const STATE_DIR: &str = "/var/lib/alcard";
const PENDING: &str = "/var/lib/alcard/hello.pending";
const MARKER: &str = "ALCARD_FIRSTBOOT_V1\n";
const PROGRESS: &str = "/var/lib/alcard/hello.progress";
const COMPLETE: &str = "/var/lib/alcard/hello.complete";
const MAX_REQUEST: u64 = 32768;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Credentials {
    pub user_password: String,
    pub user_confirm: String,
    pub root_password: String,
    pub root_confirm: String,
}
impl Drop for Credentials {
    fn drop(&mut self) {
        self.user_password.zeroize();
        self.user_confirm.zeroize();
        self.root_password.zeroize();
        self.root_confirm.zeroize();
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ApplyRequest {
    pub plan: SetupPlan,
    pub credentials: Credentials,
}

fn validate_request(request: &ApplyRequest) -> io::Result<()> {
    let c = &request.credentials;
    validate_for_firstboot(&request.plan, !c.root_password.is_empty())
        .map_err(|_| invalid("invalid_plan"))?;
    if c.user_password.chars().count() < 8 || c.user_password != c.user_confirm {
        return Err(invalid("invalid_user_password"));
    }
    if !c.root_password.is_empty() {
        if c.root_password.chars().count() < 8 || c.root_password != c.root_confirm {
            return Err(invalid("invalid_root_password"));
        }
    } else if !c.root_confirm.is_empty() {
        return Err(invalid("invalid_root_confirmation"));
    }
    for password in [
        &c.user_password,
        &c.user_confirm,
        &c.root_password,
        &c.root_confirm,
    ] {
        if password.chars().any(|ch| ch.is_control()) {
            return Err(invalid("invalid_password_character"));
        }
    }
    Ok(())
}

fn check_file(path: &Path, required_mode: u32) -> io::Result<()> {
    let meta = fs::symlink_metadata(path)?;
    if !meta.file_type().is_file()
        || meta.uid() != 0
        || meta.permissions().mode() & 0o777 != required_mode
    {
        return Err(invalid("invalid_firstboot_file_permissions"));
    }
    Ok(())
}
fn check_alpine() -> io::Result<()> {
    let os_release = fs::read_to_string("/etc/os-release")?;
    if !os_release.lines().any(|line| line.trim() == "ID=alpine") {
        return Err(invalid("alpine_guest_required"));
    }
    Ok(())
}

fn check_guest() -> io::Result<()> {
    if unsafe { libc::geteuid() } != 0 {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "root_required",
        ));
    }
    check_alpine()?;
    let dir = Path::new(STATE_DIR);
    let meta = fs::symlink_metadata(dir)?;
    if !meta.file_type().is_dir() || meta.uid() != 0 || meta.permissions().mode() & 0o777 != 0o700 {
        return Err(invalid("invalid_firstboot_directory"));
    }
    check_file(Path::new(PENDING), 0o600)?;
    if fs::read_to_string(PENDING)? != MARKER {
        return Err(invalid("firstboot_marker_invalid"));
    }
    if Path::new(COMPLETE).exists() {
        return Err(invalid("already_completed"));
    }
    Ok(())
}

fn human_user_present(passwd: &str) -> bool {
    passwd.lines().any(|line| {
        let fields: Vec<&str> = line.split(':').collect();
        let uid = fields
            .get(2)
            .and_then(|s| s.parse::<u32>().ok())
            .unwrap_or(0);
        (1000..65534).contains(&uid)
    })
}

fn root_password_field(shadow: &str) -> io::Result<&str> {
    let mut roots = shadow.lines().filter_map(|line| {
        let (name, rest) = line.split_once(':')?;
        (name == "root").then(|| rest.split(':').next().unwrap_or(""))
    });
    let hash = roots.next().ok_or_else(|| invalid("root_shadow_missing"))?;
    if roots.next().is_some() {
        return Err(invalid("root_shadow_ambiguous"));
    }
    Ok(hash)
}

fn root_password_locked(shadow: &str) -> io::Result<bool> {
    let hash = root_password_field(shadow)?;
    // An empty password is not a password lock; firstboot must lock it too.
    Ok(hash.starts_with('!') || hash.starts_with('*'))
}

fn ensure_root_password_locked() -> io::Result<()> {
    let already_locked = {
        let shadow = Zeroizing::new(fs::read_to_string("/etc/shadow")?);
        root_password_locked(&shadow)?
    };
    if !already_locked {
        command("/usr/bin/passwd", &["-l", "root"])?;
    }
    // Verify the desired state, including a retry which needed no mutation.
    // Never ignore a failed passwd invocation or unlock an existing account.
    let shadow = Zeroizing::new(fs::read_to_string("/etc/shadow")?);
    if !root_password_locked(&shadow)? {
        return Err(invalid("root_lock_not_applied"));
    }
    Ok(())
}

/// A firstboot run is required only when there is no completed setup and
/// there is no usable administrator or human user. An existing pending marker
/// takes precedence to allow recovery after a partially created account.
pub fn needs_hello() -> io::Result<bool> {
    if Path::new(COMPLETE).exists() {
        return Ok(false);
    }
    if Path::new(PENDING).exists() {
        check_file(Path::new(PENDING), 0o600)?;
        return Ok(fs::read_to_string(PENDING)? == MARKER);
    }
    let passwd = fs::read_to_string("/etc/passwd")?;
    let has_human = human_user_present(&passwd);
    let shadow = Zeroizing::new(fs::read_to_string("/etc/shadow")?);
    let root_unconfigured =
        root_password_field(&shadow)?.is_empty() || root_password_locked(&shadow)?;
    Ok(!has_human && root_unconfigured)
}

/// Explicit root --hello may initialize a machine with an already unlocked
/// root password, provided no ordinary account has been created and no prior
/// Alcard setup has completed. Never reconfigure existing human users here.
pub fn arm_explicit_root() -> io::Result<()> {
    if unsafe { libc::geteuid() } != 0 {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "root_required",
        ));
    }
    check_alpine()?;
    if Path::new(COMPLETE).exists() {
        return Err(invalid("already_completed"));
    }
    if Path::new(PENDING).exists() {
        return check_guest();
    }
    if human_user_present(&fs::read_to_string("/etc/passwd")?) {
        return Err(invalid("existing_human_user"));
    }
    if !Path::new(STATE_DIR).exists() {
        fs::create_dir(STATE_DIR)?;
        fs::set_permissions(STATE_DIR, fs::Permissions::from_mode(0o700))?;
    }
    let meta = fs::symlink_metadata(STATE_DIR)?;
    if !meta.file_type().is_dir() || meta.uid() != 0 || meta.permissions().mode() & 0o777 != 0o700 {
        return Err(invalid("invalid_firstboot_directory"));
    }
    atomic_write(Path::new(PENDING), MARKER.as_bytes(), 0o600)
}

/// Bootstrap marker is created only by the trusted root launcher on a fresh
/// no-admin Alpine install, or preserved from an interrupted firstboot.
/// Reset will later be a separate, explicitly authenticated operation.
pub fn arm_firstboot() -> io::Result<()> {
    if unsafe { libc::geteuid() } != 0 {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "root_required",
        ));
    }
    check_alpine()?;
    if Path::new(COMPLETE).exists() {
        return Err(invalid("already_completed"));
    }
    if Path::new(PENDING).exists() {
        check_guest()?;
        return Ok(());
    }
    if !needs_hello()? {
        return Err(invalid("usable_account_already_present"));
    }
    if !Path::new(STATE_DIR).exists() {
        fs::create_dir(STATE_DIR)?;
        fs::set_permissions(STATE_DIR, fs::Permissions::from_mode(0o700))?;
    }
    let meta = fs::symlink_metadata(STATE_DIR)?;
    if !meta.file_type().is_dir() || meta.uid() != 0 || meta.permissions().mode() & 0o777 != 0o700 {
        return Err(invalid("invalid_firstboot_directory"));
    }
    atomic_write(Path::new(PENDING), MARKER.as_bytes(), 0o600)
}

fn command(binary: &str, arguments: &[&str]) -> io::Result<()> {
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
fn password_change(username: &str, password: &str) -> io::Result<()> {
    // Shadow-utils chpasswd via stdin: never give secrets as arguments.
    let mut child = Command::new("/usr/sbin/chpasswd")
        .args(["-c", "SHA512"])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;
    let mut line = Zeroizing::new(Vec::with_capacity(username.len() + password.len() + 2));
    line.extend_from_slice(username.as_bytes());
    line.push(b':');
    line.extend_from_slice(password.as_bytes());
    line.push(b'\n');
    let wrote = child
        .stdin
        .take()
        .ok_or_else(|| invalid("password_pipe_unavailable"))?
        .write_all(&line);
    if wrote.is_err() {
        let _ = child.kill();
        let _ = child.wait();
        return Err(invalid("password_pipe_failed"));
    }
    if !child.wait()?.success() {
        return Err(invalid("password_change_failed"));
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

fn user_exists(username: &str) -> io::Result<bool> {
    let passwd = fs::read_to_string("/etc/passwd")?;
    Ok(passwd
        .lines()
        .any(|line| line.split(':').next() == Some(username)))
}

fn create_or_resume_user(username: &str, display_name: &str) -> io::Result<()> {
    let state = Path::new(PROGRESS);
    let existing = state.exists();
    if existing {
        check_file(state, 0o600)?;
        if fs::read_to_string(state)? != format!("{username}\n") {
            return Err(invalid("different_firstboot_account"));
        }
        // The progress marker is written only AFTER adduser succeeds.
        // Without it, an existing user is never treated as Alcard-managed.
        if !user_exists(username)? {
            return Err(invalid("missing_progress_account"));
        }
    } else {
        if user_exists(username)? {
            return Err(invalid("existing_unmanaged_user"));
        }
        let home = format!("/home/{username}");
        let name = if display_name.is_empty() {
            username
        } else {
            display_name
        };
        command(
            "/usr/sbin/adduser",
            &["-D", "-h", &home, "-g", name, "-s", "/bin/ash", username],
        )?;
        // A power cut after adduser but before this marker requires manual
        // recovery, not unverified takeover of an existing account.
        atomic_write(state, format!("{username}\n").as_bytes(), 0o600)?;
    }
    Ok(())
}

fn language_name(lang: &alcard_setup_protocol::Language) -> &'static str {
    match lang {
        alcard_setup_protocol::Language::En => "en_US.UTF-8",
        alcard_setup_protocol::Language::Ru => "ru_RU.UTF-8",
        alcard_setup_protocol::Language::Es => "es_ES.UTF-8",
    }
}
fn keyboard_name(layout: alcard_setup_protocol::Layout) -> &'static str {
    use alcard_setup_protocol::Layout;
    match layout {
        Layout::Us => "us",
        Layout::Ru => "ru",
        Layout::Es => "es",
        Layout::De => "de",
    }
}
fn configure_consoles(plan: &SetupPlan) -> io::Result<()> {
    set_language(language_name(&plan.language))?;
    let layouts = plan
        .keyboard
        .layouts
        .iter()
        .map(|l| keyboard_name(*l))
        .collect::<Vec<_>>();
    set_layouts(&layouts)?;
    set_timezone(&plan.timezone)?;
    if plan.ntp_enabled {
        command("/sbin/rc-update", &["add", ntp_service()?, "default"])?;
    }
    Ok(())
}

fn set_language(lang: &str) -> io::Result<()> {
    fs::create_dir_all("/etc/profile.d")?;
    atomic_write(
        Path::new("/etc/profile.d/alcard-locale.sh"),
        format!("export LANG='{lang}'\n").as_bytes(),
        0o644,
    )?;
    Ok(())
}

fn set_layouts(layouts: &[&str]) -> io::Result<()> {
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

fn set_timezone(tz: &str) -> io::Result<()> {
    let zone_path = PathBuf::from("/usr/share/zoneinfo").join(tz);
    if !zone_path.is_file() {
        return Err(invalid("timezone_not_installed"));
    }
    // No arbitrary path traversal: validated by protocol before this call.
    let data = fs::read(zone_path)?;
    atomic_write(Path::new("/etc/localtime"), &data, 0o644)?;
    atomic_write(
        Path::new("/etc/timezone"),
        format!("{tz}\n").as_bytes(),
        0o644,
    )?;
    Ok(())
}

fn ntp_service() -> io::Result<&'static str> {
    ["chronyd", "ntpd"]
        .into_iter()
        .find(|s| Path::new("/etc/init.d").join(s).is_file())
        .ok_or_else(|| invalid("ntp_service_required"))
}

/// Authenticated post-install actions have no access to account provisioning.
pub fn setting(key: &str, value: &str) -> io::Result<()> {
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

/// The full image ships both profiles. Only an explicit Console selection
/// removes optional components. Keep the firstboot marker until apk succeeds,
/// so an interrupted deletion can be retried without starting a second login.
/// Only packages explicitly dedicated to graphical login are candidates.
const CONSOLE_PRUNE: [&str; 8] = [
    "alcard-desktop-shell",
    "alcard-shell",
    "alcard-polkit-agent",
    "alcard-lock",
    "alcard-desktop-minimal",
    "alcard-fonts-gui",
    "greetd-agreety",
    "greetd",
];

fn package_installed(name: &str) -> io::Result<bool> {
    let status = Command::new("/sbin/apk")
        .args(["info", "-e", name])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()?;
    Ok(status.success())
}

fn prune_console_packages() -> io::Result<()> {
    // An interrupted firstboot may already have removed some packages.
    // Restrict removal to names known at compile time.
    let mut targets = Vec::new();
    for package in CONSOLE_PRUNE {
        if package_installed(package)? {
            targets.push(package);
        }
    }
    if !targets.is_empty() {
        let status = Command::new("/sbin/apk")
            .arg("del")
            .args(&targets)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()?;
        if !status.success() {
            return Err(invalid("console_package_cleanup_failed"));
        }
    }
    // Do not mark Console complete if any direct graphical packages remain.
    for package in CONSOLE_PRUNE {
        if package_installed(package)? {
            return Err(invalid("graphical_package_still_installed"));
        }
    }
    // Do not leave dangling OpenRC runlevel symlinks after package removal.
    let link = Path::new("/etc/runlevels/default/elogind");
    if fs::read_link(link).is_ok_and(|p| p == Path::new("../../init.d/elogind")) {
        fs::remove_file(link)?;
    }
    Ok(())
}

fn finish_session(plan: &SetupPlan) -> io::Result<()> {
    use alcard_setup_protocol::Session;
    // Recheck after provisioning too: a prior successful UI/preflight result
    // cannot make missing or replaced session components trustworthy.
    session_runtime::check(plan.session)?;
    let session_dir = Path::new("/etc/alcard");
    if !session_dir.exists() {
        fs::create_dir(session_dir)?;
    }
    let metadata = fs::symlink_metadata(session_dir)?;
    if !metadata.file_type().is_dir()
        || metadata.uid() != 0
        || metadata.permissions().mode() & 0o022 != 0
    {
        return Err(invalid("unsafe_session_directory"));
    }

    match plan.session {
        Session::Console => {
            // Remove only unused session packages, not the base, TUI, fonts
            // for VT, account tools, or the firstboot dispatcher.
            prune_console_packages()?;
            // Do not set console mode until package removal succeeds.
            atomic_write(&session_dir.join("default-session"), b"console\n", 0o644)?;
        }
        Session::Graphics | Session::GraphicsMinimal => {
            // A fallback selection preserves the main Shell packages so the
            // diagnostic Foot session can repair or inspect the main profile.
            atomic_write(
                &session_dir.join("default-session"),
                format!("{}\n", plan.session.installed_name()).as_bytes(),
                0o644,
            )?;
        }
    }
    Ok(())
}

fn provision(request: &ApplyRequest) -> io::Result<()> {
    at(Stage::Validation, validate_request(request))?;
    at(Stage::Guest, check_guest())?;
    // Reject an unavailable mode before creating the account or changing any
    // password. The privileged backend never trusts Hello's availability flag.
    at(Stage::Runtime, session_runtime::check(request.plan.session))?;
    // Reject foreign/unsafe policy files before touching any account or secret.
    // A retry can reconcile only our exact rules for the same progress account.
    let root_password = !request.credentials.root_password.is_empty();
    let admin = at(
        Stage::AdminPreflight,
        PreparedAdmin::prepare(&request.plan, root_password),
    )?;
    let username = request.plan.account.username.as_str();
    at(
        Stage::Account,
        create_or_resume_user(username, &request.plan.account.display_name),
    )?;
    // Always set user password before creating any administrative path.
    at(
        Stage::UserPassword,
        password_change(username, &request.credentials.user_password),
    )?;
    if root_password {
        // Establish the replacement root route before a retry can revoke doas.
        at(
            Stage::RootPassword,
            password_change("root", &request.credentials.root_password),
        )?;
    }
    at(Stage::AdminPolicies, admin.apply())?;
    if !root_password {
        // Root may be locked only when a functioning doas rule is installed.
        if !request.plan.account.doas_enabled {
            return Err(invalid("no_admin_path"));
        }
        at(Stage::RootLock, ensure_root_password_locked())?;
    }
    at(Stage::Console, configure_consoles(&request.plan))?;
    at(Stage::Session, finish_session(&request.plan))?;

    // Complete only after all operations return successfully. No passwords
    // are ever recorded in either the progress marker or the done marker.
    at(
        Stage::Complete,
        (|| {
            atomic_write(
                Path::new(COMPLETE),
                format!("{username}\n").as_bytes(),
                0o600,
            )?;
            fs::remove_file(PENDING)?;
            File::open(STATE_DIR)?.sync_all()
        })(),
    )?;
    Ok(())
}

pub fn run_firstboot_apply() -> io::Result<()> {
    // Refuse before even reading credentials if this isn't the intended guest.
    at(Stage::Guest, check_guest())?;
    let mut frame = Zeroizing::new(Vec::new());
    at(
        Stage::Request,
        io::stdin().take(MAX_REQUEST + 1).read_to_end(&mut frame),
    )?;
    if frame.len() as u64 > MAX_REQUEST {
        return Err(invalid("request_too_large"));
    }
    let request: ApplyRequest = at(
        Stage::Request,
        serde_json::from_slice(&frame).map_err(|_| invalid("invalid_apply_request")),
    )?;
    // The input byte frame and decoded credential strings are wiped on drop.
    provision(&request)
}

#[cfg(test)]
#[path = "unit_tests/apply.rs"]
mod tests;
