//! Audora desktop shell library (`apps/desktop/src-tauri`).
//!
//! Bridges `audora_core` with the native `tao` + `wry` desktop window, local asset server,
//! native self-updater (`/__audora_update`), and OS installer cleanup hooks (`--uninstall-cleanup`).

pub use audora_core::*;

use std::env;
use std::path::PathBuf;

/// Resolves the platform-standard application data directory (`com.audora.desktop`).
#[must_use]
pub fn default_app_data_dir() -> PathBuf {
    if let Ok(custom) = env::var("AUDORA_DATA_DIR") {
        return PathBuf::from(custom);
    }
    let home = env::var("HOME")
        .or_else(|_| env::var("USERPROFILE"))
        .unwrap_or_else(|_| ".".to_owned());

    if cfg!(target_os = "macos") {
        PathBuf::from(home)
            .join("Library")
            .join("Application Support")
            .join("com.audora.desktop")
    } else if cfg!(target_os = "windows") {
        let appdata = env::var("APPDATA").unwrap_or(home);
        PathBuf::from(appdata)
            .join("com.audora.desktop")
            .join("data")
    } else {
        PathBuf::from(home)
            .join(".local")
            .join("share")
            .join("com.audora.desktop")
    }
}

/// Resolves the default SQLite database path inside `default_app_data_dir()`.
#[must_use]
pub fn default_database_path() -> PathBuf {
    if let Ok(custom) = env::var("AUDORA_DB_PATH") {
        return PathBuf::from(custom);
    }
    default_app_data_dir().join("audora.sqlite")
}
