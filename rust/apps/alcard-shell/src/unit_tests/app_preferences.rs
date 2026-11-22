use super::*;
use crate::{
    model::{AppTarget, Notice},
    theme::Theme,
};
use alcard_ui::preferences::Overrides;
use std::{
    fs,
    path::PathBuf,
    sync::atomic::{AtomicUsize, Ordering},
};

static NEXT: AtomicUsize = AtomicUsize::new(0);
struct ConfigFile(PathBuf);
impl ConfigFile {
    fn new() -> Self {
        let directory = std::env::temp_dir().join(format!(
            "alcard-shell-preferences-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&directory).unwrap();
        Self(directory.join("shell.toml"))
    }
    fn write(&self, contents: &str) {
        fs::write(&self.0, contents).unwrap();
    }
    fn source(&self) -> ConfigSource {
        ConfigSource::new(Some(self.0.clone()), Overrides::default())
    }
}
impl Drop for ConfigFile {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(self.0.parent().unwrap());
    }
}

#[test]
fn reload_commits_preferences_together_and_rejects_a_partial_bad_candidate() {
    let file = ConfigFile::new();
    file.write("theme='emerald'\nlocale='en'\naccent='#AABBCC'\n[idle]\nlock_after_seconds=120\nscreen_off_after_seconds=180");
    let mut app = Application::new(Shell::new(Locale::En, Theme::classic()));
    app.set_preferences(file.source(), None);
    app.state.set_applications(
        (0..12)
            .map(|index| MenuApp {
                desktop_id: format!("app{index}.desktop"),
                name: format!("App {index}"),
                icon: None,
                startup_wm_class: None,
            })
            .collect(),
    );
    app.windows(vec![WindowInfo {
        id: 41,
        title: "Live document".into(),
        app_id: "app1".into(),
        active: false,
        activity: 3,
    }]);
    app.begin_window_operation(Request::ActivateWindow(41), Instant::now());
    app.launches
        .start("app0.desktop", "App 0", &[], Instant::now(), || {
            Err(desktop::LaunchError::MissingProgram)
        });
    app.sync_launches();
    app.state.sync_notices(vec![Notice {
        id: 7,
        generation: 5,
        title: "Keep this history".into(),
        description: "Unread body".into(),
        app_name: "Application".into(),
        desktop_entry: None,
        created_at: 1,
        urgency: 1,
        unread: true,
        actions: Vec::new(),
        active: false,
    }]);
    app.state.enter(Screen::Menu);
    app.state.selected = 9;
    app.state.page = 1;
    app.state.application_view = ApplicationView::Details {
        target: AppTarget::Desktop("app9.desktop".into()),
        scroll: 0,
    };
    app.state.request_failed(Request::ActivateWindow(99));
    let before = app.input_anchor();
    let launches = app.state.launches.clone();
    let attempts = app.launch_snapshot.clone();
    let operations = app.state.window_operations.clone();
    let windows = app.state.live_windows.clone();
    let notices = app.state.notices.clone();

    assert!(app.reload_preferences());
    assert_eq!(app.state.theme.id, "emerald");
    assert_eq!(app.state.theme.accent, 0xAABBCC);
    assert_eq!(app.state.locale, Locale::En);
    assert_eq!(
        app.idle_preferences(),
        IdlePreferences {
            lock_after_seconds: 120,
            screen_off_after_seconds: 180,
        }
    );
    assert!(before == app.input_anchor());
    assert_eq!(app.state.page, 1);
    assert_eq!(app.state.launches, launches);
    assert_eq!(app.launch_snapshot, attempts);
    assert_eq!(app.state.window_operations, operations);
    assert_eq!(app.state.live_windows, windows);
    assert_eq!(app.state.notices, notices);

    let good_theme = app.state.theme.clone();
    file.write("theme='classic'\nlocale='ru'\naccent='bad'\n[idle]\nlock_after_seconds=0\nscreen_off_after_seconds=0");
    assert!(!app.reload_preferences());
    assert_eq!(app.state.theme, good_theme);
    assert_eq!(app.state.locale, Locale::En);
    assert_eq!(
        app.idle_preferences(),
        IdlePreferences {
            lock_after_seconds: 120,
            screen_off_after_seconds: 180,
        }
    );
    file.write(
        "theme='classic'\nlocale='ru'\n[idle]\nlock_after_seconds=0\nscreen_off_after_seconds=1",
    );
    assert!(!app.reload_preferences());
    assert_eq!(app.state.theme, good_theme);
    assert_eq!(app.state.locale, Locale::En);
    assert_eq!(app.idle_preferences().lock_after_seconds, 120);
    assert!(before == app.input_anchor());
    assert_eq!(app.state.launches, launches);
    assert_eq!(app.state.notices, notices);
    assert!(app.config_error.is_some());
}

#[test]
fn locale_reload_retranslates_existing_failure_without_restarting_attempt() {
    let file = ConfigFile::new();
    file.write("theme='classic'\nlocale='ru'");
    let mut app = Application::new(Shell::new(Locale::En, Theme::classic()));
    app.set_preferences(file.source(), None);
    app.launches
        .start("test.desktop", "Test", &[], Instant::now(), || {
            Err(desktop::LaunchError::MissingProgram)
        });
    app.sync_launches();
    let attempt = app.state.launches[0].attempt;
    assert!(app.state.launches[0].reason.contains("executable"));
    assert!(app.reload_preferences());
    assert_eq!(app.state.locale, Locale::Ru);
    assert_eq!(app.state.launches[0].attempt, attempt);
    assert_eq!(app.state.launches[0].phase, LaunchPhase::Failed);
    assert!(app.state.launches[0].reason.contains("исполняемый"));
    assert_eq!(app.launches.take_update(), None);
}
