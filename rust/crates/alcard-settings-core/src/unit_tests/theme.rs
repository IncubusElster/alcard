use super::*;
#[test]
fn both_bundled_themes_parse_and_swap_icons_and_colors() {
    let a = Theme::classic();
    let b = Theme::emerald();
    assert_ne!(a.accent, b.accent);
    assert_ne!(a.icon_style, b.icon_style);
    assert_eq!(a.id, "classic");
    assert_eq!(b.id, "emerald");
}
#[test]
fn malformed_theme_fails_without_partial_application() {
    assert!(Theme::parse("[theme]\nname='oops'\nicon_style='solid'").is_err());
    assert!(
        Theme::parse(&include_str!("../../themes/classic.toml").replace("#75DCEB", "red")).is_err()
    );
}

#[test]
fn legacy_themes_keep_defaults_and_versioned_themes_reject_unknown_schemas() {
    let legacy = include_str!("../../themes/classic.toml")
        .lines()
        .filter(|line| {
            !line.starts_with("schema_version")
                && !line.starts_with("id =")
                && !line.starts_with("success =")
                && !line.starts_with("warning =")
                && !line.starts_with("error =")
        })
        .collect::<Vec<_>>()
        .join("\n");
    let custom = Theme::parse(&legacy).unwrap();
    assert_eq!(custom.id, "custom");
    assert_eq!(custom.success, Theme::classic().success);
    assert_eq!(custom.warning, Theme::classic().warning);
    assert_eq!(custom.error, Theme::classic().error);
    for version in ["2", "-1", "\"1\"", "true"] {
        let future = include_str!("../../themes/classic.toml")
            .replace("schema_version = 1", &format!("schema_version = {version}"));
        assert!(Theme::parse(&future).is_err());
    }
    assert!(Theme::parse(&" ".repeat(MAX_THEME_BYTES as usize + 1)).is_err());
}

#[test]
fn identities_and_optional_state_colors_are_validated() {
    let source = include_str!("../../themes/classic.toml");
    for id in ["", "../classic", "name with spaces", "UPPER"] {
        assert!(
            Theme::parse(&source.replace("id = \"classic\"", &format!("id = \"{id}\""))).is_err()
        );
    }
    for color in ["red", "#12345678", "#GG0011", "#１２３"] {
        assert!(Theme::parse(&source.replace("#85D59C", color)).is_err());
    }
    let renamed =
        Theme::parse(&source.replace("name = \"Classic\"", "name = \"Personal display label\""))
            .unwrap();
    assert_eq!(renamed.id, "classic");
    assert_eq!(parse_color("#aB12cD"), Ok(0xAB12CD));
}
