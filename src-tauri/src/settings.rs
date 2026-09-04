//! User settings, persisted to `settings.json` in the data dir.

use std::io;
use std::path::PathBuf;
use std::sync::RwLock;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", default)]
pub struct Settings {
    /// Where the mirror lives. Must be an absolute path.
    pub sync_folder: PathBuf,
    /// How often to look for changes on iCloud (local changes sync at once).
    pub poll_interval_secs: u64,
    pub close_to_tray: bool,
    pub notifications_enabled: bool,
    /// User-requested pause; survives restarts.
    pub paused: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            sync_folder: crate::paths::default_sync_folder(),
            poll_interval_secs: 120,
            close_to_tray: true,
            notifications_enabled: true,
            paused: false,
        }
    }
}

impl Settings {
    /// Reject values the sync loop cannot work with. Returns a user-facing
    /// message.
    pub fn validate(&self) -> Result<(), String> {
        if !self.sync_folder.is_absolute() {
            return Err("The sync folder must be an absolute path.".into());
        }
        if self.sync_folder.parent().is_none() {
            return Err("The sync folder cannot be the filesystem root.".into());
        }
        if let Some(home) = dirs::home_dir() {
            if self.sync_folder == home {
                return Err("The sync folder cannot be your home folder itself.".into());
            }
        }
        if !(15..=86_400).contains(&self.poll_interval_secs) {
            return Err("The check interval must be between 15 seconds and 24 hours.".into());
        }
        Ok(())
    }
}

pub struct SettingsState(pub RwLock<Settings>);

fn settings_path() -> PathBuf {
    crate::paths::data_dir().join("settings.json")
}

pub fn load() -> Settings {
    std::fs::read(settings_path())
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default()
}

pub fn save(settings: &Settings) -> io::Result<()> {
    let path = settings_path();
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let json = serde_json::to_vec_pretty(settings).map_err(io::Error::other)?;
    // Temp file + rename so a crash mid-write cannot leave a truncated file
    // that silently resets every setting.
    let mut tmp = path.clone().into_os_string();
    tmp.push(".tmp");
    let tmp = PathBuf::from(tmp);
    std::fs::write(&tmp, json)?;
    std::fs::rename(&tmp, &path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validation_rejects_dangerous_folders_and_silly_intervals() {
        let ok = Settings::default();
        assert!(ok.validate().is_ok());
        let mut s = ok.clone();
        s.sync_folder = PathBuf::from("relative/dir");
        assert!(s.validate().is_err());
        s.sync_folder = PathBuf::from("/");
        assert!(s.validate().is_err());
        if let Some(home) = dirs::home_dir() {
            s.sync_folder = home;
            assert!(s.validate().is_err());
        }
        let mut s = ok.clone();
        s.poll_interval_secs = 5;
        assert!(s.validate().is_err());
        s.poll_interval_secs = 86_401;
        assert!(s.validate().is_err());
    }

    #[test]
    fn missing_fields_take_defaults() {
        let s: Settings = serde_json::from_str(r#"{"pollIntervalSecs":300}"#).unwrap();
        assert_eq!(s.poll_interval_secs, 300);
        assert!(s.close_to_tray);
        assert_eq!(s.sync_folder, crate::paths::default_sync_folder());
    }
}
