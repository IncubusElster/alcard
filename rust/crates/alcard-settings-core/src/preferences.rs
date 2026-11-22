//! Persistent, data-only Shell preferences. Loading never writes the user's
//! file, executes commands, fetches assets, or creates a background poller.
use crate::theme::{parse_color, read_bounded_text, IconStyle, Theme};
use std::{
    env, fmt, io,
    path::{Path, PathBuf},
};

const MAX_CONFIG_BYTES: u64 = 16 * 1024;
const MAX_IDLE_SECONDS: u32 = 24 * 60 * 60;

/// Low-charge notifications are enabled by default. Suspending a session is
/// opt-in and is only a request to the Shell's existing guarded sleep path.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BatteryPreferences {
    pub low_percent: u8,
    pub critical_percent: u8,
    pub suspend_on_critical: bool,
}

impl Default for BatteryPreferences {
    fn default() -> Self {
        Self {
            low_percent: 20,
            critical_percent: 5,
            suspend_on_critical: false,
        }
    }
}

impl BatteryPreferences {
    pub fn validate(self) -> Result<Self, ConfigError> {
        if self.critical_percent == 0
            || self.critical_percent >= self.low_percent
            || self.low_percent > 100
        {
            return Err(ConfigError::Invalid(
                "battery thresholds must satisfy 1 <= critical_percent < low_percent <= 100",
            ));
        }
        Ok(self)
    }

    /// A small upward fluctuation must not rearm warnings or automatic sleep.
    pub fn recovery_percent(self) -> u8 {
        self.low_percent.saturating_add(3).min(100)
    }
}

/// Per-session idle policy. Zero disables that action; screen-off requires
/// an earlier or simultaneous automatic lock so it cannot hide an open session.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct IdlePreferences {
    pub lock_after_seconds: u32,
    pub screen_off_after_seconds: u32,
}

impl Default for IdlePreferences {
    fn default() -> Self {
        Self {
            lock_after_seconds: 300,
            screen_off_after_seconds: 360,
        }
    }
}

impl IdlePreferences {
    pub fn validate(self) -> Result<Self, ConfigError> {
        if self.lock_after_seconds > MAX_IDLE_SECONDS
            || self.screen_off_after_seconds > MAX_IDLE_SECONDS
        {
            return Err(ConfigError::Invalid(
                "idle timeouts must be integer seconds from 0 to 86400",
            ));
        }
        if self.screen_off_after_seconds > 0 {
            if self.lock_after_seconds == 0 {
                return Err(ConfigError::Invalid(
                    "idle.screen_off_after_seconds requires a nonzero idle.lock_after_seconds; set both to 0 to disable autolock",
                ));
            }
            if self.screen_off_after_seconds < self.lock_after_seconds {
                return Err(ConfigError::Invalid(
                    "idle.screen_off_after_seconds must be 0 or at least idle.lock_after_seconds",
                ));
            }
        }
        Ok(self)
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Language {
    #[default]
    Auto,
    En,
    Ru,
    Es,
}

impl Language {
    /// Resolve the configured choice after CLI/system/user precedence is applied.
    pub fn resolve(self) -> alcard_l10n::Locale {
        use alcard_l10n::Locale;
        match self {
            Self::Auto => Locale::detect(),
            Self::En => Locale::En,
            Self::Ru => Locale::Ru,
            Self::Es => Locale::Es,
        }
    }

    pub fn parse(value: &str) -> Result<Self, String> {
        match value {
            "auto" => Ok(Self::Auto),
            "en" => Ok(Self::En),
            "ru" => Ok(Self::Ru),
            "es" => Ok(Self::Es),
            _ => Err("locale must be auto, en, ru or es".into()),
        }
    }
}

/// Explicit CLI values override the same preferences read from the file.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Overrides {
    pub theme: Option<String>,
    pub icon_style: Option<IconStyle>,
    pub accent: Option<u32>,
    pub locale: Option<Language>,
    pub idle: Option<IdlePreferences>,
    pub battery: Option<BatteryPreferences>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Resolved {
    pub theme: Theme,
    pub locale: Language,
    pub idle: IdlePreferences,
    pub battery: BatteryPreferences,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ConfigError {
    Read(io::ErrorKind),
    Invalid(&'static str),
    Theme(String),
}

impl fmt::Display for ConfigError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Read(kind) => write!(formatter, "could not read shell.toml: {kind:?}"),
            Self::Invalid(reason) => formatter.write_str(reason),
            Self::Theme(reason) => write!(formatter, "could not load selected theme: {reason}"),
        }
    }
}
impl std::error::Error for ConfigError {}

#[derive(Clone, Debug)]
pub struct ConfigSource {
    path: Option<PathBuf>,
    overrides: Overrides,
    working_dir: PathBuf,
    use_system_language: bool,
}

impl ConfigSource {
    pub fn from_environment(overrides: Overrides) -> Self {
        let directory = env::var_os("XDG_CONFIG_HOME")
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
            .filter(|path| path.is_absolute())
            .or_else(|| {
                env::var_os("HOME")
                    .map(PathBuf::from)
                    .filter(|path| path.is_absolute())
                    .map(|path| path.join(".config"))
            });
        let mut source = Self::new(
            directory.map(|path| path.join("alcard/shell.toml")),
            overrides,
        );
        source.use_system_language = true;
        source
    }

