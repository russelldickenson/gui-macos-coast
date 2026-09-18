//! User settings: `~/Library/Application Support/Coast/config.toml`.
//!
//! ```toml
//! font_family = "JetBrains Mono"
//! font_size = 13.0
//! line_spacing = 0.0 # extra px between rows, 0-4
//! theme = "Tokyo Night"
//! chrome_theme = "Dark Gray"
//! breadcrumb_theme = "Dark"
//! scrollback_lines = 10000
//! cursor_blink = true
//! cursor_shape = "block" # "block" | "beam" | "underline"
//! default_cols = 100
//! default_rows = 30
//! confirm_close_tab = true
//! show_breadcrumb = false
//! cursor_color = "#ff9900" # omit to use the theme's own cursor color
//! ```

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

pub mod session_state;

fn default_font_family() -> String {
    "Menlo".to_string()
}

fn default_font_size() -> f32 {
    12.0
}

fn default_line_spacing() -> f32 {
    0.0
}

fn default_theme() -> String {
    "Campbell".to_string()
}

fn default_chrome_theme() -> String {
    "Light".to_string()
}

fn default_breadcrumb_theme() -> String {
    "Dark".to_string()
}

fn default_scrollback_lines() -> usize {
    10_000
}

fn default_cursor_blink() -> bool {
    true
}

fn default_cursor_shape() -> String {
    "block".to_string()
}

fn default_cols() -> u16 {
    100
}

fn default_rows() -> u16 {
    30
}

fn default_confirm_close_tab() -> bool {
    true
}

fn default_show_breadcrumb() -> bool {
    false
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Settings {
    #[serde(default = "default_font_family")]
    pub font_family: String,
    #[serde(default = "default_font_size")]
    pub font_size: f32,
    /// Extra px added between rows, on top of the font's own line height.
    #[serde(default = "default_line_spacing")]
    pub line_spacing: f32,
    #[serde(default = "default_theme")]
    pub theme: String,
    #[serde(default = "default_chrome_theme")]
    pub chrome_theme: String,
    #[serde(default = "default_scrollback_lines")]
    pub scrollback_lines: usize,
    #[serde(default = "default_cursor_blink")]
    pub cursor_blink: bool,
    /// "block" | "beam" | "underline".
    #[serde(default = "default_cursor_shape")]
    pub cursor_shape: String,
    /// Default window size for new windows, in character columns/rows.
    #[serde(default = "default_cols")]
    pub default_cols: u16,
    #[serde(default = "default_rows")]
    pub default_rows: u16,
    /// Ask for confirmation before closing a tab that's either the last
    /// one open (which quits the app) or has a foreground process running.
    #[serde(default = "default_confirm_close_tab")]
    pub confirm_close_tab: bool,
    /// Show a directory breadcrumb above each terminal tab's content for
    /// clicking through to a parent directory or a subdirectory.
    #[serde(default = "default_show_breadcrumb")]
    pub show_breadcrumb: bool,
    /// Override the active theme's cursor color, as `#rrggbb`. `None` uses
    /// the theme's own cursor color.
    #[serde(default)]
    pub cursor_color: Option<String>,
    /// Color theme for the directory breadcrumb bar — independent of
    /// `chrome_theme` (the tab strip's own light/dark setting).
    #[serde(default = "default_breadcrumb_theme")]
    pub breadcrumb_theme: String,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            font_family: default_font_family(),
            font_size: default_font_size(),
            line_spacing: default_line_spacing(),
            theme: default_theme(),
            chrome_theme: default_chrome_theme(),
            scrollback_lines: default_scrollback_lines(),
            cursor_blink: default_cursor_blink(),
            cursor_shape: default_cursor_shape(),
            default_cols: default_cols(),
            default_rows: default_rows(),
            confirm_close_tab: default_confirm_close_tab(),
            show_breadcrumb: default_show_breadcrumb(),
            cursor_color: None,
            breadcrumb_theme: default_breadcrumb_theme(),
        }
    }
}

impl Settings {
    /// Load from disk, falling back to defaults when the file is missing.
    /// A present-but-unparseable file is an error.
    pub fn load(path: &std::path::Path) -> anyhow::Result<Self> {
        if !path.exists() {
            return Ok(Self::default());
        }
        let raw = std::fs::read_to_string(path)?;
        Ok(toml::from_str(&raw)?)
    }

    /// Load from the standard location.
    pub fn load_default() -> anyhow::Result<Self> {
        Self::load(&config_path())
    }

    /// Write to the standard location, creating the containing directory if
    /// needed.
    pub fn save(&self) -> anyhow::Result<()> {
        let path = config_path();
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        std::fs::write(path, toml::to_string_pretty(self)?)?;
        Ok(())
    }

