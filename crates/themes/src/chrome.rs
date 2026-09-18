//! Chrome themes: colors for the titlebar/tab strip, kept separate from the
//! terminal [`crate::Theme`] so the window chrome can stay a fixed native
//! look (or a user-picked one) independent of whatever color scheme a shell
//! session happens to be using.
//!
//! User chrome themes live in
//! `~/Library/Application Support/Coast/chrome-themes/*.toml` and use the
//! same `#rrggbb` hex strings as terminal themes.

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

/// Colors for the titlebar/tab strip.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChromeTheme {
    pub name: String,
    /// The tab strip's own background.
    pub bar: Color,
    /// The active tab's fill.
    pub active_tab: Color,
    /// The active tab's title/glyph color.
    pub active_tab_text: Color,
    /// An inactive tab's fill.
    pub inactive_tab: Color,
    /// An inactive tab's title/glyph color.
    pub inactive_tab_text: Color,
    /// Hover fill for the `+` button and a tab's close button.
    pub hover: Color,
    /// Hairline dividers (under the bar, around the active tab).
    pub separator: Color,
}

impl ChromeTheme {
    pub fn find(name: &str) -> Option<ChromeTheme> {
        builtin_chrome_themes()
            .into_iter()
            .find(|t| t.name.eq_ignore_ascii_case(name))
    }
}

pub fn builtin_chrome_themes() -> Vec<ChromeTheme> {
    vec![light(), dark_gray()]
}

/// macOS's own unified-toolbar gray, with a white active tab.
pub fn light() -> ChromeTheme {
    ChromeTheme {
        name: "Light".to_string(),
        bar: c!("#ececec"),
        active_tab: c!("#ffffff"),
        active_tab_text: c!("#1c1c1e"),
        inactive_tab: c!("#ececec"),
        inactive_tab_text: c!("#6e6e73"),
        hover: c!("#dedede"),
        separator: c!("#d0d0d0"),
    }
}

pub fn dark_gray() -> ChromeTheme {
    ChromeTheme {
        name: "Dark Gray".to_string(),
        bar: c!("#2e3440"),
        active_tab: c!("#3b4252"),
        active_tab_text: c!("#ffffff"),
        inactive_tab: c!("#2e3440"),
        inactive_tab_text: c!("#d7d7d7"),
        hover: c!("#787878"),
        separator: c!("#4a4a4a"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtins_are_findable_and_distinct() {
        for theme in builtin_chrome_themes() {
            assert_eq!(ChromeTheme::find(&theme.name), Some(theme));
        }
        assert!(ChromeTheme::find("no-such-theme").is_none());
    }

    #[test]
    fn chrome_theme_toml_roundtrip() {
        let theme = ChromeTheme::find("Dark Gray").unwrap();
        let toml = toml::to_string(&theme).unwrap();
        let back: ChromeTheme = toml::from_str(&toml).unwrap();
        assert_eq!(theme, back);
    }
}
