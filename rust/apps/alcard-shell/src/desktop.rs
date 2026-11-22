//! XDG application discovery and direct argv-based desktop launching.
//! No implicit command shell, program guessing or elevated privileges.
use crate::i18n::Locale;
use std::{
    collections::{BTreeMap, HashSet},
    env,
    ffi::OsStr,
    fmt, fs,
    io::{self, Read},
    os::unix::{
        fs::{OpenOptionsExt, PermissionsExt},
        process::CommandExt,
    },
    path::{Path, PathBuf},
    process::{Child, Command},
};
const MAX_DESKTOP_BYTES: u64 = 256 * 1024;
const MAX_TERMINAL_CONFIG_BYTES: u64 = 1024 * 1024;
const MAX_SCAN_ENTRIES: usize = 8192;

/// Classified, non-secret launch diagnostics. Never retain command arguments.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LaunchError {
    MissingDesktopEntry,
    MissingProgram,
    PermissionDenied,
    InvalidWorkingDirectory,
    UnavailableWorkingDirectory,
    MissingTerminal,
    MissingTerminalConfig,
    InvalidTerminalConfig,
    InvalidDesktopIdentity,
    Spawn(io::ErrorKind),
}
impl fmt::Display for LaunchError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let reason = match self {
            Self::MissingDesktopEntry => "application entry is no longer available",
            Self::MissingProgram => "application executable is not available",
            Self::PermissionDenied => "permission to start the application was denied",
            Self::InvalidWorkingDirectory => "application working directory is invalid",
            Self::UnavailableWorkingDirectory => "application working directory is unavailable",
            Self::MissingTerminal => "the Foot terminal is unavailable",
            Self::MissingTerminalConfig => "the Alcard Foot profile is unavailable",
            Self::InvalidTerminalConfig => "the Alcard Foot profile is invalid or unreadable",
            Self::InvalidDesktopIdentity => "application identity is invalid",
            Self::Spawn(kind) => {
                return write!(formatter, "could not start the application: {kind:?}")
            }
        };
        formatter.write_str(reason)
    }
}
impl std::error::Error for LaunchError {}

#[derive(Clone, Debug)]
pub struct DesktopApp {
    pub id: String,
    pub name: String,
    pub icon: Option<String>,
    /// Wayland compositor app_id fallback when desktop ID differs.
    pub startup_wm_class: Option<String>,
    program: String,
    argv: Vec<String>,
    cwd: Option<PathBuf>,
    terminal: bool,
}
impl DesktopApp {
    pub fn launch(&self) -> Result<Child, LaunchError> {
        if let Some(cwd) = &self.cwd {
            validate_working_directory(cwd)?;
        }
        let program = resolve_executable(&self.program)?;
        let mut command = if self.terminal {
            let terminal = resolve_executable("foot").map_err(|_| LaunchError::MissingTerminal)?;
            let config = foot_profile(env::var_os("ALCARD_FOOT_CONFIG").as_deref())?;
            terminal_command(&terminal, &config, &self.id, &self.name, &program)?
        } else {
            Command::new(program)
        };
        command.args(&self.argv);
        if let Some(dir) = &self.cwd {
            command.current_dir(dir);
        }
        unblock_reload_signal(&mut command);
        command.spawn().map_err(|error| match error.kind() {
            io::ErrorKind::NotFound => LaunchError::MissingProgram,
            io::ErrorKind::PermissionDenied => LaunchError::PermissionDenied,
            kind => LaunchError::Spawn(kind),
        })
    }
}

