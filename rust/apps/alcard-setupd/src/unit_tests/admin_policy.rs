use super::*;

#[test]
fn native_doas_probe_accepts_only_the_password_required_policy_path() {
    assert!(password_route(Some(1), b"doas: Authentication required\n"));
    for (status, output) in [
        (Some(0), &b""[..]), // A later foreign nopass rule is not our route.
        (Some(1), &b"doas: Operation not permitted\n"[..]),
        (Some(1), &b"doas: syntax error\n"[..]),
        (Some(1), &b"doas: not installed setuid\n"[..]),
        (Some(1), &b"doas: Authentication required\nextra"[..]),
        (Some(2), &b"doas: Authentication required\n"[..]),
        (None, &b"doas: Authentication required\n"[..]),
    ] {
        assert!(!password_route(status, output));
    }
}

#[test]
fn distro_doas_config_accepts_stock_modes_but_rejects_unsafe_files() {
    for mode in [0o100600, 0o100640, 0o100644] {
        assert!(trusted_doas_config(true, 0, mode));
    }
    assert!(!trusted_doas_config(false, 0, 0o120644));
    assert!(!trusted_doas_config(true, 1000, 0o100600));
    assert!(!trusted_doas_config(true, 0, 0o100660));
    assert!(!trusted_doas_config(true, 0, 0o100646));
}

#[test]
fn identities_preserve_explicit_doas_and_root_choices() {
    assert_eq!(
        identities("operator", true, false).unwrap(),
        ["unix-user:operator"]
    );
    assert_eq!(
        identities("operator", true, true).unwrap(),
        ["unix-user:operator", "unix-user:root"]
    );
    assert_eq!(
        identities("operator", false, true).unwrap(),
        ["unix-user:root"]
    );
    assert!(identities("operator", false, false).is_err());
    assert!(identities("operator\";", true, false).is_err());
}

#[test]
fn polkit_only_selects_identities_for_own_active_local_session() {
    let rule = polkit_rule("operator", true, false).unwrap();
    assert!(rule.contains("polkit.addAdminRule("));
    assert!(rule.contains("subject.user === \"operator\" && subject.local && subject.active"));
    assert!(rule.contains("return [\"unix-user:operator\"]"));
    assert!(!rule.contains("addRule("));
    assert!(!rule.contains("Result.YES"));
    assert!(!rule.contains("unix-group:"));
}

#[test]
fn retry_can_revoke_only_the_same_accounts_managed_rules() {
    let doas = doas_rule("operator");
    let known = [doas.clone()];
    assert_eq!(
        policy_change(Some(&doas), None, &known).unwrap(),
        Change::Remove
    );
    assert!(policy_change(Some(&doas_rule("someone_else")), None, &known).is_err());
    assert!(policy_change(Some("permit nopass operator as root\n"), None, &known).is_err());

    let user = polkit_rule("operator", true, false).unwrap();
    let root = polkit_rule("operator", false, true).unwrap();
    let both = polkit_rule("operator", true, true).unwrap();
    let known = [user.clone(), root.clone(), both.clone()];
    assert_eq!(
        policy_change(Some(&both), Some(&root), &known).unwrap(),
        Change::Replace
    );
    assert_eq!(
        policy_change(Some(&user), Some(&user), &known).unwrap(),
        Change::Keep
    );
    // Console removes a policy from an interrupted graphical apply.
    assert_eq!(
        policy_change(Some(&user), None, &known).unwrap(),
        Change::Remove
    );
    assert_eq!(policy_change(None, None, &known).unwrap(), Change::Keep);
    assert!(policy_change(
        Some(&polkit_rule("someone_else", true, false).unwrap()),
        Some(&user),
        &known
    )
    .is_err());
}

#[test]
fn managed_file_metadata_rejects_links_other_owners_and_changed_permissions() {
    assert!(safe_file(true, 0, 0o100600, 0o600));
    assert!(safe_file(true, 0, 0o100644, 0o644));
    assert!(!safe_file(false, 0, 0o120644, 0o644));
    assert!(!safe_file(true, 1000, 0o100644, 0o644));
    assert!(!safe_file(true, 0, 0o100664, 0o644));
    assert!(!safe_file(true, 0, 0o104644, 0o644));
}

#[test]
fn polkit_can_read_stock_group_directory_without_widening_permissions() {
    assert!(polkit_directory_readable(0o750, 28, Some(28), true));
    assert!(polkit_directory_readable(0o755, 0, None, true));
    assert!(!polkit_directory_readable(0o750, 0, Some(28), true));
    assert!(!polkit_directory_readable(0o700, 28, Some(28), true));
    assert!(!polkit_directory_readable(0o710, 28, Some(28), true));
    assert!(polkit_directory_readable(0o710, 28, Some(28), false));
    assert!(!polkit_directory_readable(0o705, 28, Some(28), true));
}
