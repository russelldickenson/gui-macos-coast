//! The terminal tabs open at last quit, for restoring them on relaunch:
//! `~/Library/Application Support/Coast/session.toml`.
//!
//! Saved only at quit (not continuously), so a force-quit/crash falls back
//! to whatever was saved at the last clean quit, or a single default tab
//! if there's none.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// One restorable terminal tab.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SavedTab {
    pub cwd: PathBuf,
}

/// The full set of terminal tabs open at last quit.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct SavedSession {
    pub tabs: Vec<SavedTab>,
}

impl SavedSession {
    /// Load from a specific path. `None` for a missing *or* unparseable
    /// file — corrupt/foreign state should fall back to default startup,
    /// not abort the app.
    pub fn load_from(path: &Path) -> Option<Self> {
        let raw = std::fs::read_to_string(path).ok()?;
        toml::from_str(&raw).ok()
    }

    /// Load from the standard location.
    pub fn load() -> Option<Self> {
        Self::load_from(&session_path())
    }

    /// Write to a specific path, creating the containing directory if
    /// needed.
    pub fn save_to(&self, path: &Path) -> anyhow::Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        std::fs::write(path, toml::to_string_pretty(self)?)?;
        Ok(())
    }

    /// Write to the standard location.
    pub fn save(&self) -> anyhow::Result<()> {
        self.save_to(&session_path())
    }
}

fn session_path() -> PathBuf {
    crate::support_dir().join("session.toml")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn save_and_reload_roundtrip() {
        let dir =
            std::env::temp_dir().join(format!("coast-session-state-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("session.toml");

        let saved = SavedSession {
            tabs: vec![
                SavedTab { cwd: PathBuf::from("/tmp/project-a") },
                SavedTab { cwd: PathBuf::from("/tmp/project-b") },
            ],
        };
        saved.save_to(&path).expect("save session state");

        let reloaded = SavedSession::load_from(&path).expect("reload session state");
        assert_eq!(reloaded, saved);

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn missing_file_gives_none() {
        assert!(SavedSession::load_from(Path::new("/nonexistent/coast-session.toml")).is_none());
    }
}