fn unblock_reload_signal(command: &mut Command) {
    // Shell routes SIGHUP through signalfd. Ordinary child applications must
    // retain normal terminal hangup handling, rather than inherit that mask.
    // Only async-signal-safe libc operations run after fork; no allocation.
    unsafe {
        command.pre_exec(|| {
            let mut mask = std::mem::MaybeUninit::<nix::libc::sigset_t>::uninit();
            if nix::libc::sigemptyset(mask.as_mut_ptr()) != 0
                || nix::libc::sigaddset(mask.as_mut_ptr(), nix::libc::SIGHUP) != 0
                || nix::libc::sigprocmask(
                    nix::libc::SIG_UNBLOCK,
                    mask.as_ptr(),
                    std::ptr::null_mut(),
                ) != 0
            {
                return Err(io::Error::last_os_error());
            }
            Ok(())
        });
    }
}

fn validate_working_directory(path: &Path) -> Result<(), LaunchError> {
    if !path.is_absolute() {
        return Err(LaunchError::InvalidWorkingDirectory);
    }
    match fs::metadata(path) {
        Ok(metadata) if metadata.is_dir() => Ok(()),
        Ok(_) => Err(LaunchError::InvalidWorkingDirectory),
        Err(_) => Err(LaunchError::UnavailableWorkingDirectory),
    }
}

fn executable(path: &Path) -> Result<PathBuf, LaunchError> {
    match fs::metadata(path) {
        Ok(metadata) if !metadata.is_file() => Err(LaunchError::MissingProgram),
        Ok(metadata) if metadata.permissions().mode() & 0o111 == 0 => {
            Err(LaunchError::PermissionDenied)
        }
        Ok(_) => Ok(path.to_owned()),
        Err(error) if error.kind() == io::ErrorKind::PermissionDenied => {
            Err(LaunchError::PermissionDenied)
        }
        Err(_) => Err(LaunchError::MissingProgram),
    }
}

fn resolve_executable(command: &str) -> Result<PathBuf, LaunchError> {
    if command.is_empty() || command.contains('\0') {
        return Err(LaunchError::MissingProgram);
    }
    let p = Path::new(command);
    if p.is_absolute() {
        return executable(p);
    }
    if command.contains('/') {
        return Err(LaunchError::MissingProgram);
    }
    let mut denied = false;
    for path in env::split_paths(&env::var_os("PATH").unwrap_or_default())
        .filter(|dir| dir.is_absolute())
        .map(|dir| dir.join(command))
    {
        match executable(&path) {
            Ok(path) => return Ok(path),
            Err(LaunchError::PermissionDenied) => denied = true,
            _ => {}
        }
    }
    Err(if denied {
        LaunchError::PermissionDenied
    } else {
        LaunchError::MissingProgram
    })
}

fn executable_in_path(command: &str) -> Option<PathBuf> {
    resolve_executable(command).ok()
}

fn foot_profile(explicit: Option<&OsStr>) -> Result<PathBuf, LaunchError> {
    let path = PathBuf::from(explicit.unwrap_or_else(|| OsStr::new("/etc/alcard/foot.ini")));
    if !path.is_absolute() {
        return Err(LaunchError::InvalidTerminalConfig);
    }
    // An explicit development profile is still an arbitrary filesystem path.
    // Never block the Shell event loop while opening a FIFO or other device.
    let file = fs::OpenOptions::new()
        .read(true)
        .custom_flags(nix::libc::O_NONBLOCK)
        .open(&path)
        .map_err(|error| {
            if error.kind() == io::ErrorKind::NotFound {
                LaunchError::MissingTerminalConfig
            } else {
                LaunchError::InvalidTerminalConfig
            }
        })?;
    let metadata = file
        .metadata()
        .map_err(|_| LaunchError::InvalidTerminalConfig)?;
    if !metadata.is_file() || metadata.len() > MAX_TERMINAL_CONFIG_BYTES {
        return Err(LaunchError::InvalidTerminalConfig);
    }
    Ok(path)
}

