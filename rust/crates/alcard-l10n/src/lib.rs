//! One shared source for every displayed UI string. No translated text in Rust.
use std::{collections::BTreeMap, sync::OnceLock};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Locale {
    En,
    Ru,
    Es,
}

impl Locale {
    pub const ALL: [Locale; 3] = [Locale::En, Locale::Ru, Locale::Es];
    pub const fn code(self) -> &'static str {
        match self {
            Self::En => "en",
            Self::Ru => "ru",
            Self::Es => "es",
        }
    }
    pub const fn name_key(self) -> &'static str {
        match self {
            Self::En => "language.en",
            Self::Ru => "language.ru",
            Self::Es => "language.es",
        }
    }
}

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
