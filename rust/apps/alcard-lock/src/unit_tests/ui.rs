use super::*;
fn locked_form() -> Form {
    let mut form = Form::default();
    form.ready("alcard");
    form.locked();
    form
}
fn prompt(form: &mut Form, attempt: u64, serial: u64) {
    form.receive(
        Event::Challenge {
            attempt,
            serial,
            prompt: "Password:".into(),
            echo: false,
        },
        Language::En,
    );
}
#[test]
fn authentication_cannot_begin_or_succeed_before_compositor_lock() {
    let mut form = Form::default();
    form.ready("alcard");
    assert!(form.begin().is_none());
    assert!(form
        .receive(
            Event::Finished {
                attempt: 1,
                result: Ok(())
            },
            Language::En
        )
        .is_none());
    assert_eq!(form.phase, Phase::Securing);
    form.locked();
    assert!(matches!(form.begin(), Some(Command::Begin { attempt: 1 })));
}
#[test]
fn digits_four_to_eight_and_unicode_remain_password_text() {
    let mut form = locked_form();
    form.begin();
    prompt(&mut form, 1, 1);
    form.insert("45678Яñ");
    assert_eq!(form.masked_len(), 7);
    let Some(Command::Respond {
        attempt,
        serial,
        secret,
    }) = form.submit()
    else {
        panic!("response")
    };
    assert_eq!((attempt, serial, secret.as_str()), (1, 1, "45678Яñ"));
    assert_eq!(form.masked_len(), 0);
    assert!(form.submit().is_none());
}
#[test]
fn escape_and_stale_success_never_unlock_a_new_attempt() {
    let mut form = locked_form();
    form.begin();
    prompt(&mut form, 1, 1);
    form.insert("secret");
    assert!(matches!(
        form.cancel(),
        Some(Command::Cancel { attempt: 1 })
    ));
    assert_eq!(form.masked_len(), 0);
    assert_eq!(form.phase, Phase::Idle);
    form.begin();
    assert!(form
        .receive(
            Event::Finished {
                attempt: 1,
                result: Ok(())
            },
            Language::En
        )
        .is_none());
    assert_eq!(form.attempt(), Some(2));
    prompt(&mut form, 2, 1);
    assert!(form
        .receive(
            Event::Finished {
                attempt: 2,
                result: Ok(())
            },
            Language::En
        )
        .is_none());
    assert_eq!(form.phase, Phase::Prompt);
    form.submit();
    assert_eq!(
        form.receive(
            Event::Finished {
                attempt: 2,
                result: Ok(())
            },
            Language::En
        ),
        Some(2)
    );
    assert!(form.cancel().is_none());
    assert_eq!(form.phase, Phase::Unlocking);
}
#[test]
fn sequential_challenges_clear_responses_and_failure_keeps_the_lock() {
    let mut form = locked_form();
    form.begin();
    prompt(&mut form, 1, 1);
    form.insert("old secret");
    form.submit();
    prompt(&mut form, 1, 2);
    assert_eq!(form.masked_len(), 0);
    form.insert("otp");
    form.receive(
        Event::Finished {
            attempt: 1,
            result: Err(Failure::Denied),
        },
        Language::Ru,
    );
    assert_eq!(form.phase, Phase::Idle);
    assert_eq!(form.masked_len(), 0);
    assert!(form.error);
    assert!(matches!(form.begin(), Some(Command::Begin { attempt: 2 })));
}