    /// Resolve the configured theme (built-in or user TOML), falling back
    /// to Campbell when the name is unknown, with `cursor_color` (if set
    /// and valid) overriding the theme's own cursor color.
    pub fn theme(&self) -> coast_themes::Theme {
        let mut theme = coast_themes::Theme::find(&self.theme)
            .or_else(|| load_user_theme(&self.theme))
            .unwrap_or_else(|| {
                coast_themes::Theme::find("Campbell").expect("Campbell is built in")
            });
        if let Some(color) = self
            .cursor_color
            .as_deref()
            .and_then(|c| c.parse::<coast_themes::Color>().ok())
        {
            theme.cursor = color;
        }
        theme
    }

    /// Resolve the configured chrome (titlebar/tab strip) theme, built-in or
    /// user TOML, falling back to Light when the name is unknown.
    pub fn chrome_theme(&self) -> coast_themes::ChromeTheme {
        if let Some(theme) = coast_themes::ChromeTheme::find(&self.chrome_theme) {
            return theme;
        }
        if let Some(theme) = load_user_chrome_theme(&self.chrome_theme) {
            return theme;
        }
        coast_themes::ChromeTheme::find("Light").expect("Light is built in")
    }

    /// Resolve the configured breadcrumb theme, built-in or user TOML,
    /// falling back to Dark when the name is unknown.
    pub fn breadcrumb_theme(&self) -> coast_themes::BreadcrumbTheme {
        if let Some(theme) = coast_themes::BreadcrumbTheme::find(&self.breadcrumb_theme) {
            return theme;
        }
        if let Some(theme) = load_user_breadcrumb_theme(&self.breadcrumb_theme) {
            return theme;
        }
        coast_themes::BreadcrumbTheme::find("Dark").expect("Dark is built in")
    }
}

/// `~/Library/Application Support/Coast/config.toml`.
pub fn config_path() -> PathBuf {
    support_dir().join("config.toml")
}

/// `~/Library/Application Support/Coast/themes/<name>.toml`.
pub fn user_theme_path(name: &str) -> PathBuf {
    support_dir().join("themes").join(format!("{name}.toml"))
}

fn load_user_theme(name: &str) -> Option<coast_themes::Theme> {
    let path = user_theme_path(name);
    let raw = std::fs::read_to_string(path).ok()?;
    toml::from_str(&raw).ok()
}

/// Names of every user theme saved under `themes/*.toml` (e.g. imported
/// from a Terminal.app `.terminal` file), for listing alongside the
/// built-ins.
pub fn user_theme_names() -> Vec<String> {
    let dir = support_dir().join("themes");
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut names: Vec<String> = entries
        .filter_map(|entry| entry.ok())
        .filter(|entry| entry.path().extension().is_some_and(|ext| ext == "toml"))
        .filter_map(|entry| entry.path().file_stem().map(|s| s.to_string_lossy().to_string()))
        .collect();
    names.sort();
    names
}

/// Write a theme to `themes/<name>.toml`, creating the directory if
/// needed. `/` in the name is replaced so it can't escape the directory.
pub fn save_user_theme(theme: &coast_themes::Theme) -> anyhow::Result<()> {
    let safe_name = theme.name.replace('/', "-");
    let path = user_theme_path(&safe_name);
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(path, toml::to_string_pretty(theme)?)?;
    Ok(())
}

/// `~/Library/Application Support/Coast/chrome-themes/<name>.toml`.
pub fn user_chrome_theme_path(name: &str) -> PathBuf {
    support_dir()
        .join("chrome-themes")
        .join(format!("{name}.toml"))
}

fn load_user_chrome_theme(name: &str) -> Option<coast_themes::ChromeTheme> {
    let path = user_chrome_theme_path(name);
    let raw = std::fs::read_to_string(path).ok()?;
    toml::from_str(&raw).ok()
}

/// `~/Library/Application Support/Coast/breadcrumb-themes/<name>.toml`.
pub fn user_breadcrumb_theme_path(name: &str) -> PathBuf {
    support_dir()
        .join("breadcrumb-themes")
        .join(format!("{name}.toml"))
}

fn load_user_breadcrumb_theme(name: &str) -> Option<coast_themes::BreadcrumbTheme> {
    let path = user_breadcrumb_theme_path(name);
    let raw = std::fs::read_to_string(path).ok()?;
    toml::from_str(&raw).ok()
}

