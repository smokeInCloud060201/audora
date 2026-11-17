//! SQLite connection manager, WAL configuration, atomic pre-migration backup, and migration runner.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

use rusqlite::{Connection, ErrorCode as SqliteErrorCode, TransactionBehavior};

use crate::error::{redact_path, AppError, AppResult};

/// Embedded SQL for migration `0001_initial.sql`.
pub const MIGRATION_0001_INITIAL_SQL: &str = include_str!("../../migrations/0001_initial.sql");

/// Embedded SQL for migration `0002_playlists.sql`.
pub const MIGRATION_0002_PLAYLISTS_SQL: &str = include_str!("../../migrations/0002_playlists.sql");

/// Embedded SQL for migration `0003_ui_state.sql`.
pub const MIGRATION_0003_UI_STATE_SQL: &str = include_str!("../../migrations/0003_ui_state.sql");

/// A versioned SQL migration specification.
#[derive(Debug, Clone, Copy)]
pub struct MigrationSpec {
    pub version: i64,
    pub name: &'static str,
    pub sql: &'static str,
}

/// Built-in ordered schema migrations.
pub const BUILTIN_MIGRATIONS: &[MigrationSpec] = &[
    MigrationSpec {
        version: 1,
        name: "0001_initial",
        sql: MIGRATION_0001_INITIAL_SQL,
    },
    MigrationSpec {
        version: 2,
        name: "0002_playlists",
        sql: MIGRATION_0002_PLAYLISTS_SQL,
    },
    MigrationSpec {
        version: 3,
        name: "0003_ui_state",
        sql: MIGRATION_0003_UI_STATE_SQL,
    },
];

/// Thread-safe SQLite database handle with migration and backup guarantees.
#[derive(Clone)]
pub struct Database {
    conn: Arc<Mutex<Connection>>,
    db_path: Option<PathBuf>,
    last_backup_path: Option<PathBuf>,
}

impl Database {
    /// Opens an on-disk SQLite database, enables WAL and foreign keys, creates a pre-migration
    /// backup if pending migrations exist, and applies all built-in migrations transactionally.
    pub fn open(path: impl AsRef<Path>) -> AppResult<Self> {
        Self::open_with_migrations(path, BUILTIN_MIGRATIONS)
    }

    /// Opens an on-disk SQLite database, or if the file is genuinely corrupted, preserves the
    /// damaged file at `<db_path>.corrupt.bak` and restores from the most recent valid backup
    /// or initializes a fresh database (`Sprint 5.1`).
    ///
    /// Lock contention (`SQLITE_BUSY` / `SQLITE_LOCKED`), permission errors, and rolled-back
    /// migration errors return `Err` directly without touching live `-wal`/`-shm` files.
    pub fn open_or_recover(path: impl AsRef<Path>) -> AppResult<Self> {
        let db_path = path.as_ref().to_path_buf();
        match Self::open_internal(&db_path, BUILTIN_MIGRATIONS) {
            Ok(db) => match db.verify_integrity() {
                Ok(true) => return Ok(db),
                Ok(false) => {
                    tracing::warn!(
                        path = %redact_path(&db_path),
                        "Database integrity_check failed; preserving corrupt file before recovery"
                    );
                }
                Err(err) => return Err(err),
            },
            Err((err, is_corrupt)) => {
                if !is_corrupt {
                    return Err(err);
                }
                tracing::warn!(
                    code = err.code().as_str(),
                    path = %redact_path(&db_path),
                    "Database file corrupted; preserving corrupt file before recovery"
                );
            }
        }

        let corrupt_bak = Self::backup_path_for(&db_path, "corrupt.bak");
        if db_path.exists() {
            let file_len = fs::metadata(&db_path).map(|m| m.len()).unwrap_or(0);
            if file_len > 0 {
                fs::copy(&db_path, &corrupt_bak).map_err(|io_err| {
                    tracing::error!(
                        path = %redact_path(&db_path),
                        "Failed to preserve corrupt SQLite file before recovery: {io_err}"
                    );
                    AppError::persistence(
                        "Database file appears damaged and could not be backed up safely.",
                    )
                })?;
            }
            let wal_path = PathBuf::from(format!("{}-wal", db_path.display()));
            let shm_path = PathBuf::from(format!("{}-shm", db_path.display()));
            let _ = fs::remove_file(&wal_path);
            let _ = fs::remove_file(&shm_path);

            let candidates = Self::discover_backup_candidates(&db_path);
            let mut restored_ok = false;
            for candidate in candidates {
                if fs::copy(&candidate, &db_path).is_ok() {
                    if let Ok(mut db) = Self::open(&db_path) {
                        if db.verify_integrity().unwrap_or(false) {
                            if corrupt_bak.exists() {
                                db.last_backup_path = Some(corrupt_bak);
                            }
                            return Ok(db);
                        }
                    }
                    let _ = fs::remove_file(&wal_path);
                    let _ = fs::remove_file(&shm_path);
                }
                if !restored_ok {
                    let _ = fs::remove_file(&db_path);
                }
                let _ = &mut restored_ok;
            }
            let _ = fs::remove_file(&db_path);
        }

        let mut recovered = Self::open(&db_path)?;
        if corrupt_bak.exists() {
            recovered.last_backup_path = Some(corrupt_bak);
        }
        Ok(recovered)
    }

