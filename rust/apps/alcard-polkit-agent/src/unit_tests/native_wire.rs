use super::*;
#[test]
fn authority_registration_uses_string_path_not_object_path() {
    let subject = (
        "unix-session",
        HashMap::from([("session-id".to_string(), "session".to_variant())]),
    );
    assert_eq!(
        (subject.clone(), "C.UTF-8", AGENT_PATH)
            .to_variant()
            .type_()
            .as_str(),
        "((sa{sv})ss)"
    );
    assert_eq!(
        (subject, AGENT_PATH).to_variant().type_().as_str(),
        "((sa{sv})s)"
    );
}
