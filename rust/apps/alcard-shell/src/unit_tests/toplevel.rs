use super::*;
#[test]
fn long_titles_show_truncation_and_invalid_identities_cannot_match_a_prefix() {
    assert_eq!(bounded("Long title", 5), "Long…");
    assert_eq!(bounded("Exact", 5), "Exact");
    assert_eq!(bounded("\nTitle\r", 5), "Title");
    assert_eq!(bounded("Title", 0), "");
    assert_eq!(identity("app.editor".into()), "app.editor");
    assert!(identity("app.editor\n".into()).is_empty());
    assert!(identity("a".repeat(4097)).is_empty());
}
#[test]
fn activation_request_remains_pending_without_wayland_seat() {
    let mut state = State::new();
    state.request_home();
    state.request_shell_activation(None);
    assert!(state.pending_shell_activation);
}
#[test]
fn shell_window_is_excluded_but_normal_apps_are_not() {
    let mut item = WindowInfo {
        id: 1,
        title: "Editor".into(),
        app_id: "app.editor".into(),
        active: false,
        activity: 0,
    };
    assert!(!is_own_window(&item));
    item.app_id = "alcard-shell".into();
    assert!(is_own_window(&item));
    item.app_id = "app.editor".into();
    item.title = "Alcard Shell (spoofed title)".into();
    assert!(!is_own_window(&item));
}
#[test]
fn manager_shutdown_is_reported_once_and_clears_pending_activation() {
    let mut state = State::new();
    state.request_home();
    state.stop();
    assert!(matches!(state.take_update(), Some(Err(()))));
    assert!(state.take_update().is_none());
    assert!(!state.pending_shell_activation);
    assert!(state.close(42).is_err());
    assert!(state.activate(42, None).is_err());
}