    fn backup_path_for(db_path: &Path, suffix: &str) -> PathBuf {
        PathBuf::from(format!("{}.{suffix}", db_path.display()))
    }

    fn discover_backup_candidates(db_path: &Path) -> Vec<PathBuf> {
        let mut found: Vec<(SystemTime, PathBuf)> = Vec::new();
        let Some(parent) = db_path.parent() else {
            return Vec::new();
        };
        let Some(file_name) = db_path.file_name().and_then(|s| s.to_str()) else {
            return Vec::new();
        };
        let manual_prefix = format!("{file_name}.manual.bak");
        let pre_prefix = format!("{file_name}.pre-v");

        let read_dir_parent = if parent.as_os_str().is_empty() {
            Path::new(".")
        } else {
            parent
        };
        if let Ok(entries) = fs::read_dir(read_dir_parent) {
            for entry in entries.filter_map(Result::ok) {
                let p = entry.path();
                let Some(name) = p.file_name().and_then(|s| s.to_str()) else {
                    continue;
                };
                let is_backup = name == manual_prefix
                    || (name.starts_with(&pre_prefix) && name.ends_with(".bak"));
                if is_backup && p.is_file() {
                    let mtime = entry
                        .metadata()
                        .ok()
                        .and_then(|m| m.modified().ok())
                        .unwrap_or(SystemTime::UNIX_EPOCH);
                    found.push((mtime, p));
                }
            }
        }

        found.sort_by_key(|b| std::cmp::Reverse(b.0));
        found.into_iter().map(|(_, p)| p).collect()
    }

