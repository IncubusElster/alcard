use super::*;

#[test]
fn automatic_language_uses_message_locale_precedence_and_skips_empty_values() {
    assert_eq!(
        environment_locale([
            Some("ru_RU.UTF-8"),
            Some("es_ES.UTF-8"),
            Some("en_US.UTF-8")
        ]),
        Locale::Ru
    );
    assert_eq!(
        environment_locale([Some(""), Some("es_UY.UTF-8"), Some("ru_RU.UTF-8")]),
        Locale::Es
    );
    assert_eq!(
        environment_locale([None, Some(""), Some("ru_RU.UTF-8")]),
        Locale::Ru
    );
}

#[test]
fn automatic_language_normalizes_case_and_does_not_skip_unknown_overrides() {
    assert_eq!(
        environment_locale([Some("RU_ru.UTF-8"), None, None]),
        Locale::Ru
    );
    assert_eq!(
        environment_locale([None, Some("ES_uy.UTF-8"), None]),
        Locale::Es
    );
    for value in ["C", "POSIX", "de_DE.UTF-8", "я"] {
        assert_eq!(
            environment_locale([Some(value), Some("ru_RU.UTF-8"), Some("es_ES.UTF-8")]),
            Locale::En
        );
    }
    assert_eq!(environment_locale([None, None, None]), Locale::En);
}

#[test]
fn system_codes_remain_strict_and_language_selection_preserves_borrowed_text() {
    for locale in Locale::ALL {
        assert_eq!(Locale::from_code(locale.code()), Some(locale));
    }
    for value in ["", "auto", "RU", "ru_RU.UTF-8", "de"] {
        assert_eq!(Locale::from_code(value), None);
    }
    let en = String::from("English");
    let ru = String::from("Русский");
    let es = String::from("Español");
    assert_eq!(Locale::En.text(&en, &ru, &es), en);
    assert_eq!(Locale::Ru.text(&en, &ru, &es), ru);
    assert_eq!(Locale::Es.text(&en, &ru, &es), es);
}