    /// Also useful to callers which already own an explicit config location.
    /// Relative locations are bound to the working directory at construction.
    pub fn new(path: Option<PathBuf>, overrides: Overrides) -> Self {
        let working_dir = env::current_dir().unwrap_or_default();
        let path = path.map(|path| {
            if path.is_absolute() {
                path
            } else {
                working_dir.join(path)
            }
        });
        Self {
            path,
            overrides,
            working_dir,
            use_system_language: false,
        }
    }

    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    /// Build an entire candidate. The caller commits it only after this succeeds.
    /// A missing shell.toml means default preferences, including on reload.
    pub fn load(&self) -> Result<Resolved, ConfigError> {
        let preferences = match &self.path {
            Some(path) => match read_bounded_text(path, MAX_CONFIG_BYTES) {
                Ok(text) => parse(&text)?,
                Err(error) if error.kind() == io::ErrorKind::NotFound => Overrides::default(),
                Err(error) => return Err(ConfigError::Read(error.kind())),
            },
            None => Overrides::default(),
        };
        self.resolve(preferences)
    }

    /// A bad startup file cannot take away the session navigation UI. Retain
    /// valid explicit CLI preferences; a broken CLI theme still falls back to
    /// the built-in Classic palette. No partial file preferences are applied.
    pub fn startup(&self) -> (Resolved, Option<ConfigError>) {
        match self.load() {
            Ok(preferences) => (preferences, None),
            Err(error) => {
                let fallback = self.resolve(Overrides::default()).unwrap_or_else(|_| {
                    let mut theme = Theme::classic();
                    if let Some(style) = self.overrides.icon_style {
                        theme.icon_style = style;
                    }
                    if let Some(accent) = self.overrides.accent.filter(|color| *color <= 0xFFFFFF) {
                        theme.accent = accent;
                    }
                    Resolved {
                        theme,
                        locale: self.overrides.locale.unwrap_or_default(),
                        idle: self
                            .overrides
                            .idle
                            .filter(|idle| idle.validate().is_ok())
                            .unwrap_or_default(),
                        battery: self
                            .overrides
                            .battery
                            .filter(|battery| battery.validate().is_ok())
                            .unwrap_or_default(),
                    }
                });
                (fallback, Some(error))
            }
        }
    }