    /// Runs `PRAGMA wal_checkpoint(TRUNCATE)` so committed WAL frames are flushed cleanly before exit.
    pub fn checkpoint_wal(&self) -> AppResult<()> {
        self.with_conn(|conn| {
            conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);")?;
            Ok(())
        })
    }

    /// Runs `PRAGMA integrity_check` to verify SQLite B-tree and page consistency.
    pub fn verify_integrity(&self) -> AppResult<bool> {
        self.with_conn(|conn| {
            let status: String = conn.query_row("PRAGMA integrity_check;", [], |row| row.get(0))?;
            Ok(status.eq_ignore_ascii_case("ok"))
        })
    }

    /// Creates an on-demand manual backup snapshot (`<db_path>.manual.bak`) atomically while holding the DB lock.
    pub fn create_manual_backup(&self) -> AppResult<Option<PathBuf>> {
        let Some(ref db_path) = self.db_path else {
            return Ok(None);
        };
        if !db_path.exists() {
            return Ok(None);
        }
        let backup_path = Self::backup_path_for(db_path, "manual.bak");
        let tmp_backup = Self::backup_path_for(db_path, "manual.bak.tmp");
        self.with_conn(|conn| {
            let _ = conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);");
            let _ = fs::remove_file(&tmp_backup);
            let vacuum_ok = tmp_backup
                .to_str()
                .is_some_and(|s| conn.execute("VACUUM INTO ?1;", [s]).is_ok());
            if vacuum_ok {
                fs::rename(&tmp_backup, &backup_path).map_err(|io_err| {
                    let _ = fs::remove_file(&tmp_backup);
                    tracing::error!(
                        path = %redact_path(&backup_path),
                        "Failed to finalize manual database backup: {io_err}"
                    );
                    AppError::persistence("Failed to create manual database backup.")
                })?;
            } else {
                let _ = fs::remove_file(&tmp_backup);
                fs::copy(db_path, &backup_path).map_err(|io_err| {
                    tracing::error!(
                        path = %redact_path(&backup_path),
                        "Failed to copy manual database backup: {io_err}"
                    );
                    AppError::persistence("Failed to create manual database backup.")
                })?;
            }
            Ok(())
        })?;
        Ok(Some(backup_path))
    }

    /// Opens an on-disk SQLite database with a custom migration slice (used for testing recovery).
    pub fn open_with_migrations(
        path: impl AsRef<Path>,
        migrations: &[MigrationSpec],
    ) -> AppResult<Self> {
        Self::open_internal(path.as_ref(), migrations).map_err(|(err, _)| err)
    }

    fn is_sqlite_corruption_error(err: &rusqlite::Error) -> bool {
        matches!(
            err,
            rusqlite::Error::SqliteFailure(inner, _)
                if matches!(
                    inner.code,
                    SqliteErrorCode::DatabaseCorrupt | SqliteErrorCode::NotADatabase
                )
        )
    }

    fn open_internal(
        db_path_ref: &Path,
        migrations: &[MigrationSpec],
    ) -> Result<Self, (AppError, bool)> {
        let db_path = db_path_ref.to_path_buf();
        if let Some(parent) = db_path.parent() {
            if !parent.as_os_str().is_empty() {
                fs::create_dir_all(parent).map_err(|io_err| {
                    tracing::error!(
                        path = %redact_path(parent),
                        "Failed to create database directory: {io_err}"
                    );
                    (
                        AppError::persistence("Failed to create application database directory."),
                        false,
                    )
                })?;
            }
        }

        let conn = Connection::open(&db_path).map_err(|e| {
            let corrupt = Self::is_sqlite_corruption_error(&e);
            (AppError::from(e), corrupt)
        })?;
        if let Err(e) = Self::configure_base_pragmas(&conn) {
            let corrupt = Self::is_sqlite_corruption_error(&e);
            return Err((AppError::from(e), corrupt));
        }
        if let Err(e) = Self::ensure_migrations_table(&conn) {
            let corrupt = Self::is_sqlite_corruption_error(&e);
            return Err((AppError::from(e), corrupt));
        }

        let current_version = match Self::current_schema_version(&conn) {
            Ok(v) => v,
            Err(e) => {
                let corrupt = Self::is_sqlite_corruption_error(&e);
                return Err((AppError::from(e), corrupt));
            }
        };
        let has_pending = migrations.iter().any(|m| m.version > current_version);

        // Checkpoint WAL first so any committed frames are included in the pre-migration backup (`H-1`).
        let mut last_backup_path = None;
        if has_pending && current_version >= 1 && db_path.exists() {
            let _ = conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);");
            let backup_path =
                Self::backup_path_for(&db_path, &format!("pre-v{current_version}.bak"));
            fs::copy(&db_path, &backup_path).map_err(|io_err| {
                tracing::error!(
                    path = %redact_path(&backup_path),
                    "Failed to create pre-migration backup: {io_err}"
                );
                (
                    AppError::persistence(
                        "Failed to create pre-migration backup before upgrading schema.",
                    ),
                    false,
                )
            })?;
            last_backup_path = Some(backup_path);
        }

        let mut db = Self {
            conn: Arc::new(Mutex::new(conn)),
            db_path: Some(db_path),
            last_backup_path,
        };
        db.apply_migrations(migrations).map_err(|e| (e, false))?;
        db.enable_foreign_keys().map_err(|e| (e, false))?;
        Ok(db)
    }

    /// Opens an in-memory SQLite database with built-in migrations applied (for fast unit tests).
    pub fn open_in_memory() -> AppResult<Self> {
        let conn = Connection::open_in_memory()?;
        Self::configure_base_pragmas(&conn)?;
        Self::ensure_migrations_table(&conn)?;
        let mut db = Self {
            conn: Arc::new(Mutex::new(conn)),
            db_path: None,
            last_backup_path: None,
        };
        db.apply_migrations(BUILTIN_MIGRATIONS)?;
        db.enable_foreign_keys()?;
        Ok(db)
    }

    /// Returns the path of the most recent pre-migration backup file, if one was created.
    #[must_use]
    pub fn last_backup_path(&self) -> Option<&Path> {
        self.last_backup_path.as_deref()
    }

    /// Returns the database file path if backed by disk.
    #[must_use]
    pub fn db_path(&self) -> Option<&Path> {
        self.db_path.as_deref()
    }

    /// Executes a closure with exclusive access to the underlying `rusqlite::Connection`.
    pub fn with_conn<F, T>(&self, f: F) -> AppResult<T>
    where
        F: FnOnce(&mut Connection) -> AppResult<T>,
    {
        let mut guard = self
            .conn
            .lock()
            .map_err(|_| AppError::internal("Database connection mutex poisoned"))?;
        f(&mut guard)
    }

    /// Returns the highest applied migration version (`0` if none applied).
    pub fn schema_version(&self) -> AppResult<i64> {
        self.with_conn(|conn| Self::current_schema_version(conn).map_err(AppError::from))
    }

    /// Reads the canonical persisted UI state JSON snapshot from `app_ui_state`, if present.
    pub fn get_ui_state_json(&self) -> AppResult<Option<String>> {
        self.with_conn(|conn| {
            let mut stmt =
                conn.prepare("SELECT payload_json FROM app_ui_state WHERE key = 'v2' LIMIT 1;")?;
            let mut rows = stmt.query([])?;
            if let Some(row) = rows.next()? {
                let val: String = row.get(0)?;
                Ok(Some(val))
            } else {
                Ok(None)
            }
        })
    }

    /// Atomically upserts the canonical persisted UI state JSON snapshot inside `audora.sqlite`.
    pub fn set_ui_state_json(&self, payload_json: &str) -> AppResult<()> {
        self.with_conn(|conn| {
            conn.execute(
                "INSERT INTO app_ui_state (key, payload_json, updated_at)
                 VALUES ('v2', ?1, strftime('%Y-%m-%dT%H:%M:%SZ', 'now'))
                 ON CONFLICT(key) DO UPDATE SET
                   payload_json = excluded.payload_json,
                   updated_at = excluded.updated_at;",
                [payload_json],
            )?;
            Ok(())
        })
    }

    /// Clears persisted UI state from `app_ui_state`.
    pub fn clear_ui_state(&self) -> AppResult<()> {
        self.with_conn(|conn| {
            conn.execute("DELETE FROM app_ui_state;", [])?;
            Ok(())
        })
    }

    /// Wipes all user records across relational tables and `app_ui_state`, then checkpoints WAL.
    pub fn reset_all_user_data(&self) -> AppResult<()> {
        self.with_conn(|conn| {
            conn.execute_batch(
                "DELETE FROM notes;
                 DELETE FROM playlist_items;
                 DELETE FROM playlists;
                 DELETE FROM media;
                 DELETE FROM settings;
                 DELETE FROM app_ui_state;
                 PRAGMA wal_checkpoint(TRUNCATE);",
            )?;
            Ok(())
        })
    }

    fn configure_base_pragmas(conn: &Connection) -> rusqlite::Result<()> {
        conn.busy_timeout(Duration::from_millis(5_000))?;
        let _ = conn.pragma_update(None, "journal_mode", "WAL");
        conn.pragma_update(None, "synchronous", "NORMAL")?;
        conn.pragma_update(None, "foreign_keys", "OFF")?;
        Ok(())
    }

    fn enable_foreign_keys(&self) -> AppResult<()> {
        self.with_conn(|conn| {
            conn.pragma_update(None, "foreign_keys", "ON")?;
            Ok(())
        })
    }

    fn ensure_migrations_table(conn: &Connection) -> rusqlite::Result<()> {
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS schema_migrations (
                version INTEGER PRIMARY KEY NOT NULL,
                name TEXT NOT NULL,
                applied_at TEXT NOT NULL
            );",
        )?;
        Ok(())
    }

    fn current_schema_version(conn: &Connection) -> rusqlite::Result<i64> {
        let version: Option<i64> =
            conn.query_row("SELECT MAX(version) FROM schema_migrations", [], |row| {
                row.get(0)
            })?;
        Ok(version.unwrap_or(0))
    }

    fn apply_migrations(&mut self, migrations: &[MigrationSpec]) -> AppResult<()> {
        self.with_conn(|conn| {
            let current = Self::current_schema_version(conn)?;
            for migration in migrations {
                if migration.version <= current {
                    continue;
                }
                let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
                if let Err(err) = tx.execute_batch(migration.sql) {
                    tracing::error!(
                        version = migration.version,
                        name = migration.name,
                        "Schema migration failed; rolling back transaction: {err}"
                    );
                    return Err(AppError::persistence(format!(
                        "Schema migration {} ({}) failed; changes were rolled back and backup preserved.",
                        migration.version, migration.name
                    )));
                }
                tx.execute(
                    "INSERT INTO schema_migrations (version, name, applied_at) VALUES (?1, ?2, strftime('%Y-%m-%dT%H:%M:%SZ', 'now'))",
                    rusqlite::params![migration.version, migration.name],
                )?;
                tx.commit()?;
            }
            Ok(())
        })
    }
}
