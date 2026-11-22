use super::*;
use std::{
    fs,
    sync::atomic::{AtomicUsize, Ordering},
};

static NEXT: AtomicUsize = AtomicUsize::new(0);
struct Tmp(PathBuf);
impl Tmp {
    fn new() -> Self {
        let path = env::temp_dir().join(format!(
            "alcard-preferences-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&path).unwrap();
        Self(path)
    }
    fn config(&self, value: &str) -> ConfigSource {
        let path = self.0.join("shell.toml");
        fs::write(&path, value).unwrap();
        ConfigSource::new(Some(path), Overrides::default())
    }
}
impl Drop for Tmp {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn optional_preferences_parse_and_invalid_files_fail_as_a_whole() {
    assert_eq!(parse("").unwrap(), Overrides::default());
    let preferences = parse(
        "schema_version=1\ntheme='emerald'\nicon_style='solid'\naccent='#ab12CD'\nlocale='es'",
    )
    .unwrap();
    assert_eq!(preferences.theme.as_deref(), Some("emerald"));
    assert_eq!(preferences.icon_style, Some(IconStyle::Solid));
    assert_eq!(preferences.accent, Some(0xAB12CD));
    assert_eq!(preferences.locale, Some(Language::Es));
    for invalid in [
        "schema_version=2",
        "schema_version='1'",
        "themes='classic'",
        "theme=true",
        "theme=''",
        "theme='bad\npath'",
        "accent='blue'",
        "icon_style='fancy'",
        "locale='de'",
        "locale=4",
        "[shell]\ntheme='classic'",
    ] {
        assert!(parse(invalid).is_err(), "accepted {invalid}");
    }
    assert!(parse(&" ".repeat(MAX_CONFIG_BYTES as usize + 1)).is_err());
}

#[test]
fn idle_defaults_preserve_legacy_files_and_partial_tables_are_explicit() {
    let defaults = IdlePreferences::default();
    assert_eq!(defaults.lock_after_seconds, 300);
    assert_eq!(defaults.screen_off_after_seconds, 360);
    assert_eq!(parse("theme='emerald'").unwrap().idle, None);
    assert_eq!(parse("[idle]").unwrap().idle, Some(defaults));
    assert_eq!(
        ConfigSource::new(None, Overrides::default())
            .load()
            .unwrap()
            .idle,
        defaults
    );
    assert_eq!(
        parse("[idle]\nlock_after_seconds=120").unwrap().idle,
        Some(IdlePreferences {
            lock_after_seconds: 120,
            screen_off_after_seconds: 360,
        })
    );
    assert_eq!(
        parse("[idle]\nscreen_off_after_seconds=0").unwrap().idle,
        Some(IdlePreferences {
            lock_after_seconds: 300,
            screen_off_after_seconds: 0,
        })
    );
    for (lock, off) in [(0, 0), (1, 1), (300, 360), (86400, 86400), (86400, 0)] {
        let text = format!("[idle]\nlock_after_seconds={lock}\nscreen_off_after_seconds={off}");
        assert_eq!(
            parse(&text).unwrap().idle,
            Some(IdlePreferences {
                lock_after_seconds: lock,
                screen_off_after_seconds: off,
            })
        );
    }
}

#[test]
fn battery_defaults_preserve_legacy_configs_and_automatic_sleep_is_explicit() {
    let defaults = BatteryPreferences::default();
    assert_eq!(defaults.low_percent, 20);
    assert_eq!(defaults.critical_percent, 5);
    assert!(!defaults.suspend_on_critical);
    assert_eq!(defaults.recovery_percent(), 23);
    assert_eq!(parse("theme='classic'").unwrap().battery, None);
    assert_eq!(parse("[battery]").unwrap().battery, Some(defaults));
    assert_eq!(
        ConfigSource::new(None, Overrides::default())
            .load()
            .unwrap()
            .battery,
        defaults
    );
    assert_eq!(
        parse("[battery]\nsuspend_on_critical=true")
            .unwrap()
            .battery,
        Some(BatteryPreferences {
            suspend_on_critical: true,
            ..defaults
        })
    );
    for (low, critical) in [(2, 1), (20, 5), (100, 99)] {
        let config = format!("[battery]\nlow_percent={low}\ncritical_percent={critical}");
        let parsed = parse(&config).unwrap().battery.unwrap();
        assert_eq!(
            (parsed.low_percent, parsed.critical_percent),
            (low, critical)
        );
        assert!(parsed.recovery_percent() <= 100);
    }
}

#[test]
fn battery_policy_rejects_invalid_types_thresholds_and_unknown_settings() {
    for invalid in [
        "battery=true",
        "[[battery]]",
        "[battery]\nlow_percent=0",
        "[battery]\ncritical_percent=0",
        "[battery]\nlow_percent=5",
        "[battery]\ncritical_percent=20",
        "[battery]\ncritical_percent=21",
        "[battery]\nlow_percent=101",
        "[battery]\ncritical_percent=-1",
        "[battery]\nlow_percent='20'",
        "[battery]\nlow_percent=20.0",
        "[battery]\nsuspend_on_critical='true'",
        "[battery]\nsuspend_on_critical=1",
        "[battery]\nauto_suspend=true",
        "[battery]\ncritical_percent=5\ncritical_percent=6",
    ] {
        assert!(parse(invalid).is_err(), "accepted {invalid}");
    }
}

#[test]
fn battery_load_is_atomic_and_explicit_overrides_are_validated() {
    let tmp = Tmp::new();
    let mut source = tmp.config("theme='emerald'\nlocale='es'\n[battery]\nlow_percent=30\ncritical_percent=10\nsuspend_on_critical=true");
    let first = source.load().unwrap();
    assert_eq!(first.battery.low_percent, 30);
    assert_eq!(first.battery.critical_percent, 10);
    assert!(first.battery.suspend_on_critical);
    fs::write(
        source.path().unwrap(),
        "theme='classic'\nlocale='ru'\n[battery]\nlow_percent=5",
    )
    .unwrap();
    assert!(source.load().is_err());
    let (fallback, warning) = source.startup();
    assert!(warning.is_some());
    assert_eq!(fallback.locale, Language::Auto);
    assert_eq!(fallback.battery, BatteryPreferences::default());
    let explicit = BatteryPreferences {
        low_percent: 15,
        critical_percent: 3,
        suspend_on_critical: false,
    };
    source.overrides.battery = Some(explicit);
    source.overrides.theme = Some("missing-theme.toml".into());
    assert_eq!(source.startup().0.battery, explicit);
    source.overrides.battery = Some(BatteryPreferences {
        critical_percent: 0,
        ..explicit
    });
    assert_eq!(source.startup().0.battery, BatteryPreferences::default());
}

#[test]
fn idle_policy_rejects_unsafe_order_types_bounds_typos_and_duplicates() {
    for invalid in [
        "idle=true",
        "idle=[]",
        "idle='300'",
        "[idle]\nlock_after_seconds=0",
        "[idle]\nlock_after_seconds=400",
        "[idle]\nlock_after_seconds=0\nscreen_off_after_seconds=1",
        "[idle]\nlock_after_seconds=360\nscreen_off_after_seconds=359",
        "[idle]\nlock_after_seconds=-1",
        "[idle]\nscreen_off_after_seconds=-1",
        "[idle]\nlock_after_seconds=86401\nscreen_off_after_seconds=0",
        "[idle]\nscreen_off_after_seconds=86401",
        "[idle]\nlock_after_seconds=9223372036854775807",
        "[idle]\nlock_after_seconds='300'",
        "[idle]\nscreen_off_after_seconds=360.0",
        "[idle]\nlock_after_seconds=true",
        "[idle]\nautolock=300",
        "[idle]\nlock_after_seconds=300\nlock_after_seconds=301",
        "[[idle]]\nlock_after_seconds=300",
    ] {
        assert!(parse(invalid).is_err(), "accepted {invalid}");
    }
    assert!(parse("[idle]\nlock_after_seconds=0")
        .unwrap_err()
        .to_string()
        .contains("set both to 0"));
}

#[test]
fn idle_load_failure_is_atomic_and_later_reload_uses_a_complete_candidate() {
    let tmp = Tmp::new();
    let source = tmp.config(
        "theme='emerald'\nlocale='ru'\n[idle]\nlock_after_seconds=120\nscreen_off_after_seconds=180",
    );
    let first = source.load().unwrap();
    assert_eq!(first.idle.lock_after_seconds, 120);
    assert_eq!(first.idle.screen_off_after_seconds, 180);
    fs::write(
        source.path().unwrap(),
        "theme='classic'\nlocale='es'\n[idle]\nlock_after_seconds=0",
    )
    .unwrap();
    assert!(source.load().is_err());
    let (fallback, warning) = source.startup();
    assert!(warning.is_some());
    assert_eq!(fallback.theme.id, "classic");
    assert_eq!(fallback.locale, Language::Auto);
    assert_eq!(fallback.idle, IdlePreferences::default());
    fs::write(
        source.path().unwrap(),
        "theme='emerald'\nlocale='es'\n[idle]\nlock_after_seconds=0\nscreen_off_after_seconds=0",
    )
    .unwrap();
    let next = source.load().unwrap();
    assert_eq!(next.theme.id, "emerald");
    assert_eq!(next.locale, Language::Es);
    assert_eq!(next.idle.lock_after_seconds, 0);
    assert_eq!(next.idle.screen_off_after_seconds, 0);
    let restarted = ConfigSource::new(source.path.clone(), Overrides::default());
    assert_eq!(restarted.load().unwrap(), next);
    fs::remove_file(source.path().unwrap()).unwrap();
    assert_eq!(source.load().unwrap().idle, IdlePreferences::default());
}

#[test]
fn explicit_idle_override_is_validated_and_survives_unrelated_startup_failure() {
    let tmp = Tmp::new();
    let mut source = tmp.config("[idle]\nlock_after_seconds=120");
    let disabled = IdlePreferences {
        lock_after_seconds: 0,
        screen_off_after_seconds: 0,
    };
    source.overrides.idle = Some(disabled);
    assert_eq!(source.load().unwrap().idle, disabled);
    assert_eq!(
        fs::read_to_string(source.path().unwrap()).unwrap(),
        "[idle]\nlock_after_seconds=120"
    );
    source.overrides.theme = Some("missing-theme.toml".into());
    let (fallback, warning) = source.startup();
    assert!(warning.is_some());
    assert_eq!(fallback.idle, disabled);
    source.overrides.theme = None;
    source.overrides.idle = Some(IdlePreferences {
        lock_after_seconds: 0,
        screen_off_after_seconds: 360,
    });
    assert!(source.load().is_err());
    let (fallback, warning) = source.startup();
    assert!(warning.is_some());
    assert_eq!(fallback.idle, IdlePreferences::default());
}

#[test]
fn explicit_cli_values_win_without_rewriting_the_config() {
    let tmp = Tmp::new();
    let text = "theme='emerald'\nicon_style='outline'\naccent='#123456'\nlocale='es'";
    let mut source = tmp.config(text);
    source.overrides = Overrides {
        theme: Some("classic".into()),
        icon_style: Some(IconStyle::Solid),
        accent: Some(0xABCDEF),
        locale: Some(Language::Ru),
        idle: None,
        battery: None,
    };
    let resolved = source.load().unwrap();
    assert_eq!(resolved.theme.id, "classic");
    assert_eq!(resolved.theme.icon_style, IconStyle::Solid);
    assert_eq!(resolved.theme.accent, 0xABCDEF);
    assert_eq!(resolved.locale, Language::Ru);
    assert_eq!(fs::read_to_string(source.path().unwrap()).unwrap(), text);
    source.overrides = Overrides {
        locale: Some(Language::En),
        ..Overrides::default()
    };
    let next = source.load().unwrap();
    assert_eq!(next.theme.id, "emerald");
    assert_eq!(next.theme.accent, 0x123456);
    assert_eq!(next.locale, Language::En);
}

#[test]
fn a_fresh_loader_restores_saved_preferences_and_missing_file_is_default() {
    let tmp = Tmp::new();
    let source = tmp.config("theme='emerald'\nlocale='ru'");
    let first = source.load().unwrap();
    let restarted = ConfigSource::new(source.path.clone(), Overrides::default());
    assert_eq!(restarted.load().unwrap(), first);
    fs::remove_file(source.path().unwrap()).unwrap();
    let fallback = source.load().unwrap();
    assert_eq!(fallback.theme.id, "classic");
    assert_eq!(fallback.locale, Language::Auto);
}

#[test]
fn startup_fallback_retains_valid_cli_but_never_partial_file_preferences() {
    let tmp = Tmp::new();
    let mut source = tmp.config("theme='emerald'\nlocale=false");
    source.overrides = Overrides {
        locale: Some(Language::Ru),
        accent: Some(0xABCDEF),
        ..Overrides::default()
    };
    let (fallback, warning) = source.startup();
    assert!(warning.is_some());
    assert_eq!(fallback.theme.id, "classic");
    assert_eq!(fallback.theme.accent, 0xABCDEF);
    assert_eq!(fallback.locale, Language::Ru);
    source.overrides.theme = Some(
        tmp.0
            .join("missing-theme.toml")
            .to_string_lossy()
            .into_owned(),
    );
    let (fallback, warning) = source.startup();
    assert!(warning.is_some());
    assert_eq!(fallback.theme.id, "classic");
    assert_eq!(fallback.locale, Language::Ru);
}

#[test]
fn relative_theme_paths_have_explicit_config_and_cli_bases() {
    let tmp = Tmp::new();
    fs::create_dir(tmp.0.join("config")).unwrap();
    fs::write(
        tmp.0.join("config/theme.toml"),
        include_str!("../../themes/emerald.toml"),
    )
    .unwrap();
    fs::write(
        tmp.0.join("theme.toml"),
        include_str!("../../themes/classic.toml"),
    )
    .unwrap();
    let path = tmp.0.join("config/shell.toml");
    fs::write(&path, "theme='theme.toml'").unwrap();
    let mut source = ConfigSource::new(Some(path), Overrides::default());
    assert_eq!(source.load().unwrap().theme.id, "emerald");
    // Model the captured launch directory without mutating process-wide cwd.
    source.working_dir = tmp.0.clone();
    source.overrides.theme = Some("theme.toml".into());
    assert_eq!(source.load().unwrap().theme.id, "classic");
}

#[test]
fn regular_file_checks_bound_reads_and_accept_regular_symlinks() {
    let tmp = Tmp::new();
    let source = tmp.config("theme='emerald'");
    let link = tmp.0.join("linked.toml");
    std::os::unix::fs::symlink(source.path().unwrap(), &link).unwrap();
    assert_eq!(
        ConfigSource::new(Some(link), Overrides::default())
            .load()
            .unwrap()
            .theme
            .id,
        "emerald"
    );
    let directory = ConfigSource::new(Some(tmp.0.clone()), Overrides::default());
    assert!(matches!(
        directory.load(),
        Err(ConfigError::Read(io::ErrorKind::InvalidData))
    ));
    fs::write(
        source.path().unwrap(),
        vec![b' '; MAX_CONFIG_BYTES as usize + 1],
    )
    .unwrap();
    assert!(matches!(
        source.load(),
        Err(ConfigError::Read(io::ErrorKind::InvalidData))
    ));
    fs::write(source.path().unwrap(), [0xff, 0xfe]).unwrap();
    assert!(source.load().is_err());
}

#[test]
fn fifo_config_and_theme_paths_fail_without_waiting_for_a_writer() {
    use std::{ffi::CString, os::unix::ffi::OsStrExt};
    let tmp = Tmp::new();
    let fifo = tmp.0.join("blocked.toml");
    let cpath = CString::new(fifo.as_os_str().as_bytes()).unwrap();
    // SAFETY: the owned CString is valid and NUL-terminated for this call.
    assert_eq!(unsafe { libc::mkfifo(cpath.as_ptr(), 0o600) }, 0);
    let source = ConfigSource::new(Some(fifo.clone()), Overrides::default());
    assert!(matches!(
        source.load(),
        Err(ConfigError::Read(io::ErrorKind::InvalidData))
    ));
    assert!(Theme::from_file(&fifo).is_err());
}

#[test]
fn settings_edits_merge_and_invalid_edits_leave_the_previous_file_intact() {
    let tmp = Tmp::new();
    let source = tmp.config("# keep this comment\nschema_version=1\n");
    std::thread::scope(|scope| {
        scope.spawn(|| {
            source
                .save(Some("idle"), "lock_after_seconds", 120.into())
                .unwrap()
        });
        scope.spawn(|| source.save(None, "theme", "emerald".into()).unwrap());
    });
    let saved = fs::read_to_string(source.path().unwrap()).unwrap();
    assert!(saved.contains("# keep this comment"));
    assert_eq!(source.load().unwrap().idle.lock_after_seconds, 120);
    assert_eq!(source.load().unwrap().theme.id, "emerald");
    assert!(source
        .save(Some("idle"), "screen_off_after_seconds", 1.into())
        .is_err());
    assert_eq!(fs::read_to_string(source.path().unwrap()).unwrap(), saved);
}