fn terminal_command(
    terminal: &Path,
    config: &Path,
    desktop_id: &str,
    name: &str,
    program: &Path,
) -> Result<Command, LaunchError> {
    let identity = desktop_id.strip_suffix(".desktop").unwrap_or(desktop_id);
    if identity.is_empty() || identity.len() > 4096 || identity.chars().any(char::is_control) {
        return Err(LaunchError::InvalidDesktopIdentity);
    }
    let mut command = Command::new(terminal);
    command
        .arg("--app-id")
        .arg(identity)
        .arg("--title")
        .arg(name)
        .arg("--config")
        .arg(config)
        .arg("--")
        .arg(program);
    Ok(command)
}
pub fn default_dirs() -> Vec<PathBuf> {
    let mut result = Vec::new();
    let data = env::var_os("XDG_DATA_HOME")
        .filter(|s| !s.is_empty())
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| {
            env::var_os("HOME")
                .map(PathBuf::from)
                .filter(|p| p.is_absolute())
                .map(|p| p.join(".local/share"))
        });
    if let Some(data) = data {
        result.push(data);
    }
    let system = env::var_os("XDG_DATA_DIRS")
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "/usr/local/share:/usr/share".into());
    for p in env::split_paths(&system) {
        if p.is_absolute() && !result.contains(&p) {
            result.push(p);
        }
    }
    result
}
fn unescape(s: &str) -> String {
    let mut chars = s.chars();
    let mut out = String::new();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('s') => out.push(' '),
            Some('t') => out.push('\t'),
            Some('n') => out.push('\n'),
            Some('r') => out.push('\r'),
            Some('\\') => out.push('\\'),
            Some(other) => {
                out.push('\\');
                out.push(other);
            }
            None => out.push('\\'),
        }
    }
    out
}
fn main_group(text: &str) -> BTreeMap<String, String> {
    let mut main = false;
    let mut items = BTreeMap::new();
    for line in text.lines().map(str::trim) {
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if line.starts_with('[') && line.ends_with(']') {
            main = line == "[Desktop Entry]";
        } else if main {
            if let Some((key, value)) = line.split_once('=') {
                items.insert(key.trim().into(), value.trim().into());
            }
        }
    }
    items
}
fn is_true(v: Option<&String>) -> bool {
    v.is_some_and(|s| s == "true")
}
fn allowed_on_desktop(m: &BTreeMap<String, String>, current: &[String]) -> bool {
    let list = |key| -> Vec<&str> {
        m.get(key).map_or(Vec::new(), |s| {
            s.split(';').filter(|x| !x.is_empty()).collect()
        })
    };
    let only = list("OnlyShowIn");
    let exclude = list("NotShowIn");
    (only.is_empty() || only.iter().any(|o| current.iter().any(|d| d == o)))
        && !exclude.iter().any(|o| current.iter().any(|d| d == o))
}
fn translated_name(m: &BTreeMap<String, String>, locale: Locale) -> Option<String> {
    let preferred = format!("Name[{}]", locale.code());
    let regional = format!("Name[{}_", locale.code());
    let value = m
        .get(&preferred)
        .or_else(|| {
            m.iter()
                .find(|(k, _)| k.starts_with(&regional))
                .map(|(_, v)| v)
        })
        .or_else(|| m.get("Name"))?;
    let s: String = unescape(value)
        .chars()
        .filter(|c| !c.is_control())
        .take(96)
        .collect();
    (!s.trim().is_empty()).then_some(s)
}
/// Double quotes group argv elements. Backslash quotes the following character.
/// Shell operators are invalid when unquoted and are never evaluated.
fn tokenize(exec: &str) -> Result<Vec<String>, String> {
    let mut words = Vec::new();
    let mut word = String::new();
    let mut quote = false;
    let mut escaped = false;
    let mut started = false;
    for c in exec.chars() {
        if escaped {
            word.push(c);
            escaped = false;
            started = true;
            continue;
        }
        if c == '\\' {
            escaped = true;
            continue;
        }
        if c == '"' {
            quote = !quote;
            started = true;
            continue;
        }
        if c.is_whitespace() && !quote {
            if started {
                words.push(std::mem::take(&mut word));
                started = false;
            }
            continue;
        }
        if !quote
            && (matches!(
                c,
                '|' | '&' | '<' | '>' | ';' | '$' | '*' | '?' | '#' | '(' | ')'
            ) || c as u32 == 39
                || c as u32 == 96)
        {
            return Err("invalid unquoted reserved character".into());
        }
        word.push(c);
        started = true;
    }
    if escaped || quote {
        return Err("unterminated quote or escape".into());
    }
    if started {
        words.push(word);
    }
    if words.first().is_none_or(String::is_empty) {
        return Err("missing program".into());
    }
    Ok(words)
}
fn expand_exec(
    exec: &str,
    name: &str,
    icon: Option<&str>,
    file: &Path,
) -> Result<(String, Vec<String>), String> {
    let words = tokenize(&unescape(exec))?;
    let mut result = Vec::new();
    let mut file_codes = 0;
    for word in words {
        match word.as_str() {
            "%f" | "%F" | "%u" | "%U" => {
                file_codes += 1;
                continue;
            }
            "%i" => {
                if let Some(icon) = icon.filter(|s| !s.is_empty()) {
                    result.push("--icon".into());
                    result.push(icon.into());
                }
                continue;
            }
            _ => {}
        }
        let mut out = String::new();
        let mut chars = word.chars();
        while let Some(c) = chars.next() {
            if c != '%' {
                out.push(c);
                continue;
            }
            match chars.next() {
                Some('%') => out.push('%'),
                Some('c') => out.push_str(name),
                Some('k') => out.push_str(&file.to_string_lossy()),
                Some('d' | 'D' | 'n' | 'N' | 'm' | 'v') => {}
                Some('f' | 'F' | 'u' | 'U' | 'i') => {
                    return Err("embedded file/icon field code".into())
                }
                _ => return Err("unsupported field code".into()),
            }
        }
        if word.is_empty() || !out.is_empty() {
            result.push(out);
        }
    }
    if file_codes > 1 {
        return Err("too many file/URL field codes".into());
    }
    if result.is_empty() {
        return Err("empty command".into());
    }
    let program = result.remove(0);
    if program.is_empty()
        || program.starts_with('-')
        || program.contains('=')
        || program.contains('\0')
        || (program.contains('/') && !Path::new(&program).is_absolute())
    {
        return Err("invalid executable".into());
    }
    Ok((program, result))
}
fn parse_app(
    id: String,
    text: &str,
    path: &Path,
    locale: Locale,
    desktops: &[String],
) -> Option<DesktopApp> {
    let m = main_group(text);
    if m.get("Type").is_none_or(|s| s != "Application")
        || is_true(m.get("Hidden"))
        || is_true(m.get("NoDisplay"))
        || !allowed_on_desktop(&m, desktops)
    {
        return None;
    }
    if let Some(test) = m.get("TryExec") {
        executable_in_path(&unescape(test))?;
    }
    let name = translated_name(&m, locale)?;
    let icon = m.get("Icon").map(|s| unescape(s)).filter(|s| !s.is_empty());
    let startup_wm_class = m
        .get("StartupWMClass")
        .map(|value| unescape(value))
        .filter(|value| {
            !value.is_empty() && value.len() <= 128 && !value.chars().any(char::is_control)
        });
    // Exec fallback is used for DBusActivatable=true until D-Bus activation exists.
    let (program, argv) = expand_exec(m.get("Exec")?, &name, icon.as_deref(), path).ok()?;
    let cwd = m.get("Path").map(|s| PathBuf::from(unescape(s)));
    Some(DesktopApp {
        id,
        name,
        icon,
        startup_wm_class,
        program,
        argv,
        cwd,
        terminal: is_true(m.get("Terminal")),
    })
}

