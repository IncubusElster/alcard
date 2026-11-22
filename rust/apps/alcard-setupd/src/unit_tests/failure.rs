use super::*;

#[test]
fn opaque_errors_and_unapproved_text_never_reach_diagnostics() {
    let opaque = io::Error::other("credential-marker /private/path user data");
    let wrapped = at::<()>(Stage::UserPassword, Err(opaque)).unwrap_err();
    assert_eq!(cause(&wrapped), "system_io");
    assert_eq!(stage(&wrapped), "user_password");
    assert!(!wrapped.to_string().contains("credential-marker"));
    assert_eq!(cause(&invalid("credential-marker")), "unclassified_failure");
    // A foreign error cannot spoof an approved cause with its message.
    assert_eq!(cause(&io::Error::other("invalid_plan")), "system_io");
}

#[test]
fn static_cause_and_stage_survive_without_serializing_error_details() {
    let error = at::<()>(
        Stage::AdminPreflight,
        Err(invalid("invalid_admin_policy_directory")),
    )
    .unwrap_err();
    assert_eq!(cause(&error), "invalid_admin_policy_directory");
    assert_eq!(stage(&error), "admin_preflight");
    assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
}
