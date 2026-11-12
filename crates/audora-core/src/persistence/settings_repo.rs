//! SQLite implementation of `SettingsRepository`.

use rusqlite::{params, OptionalExtension};

use crate::domain::{AppSettings, SettingsRepository};
use crate::error::{AppError, AppResult};
use crate::persistence::Database;

const APP_SETTINGS_KEY: &str = "app_preferences_v1";

#[derive(Clone)]
pub struct SqliteSettingsRepository {
    db: Database,
}

impl SqliteSettingsRepository {
    #[must_use]
    pub const fn new(db: Database) -> Self {
        Self { db }
    }
}

impl SettingsRepository for SqliteSettingsRepository {
    fn load_settings(&self) -> AppResult<AppSettings> {
        self.db.with_conn(|conn| {
            let raw: Option<String> = conn
                .query_row(
                    "SELECT value_json FROM settings WHERE key = ?1",
                    params![APP_SETTINGS_KEY],
                    |row| row.get(0),
                )
                .optional()?;

            match raw {
                Some(json_str) => serde_json::from_str::<AppSettings>(&json_str).map_err(|e| {
                    AppError::persistence(format!("Stored settings JSON is invalid: {e}"))
                }),
                None => Ok(AppSettings::default()),
            }
        })
    }

    fn save_settings(&self, settings: &AppSettings, now_iso: &str) -> AppResult<()> {
        let json_str = serde_json::to_string(settings)
            .map_err(|e| AppError::internal(format!("Failed to serialize settings: {e}")))?;

        self.db.with_conn(|conn| {
            let tx = conn.transaction()?;
            tx.execute(
                "INSERT INTO settings (key, value_json, updated_at)
                 VALUES (?1, ?2, ?3)
                 ON CONFLICT(key) DO UPDATE SET
                     value_json = excluded.value_json,
                     updated_at = excluded.updated_at",
                params![APP_SETTINGS_KEY, json_str, now_iso],
            )?;
            tx.commit()?;
            Ok(())
        })
    }
}