fn read_desktop_file(path: &Path) -> Option<String> {
    // Desktop entries can be symlinks, but their opened target must be a
    // regular file. NONBLOCK also prevents a replaced target/FIFO from freezing
    // synchronous menu refresh before we can inspect the opened descriptor.
    let file = fs::OpenOptions::new()
        .read(true)
        .custom_flags(nix::libc::O_NONBLOCK)
        .open(path)
        .ok()?;
    let metadata = file.metadata().ok()?;
    if !metadata.is_file() || metadata.len() > MAX_DESKTOP_BYTES {
        return None;
    }
    // Enforce the byte limit while reading too: metadata alone does not bound
    // a file which grows or is rewritten concurrently with refresh.
    let mut text = String::new();
    file.take(MAX_DESKTOP_BYTES + 1)
        .read_to_string(&mut text)
        .ok()?;
    (text.len() as u64 <= MAX_DESKTOP_BYTES).then_some(text)
}

#[allow(clippy::too_many_arguments)] // Bounded recursive XDG walker; explicit traversal state.
fn scan_tree(
    root: &Path,
    path: &Path,
    depth: u8,
    visited: &mut usize,
    seen: &mut HashSet<String>,
    apps: &mut Vec<DesktopApp>,
    locale: Locale,
    desktops: &[String],
) {
    if depth > 8 || *visited >= MAX_SCAN_ENTRIES || apps.len() >= 512 {
        return;
    }
    let Ok(dir) = fs::read_dir(path) else {
        return;
    };
    // Charge every directory entry before allocation/sorting, including
    // subdirectories, unrelated files and iterator errors. A large directory
    // must not bypass the shared discovery budget merely by lacking .desktop.
    let mut entries: Vec<_> = dir
        .take(MAX_SCAN_ENTRIES - *visited)
        .inspect(|_| *visited += 1)
        .filter_map(Result::ok)
        .collect();
    entries.sort_by_key(|e| e.file_name());
    for e in entries {
        // These entries have already been charged. Reaching the budget stops
        // further recursion, not processing the last reserved directory batch.
        if apps.len() >= 512 {
            break;
        }
        let p = e.path();
        let Ok(ty) = e.file_type() else {
            continue;
        };
        if ty.is_dir() {
            scan_tree(root, &p, depth + 1, visited, seen, apps, locale, desktops);
            continue;
        }
        if p.extension().is_none_or(|ext| ext != "desktop") {
            continue;
        }
        let Ok(rel) = p.strip_prefix(root) else {
            continue;
        };
        let id = rel.to_string_lossy().replace('/', "-");
        // Even Hidden=true masks a lower priority entry of the same ID.
        if !seen.insert(id.clone()) {
            continue;
        }
        let Some(s) = read_desktop_file(&p) else {
            continue;
        };
        if let Some(app) = parse_app(id, &s, &p, locale, desktops) {
            apps.push(app);
        }
    }
}
fn discover_in(dirs: &[PathBuf], locale: Locale, desktops: &[String]) -> Vec<DesktopApp> {
    let mut apps = Vec::new();
    let mut seen = HashSet::new();
    let mut visited = 0;
    for root in dirs {
        let applications = root.join("applications");
        scan_tree(
            &applications,
            &applications,
            0,
            &mut visited,
            &mut seen,
            &mut apps,
            locale,
            desktops,
        );
    }
    apps.sort_by(|a, b| {
        a.name
            .to_lowercase()
            .cmp(&b.name.to_lowercase())
            .then(a.id.cmp(&b.id))
    });
    apps
}
pub fn discover(locale: Locale) -> Vec<DesktopApp> {
    let desktops = env::var("XDG_CURRENT_DESKTOP")
        .unwrap_or_default()
        .split(':')
        .filter(|s| !s.is_empty())
        .map(ToOwned::to_owned)
        .collect::<Vec<_>>();
    discover_in(&default_dirs(), locale, &desktops)
}
#[cfg(test)]
#[path = "unit_tests/desktop.rs"]
mod tests;