    fn resolve(&self, preferences: Overrides) -> Result<Resolved, ConfigError> {
        let (selection, base) = if let Some(selection) = &self.overrides.theme {
            (selection.as_str(), self.working_dir.as_path())
        } else {
            (
                preferences.theme.as_deref().unwrap_or("classic"),
                self.path
                    .as_ref()
                    .and_then(|path| path.parent())
                    .unwrap_or(&self.working_dir),
            )
        };
        let mut theme = if let Some(theme) = Theme::builtin(selection) {
            theme
        } else {
            if selection.is_empty()
                || selection.len() > 4096
                || selection.chars().any(char::is_control)
            {
                return Err(ConfigError::Invalid(
                    "theme must be a builtin ID or local file path",
                ));
            }
            let path = Path::new(selection);
            let path = if path.is_absolute() {
                path.to_owned()
            } else {
                base.join(path)
            };
            if !path.is_absolute() {
                return Err(ConfigError::Invalid("theme path could not be resolved"));
            }
            Theme::from_file(&path).map_err(ConfigError::Theme)?
        };
        if let Some(style) = self.overrides.icon_style.or(preferences.icon_style) {
            theme.icon_style = style;
        }
        if let Some(accent) = self.overrides.accent.or(preferences.accent) {
            if accent > 0xFFFFFF {
                return Err(ConfigError::Invalid("accent must be #RRGGBB"));
            }
            theme.accent = accent;
        }
        Ok(Resolved {
            theme,
            locale: self
                .overrides
                .locale
                .or_else(|| {
                    self.use_system_language
                        .then(system_language)
                        .flatten()
                        .and_then(|s| Language::parse(s).ok())
                })
                .or(preferences.locale)
                .unwrap_or_default(),
            idle: self
                .overrides
                .idle
                .or(preferences.idle)
                .unwrap_or_default()
                .validate()?,
            battery: self
                .overrides
                .battery
                .or(preferences.battery)
                .unwrap_or_default()
                .validate()?,
        })
    }
}

/// Empty/unversioned preference files are accepted as schema 1. Other versions
/// and unknown fields fail as a whole, so typos never silently change behavior.
pub fn parse(input: &str) -> Result<Overrides, ConfigError> {
    if input.len() as u64 > MAX_CONFIG_BYTES {
        return Err(ConfigError::Invalid("shell.toml exceeds 16 KiB"));
    }
    let document: toml::Value =
        toml::from_str(input).map_err(|_| ConfigError::Invalid("invalid shell.toml TOML"))?;
    let table = document
        .as_table()
        .ok_or(ConfigError::Invalid("shell.toml must be a table"))?;
    if table.keys().any(|key| {
        !matches!(
            key.as_str(),
            "schema_version" | "theme" | "icon_style" | "accent" | "locale" | "idle" | "battery"
        )
    }) {
        return Err(ConfigError::Invalid("unknown shell.toml preference"));
    }
    if table
        .get("schema_version")
        .is_some_and(|version| version.as_integer() != Some(1))
    {
        return Err(ConfigError::Invalid(
            "unsupported shell.toml schema_version; expected 1",
        ));
    }
    let string = |key: &str| -> Result<Option<&str>, ConfigError> {
        table
            .get(key)
            .map(|value| {
                value.as_str().ok_or(ConfigError::Invalid(
                    "shell.toml preferences must be strings",
                ))
            })
            .transpose()
    };
    let theme = string("theme")?.map(str::to_owned);
    if theme.as_ref().is_some_and(|value| {
        value.is_empty() || value.len() > 4096 || value.chars().any(char::is_control)
    }) {
        return Err(ConfigError::Invalid(
            "theme must be a builtin ID or local file path",
        ));
    }
    Ok(Overrides {
        theme,
        icon_style: string("icon_style")?
            .map(IconStyle::parse)
            .transpose()
            .map_err(|_| ConfigError::Invalid("icon_style must be solid or outline"))?,
        accent: string("accent")?
            .map(parse_color)
            .transpose()
            .map_err(|_| ConfigError::Invalid("accent must be #RRGGBB"))?,
        locale: string("locale")?
            .map(Language::parse)
            .transpose()
            .map_err(|_| ConfigError::Invalid("locale must be auto, en, ru or es"))?,
        idle: table.get("idle").map(parse_idle).transpose()?,
        battery: table.get("battery").map(parse_battery).transpose()?,
    })
}

fn parse_battery(value: &toml::Value) -> Result<BatteryPreferences, ConfigError> {
    let table = value
        .as_table()
        .ok_or(ConfigError::Invalid("battery must be a table"))?;
    if table.keys().any(|key| {
        !matches!(
            key.as_str(),
            "low_percent" | "critical_percent" | "suspend_on_critical"
        )
    }) {
        return Err(ConfigError::Invalid("unknown battery preference"));
    }
    let percent = |key: &str, default: u8| -> Result<u8, ConfigError> {
        table.get(key).map_or(Ok(default), |value| {
            value
                .as_integer()
                .and_then(|value| u8::try_from(value).ok())
                .filter(|value| *value <= 100)
                .ok_or(ConfigError::Invalid(
                    "battery thresholds must be integer percentages from 1 to 100",
                ))
        })
    };
    let defaults = BatteryPreferences::default();
    BatteryPreferences {
        low_percent: percent("low_percent", defaults.low_percent)?,
        critical_percent: percent("critical_percent", defaults.critical_percent)?,
        suspend_on_critical: table
            .get("suspend_on_critical")
            .map_or(Ok(false), |value| {
                value.as_bool().ok_or(ConfigError::Invalid(
                    "battery.suspend_on_critical must be true or false",
                ))
            })?,
    }
    .validate()
}

fn parse_idle(value: &toml::Value) -> Result<IdlePreferences, ConfigError> {
    let table = value
        .as_table()
        .ok_or(ConfigError::Invalid("idle must be a table"))?;
    if table.keys().any(|key| {
        !matches!(
            key.as_str(),
            "lock_after_seconds" | "screen_off_after_seconds"
        )
    }) {
        return Err(ConfigError::Invalid("unknown idle preference"));
    }
    let seconds = |key: &str, default: u32| -> Result<u32, ConfigError> {
        match table.get(key) {
            None => Ok(default),
            Some(value) => value
                .as_integer()
                .and_then(|value| u32::try_from(value).ok())
                .filter(|value| *value <= MAX_IDLE_SECONDS)
                .ok_or(ConfigError::Invalid(
                    "idle timeouts must be integer seconds from 0 to 86400",
                )),
        }
    };
    let defaults = IdlePreferences::default();
    IdlePreferences {
        lock_after_seconds: seconds("lock_after_seconds", defaults.lock_after_seconds)?,
        screen_off_after_seconds: seconds(
            "screen_off_after_seconds",
            defaults.screen_off_after_seconds,
        )?,
    }
    .validate()
}

#[cfg(test)]
#[path = "unit_tests/preferences.rs"]
mod tests;

/// Edit one property while retaining unrelated settings, comments and formatting.
impl ConfigSource {
    pub fn save(&self, group: Option<&str>, key: &str, value: toml_edit::Value) -> io::Result<()> {
        use std::{
            fs::{self, File, OpenOptions},
            io::Write,
            os::unix::fs::OpenOptionsExt,
        };
        let path = self
            .path()
            .ok_or_else(|| io::Error::other("Missing configuration directory"))?;
        let dir = path.parent().unwrap();
        fs::create_dir_all(dir)?;
        let lock = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW)
            .open(dir.join("shell.toml.lock"))?;
        lock.lock()?;
        let data = match read_bounded_text(path, MAX_CONFIG_BYTES) {
            Ok(s) => s,
            Err(e) if e.kind() == io::ErrorKind::NotFound => String::new(),
            Err(e) => return Err(e),
        };
        let mut doc = data
            .parse::<toml_edit::DocumentMut>()
            .map_err(io::Error::other)?;
        parse(&data).map_err(io::Error::other)?;
        let table = match group {
            Some(g) => doc
                .entry(g)
                .or_insert_with(|| toml_edit::Item::Table(toml_edit::Table::new()))
                .as_table_like_mut()
                .ok_or_else(|| io::Error::other("Invalid table"))?,
            None => doc.as_table_mut() as &mut dyn toml_edit::TableLike,
        };
        table.insert(key, toml_edit::Item::Value(value));
        let content = doc.to_string();
        self.resolve(parse(&content).map_err(io::Error::other)?)
            .map_err(io::Error::other)?;
        let temp = dir.join(format!(".shell.toml.{}", std::process::id()));
        let mut file = OpenOptions::new()
            .create_new(true)
            .write(true)
            .mode(0o600)
            .open(&temp)?;
        let result = (|| {
            file.write_all(content.as_bytes())?;
            file.sync_all()?;
            fs::rename(&temp, path)?;
            File::open(dir)?.sync_all()
        })();
        if result.is_err() {
            let _ = fs::remove_file(temp);
        }
        result
    }
}

pub fn system_language() -> Option<&'static str> {
    let profile = std::fs::read_to_string("/etc/profile.d/alcard-locale.sh").ok()?;
    match profile.trim() {
        "export LANG='ru_RU.UTF-8'" => Some("ru"),
        "export LANG='es_ES.UTF-8'" => Some("es"),
        "export LANG='en_US.UTF-8'" => Some("en"),
        _ => None,
    }
}
