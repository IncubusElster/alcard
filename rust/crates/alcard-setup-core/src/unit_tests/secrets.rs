use super::*;

#[test]
fn user_password_is_required_and_confirmed() {
    let mut p = Passwords::default();
    assert!(matches!(p.validate(true), Err(Invalid::UserMissing)));
    p.set(Slot::UserPassword, Passwords::new_password("correct horse"));
    assert!(matches!(p.validate(true), Err(Invalid::UserMismatch)));
    p.set(Slot::UserConfirm, Passwords::new_password("correct horse"));
    assert!(p.validate(true).is_ok());
}

#[test]
fn root_password_is_optional_when_doas_on() {
    let mut p = Passwords::default();
    p.set(Slot::UserPassword, Passwords::new_password("eight_chars"));
    p.set(Slot::UserConfirm, Passwords::new_password("eight_chars"));
    assert!(p.validate(true).is_ok());
    assert!(matches!(p.validate(false), Err(Invalid::NoAdministrator)));
    p.set(Slot::RootPassword, Passwords::new_password("root_secret"));
    p.set(Slot::RootConfirm, Passwords::new_password("root_secret"));
    assert!(p.validate(false).is_ok());
}

#[test]
fn replacing_secret_clears_confirmation() {
    let mut p = Passwords::default();
    p.set(
        Slot::UserPassword,
        Passwords::new_password("first_password"),
    );
    p.set(Slot::UserConfirm, Passwords::new_password("first_password"));
    p.set(Slot::UserPassword, Passwords::new_password("new_password"));
    assert_eq!(p.masked(Slot::UserConfirm), "");
    assert_eq!(
        p.masked(Slot::UserPassword),
        "*".repeat("new_password".chars().count())
    );
}

#[test]
fn confirmation_without_root_password_is_rejected() {
    let mut p = Passwords::default();
    p.set(Slot::UserPassword, Passwords::new_password("user_secret"));
    p.set(Slot::UserConfirm, Passwords::new_password("user_secret"));
    p.set(Slot::RootConfirm, Passwords::new_password("root_secret"));
    assert!(matches!(p.validate(true), Err(Invalid::RootMismatch)));
}

#[test]
fn reopening_saved_password_prefills_edit_buffer_and_keeps_confirmation() {
    for (password, confirmation) in [
        (Slot::UserPassword, Slot::UserConfirm),
        (Slot::RootPassword, Slot::RootConfirm),
    ] {
        let mut p = Passwords::default();
        p.set(password, Passwords::new_password("old-secret"));
        p.set(confirmation, Passwords::new_password("old-secret"));
        let copy = p.copy_for_edit(password);
        assert_eq!(copy.as_str(), "old-secret");
        p.set(password, copy); // unchanged Enter is a no-op for confirmation
        assert_eq!(p.masked(confirmation), "*".repeat(10));
        let mut edited = p.copy_for_edit(password);
        edited.pop();
        edited.push('X');
        p.set(password, edited);
        assert_eq!(p.masked(confirmation), "");
    }
}

#[test]
fn optional_root_must_still_match_if_set() {
    let mut p = Passwords::default();
    p.set(Slot::UserPassword, Passwords::new_password("user_secret"));
    p.set(Slot::UserConfirm, Passwords::new_password("user_secret"));
    p.set(Slot::RootPassword, Passwords::new_password("root_secret"));
    assert!(matches!(p.validate(true), Err(Invalid::RootMismatch)));
    p.set(Slot::RootConfirm, Passwords::new_password("root_secret"));
    assert!(p.validate(true).is_ok());
}
