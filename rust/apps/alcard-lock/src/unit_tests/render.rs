use super::*;
use crate::protocol::Event;
fn prompt(value: &str) -> Form {
    let mut form = Form::default();
    form.ready("alcard");
    form.locked();
    form.begin();
    form.receive(
        Event::Challenge {
            attempt: 1,
            serial: 1,
            prompt: "Password:".into(),
            echo: false,
        },
        Language::En,
    );
    form.insert(value);
    form
}
#[test]
fn plaintext_never_changes_the_rendered_password_pixels() {
    let a = prompt("45678Яñ");
    let b = prompt("abcdefg");
    let mut first = Pixmap::new(WIDTH, HEIGHT).unwrap();
    let mut second = Pixmap::new(WIDTH, HEIGHT).unwrap();
    paint(
        &mut first,
        &a,
        Language::En,
        &Theme::classic(),
        "12:34",
        "2026-11-19",
    );
    paint(
        &mut second,
        &b,
        Language::En,
        &Theme::classic(),
        "12:34",
        "2026-11-19",
    );
    assert_eq!(first.data(), second.data());
}
#[test]
fn every_locker_phase_is_opaque_and_footer_bounded_in_all_locales() {
    for language in [Language::Ru, Language::En, Language::Es] {
        for theme in [Theme::classic(), Theme::emerald()] {
            for phase in [
                Phase::Securing,
                Phase::Idle,
                Phase::Prompt,
                Phase::Waiting,
                Phase::Unavailable,
                Phase::Unlocking,
            ] {
                let mut form = prompt("45678");
                form.owner = "Длинное имя / Usuario con nombre largo ".repeat(10);
                form.prompt = "Long challenge / Длинный запрос / Desafío largo ".repeat(15);
                form.notice = "Сообщение о состоянии / Estado de la sesión ".repeat(20);
                form.phase = phase;
                form.caps_lock = true;
                let mut image = Pixmap::new(WIDTH, HEIGHT).unwrap();
                paint(&mut image, &form, language, &theme, "12:34", "2026-11-19");
                assert!(image.pixels().iter().all(|pixel| pixel.alpha() == 255));
                let mut footer_image = Pixmap::new(WIDTH, HEIGHT).unwrap();
                footer(&mut footer_image, &form, language, &theme);
                let offset = FOOTER as usize * WIDTH as usize * 4;
                assert_eq!(&image.data()[offset..], &footer_image.data()[offset..]);
            }
        }
    }
}