pub(crate) fn support_dir() -> PathBuf {
    let home = std::env::var_os("HOME").expect("HOME is set");
    PathBuf::from(home)
        .join("Library")
        .join("Application Support")
        .join("Coast")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_match_spec() {
        let s = Settings::default();
        assert_eq!(s.font_family, "Menlo");
        assert_eq!(s.font_size, 12.0);
        assert_eq!(s.line_spacing, 0.0);
        assert_eq!(s.theme, "Campbell");
        assert_eq!(s.chrome_theme, "Light");
        assert_eq!(s.scrollback_lines, 10_000);
        assert!(s.cursor_blink);
        assert_eq!(s.cursor_shape, "block");
        assert_eq!(s.default_cols, 100);
        assert_eq!(s.default_rows, 30);
        assert!(s.confirm_close_tab);
        assert!(!s.show_breadcrumb);
        assert_eq!(s.breadcrumb_theme, "Dark");
    }

    #[test]
    fn save_and_reload_roundtrip() {
        let dir = std::env::temp_dir().join(format!("coast-settings-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        // SAFETY: test-only env var override; this test does not run
        // concurrently with anything else that reads HOME.
        unsafe { std::env::set_var("HOME", &dir) };

        let mut s = Settings::default();
        s.cursor_shape = "beam".to_string();
        s.default_cols = 120;
        s.show_breadcrumb = true;
        s.save().expect("save settings");

        let reloaded = Settings::load_default().expect("reload settings");
        assert_eq!(reloaded.cursor_shape, "beam");
        assert_eq!(reloaded.default_cols, 120);
        assert!(reloaded.show_breadcrumb);

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn resolves_builtin_chrome_theme() {
        let s = Settings {
            chrome_theme: "Dark Gray".to_string(),
            ..Settings::default()
        };
        assert_eq!(s.chrome_theme().name, "Dark Gray");
    }

    #[test]
    fn unknown_chrome_theme_falls_back_to_light() {
        let s = Settings {
            chrome_theme: "No Such Theme".to_string(),
            ..Settings::default()
        };
        assert_eq!(s.chrome_theme().name, "Light");
    }

    #[test]
    fn resolves_builtin_breadcrumb_theme() {
        let s = Settings {
            breadcrumb_theme: "Light".to_string(),
            ..Settings::default()
        };
        assert_eq!(s.breadcrumb_theme().name, "Light");
    }

    #[test]
    fn unknown_breadcrumb_theme_falls_back_to_dark() {
        let s = Settings {
            breadcrumb_theme: "No Such Theme".to_string(),
            ..Settings::default()
        };
        assert_eq!(s.breadcrumb_theme().name, "Dark");
    }

    #[test]
    fn parses_partial_config() {
        let s: Settings = toml::from_str(
            r#"
            font_family = "JetBrains Mono"
            font_size = 13.5
            theme = "Tokyo Night"
            "#,
        )
        .unwrap();
        assert_eq!(s.font_family, "JetBrains Mono");
        assert_eq!(s.font_size, 13.5);
        assert_eq!(s.theme, "Tokyo Night");
        assert_eq!(s.scrollback_lines, 10_000);
        assert_eq!(s.theme().name, "Tokyo Night");
    }

    #[test]
    fn missing_file_gives_defaults() {
        let s = Settings::load(std::path::Path::new("/nonexistent/coast-config.toml")).unwrap();
        assert_eq!(s.font_family, "Menlo");
    }

    #[test]
    fn unknown_theme_falls_back_to_campbell() {
        let s = Settings {
            theme: "No Such Theme".to_string(),
            ..Settings::default()
        };
        assert_eq!(s.theme().name, "Campbell");
    }

    #[test]
    fn cursor_color_overrides_theme_cursor() {
        let base = Settings::default().theme();
        let s = Settings {
            cursor_color: Some("#ff9900".to_string()),
            ..Settings::default()
        };
        let themed = s.theme();
        assert_eq!(themed.cursor, "#ff9900".parse().unwrap());
        assert_ne!(themed.cursor, base.cursor);
        // Everything else about the theme is untouched.
        assert_eq!(themed.background, base.background);
    }

    #[test]
    fn invalid_cursor_color_falls_back_to_theme_default() {
        let base = Settings::default().theme();
        let s = Settings {
            cursor_color: Some("not-a-color".to_string()),
            ..Settings::default()
        };
        assert_eq!(s.theme().cursor, base.cursor);
    }

    #[test]
    #[ignore = "manual sanity check: imports a real .terminal file and round-trips it through save/load"]
    fn scratch_import_save_load_roundtrip() {
        let dir = std::env::temp_dir().join(format!("coast-import-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        unsafe { std::env::set_var("HOME", &dir) };

        let theme = coast_themes::import_terminal_file(std::path::Path::new(
            "/Users/russell/Downloads/macOS - terminal - Nord theme.terminal",
        ))
        .expect("import");
        save_user_theme(&theme).expect("save");

        assert!(user_theme_names().contains(&theme.name));

        let s = Settings {
            theme: theme.name.clone(),
            ..Settings::default()
        };
        assert_eq!(s.theme().name, theme.name);
        assert_eq!(s.theme().background, theme.background);

        std::fs::remove_dir_all(&dir).ok();
    }
}
