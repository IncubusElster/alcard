use super::*;
fn valid() -> ApplyRequest {
    ApplyRequest {
        plan: serde_json::from_str(
            r#"{
                "schema_version":1,"language":"ru",
                "account":{"username":"operator","display_name":"Operator","doas_enabled":true},
                "keyboard":{"layouts":["us","ru"]},"session":"console",
                "timezone":"Etc/UTC","ntp_enabled":false
            }"#,
        )
        .unwrap(),
        credentials: Credentials {
            user_password: "user-password".into(),
            user_confirm: "user-password".into(),
            root_password: String::new(),
            root_confirm: String::new(),
        },
    }
}
#[test]
fn root_detection_requires_locked_root_and_no_human_user() {
    // Pure logic is tested separately below via sample fixtures.
    assert!(human_user_present(
        "alice:x:1000:1000::/home/alice:/bin/ash\n"
    ));
    assert!(!human_user_present(
        "root:x:0:0::/root:/bin/ash\nnobody:x:65534:65534::/:/sbin/nologin\n"
    ));
}

#[test]
fn root_lock_postcondition_distinguishes_blank_locked_and_configured_passwords() {
    for field in ["!", "!!", "*", "!$6$existing"] {
        assert!(root_password_locked(&format!("root:{field}:0:0:99999:7:::\n")).unwrap());
    }
    for field in ["", "$6$existing"] {
        assert!(!root_password_locked(&format!("root:{field}:0:0:99999:7:::\n")).unwrap());
    }
    assert!(root_password_locked("someone:!:0:0:99999:7:::\n").is_err());
    assert!(root_password_locked("root:!:\nroot:!:\n").is_err());
}

#[test]
fn real_vm_apply_payload_validates_without_host_mutation() {
    let request: ApplyRequest = serde_json::from_str(
        r#"{
            "plan":{"schema_version":1,"language":"en",
                "account":{"username":"alcardci","display_name":"Alcard CI","doas_enabled":true},
                "keyboard":{"layouts":["us"]},"session":"graphics",
                "timezone":"Etc/UTC","ntp_enabled":false},
            "credentials":{"user_password":"0123456789abcdef01234567",
                "user_confirm":"0123456789abcdef01234567","root_password":"","root_confirm":""}
        }"#,
    )
    .unwrap();
    assert!(validate_request(&request).is_ok());
}

#[test]
fn validates_account_without_changing_host() {
    assert!(validate_request(&valid()).is_ok());
}
#[test]
fn rejects_bad_password_or_missing_admin() {
    let mut req = valid();
    req.credentials.user_confirm = "different".into();
    assert!(validate_request(&req).is_err());
    req.credentials.user_confirm = "user-password".into();
    req.plan.account.doas_enabled = false;
    assert!(validate_request(&req).is_err());
    req.credentials.root_password = "root-password".into();
    req.credentials.root_confirm = "root-password".into();
    assert!(validate_request(&req).is_ok());
}
#[test]
fn rejects_password_control_characters() {
    let mut req = valid();
    req.credentials.user_password.push('\n');
    req.credentials.user_confirm.push('\n');
    assert!(validate_request(&req).is_err());
}
#[test]
fn optional_keyboard_names_are_cardputer_specific() {
    use alcard_setup_protocol::Layout;
    assert_eq!(keyboard_name(Layout::Us), "us");
    assert_eq!(keyboard_name(Layout::Ru), "ru");
    assert_eq!(keyboard_name(Layout::Es), "es");
    assert_eq!(keyboard_name(Layout::De), "de");
    assert_eq!(valid().plan.keyboard.layouts.first(), Some(&Layout::Us));
}

#[test]
fn console_cleanup_targets_only_graphical_components() {
    assert_eq!(
        CONSOLE_PRUNE,
        [
            "alcard-desktop-shell",
            "alcard-shell",
            "alcard-polkit-agent",
            "alcard-lock",
            "alcard-desktop-minimal",
            "alcard-fonts-gui",
            "greetd-agreety",
            "greetd"
        ]
    );
    assert!(!CONSOLE_PRUNE.contains(&"alcard-hello"));
    assert!(!CONSOLE_PRUNE.contains(&"alcard-fonts-console"));
    assert!(!CONSOLE_PRUNE.contains(&"alpine-base"));
}

#[test]
fn all_three_session_variants_validate_without_host_mutations() {
    let mut req = valid();
    for session in alcard_setup_protocol::Session::ALL {
        req.plan.session = session;
        assert!(validate_request(&req).is_ok());
    }
    // Runtime ownership/package/PAM checks run in provision before any
    // mutation and again in finish_session; never provision on the host.
}

#[test]
fn firstboot_rejects_a_directory_as_a_marker() {
    let error = check_file(&std::env::temp_dir(), 0o600).unwrap_err();
    assert_eq!(
        crate::failure::cause(&error),
        "invalid_firstboot_file_permissions"
    );
}
