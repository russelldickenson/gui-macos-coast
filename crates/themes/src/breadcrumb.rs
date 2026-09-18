//! Breadcrumb themes: colors for the directory breadcrumb bar, kept
//! independent of both the terminal [`crate::Theme`] and the tab strip's
//! [`crate::ChromeTheme`] so each can be switched light/dark on its own.
//!
//! User breadcrumb themes live in
//! `~/Library/Application Support/Coast/breadcrumb-themes/*.toml` and use
//! the same `#rrggbb` hex strings as terminal themes.

use serde::{Deserialize, Serialize};

use crate::Color;

macro_rules! c {
    ($hex:expr) => {
        Color::new(
            u8::from_str_radix(&$hex[1..3], 16).unwrap(),
            u8::from_str_radix(&$hex[3..5], 16).unwrap(),
            u8::from_str_radix(&$hex[5..7], 16).unwrap(),
        )
    };
}

/// Colors for the directory breadcrumb bar.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BreadcrumbTheme {
    pub name: String,
    /// Bar and pill fill (the same color, so pills read as sitting flush
    /// on the bar rather than as separate blocks against it).
    pub pill: Color,
    /// A pill's hairline border.
    pub pill_border: Color,
    /// A pill's fill when hovered.
    pub pill_hover: Color,
    /// A pill's label text.
    pub text: Color,
    /// Chevrons, the ellipsis, and other muted decoration.
    pub muted: Color,
}

impl BreadcrumbTheme {
    pub fn find(name: &str) -> Option<BreadcrumbTheme> {
        builtin_breadcrumb_themes()
            .into_iter()
            .find(|t| t.name.eq_ignore_ascii_case(name))
    }
}

pub fn builtin_breadcrumb_themes() -> Vec<BreadcrumbTheme> {
    vec![light(), dark()]
}

pub fn light() -> BreadcrumbTheme {
    BreadcrumbTheme {
        name: "Light".to_string(),
        pill: c!("#ffffff"),
        pill_border: c!("#d0d0d0"),
        pill_hover: c!("#ececec"),
        text: c!("#1c1c1e"),
        muted: c!("#8e8e93"),
    }
}

pub fn dark() -> BreadcrumbTheme {
    BreadcrumbTheme {
        name: "Dark".to_string(),
        pill: c!("#2e3440"),
        pill_border: c!("#40424f"),
        pill_hover: c!("#363848"),
        text: c!("#d7d8e2"),
        muted: c!("#7c7e90"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtins_are_findable_and_distinct() {
        for theme in builtin_breadcrumb_themes() {
            assert_eq!(BreadcrumbTheme::find(&theme.name), Some(theme));
        }
        assert!(BreadcrumbTheme::find("no-such-theme").is_none());
    }

    #[test]
    fn breadcrumb_theme_toml_roundtrip() {
        let theme = BreadcrumbTheme::find("Dark").unwrap();
        let toml = toml::to_string(&theme).unwrap();
        let back: BreadcrumbTheme = toml::from_str(&toml).unwrap();
        assert_eq!(theme, back);
    }
}
