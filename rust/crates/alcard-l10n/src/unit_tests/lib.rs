use super::*;
#[test]
fn all_catalogs_have_identical_keys_and_nonblank_values() {
    let en = catalog(Locale::En);
    for locale in Locale::ALL {
        let other = catalog(locale);
        assert_eq!(
            en.keys().collect::<Vec<_>>(),
            other.keys().collect::<Vec<_>>()
        );
        assert!(other.values().all(|v| !v.trim().is_empty()));
    }
}
#[test]
fn translations_change_with_locale() {
    assert_ne!(tr(Locale::En, "action.next"), tr(Locale::Ru, "action.next"));
    assert_ne!(tr(Locale::En, "action.next"), tr(Locale::Es, "action.next"));
}
