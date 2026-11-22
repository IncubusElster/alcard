//! Language selection shared by console and graphical frontends.

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum Locale {
    #[default]
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

    /// Exact codes accepted by the system-language setting.
    pub fn from_code(value: &str) -> Option<Self> {
        match value {
            "en" => Some(Self::En),
            "ru" => Some(Self::Ru),
            "es" => Some(Self::Es),
            _ => None,
        }
    }

    /// Automatic UI language; explicit CLI and system settings are resolved first.
    pub fn detect() -> Self {
        let values = ["LC_ALL", "LC_MESSAGES", "LANG"].map(|key| std::env::var(key).ok());
        environment_locale(values.each_ref().map(|value| value.as_deref()))
    }

    /// Select an existing feature-specific label without copying or allocating it.
    pub const fn text<'a>(self, en: &'a str, ru: &'a str, es: &'a str) -> &'a str {
        match self {
            Self::En => en,
            Self::Ru => ru,
            Self::Es => es,
        }
    }
}

fn environment_locale(values: [Option<&str>; 3]) -> Locale {
    let value = values
        .into_iter()
        .flatten()
        .find(|value| !value.is_empty())
        .unwrap_or_default();
    let prefix = value.get(..2).unwrap_or_default();
    if prefix.eq_ignore_ascii_case("ru") {
        Locale::Ru
    } else if prefix.eq_ignore_ascii_case("es") {
        Locale::Es
    } else {
        Locale::En
    }
}

#[cfg(test)]
#[path = "unit_tests/locale.rs"]
mod tests;
