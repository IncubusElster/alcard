use super::*;
fn sample() -> SetupPlan {
    SetupPlan {
        schema_version: 1,
        language: Language::En,
        account: AccountPlan {
            username: "operator".into(),
            display_name: "Operator".into(),
            doas_enabled: true,
        },
        keyboard: KeyboardPlan {
            layouts: vec![Layout::Us, Layout::Ru],
        },
        session: Session::Console,
        timezone: "Etc/UTC".into(),
        ntp_enabled: false,
    }
}

#[test]
fn validates_supported_offline_plan() {
    assert!(validate(&sample()).is_ok());
}

#[test]
fn rejects_unsafe_gecos() {
    for bad in ["bad:name", "broken\nname", "zero\0byte"] {
        let mut plan = sample();
        plan.account.display_name = bad.into();
        assert_eq!(validate(&plan), Err(CheckError::DisplayNameTooLong));
    }
}

#[test]
fn rejects_noncanonical_usernames() {
    for name in [
        "",
        "User",
        "2user",
        "-user",
        "test name",
        "пользователь",
        "a/b",
        "a@b",
    ] {
        let mut plan = sample();
        plan.account.username = name.into();
        assert_eq!(validate(&plan), Err(CheckError::InvalidUsername), "{name}");
    }
    assert!(is_valid_username("_user-42"));
}

#[test]
fn rejects_duplicate_and_missing_layouts() {
    let mut plan = sample();
    plan.keyboard.layouts = vec![];
    assert_eq!(validate(&plan), Err(CheckError::MissingKeyboardLayout));
    plan.keyboard.layouts = vec![Layout::Us, Layout::Us];
    assert_eq!(validate(&plan), Err(CheckError::DuplicateKeyboardLayout));
}

#[test]
fn requires_original_us_first_with_optional_languages() {
    let mut plan = sample();
    plan.keyboard.layouts = vec![Layout::Ru, Layout::Us];
    assert_eq!(validate(&plan), Err(CheckError::UsKeyboardRequired));
    plan.keyboard.layouts = vec![Layout::Us, Layout::De, Layout::Es, Layout::Ru];
    assert!(validate(&plan).is_ok());
    let raw = serde_json::to_string(&plan).unwrap();
    assert!(!raw.contains("br"));
    assert!(!raw.contains("fr"));
}

#[test]
fn requires_verified_administration_for_every_session() {
    let mut plan = sample();
    plan.account.doas_enabled = false;
    assert_eq!(
        validate(&plan),
        Err(CheckError::AdminAuthorizationUnverified)
    );
    plan.account.doas_enabled = true;
    for session in Session::ALL {
        plan.session = session;
        assert!(validate(&plan).is_ok()); // root backend checks availability
    }
}

#[test]
fn legacy_graphical_plans_keep_the_foot_session() {
    let legacy: Session = serde_json::from_str("\"graphical\"").unwrap();
    assert_eq!(legacy, Session::GraphicsMinimal);
    assert_eq!(legacy.installed_name(), "labwc");
    assert_eq!(
        serde_json::to_string(&legacy).unwrap(),
        "\"graphics_minimal\""
    );
    for (name, expected) in [
        ("console", Session::Console),
        ("graphics", Session::Graphics),
        ("graphics_minimal", Session::GraphicsMinimal),
    ] {
        let parsed: Session = serde_json::from_value(serde_json::json!(name)).unwrap();
        assert_eq!(parsed, expected);
    }
    assert!(serde_json::from_str::<Session>("\"shell /tmp/helper\"").is_err());
}

#[test]
fn does_not_accept_secret_fields_or_unknown_structures() {
    let mut value = serde_json::to_value(sample()).unwrap();
    value["account"]["root_password"] = serde_json::json!("leak");
    assert!(serde_json::from_value::<SetupPlan>(value).is_err());
}

#[test]
fn firstboot_supports_root_admin_when_password_is_provided() {
    let mut plan = sample();
    plan.account.doas_enabled = false;
    assert_eq!(
        validate_for_firstboot(&plan, false),
        Err(CheckError::AdminAuthorizationUnverified)
    );
    assert!(validate_for_firstboot(&plan, true).is_ok());
}

#[test]
fn timezone_rejects_traversal() {
    for bad in [
        "",
        ".",
        "..",
        "../etc/passwd",
        "/UTC",
        "Area//City",
        "Zone/../../etc",
        "bad\\zone",
    ] {
        assert!(!is_valid_timezone(bad));
    }
    assert!(is_valid_timezone("America/Montevideo"));
    assert!(is_valid_timezone("Etc/UTC"));
}

#[test]
fn schema_must_be_versioned() {
    let mut plan = sample();
    plan.schema_version = 2;
    assert_eq!(validate(&plan), Err(CheckError::UnsupportedSchema));
}
