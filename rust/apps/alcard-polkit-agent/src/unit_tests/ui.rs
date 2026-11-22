use super::*;
fn form(names: &[&str]) -> Form {
    Form::new(
        7,
        "org.test.action".into(),
        "Reason".into(),
        names
            .iter()
            .enumerate()
            .map(|(index, s)| Identity {
                index,
                label: s.to_string(),
            })
            .collect(),
    )
}
fn prompt(f: &mut Form, serial: u64, echo: bool) {
    f.receive(
        Event::Challenge {
            id: 7,
            serial,
            prompt: "Password:".into(),
            echo,
        },
        Language::En,
    );
}
#[test]
fn multiple_identities_require_explicit_choice_and_preserve_authority_index() {
    let mut f = form(&["root", "operator"]);
    assert!(f.auto_select().is_none());
    assert!(f.enter().is_none());
    f.insert("not accepted");
    assert_eq!(f.masked_len(), 0);
    f.navigate(true);
    f.navigate(true);
    assert!(matches!(f.enter(), Some(Command::Choose { index: 1, .. })));
    assert!(f.enter().is_none());
}
#[test]
fn all_digits_are_response_text_and_submission_is_one_shot() {
    let mut f = form(&["operator"]);
    f.auto_select();
    prompt(&mut f, 1, false);
    f.insert("45678Яñ");
    f.backspace();
    assert_eq!(f.masked_len(), 6);
    let Some(Command::Respond {
        secret, serial: 1, ..
    }) = f.enter()
    else {
        panic!("response missing")
    };
    assert_eq!(secret.as_str(), "45678Я");
    assert_eq!(f.masked_len(), 0);
    assert!(f.enter().is_none());
    f.insert("late");
    assert_eq!(f.masked_len(), 0);
}
#[test]
fn sequential_pam_challenges_and_echo_on_never_reuse_old_response() {
    let mut f = form(&["operator"]);
    f.auto_select();
    prompt(&mut f, 1, false);
    f.insert("old");
    f.enter();
    prompt(&mut f, 2, true);
    assert_eq!(f.masked_len(), 0);
    f.insert("123456");
    assert_eq!(f.masked_len(), 6);
    prompt(&mut f, 1, false); // stale event cannot wipe or reopen the field
    assert_eq!(f.masked_len(), 6);
    assert!(matches!(
        f.enter(),
        Some(Command::Respond { serial: 2, .. })
    ));
}
#[test]
fn cancellation_and_retry_require_native_transition() {
    let mut f = form(&["operator"]);
    f.auto_select();
    prompt(&mut f, 1, false);
    f.insert("wrong");
    f.enter();
    assert!(f.enter().is_none());
    f.receive(Event::Retry { id: 7 }, Language::En);
    assert!(matches!(f.enter(), Some(Command::Retry { id: 7 })));
    prompt(&mut f, 2, false);
    f.insert("secret");
    f.cancel();
    assert_eq!(f.masked_len(), 0);
    assert!(f.enter().is_none());
}
#[test]
fn details_never_contain_entered_secret_and_do_not_submit() {
    let mut f = form(&["operator"]);
    f.auto_select();
    prompt(&mut f, 1, false);
    f.insert("SensitiveTestValue");
    f.details = true;
    assert!(!f.detail_lines().join(" ").contains("SensitiveTestValue"));
    assert!(f.enter().is_none());
    assert_eq!(f.masked_len(), 18);
}
