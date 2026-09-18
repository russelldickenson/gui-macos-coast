//! Color themes: structs, built-ins, and user theme loading.
//!
//! User themes live in `~/Library/Application Support/Coast/themes/*.toml`
//! and use `#rrggbb` hex strings, e.g.
//!
//! ```toml
//! name = "My Theme"
//! background = "#1e1e2e"
//! foreground = "#cdd6f4"
//! # ... plus cursor, selection, and the 16 ansi colors
//! ```

mod breadcrumb;
mod builtin;
mod chrome;
mod terminal_import;

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

pub use breadcrumb::{builtin_breadcrumb_themes, BreadcrumbTheme};
pub use builtin::builtin_themes;
pub use chrome::{builtin_chrome_themes, ChromeTheme};
pub use terminal_import::import_terminal_file;

/// An sRGB color.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Color {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

impl Color {
    pub const fn new(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b }
    }

    /// Packed `0xRRGGBB`, handy for GPU UIs.
    pub const fn rgb(self) -> u32 {
        (self.r as u32) << 16 | (self.g as u32) << 8 | self.b as u32
    }

    pub fn to_hex(self) -> String {
        format!("#{:02x}{:02x}{:02x}", self.r, self.g, self.b)
    }
}

impl FromStr for Color {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let hex = s.strip_prefix('#').unwrap_or(s);
        if hex.len() != 6 || !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
            anyhow::bail!("invalid color {s:?}, expected #rrggbb");
        }
        Ok(Self {
            r: u8::from_str_radix(&hex[0..2], 16)?,
            g: u8::from_str_radix(&hex[2..4], 16)?,
            b: u8::from_str_radix(&hex[4..6], 16)?,
        })
    }
}

impl Serialize for Color {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.to_hex())
    }
}

impl<'de> Deserialize<'de> for Color {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let raw = String::deserialize(d)?;
        raw.parse().map_err(serde::de::Error::custom)
    }
}

impl fmt::Display for Color {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.to_hex())
    }
}

/// A full terminal + chrome theme.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Theme {
    pub name: String,
    pub background: Color,
    pub foreground: Color,
    pub cursor: Color,
    pub cursor_text: Color,
    pub selection_background: Color,
    pub black: Color,
    pub red: Color,
    pub green: Color,
    pub yellow: Color,
    pub blue: Color,
    pub magenta: Color,
    pub cyan: Color,
    pub white: Color,
    pub bright_black: Color,
    pub bright_red: Color,
    pub bright_green: Color,
    pub bright_yellow: Color,
    pub bright_blue: Color,
    pub bright_magenta: Color,
    pub bright_cyan: Color,
    pub bright_white: Color,
}

impl Theme {
    /// The 16 ANSI colors in order 0-15.
    pub fn ansi(&self) -> [Color; 16] {
        [
            self.black,
            self.red,
            self.green,
            self.yellow,
            self.blue,
            self.magenta,
            self.cyan,
            self.white,
            self.bright_black,
            self.bright_red,
            self.bright_green,
            self.bright_yellow,
            self.bright_blue,
            self.bright_magenta,
            self.bright_cyan,
            self.bright_white,
        ]
    }

    pub fn find(name: &str) -> Option<Theme> {
        builtin_themes()
            .into_iter()
            .find(|t| t.name.eq_ignore_ascii_case(name))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[ignore = "manual sanity check against a real .terminal file on this machine"]
    fn scratch_import_real_terminal_file() {
        let theme = import_terminal_file(std::path::Path::new(
            "/Users/russell/Downloads/macOS - terminal - Nord theme.terminal",
        ))
        .expect("import");
        println!("{theme:#?}");
    }

    #[test]
    fn color_hex_roundtrip() {
        let c: Color = "#1e1e2e".parse().unwrap();
        assert_eq!((c.r, c.g, c.b), (0x1e, 0x1e, 0x2e));
        assert_eq!(c.to_hex(), "#1e1e2e");
        assert_eq!(c.rgb(), 0x1e1e2e);
    }

    #[test]
    fn color_rejects_garbage() {
        assert!("#xyz".parse::<Color>().is_err());
        assert!("red".parse::<Color>().is_err());
        assert!("#12345".parse::<Color>().is_err());
    }

    #[test]
    fn builtins_have_sixteen_distinctish_colors() {
        for theme in builtin_themes() {
            let ansi = theme.ansi();
            assert_eq!(ansi.len(), 16, "{}", theme.name);
            assert_eq!(Theme::find(&theme.name).as_ref(), Some(&theme));
        }
        assert!(Theme::find("no-such-theme").is_none());
    }

    #[test]
    fn theme_toml_roundtrip() {
        let theme = Theme::find("Campbell").unwrap();
        let toml = toml::to_string(&theme).unwrap();
        let back: Theme = toml::from_str(&toml).unwrap();
        assert_eq!(theme, back);
    }
}
