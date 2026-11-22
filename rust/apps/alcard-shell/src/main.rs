//! Alcard Shell. All graphical surfaces use SCTK, tiny-skia and managed SHM.
mod app;
mod authentication;
mod battery;
mod battery_journal;
mod controls;
mod desktop;
use alcard_ui::{font, shm, theme};
mod home;
mod i18n;
mod icon_cache;
mod idle;
mod input;
mod launch;
mod layer;
mod lock_presence;
mod locking;
mod modal;
mod model;
mod notifications;
mod radios;
mod render;
mod sleep;
mod sleep_intent;
mod status;
mod system;
mod toplevel;
mod wayland;
use alcard_ui::preferences::{ConfigSource, Language, Overrides};
use model::Shell;
use std::error::Error;

fn usage() {
    println!("Usage: alcard-shell [--home | --locale auto|en|ru|es --theme classic|emerald|PATH --icon-style solid|outline --accent '#RRGGBB']");
    println!(
        "Keys: 4/Enter primary, 5/Space options/select, 6/Tab special, 7/F3 extra, 8/Esc back."
    );
    println!("Arrows navigate, Home returns home. 7/F3 opens search in Menu only.");
    println!(
        "On Home, 5/Space opens Quick settings: brightness, volume, Wi-Fi, Bluetooth, logout, restart, power off, lock, and Sleep."
    );
    println!("Quick settings: Up/Down select a row; Left/Right adjust brightness or volume; 5 toggles mute on the volume row. Wi-Fi/Bluetooth: 4 opens the built-in list, 5 turns the radio on/off, 7 refreshes, 8 returns. System actions use 4. Only detected, permitted devices are adjustable.");
    println!("Sleep requires a noninteractive login1 permission and a confirmed native lock before the request. External sleep uses login1's bounded delay; resume preserves the lock. Hardware suspend/resume verification remains pending.");
    println!(
        "Text editing: digits 4-8 and Space type text; Enter applies, Esc cancels, Ctrl+U clears."
    );
    println!("Preferences: $XDG_CONFIG_HOME/alcard/shell.toml or ~/.config/alcard/shell.toml. Explicit CLI values take priority.");
    println!("SIGHUP reloads valid preferences atomically; errors retain the previous working configuration.");
    println!("[idle] defaults: lock_after_seconds=300, screen_off_after_seconds=360. Screen-off waits for a confirmed lock; set both to 0 to disable. SIGHUP also reloads these timeouts.");
    println!("[battery] defaults: low_percent=20, critical_percent=5, suspend_on_critical=false. Warnings require fresh discharge readings; optional automatic sleep uses the existing confirmed lock, once per discharge episode. Resume and Shell restart do not repeat it.");
    println!("Normal mode discovers and launches XDG .desktop applications as the user.");
    println!("Normal mode uses wlr foreign-toplevel for real Recent windows when supported.");
    println!("--home queues an activation request in the existing Shell (never launches a second instance).");
    println!("The sample labwc Super keybind is opt-in; graphical polkit is a separate session process; hardware verification remains pending.");
}
fn main() -> Result<(), Box<dyn Error>> {
    // A compositor shortcut always targets the existing process; there is no
    // GUI initialization, background service or shell execution in this path.
    let invocation: Vec<String> = std::env::args().skip(1).collect();
    if invocation.iter().any(|argument| argument == "--home") {
        if invocation.len() != 1 {
            return Err("--home cannot be combined with other options".into());
        }
        home::request_home()?;
        return Ok(());
    }
    let mut overrides = Overrides::default();
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--help" | "-h" => {
                usage();
                return Ok(());
            }
            "--locale" => {
                overrides.locale = Some(Language::parse(
                    &args.next().ok_or("--locale requires a value")?,
                )?);
            }
            "--theme" => {
                overrides.theme = Some(args.next().ok_or("--theme requires a value")?);
            }
            "--icon-style" => {
                overrides.icon_style = Some(theme::IconStyle::parse(
                    &args.next().ok_or("--icon-style requires a value")?,
                )?);
            }
            "--accent" => {
                overrides.accent = Some(theme::parse_color(
                    &args.next().ok_or("--accent requires a value")?,
                )?);
            }
            _ => return Err(format!("unknown flag: {arg}").into()),
        }
    }
    if std::env::var_os("WAYLAND_DISPLAY").is_none() {
        return Err("a Wayland session is required (WAYLAND_DISPLAY is unset)".into());
    }
    let source = ConfigSource::from_environment(overrides);
    let (preferences, warning) = source.startup();
    let locale = app::resolve_locale(preferences.locale);
    println!(
        "Alcard Shell (locale={}, theme={})",
        locale.code(),
        preferences.theme.name
    );
    let mut application = app::Application::new(Shell::new(locale, preferences.theme));
    application.set_idle_preferences(preferences.idle);
    application.set_battery_preferences(preferences.battery);
    application.initialize_battery_journal();
    application.set_preferences(source, warning);
    wayland::run(application)
}
