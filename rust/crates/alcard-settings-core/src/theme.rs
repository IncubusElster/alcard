//! Small declarative, data-only themes; no scripts, remote assets or runtime.
use std::{
    fs,
    io::{self, Read},
    os::unix::fs::OpenOptionsExt,
    path::Path,
};

const MAX_THEME_BYTES: u64 = 16 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IconStyle {
    Solid,
    Outline,
}

impl IconStyle {
    pub fn parse(value: &str) -> Result<Self, String> {
        match value {
            "solid" => Ok(Self::Solid),
            "outline" => Ok(Self::Outline),
            _ => Err("icon_style must be solid or outline".into()),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Theme {
    /// Machine identity is independent of a translated/display theme name.
    pub id: String,
    pub name: String,
    pub icon_style: IconStyle,
    pub background: u32,
    pub surface: u32,
    pub selected: u32,
    pub text: u32,
    pub muted: u32,
    pub accent: u32,
    pub border: u32,
    pub disabled: u32,
    pub success: u32,
    pub warning: u32,
    pub error: u32,
}

impl Theme {
    pub fn classic() -> Self {
        Self::parse(include_str!("../themes/classic.toml")).expect("bundled classic theme")
    }

    pub fn emerald() -> Self {
        Self::parse(include_str!("../themes/emerald.toml")).expect("bundled emerald theme")
    }

    pub fn builtin(id: &str) -> Option<Self> {
        match id {
            "classic" => Some(Self::classic()),
            "emerald" => Some(Self::emerald()),
            _ => None,
        }
    }

    pub fn from_file(path: &Path) -> Result<Self, String> {
        let data = read_bounded_text(path, MAX_THEME_BYTES)
            .map_err(|error| format!("could not read theme: {:?}", error.kind()))?;
        Self::parse(&data)
    }

    pub fn parse(input: &str) -> Result<Self, String> {
        if input.len() as u64 > MAX_THEME_BYTES {
            return Err("theme exceeds 16 KiB".into());
        }
        let doc: toml::Value = toml::from_str(input).map_err(|_| "invalid theme TOML")?;
        // Unversioned S2.7 themes retain their existing interpretation.
        if doc
            .get("schema_version")
            .is_some_and(|version| version.as_integer() != Some(1))
        {
            return Err("unsupported theme schema_version; expected 1".into());
        }
        let theme = doc
            .get("theme")
            .and_then(toml::Value::as_table)
            .ok_or("missing [theme]")?;
        let palette = doc
            .get("palette")
            .and_then(toml::Value::as_table)
            .ok_or("missing [palette]")?;
        let name = theme
            .get("name")
            .and_then(toml::Value::as_str)
            .ok_or("missing theme name")?;
        if name.trim().is_empty() || name.chars().count() > 48 || name.chars().any(char::is_control)
        {
            return Err("invalid theme name".into());
        }
        let id = match theme.get("id") {
            None => "custom",
            Some(value) => value
                .as_str()
                .filter(|id| valid_id(id))
                .ok_or("invalid theme id")?,
        };
        let icon_style = IconStyle::parse(
            theme
                .get("icon_style")
                .and_then(toml::Value::as_str)
                .ok_or("missing theme.icon_style")?,
        )?;
        let color = |key: &str| -> Result<u32, String> {
            let s = palette
                .get(key)
                .and_then(toml::Value::as_str)
                .ok_or_else(|| format!("missing palette.{key}"))?;
            parse_color(s).map_err(|_| format!("invalid color palette.{key}"))
        };
        let optional = |key: &str, default| {
            if palette.contains_key(key) {
                color(key)
            } else {
                Ok(default)
            }
        };
        Ok(Self {
            id: id.into(),
            name: name.to_string(),
            icon_style,
            background: color("background")?,
            surface: color("surface")?,
            selected: color("selected")?,
            text: color("text")?,
            muted: color("muted")?,
            accent: color("accent")?,
            border: color("border")?,
            disabled: color("disabled")?,
            success: optional("success", 0x85D59C)?,
            warning: optional("warning", 0xF5C56B)?,
            error: optional("error", 0xF08B8B)?,
        })
    }
}

fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 64
        && id.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'-' | b'_' | b'.')
        })
        && id
            .bytes()
            .next()
            .is_some_and(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit())
}

pub fn parse_color(value: &str) -> Result<u32, String> {
    if value.len() != 7
        || !value.starts_with('#')
        || !value[1..].bytes().all(|byte| byte.is_ascii_hexdigit())
    {
        return Err("color must be #RRGGBB".into());
    }
    u32::from_str_radix(&value[1..], 16).map_err(|_| "color must be #RRGGBB".into())
}

/// Open first, inspect that descriptor, then bound the actual read. Symlinks to
/// regular local files are accepted; FIFOs/devices can never become a read.
pub(crate) fn read_bounded_text(path: &Path, maximum: u64) -> io::Result<String> {
    let file = fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NONBLOCK)
        .open(path)?;
    let metadata = file.metadata()?;
    if !metadata.is_file() || metadata.len() > maximum {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "not a bounded regular file",
        ));
    }
    let mut text = String::new();
    file.take(maximum + 1).read_to_string(&mut text)?;
    if text.len() as u64 > maximum {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "file exceeds byte limit",
        ));
    }
    Ok(text)
}

#[cfg(test)]
#[path = "unit_tests/theme.rs"]
mod tests;
