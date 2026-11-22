//! Shared UI language identity and embedded Settings/Hello catalogs.
use std::{collections::BTreeMap, sync::OnceLock};

mod locale;
pub use locale::Locale;

type Catalog = BTreeMap<String, String>;
static EN: OnceLock<Catalog> = OnceLock::new();
static RU: OnceLock<Catalog> = OnceLock::new();
static ES: OnceLock<Catalog> = OnceLock::new();

fn catalog(locale: Locale) -> &'static Catalog {
    let (lock, data) = match locale {
        Locale::En => (&EN, include_str!("../locales/en.json")),
        Locale::Ru => (&RU, include_str!("../locales/ru.json")),
        Locale::Es => (&ES, include_str!("../locales/es.json")),
    };
    lock.get_or_init(|| serde_json::from_str(data).expect("invalid Alcard localization catalog"))
}

/// Localization failures are errors, never silently show raw keys to users.
pub fn tr(locale: Locale, key: &str) -> &'static str {
    catalog(locale)
        .get(key)
        .map(String::as_str)
        .unwrap_or_else(|| panic!("missing localization key {key} in {}", locale.code()))
}

#[cfg(test)]
#[path = "unit_tests/lib.rs"]
mod tests;
