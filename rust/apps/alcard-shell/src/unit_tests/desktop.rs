use super::*;

#[test]
fn launched_child_does_not_inherit_shell_reload_signal_mask() {
    use smithay_client_toolkit::reexports::calloop::signals::{Signal, Signals};
    use std::os::unix::process::ExitStatusExt;
    let _reload_source = Signals::new(&[Signal::SIGHUP]).unwrap();
    let mut command = Command::new("/bin/sh");
    command.args(["-c", "kill -HUP $$; exit 99"]);
    unblock_reload_signal(&mut command);
    let status = command.status().unwrap();
    assert_eq!(status.signal(), Some(nix::libc::SIGHUP));
}
use std::sync::atomic::{AtomicUsize, Ordering};
static NEXT: AtomicUsize = AtomicUsize::new(0);
struct Tmp(PathBuf);
impl Tmp {
    fn new() -> Self {
        let p = env::temp_dir().join(format!(
            "alcard-desktop-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&p).unwrap();
        Self(p)
    }
    fn add(&self, id: &str, text: &str) {
        let p = self.0.join("applications").join(id);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, text).unwrap();
    }
}
impl Drop for Tmp {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
#[test]
fn hides_entries_and_filters_desktops() {
    let t = Tmp::new();
    t.add(
        "hidden.desktop",
        "[Desktop Entry]\nType=Application\nName=Hidden\nHidden=true\nExec=true\n",
    );
    t.add(
        "nodisplay.desktop",
        "[Desktop Entry]\nType=Application\nName=No\nNoDisplay=true\nExec=true\n",
    );
    t.add(
        "kde.desktop",
        "[Desktop Entry]\nType=Application\nName=KDE\nOnlyShowIn=KDE;\nExec=true\n",
    );
    t.add("bad.desktop","[Desktop Entry]\nType=Application\nName=Missing\nTryExec=/alcard/not/installed\nExec=true\n");
    t.add(
        "valid.desktop",
        "[Desktop Entry]\nType=Application\nName=Default\nName[ru]=Настоящее\nExec=true\n",
    );
    let v = discover_in(std::slice::from_ref(&t.0), Locale::Ru, &["ALCARD".into()]);
    assert_eq!(v.len(), 1);
    assert_eq!(v[0].name, "Настоящее");
}
#[test]
fn user_hidden_masks_lower_priority_and_nested_ids_are_stable() {
    let user = Tmp::new();
    let system = Tmp::new();
    user.add("same.desktop", "[Desktop Entry]\nHidden=true\n");
    system.add(
        "same.desktop",
        "[Desktop Entry]\nType=Application\nName=Ignored\nExec=true\n",
    );
    user.add(
        "folder/hello.desktop",
        "[Desktop Entry]\nType=Application\nName=User\nExec=true\n",
    );
    system.add(
        "folder/hello.desktop",
        "[Desktop Entry]\nType=Application\nName=System\nExec=true\n",
    );
    let apps = discover_in(&[user.0.clone(), system.0.clone()], Locale::En, &[]);
    assert_eq!(apps.len(), 1);
    assert_eq!(apps[0].id, "folder-hello.desktop");
    assert_eq!(apps[0].name, "User");
}
#[test]
fn startup_wm_class_is_loaded_only_as_a_matching_hint() {
    let t = Tmp::new();
    t.add(
        "com.example.Editor.desktop",
        "[Desktop Entry]\nType=Application\nName=Editor\nExec=true\nStartupWMClass=EditorWindow\n",
    );
    let apps = discover_in(std::slice::from_ref(&t.0), Locale::En, &[]);
    assert_eq!(apps.len(), 1);
    assert_eq!(apps[0].id, "com.example.Editor.desktop");
    assert_eq!(apps[0].startup_wm_class.as_deref(), Some("EditorWindow"));
}

#[test]
fn exec_argv_is_not_shell_interpreted() {
    let f = Path::new("/tmp/test.desktop");
    let (p, args) = expand_exec(
        "myapp --title \"%c\" --percent=%% \"two words\" %i %f",
        "A Window",
        Some("test-icon"),
        f,
    )
    .unwrap();
    assert_eq!(p, "myapp");
    assert_eq!(
        args,
        vec![
            "--title",
            "A Window",
            "--percent=%",
            "two words",
            "--icon",
            "test-icon"
        ]
    );
    assert!(expand_exec("myapp %Z", "x", None, f).is_err());
    assert!(expand_exec("myapp %f %U", "x", None, f).is_err());
    assert!(expand_exec("myapp | bad", "x", None, f).is_err());
    assert!(expand_exec("bin/relative-app", "x", None, f).is_err());
    assert!(expand_exec("myapp 'quotes'", "x", None, f).is_err());
}
#[test]
fn direct_spawn_executes_a_known_binary_without_shell() {
    let t = Tmp::new();
    if !Path::new("/usr/bin/true").exists() {
        return;
    }
    t.add(
        "true.desktop",
        "[Desktop Entry]\nType=Application\nName=True\nExec=/usr/bin/true\n",
    );
    let apps = discover_in(std::slice::from_ref(&t.0), Locale::En, &[]);
    assert_eq!(apps.len(), 1);
    let status = apps[0].launch().unwrap().wait().unwrap();
    assert!(status.success());
}

#[test]
fn explicit_working_directory_is_preserved_and_never_silently_ignored() {
    let tmp = Tmp::new();
    let regular = tmp.0.join("regular-file");
    fs::write(&regular, "not a directory").unwrap();
    let missing = tmp.0.join("absent-directory");
    for (path, expected) in [
        (PathBuf::new(), LaunchError::InvalidWorkingDirectory),
        (
            PathBuf::from("relative"),
            LaunchError::InvalidWorkingDirectory,
        ),
        (regular, LaunchError::InvalidWorkingDirectory),
        (missing, LaunchError::UnavailableWorkingDirectory),
    ] {
        let app = parse_app(
            "cwd.desktop".into(),
            &format!(
                "[Desktop Entry]\nType=Application\nName=Cwd\nExec=/usr/bin/true\nPath={}\n",
                path.display()
            ),
            &tmp.0.join("cwd.desktop"),
            Locale::En,
            &[],
        )
        .unwrap();
        assert_eq!(app.cwd.as_deref(), Some(path.as_path()));
        assert_eq!(app.launch().unwrap_err(), expected);
    }
}

#[test]
fn valid_explicit_working_directory_is_used_by_the_child() {
    let tmp = Tmp::new();
    fs::write(tmp.0.join("alcard-cwd-marker"), "present").unwrap();
    let app = parse_app(
        "cwd.desktop".into(),
        &format!(
            "[Desktop Entry]\nType=Application\nName=Cwd\nExec=/usr/bin/test -f alcard-cwd-marker\nPath={}\n",
            tmp.0.display()
        ),
        &tmp.0.join("cwd.desktop"),
        Locale::En,
        &[],
    )
    .unwrap();
    assert!(app.launch().unwrap().wait().unwrap().success());
}

#[test]
fn missing_and_nonexecutable_programs_have_distinct_errors() {
    let tmp = Tmp::new();
    let missing = tmp.0.join("missing");
    assert_eq!(
        resolve_executable(missing.to_str().unwrap()),
        Err(LaunchError::MissingProgram)
    );
    let denied = tmp.0.join("not-executable");
    fs::write(&denied, "data").unwrap();
    fs::set_permissions(&denied, fs::Permissions::from_mode(0o600)).unwrap();
    assert_eq!(
        resolve_executable(denied.to_str().unwrap()),
        Err(LaunchError::PermissionDenied)
    );
    for invalid in ["", "relative/program", "with\0nul"] {
        assert_eq!(
            resolve_executable(invalid),
            Err(LaunchError::MissingProgram)
        );
    }
}

#[test]
fn foot_adapter_preserves_identity_title_and_literal_argv() {
    let mut command = terminal_command(
        Path::new("/usr/bin/foot"),
        Path::new("/etc/alcard/foot.ini"),
        "org.alcard.Hello.desktop",
        "Привет, мир / Hola",
        Path::new("/usr/bin/alcard-hello"),
    )
    .unwrap();
    command.args(["two words", "$(not-executed)", "--flag=value"]);
    assert_eq!(command.get_program(), OsStr::new("/usr/bin/foot"));
    assert_eq!(
        command.get_args().collect::<Vec<_>>(),
        [
            "--app-id",
            "org.alcard.Hello",
            "--title",
            "Привет, мир / Hola",
            "--config",
            "/etc/alcard/foot.ini",
            "--",
            "/usr/bin/alcard-hello",
            "two words",
            "$(not-executed)",
            "--flag=value",
        ]
        .map(OsStr::new)
    );
    for invalid in ["", ".desktop", "bad\nidentity"] {
        assert!(matches!(
            terminal_command(
                Path::new("/usr/bin/foot"),
                Path::new("/etc/alcard/foot.ini"),
                invalid,
                "Title",
                Path::new("/usr/bin/true")
            ),
            Err(LaunchError::InvalidDesktopIdentity)
        ));
    }
}

#[test]
fn foot_profile_requires_a_readable_absolute_regular_file() {
    let tmp = Tmp::new();
    let profile = tmp.0.join("foot.ini");
    fs::write(&profile, "[main]\nfont=monospace:size=8\n").unwrap();
    assert_eq!(foot_profile(Some(profile.as_os_str())), Ok(profile.clone()));
    assert_eq!(
        foot_profile(Some(tmp.0.join("missing.ini").as_os_str())),
        Err(LaunchError::MissingTerminalConfig)
    );
    for invalid in [
        OsStr::new(""),
        OsStr::new("relative.ini"),
        tmp.0.as_os_str(),
    ] {
        assert_eq!(
            foot_profile(Some(invalid)),
            Err(LaunchError::InvalidTerminalConfig)
        );
    }
    fs::OpenOptions::new()
        .write(true)
        .open(&profile)
        .unwrap()
        .set_len(MAX_TERMINAL_CONFIG_BYTES + 1)
        .unwrap();
    assert_eq!(
        foot_profile(Some(profile.as_os_str())),
        Err(LaunchError::InvalidTerminalConfig)
    );
}

#[test]
fn a_fifo_cannot_block_profile_validation() {
    use std::{ffi::CString, os::unix::ffi::OsStrExt};
    let tmp = Tmp::new();
    let fifo = tmp.0.join("profile-fifo");
    let cpath = CString::new(fifo.as_os_str().as_bytes()).unwrap();
    // SAFETY: cpath is a valid NUL-terminated path owned for this call.
    assert_eq!(unsafe { nix::libc::mkfifo(cpath.as_ptr(), 0o600) }, 0);
    assert_eq!(
        foot_profile(Some(fifo.as_os_str())),
        Err(LaunchError::InvalidTerminalConfig)
    );
}

#[test]
fn discovery_skips_fifo_entries_without_blocking_and_keeps_valid_entries() {
    use std::{ffi::CString, os::unix::ffi::OsStrExt};
    let tmp = Tmp::new();
    tmp.add(
        "valid.desktop",
        "[Desktop Entry]\nType=Application\nName=Valid\nExec=/usr/bin/true\n",
    );
    let fifo = tmp.0.join("applications/blocked.desktop");
    let cpath = CString::new(fifo.as_os_str().as_bytes()).unwrap();
    // SAFETY: cpath is a valid NUL-terminated path owned for this call.
    assert_eq!(unsafe { nix::libc::mkfifo(cpath.as_ptr(), 0o600) }, 0);
    let apps = discover_in(std::slice::from_ref(&tmp.0), Locale::En, &[]);
    assert_eq!(apps.len(), 1);
    assert_eq!(apps[0].id, "valid.desktop");
}

#[test]
fn discovery_budget_counts_all_entries_before_collecting_a_directory() {
    let tmp = Tmp::new();
    fs::write(tmp.0.join("one.txt"), "ordinary file").unwrap();
    fs::write(tmp.0.join("two.txt"), "ordinary file").unwrap();
    fs::create_dir(tmp.0.join("subdirectory")).unwrap();
    let mut visited = MAX_SCAN_ENTRIES - 2;
    let mut seen = HashSet::new();
    let mut apps = Vec::new();
    scan_tree(
        &tmp.0,
        &tmp.0,
        0,
        &mut visited,
        &mut seen,
        &mut apps,
        Locale::En,
        &[],
    );
    assert_eq!(visited, MAX_SCAN_ENTRIES);
    assert!(apps.is_empty());

    // A batch reserved at the exact boundary must still be processed.
    let valid = Tmp::new();
    valid.add(
        "last.desktop",
        "[Desktop Entry]\nType=Application\nName=Last\nExec=/usr/bin/true\n",
    );
    let root = valid.0.join("applications");
    visited = MAX_SCAN_ENTRIES - 1;
    scan_tree(
        &root,
        &root,
        0,
        &mut visited,
        &mut seen,
        &mut apps,
        Locale::En,
        &[],
    );
    assert_eq!(visited, MAX_SCAN_ENTRIES);
    assert_eq!(apps.len(), 1);
    assert_eq!(apps[0].id, "last.desktop");
}

#[test]
fn desktop_reads_accept_regular_symlinks_and_enforce_the_byte_boundary() {
    let tmp = Tmp::new();
    let regular = tmp.0.join("entry.desktop");
    fs::write(&regular, "x".repeat(MAX_DESKTOP_BYTES as usize)).unwrap();
    assert_eq!(
        read_desktop_file(&regular).unwrap().len(),
        MAX_DESKTOP_BYTES as usize
    );
    let linked = tmp.0.join("linked.desktop");
    std::os::unix::fs::symlink(&regular, &linked).unwrap();
    assert!(read_desktop_file(&linked).is_some());
    fs::OpenOptions::new()
        .append(true)
        .open(&regular)
        .unwrap()
        .set_len(MAX_DESKTOP_BYTES + 1)
        .unwrap();
    assert!(read_desktop_file(&regular).is_none());
    assert!(read_desktop_file(&linked).is_none());
    assert!(read_desktop_file(&tmp.0).is_none());
}

#[test]
fn dbus_entry_without_exec_is_deferred() {
    let t = Tmp::new();
    t.add(
        "dbus.desktop",
        "[Desktop Entry]\nType=Application\nName=DBus\nDBusActivatable=true\n",
    );
    t.add(
        "normal.desktop",
        "[Desktop Entry]\nType=Application\nName=Normal\nExec=true\n",
    );
    assert_eq!(
        discover_in(std::slice::from_ref(&t.0), Locale::En, &[]).len(),
        1
    );
}

#[test]
fn desktop_exec_preserves_empty_arguments_and_decodes_both_escape_layers() {
    let (_, argv) = expand_exec(
        r#"app --name "" "\\$HOME" "\\\\" %f %d"#,
        "name",
        None,
        Path::new("/tmp/example.desktop"),
    )
    .unwrap();
    assert_eq!(argv, ["--name", "", "$HOME", "\\"]);
}
