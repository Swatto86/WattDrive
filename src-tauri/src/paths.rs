//! Per-user locations. Data (settings, sync database, log) lives in
//! `$XDG_DATA_HOME/WattDrive` (`~/.local/share/WattDrive`); the sync folder
//! itself is a setting and defaults to `~/iCloud Drive`.

use std::path::PathBuf;

pub fn data_dir() -> PathBuf {
    dirs::data_local_dir()
        .unwrap_or_else(std::env::temp_dir)
        .join("WattDrive")
}

pub fn default_sync_folder() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(std::env::temp_dir)
        .join("iCloud Drive")
}

pub fn state_db_path() -> PathBuf {
    data_dir().join("sync.db")
}

pub fn log_path() -> PathBuf {
    data_dir().join("wattdrive.log")
}

/// Short host name for conflict-copy markers.
pub fn host_name() -> String {
    std::fs::read_to_string("/etc/hostname")
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "linux".to_string())
}
