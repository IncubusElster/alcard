use super::*;
use crate::protocol::{Event, Identity};
#[test]
fn same_length_passwords_produce_identical_frames_in_every_locale() {
    for language in [Language::En, Language::Ru, Language::Es] {
        let make = |password: &str| {
            let mut form = Form::new(
                1,
                "org.test.action".into(),
                "Public reason".into(),
                vec![Identity {
                    index: 0,
                    label: "operator".into(),
                }],
            );
            form.auto_select();
            form.receive(
                Event::Challenge {
                    id: 1,
                    serial: 1,
                    prompt: "Password:".into(),
                    echo: false,
                },
                language,
            );
            form.insert(password);
            let mut image = Pixmap::new(320, 170).unwrap();
            paint(&mut image, &form, language, &Theme::classic());
            image
        };
        assert_eq!(make("secretA").data(), make("secretB").data());
    }
}
#[test]
fn details_and_long_prompts_are_bounded_in_both_themes() {
    for theme in [Theme::classic(), Theme::emerald()] {
        let mut form = Form::new(1, "action".into(), "Я".repeat(2048), vec![]);
        form.details = true;
        form.scroll = 40;
        let mut image = Pixmap::new(320, 170).unwrap();
        paint(&mut image, &form, Language::Ru, &theme);
        assert_eq!(image.data().len(), 320 * 170 * 4);
        assert!(image.pixels().iter().all(|p| p.alpha() == 255));
    }
}
