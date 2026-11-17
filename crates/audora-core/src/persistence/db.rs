//! SQLite connection manager, WAL configuration, atomic pre-migration backup, and migration runner.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

use rusqlite::{
    params, Connection, ErrorCode as SqliteErrorCode, OptionalExtension, TransactionBehavior,
};

use crate::domain::{
    AppSettings, Availability, MediaItem, MediaKind, Note, PersistedPlaybackSession, Playlist,
    RelationalAppStateDto,
};
use crate::error::{redact_path, AppError, AppResult};

/// Embedded SQL for migration `0001_initial.sql`.
pub const MIGRATION_0001_INITIAL_SQL: &str = include_str!("../../migrations/0001_initial.sql");

/// Embedded SQL for migration `0002_playlists.sql`.
pub const MIGRATION_0002_PLAYLISTS_SQL: &str = include_str!("../../migrations/0002_playlists.sql");

/// Embedded SQL for migration `0003_ui_state.sql`.
pub const MIGRATION_0003_UI_STATE_SQL: &str = include_str!("../../migrations/0003_ui_state.sql");

/// Embedded SQL for migration `0004_relational_unification.sql`.
pub const MIGRATION_0004_RELATIONAL_UNIFICATION_SQL: &str =
    include_str!("../../migrations/0004_relational_unification.sql");

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
    MigrationSpec {
        version: 4,
        name: "0004_relational_unification",
        sql: MIGRATION_0004_RELATIONAL_UNIFICATION_SQL,
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
        if db.schema_version().unwrap_or(0) >= 4 {
            let _ = db.migrate_legacy_ui_state_blob();
        }
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

    /// Exports the authoritative relational state (`media`, `notes`, `playlists`, `settings`) as JSON,
    /// returning `None` when the database contains no user records yet.
    pub fn get_ui_state_json(&self) -> AppResult<Option<String>> {
        let Some(dto) = self.export_relational_state()? else {
            return Ok(None);
        };
        serde_json::to_string(&dto)
            .map(Some)
            .map_err(|e| AppError::internal(format!("Failed to serialize relational state: {e}")))
    }

    /// Transactionally imports a state JSON document directly into normalized relational tables
    /// (`media`, `notes`, `playlists`, `playlist_items`, `settings`) and clears legacy `app_ui_state`.
    pub fn set_ui_state_json(&self, payload_json: &str) -> AppResult<()> {
        self.import_relational_state_json(payload_json)
    }

    /// Clears persisted user state across relational tables and `app_ui_state`.
    pub fn clear_ui_state(&self) -> AppResult<()> {
        self.reset_all_user_data()
    }

    fn migrate_legacy_ui_state_blob(&self) -> AppResult<bool> {
        let legacy_raw: Option<String> = self.with_conn(|conn| {
            let val = conn
                .query_row(
                    "SELECT payload_json FROM app_ui_state WHERE key = 'v2' LIMIT 1;",
                    [],
                    |row| row.get(0),
                )
                .optional()?;
            Ok(val)
        })?;
        let Some(raw) = legacy_raw else {
            return Ok(false);
        };
        let media_count: i64 = self.with_conn(|conn| {
            let cnt = conn.query_row("SELECT COUNT(*) FROM media;", [], |r| r.get(0))?;
            Ok(cnt)
        })?;
        if media_count == 0 && raw.trim().starts_with('{') && raw.trim() != "{}" {
            self.import_relational_state_json(&raw)?;
        }
        self.with_conn(|conn| {
            conn.execute("DELETE FROM app_ui_state;", [])?;
            Ok(())
        })?;
        Ok(true)
    }

    /// Queries normalized relational tables (`media`, `notes`, `playlists`, `playlist_items`, `settings`).
    pub fn export_relational_state(&self) -> AppResult<Option<RelationalAppStateDto>> {
        self.with_conn(|conn| {
            let mut media_stmt = conn.prepare(
                "SELECT m.id, m.path, m.display_name, m.media_kind, m.duration_ms,
                        m.duration_probed, m.last_position_ms, m.completed, m.file_size_bytes,
                        m.fingerprint, m.availability, m.added_at, m.last_opened_at,
                        m.last_seen_at, m.source_folder_id,
                        (SELECT COUNT(*) FROM notes n WHERE n.media_id = m.id) AS note_count
                 FROM media m
                 ORDER BY COALESCE(m.last_opened_at, m.added_at) DESC, m.display_name ASC",
            )?;
            let media_rows = media_stmt.query_map([], |row| {
                let kind_str: String = row.get("media_kind")?;
                let avail_str: String = row.get("availability")?;
                let dur_i64: Option<i64> = row.get("duration_ms")?;
                let probed_int: i64 = row.get("duration_probed").unwrap_or(0);
                let pos_i64: i64 = row.get("last_position_ms")?;
                let comp_int: i64 = row.get("completed")?;
                let sz_i64: Option<i64> = row.get("file_size_bytes")?;
                let nc_i64: i64 = row.get("note_count").unwrap_or(0);
                Ok(MediaItem {
                    id: row.get("id")?,
                    path: row.get("path")?,
                    display_name: row.get("display_name")?,
                    media_kind: MediaKind::from_db_str(&kind_str).unwrap_or(MediaKind::Video),
                    duration_ms: dur_i64.and_then(|v| u64::try_from(v).ok()),
                    duration_probed: probed_int != 0,
                    last_position_ms: u64::try_from(pos_i64).unwrap_or(0),
                    completed: comp_int != 0,
                    file_size_bytes: sz_i64.and_then(|v| u64::try_from(v).ok()),
                    fingerprint: row.get("fingerprint")?,
                    availability: Availability::from_db_str(&avail_str)
                        .unwrap_or(Availability::Missing),
                    added_at: row.get("added_at")?,
                    last_opened_at: row.get("last_opened_at")?,
                    last_seen_at: row.get("last_seen_at")?,
                    note_count: usize::try_from(nc_i64).unwrap_or(0),
                    source_folder_id: row.get("source_folder_id").unwrap_or(None),
                })
            })?;
            let mut media = Vec::new();
            for r in media_rows {
                media.push(r?);
            }

            let mut note_stmt = conn.prepare(
                "SELECT id, media_id, position_ms, is_quick_marker, body, created_at, updated_at
                 FROM notes
                 ORDER BY position_ms ASC, created_at ASC",
            )?;
            let note_rows = note_stmt.query_map([], |row| {
                let pos_i64: i64 = row.get("position_ms")?;
                let qm_i64: i64 = row.get("is_quick_marker")?;
                Ok(Note {
                    id: row.get("id")?,
                    media_id: row.get("media_id")?,
                    position_ms: u64::try_from(pos_i64).unwrap_or(0),
                    is_quick_marker: qm_i64 != 0,
                    body: row.get("body")?,
                    created_at: row.get("created_at")?,
                    updated_at: row.get("updated_at")?,
                })
            })?;
            let mut notes = Vec::new();
            for r in note_rows {
                notes.push(r?);
            }

            let mut pl_stmt = conn.prepare(
                "SELECT id, name, description, created_at, updated_at
                 FROM playlists
                 ORDER BY updated_at DESC, created_at DESC",
            )?;
            let pl_rows = pl_stmt.query_map([], |row| {
                Ok((
                    row.get::<_, String>("id")?,
                    row.get::<_, String>("name")?,
                    row.get::<_, String>("description")?,
                    row.get::<_, String>("created_at")?,
                    row.get::<_, String>("updated_at")?,
                ))
            })?;
            let mut item_stmt = conn.prepare(
                "SELECT media_id FROM playlist_items WHERE playlist_id = ?1 ORDER BY position ASC",
            )?;
            let mut playlists = Vec::new();
            for pl_res in pl_rows {
                let (id, name, description, created_at, updated_at) = pl_res?;
                let item_rows = item_stmt.query_map(params![id], |r| r.get::<_, String>(0))?;
                let mut item_ids = Vec::new();
                for ir in item_rows {
                    item_ids.push(ir?);
                }
                playlists.push(Playlist {
                    id,
                    name,
                    description,
                    item_ids,
                    created_at,
                    updated_at,
                });
            }

            let raw_settings: Option<String> = conn
                .query_row(
                    "SELECT value_json FROM settings WHERE key = 'app_preferences_v1'",
                    [],
                    |r| r.get(0),
                )
                .optional()?;
            let raw_session: Option<String> = conn
                .query_row(
                    "SELECT value_json FROM settings WHERE key = 'playback_session_v1'",
                    [],
                    |r| r.get(0),
                )
                .optional()?;

            if media.is_empty()
                && notes.is_empty()
                && playlists.is_empty()
                && raw_settings.is_none()
                && raw_session.is_none()
            {
                return Ok(None);
            }

            let settings = raw_settings
                .as_deref()
                .and_then(|s| serde_json::from_str::<AppSettings>(s).ok())
                .unwrap_or_default();
            let playback_session = raw_session
                .as_deref()
                .and_then(|s| serde_json::from_str::<PersistedPlaybackSession>(s).ok());

            Ok(Some(RelationalAppStateDto {
                media,
                notes,
                playlists,
                settings,
                playback_session,
            }))
        })
    }

    /// Transactionally imports a JSON state document into normalized relational tables.
    pub fn import_relational_state_json(&self, payload_json: &str) -> AppResult<()> {
        let raw_val: serde_json::Value = serde_json::from_str(payload_json)
            .map_err(|e| AppError::validation(format!("Invalid state JSON payload: {e}")))?;
        let Some(obj) = raw_val.as_object() else {
            return Err(AppError::validation("State JSON root must be an object"));
        };

        let mut base_settings_val = serde_json::to_value(AppSettings::default())
            .map_err(|e| AppError::internal(format!("Default settings serialize failed: {e}")))?;
        if let (Some(target_map), Some(src_map)) = (
            base_settings_val.as_object_mut(),
            obj.get("settings").and_then(serde_json::Value::as_object),
        ) {
            for (k, v) in src_map {
                target_map.insert(k.clone(), v.clone());
            }
        }
        let settings: AppSettings =
            serde_json::from_value(base_settings_val).unwrap_or_else(|_| AppSettings::default());

        let media: Vec<MediaItem> = obj
            .get("media")
            .and_then(|v| serde_json::from_value(v.clone()).ok())
            .unwrap_or_default();
        let notes: Vec<Note> = obj
            .get("notes")
            .and_then(|v| serde_json::from_value(v.clone()).ok())
            .unwrap_or_default();
        let playlists: Vec<Playlist> = obj
            .get("playlists")
            .and_then(|v| serde_json::from_value(v.clone()).ok())
            .unwrap_or_default();
        let playback_session: Option<PersistedPlaybackSession> = obj
            .get("playback_session")
            .and_then(|v| serde_json::from_value(v.clone()).ok());

        let settings_json = serde_json::to_string(&settings)
            .map_err(|e| AppError::internal(format!("Settings serialize failed: {e}")))?;
        let session_json = playback_session
            .as_ref()
            .and_then(|s| serde_json::to_string(s).ok());

        self.with_conn(|conn| {
            let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
            tx.execute("DELETE FROM playlist_items;", [])?;
            tx.execute("DELETE FROM playlists;", [])?;
            tx.execute("DELETE FROM notes;", [])?;
            tx.execute("DELETE FROM media;", [])?;
            tx.execute("DELETE FROM app_ui_state;", [])?;

            let mut valid_media_ids = std::collections::HashSet::with_capacity(media.len());
            let mut seen_paths = std::collections::HashSet::with_capacity(media.len());
            for m in &media {
                if !seen_paths.insert(m.path.clone()) {
                    continue;
                }
                valid_media_ids.insert(m.id.clone());
                tx.execute(
                    "INSERT INTO media (
                        id, path, display_name, media_kind, duration_ms, duration_probed,
                        last_position_ms, completed, file_size_bytes, fingerprint, availability,
                        added_at, last_opened_at, last_seen_at, source_folder_id
                    ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)",
                    params![
                        m.id,
                        m.path,
                        m.display_name,
                        m.media_kind.as_str(),
                        m.duration_ms.and_then(|v| i64::try_from(v).ok()),
                        i64::from(m.duration_probed),
                        i64::try_from(m.last_position_ms).unwrap_or(0),
                        i64::from(m.completed),
                        m.file_size_bytes.and_then(|v| i64::try_from(v).ok()),
                        m.fingerprint,
                        m.availability.as_str(),
                        m.added_at,
                        m.last_opened_at,
                        m.last_seen_at,
                        m.source_folder_id,
                    ],
                )?;
            }

            for n in &notes {
                if !valid_media_ids.contains(&n.media_id) {
                    continue;
                }
                tx.execute(
                    "INSERT OR REPLACE INTO notes (
                        id, media_id, position_ms, is_quick_marker, body, created_at, updated_at
                    ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                    params![
                        n.id,
                        n.media_id,
                        i64::try_from(n.position_ms).unwrap_or(0),
                        i64::from(n.is_quick_marker),
                        n.body,
                        n.created_at,
                        n.updated_at,
                    ],
                )?;
            }

            for pl in &playlists {
                tx.execute(
                    "INSERT OR REPLACE INTO playlists (id, name, description, created_at, updated_at)
                     VALUES (?1, ?2, ?3, ?4, ?5)",
                    params![pl.id, pl.name, pl.description, pl.created_at, pl.updated_at],
                )?;
                let mut seen_pl_items = std::collections::HashSet::new();
                let mut pos = 0_i64;
                for mid in &pl.item_ids {
                    if valid_media_ids.contains(mid) && seen_pl_items.insert(mid.clone()) {
                        tx.execute(
                            "INSERT INTO playlist_items (playlist_id, media_id, position, added_at)
                             VALUES (?1, ?2, ?3, ?4)",
                            params![pl.id, mid, pos, pl.updated_at],
                        )?;
                        pos = pos.saturating_add(1);
                    }
                }
            }

            tx.execute(
                "INSERT INTO settings (key, value_json, updated_at)
                 VALUES ('app_preferences_v1', ?1, strftime('%Y-%m-%dT%H:%M:%SZ', 'now'))
                 ON CONFLICT(key) DO UPDATE SET
                   value_json = excluded.value_json,
                   updated_at = excluded.updated_at;",
                params![settings_json],
            )?;

            if let Some(ref s_json) = session_json {
                tx.execute(
                    "INSERT INTO settings (key, value_json, updated_at)
                     VALUES ('playback_session_v1', ?1, strftime('%Y-%m-%dT%H:%M:%SZ', 'now'))
                     ON CONFLICT(key) DO UPDATE SET
                       value_json = excluded.value_json,
                       updated_at = excluded.updated_at;",
                    params![s_json],
                )?;
            }

            tx.commit()?;
            Ok(())
        })
    }

    // =========================================================================
    // O(1) Indexed Relational RPC Operations (Scales to 50,000+ Tracks & Notes)
    // =========================================================================

    /// Upserts one or more media records transactionally without touching notes or playlists.
    pub fn rpc_upsert_media_batch(&self, items: &[MediaItem]) -> AppResult<()> {
        self.with_conn(|conn| {
            let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
            for item in items {
                let existing_path: Option<String> = tx
                    .query_row(
                        "SELECT path FROM media WHERE id = ?1",
                        params![item.id],
                        |r| r.get(0),
                    )
                    .optional()?;
                if let Some(old_path) = existing_path {
                    if old_path != item.path {
                        tx.execute(
                            "UPDATE media
                             SET path = ?2,
                                 display_name = ?3,
                                 media_kind = ?4,
                                 duration_ms = ?5,
                                 duration_probed = ?6,
                                 last_position_ms = ?7,
                                 completed = ?8,
                                 file_size_bytes = ?9,
                                 availability = ?10,
                                 last_opened_at = ?11,
                                 last_seen_at = ?12,
                                 source_folder_id = ?13
                             WHERE id = ?1",
                            params![
                                item.id,
                                item.path,
                                item.display_name,
                                item.media_kind.as_str(),
                                item.duration_ms.and_then(|v| i64::try_from(v).ok()),
                                i64::from(item.duration_probed),
                                i64::try_from(item.last_position_ms).unwrap_or(0),
                                i64::from(item.completed),
                                item.file_size_bytes.and_then(|v| i64::try_from(v).ok()),
                                item.availability.as_str(),
                                item.last_opened_at,
                                item.last_seen_at,
                                item.source_folder_id,
                            ],
                        )?;
                        continue;
                    }
                }
                tx.execute(
                    "INSERT INTO media (
                        id, path, display_name, media_kind, duration_ms, duration_probed,
                        last_position_ms, completed, file_size_bytes, fingerprint, availability,
                        added_at, last_opened_at, last_seen_at, source_folder_id
                    ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)
                    ON CONFLICT(path) DO UPDATE SET
                        display_name = excluded.display_name,
                        media_kind = excluded.media_kind,
                        duration_ms = COALESCE(excluded.duration_ms, media.duration_ms),
                        duration_probed = CASE WHEN excluded.duration_probed != 0 THEN 1 ELSE media.duration_probed END,
                        last_position_ms = excluded.last_position_ms,
                        completed = excluded.completed,
                        file_size_bytes = COALESCE(excluded.file_size_bytes, media.file_size_bytes),
                        availability = excluded.availability,
                        last_opened_at = COALESCE(excluded.last_opened_at, media.last_opened_at),
                        last_seen_at = excluded.last_seen_at,
                        source_folder_id = COALESCE(excluded.source_folder_id, media.source_folder_id)",
                    params![
                        item.id,
                        item.path,
                        item.display_name,
                        item.media_kind.as_str(),
                        item.duration_ms.and_then(|v| i64::try_from(v).ok()),
                        i64::from(item.duration_probed),
                        i64::try_from(item.last_position_ms).unwrap_or(0),
                        i64::from(item.completed),
                        item.file_size_bytes.and_then(|v| i64::try_from(v).ok()),
                        item.fingerprint,
                        item.availability.as_str(),
                        item.added_at,
                        item.last_opened_at,
                        item.last_seen_at,
                        item.source_folder_id,
                    ],
                )?;
            }
            tx.commit()?;
            Ok(())
        })
    }

    /// O(1) indexed playhead progress + optional session snapshot update.
    pub fn rpc_update_progress(
        &self,
        media_id: &str,
        last_position_ms: u64,
        completed: bool,
        session: Option<&PersistedPlaybackSession>,
    ) -> AppResult<()> {
        let session_json = session.and_then(|s| serde_json::to_string(s).ok());
        self.with_conn(|conn| {
            let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
            tx.execute(
                "UPDATE media
                 SET last_position_ms = ?2,
                     completed = ?3,
                     last_seen_at = strftime('%Y-%m-%dT%H:%M:%SZ', 'now')
                 WHERE id = ?1",
                params![
                    media_id,
                    i64::try_from(last_position_ms).unwrap_or(0),
                    i64::from(completed)
                ],
            )?;
            if let Some(ref s_json) = session_json {
                tx.execute(
                    "INSERT INTO settings (key, value_json, updated_at)
                     VALUES ('playback_session_v1', ?1, strftime('%Y-%m-%dT%H:%M:%SZ', 'now'))
                     ON CONFLICT(key) DO UPDATE SET
                       value_json = excluded.value_json,
                       updated_at = excluded.updated_at;",
                    params![s_json],
                )?;
            }
            tx.commit()?;
            Ok(())
        })
    }

    /// Deletes one or more media records (`ON DELETE CASCADE` removes associated notes and playlist entries).
    pub fn rpc_delete_media_ids(&self, media_ids: &[String]) -> AppResult<usize> {
        self.with_conn(|conn| {
            let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let mut total = 0_usize;
            for id in media_ids {
                total = total
                    .saturating_add(tx.execute("DELETE FROM media WHERE id = ?1", params![id])?);
            }
            tx.commit()?;
            Ok(total)
        })
    }

    /// O(1) indexed note upsert.
    pub fn rpc_upsert_note(&self, note: &Note) -> AppResult<()> {
        self.with_conn(|conn| {
            conn.execute(
                "INSERT INTO notes (id, media_id, position_ms, is_quick_marker, body, created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
                 ON CONFLICT(id) DO UPDATE SET
                   position_ms = excluded.position_ms,
                   is_quick_marker = excluded.is_quick_marker,
                   body = excluded.body,
                   updated_at = excluded.updated_at;",
                params![
                    note.id,
                    note.media_id,
                    i64::try_from(note.position_ms).unwrap_or(0),
                    i64::from(note.is_quick_marker),
                    note.body,
                    note.created_at,
                    note.updated_at,
                ],
            )?;
            Ok(())
        })
    }

    /// O(1) indexed note deletion.
    pub fn rpc_delete_note(&self, note_id: &str) -> AppResult<bool> {
        self.with_conn(|conn| {
            let n = conn.execute("DELETE FROM notes WHERE id = ?1", params![note_id])?;
            Ok(n > 0)
        })
    }

    /// Single-playlist transactional upsert (`playlists` + ordered `playlist_items`).
    pub fn rpc_upsert_playlist(&self, playlist: &Playlist) -> AppResult<()> {
        self.with_conn(|conn| {
            let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
            tx.execute(
                "INSERT INTO playlists (id, name, description, created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5)
                 ON CONFLICT(id) DO UPDATE SET
                   name = excluded.name,
                   description = excluded.description,
                   updated_at = excluded.updated_at;",
                params![
                    playlist.id,
                    playlist.name,
                    playlist.description,
                    playlist.created_at,
                    playlist.updated_at
                ],
            )?;
            tx.execute(
                "DELETE FROM playlist_items WHERE playlist_id = ?1",
                params![playlist.id],
            )?;
            let mut seen = std::collections::HashSet::new();
            let mut pos = 0_i64;
            for mid in &playlist.item_ids {
                if !seen.insert(mid.clone()) {
                    continue;
                }
                let exists: Option<i64> = tx
                    .query_row("SELECT 1 FROM media WHERE id = ?1", params![mid], |r| {
                        r.get(0)
                    })
                    .optional()?;
                if exists.is_some() {
                    tx.execute(
                        "INSERT INTO playlist_items (playlist_id, media_id, position, added_at)
                         VALUES (?1, ?2, ?3, ?4)",
                        params![playlist.id, mid, pos, playlist.updated_at],
                    )?;
                    pos = pos.saturating_add(1);
                }
            }
            tx.commit()?;
            Ok(())
        })
    }

    /// O(1) indexed playlist deletion (`ON DELETE CASCADE` cleans up `playlist_items`).
    pub fn rpc_delete_playlist(&self, playlist_id: &str) -> AppResult<bool> {
        self.with_conn(|conn| {
            let n = conn.execute("DELETE FROM playlists WHERE id = ?1", params![playlist_id])?;
            Ok(n > 0)
        })
    }

    /// O(1) settings upsert.
    pub fn rpc_save_settings(&self, settings: &AppSettings) -> AppResult<()> {
        let json = serde_json::to_string(settings)
            .map_err(|e| AppError::internal(format!("Settings serialize failed: {e}")))?;
        self.with_conn(|conn| {
            conn.execute(
                "INSERT INTO settings (key, value_json, updated_at)
                 VALUES ('app_preferences_v1', ?1, strftime('%Y-%m-%dT%H:%M:%SZ', 'now'))
                 ON CONFLICT(key) DO UPDATE SET
                   value_json = excluded.value_json,
                   updated_at = excluded.updated_at;",
                params![json],
            )?;
            Ok(())
        })
    }

    /// O(1) playback session upsert.
    pub fn rpc_save_playback_session(&self, session: &PersistedPlaybackSession) -> AppResult<()> {
        let json = serde_json::to_string(session)
            .map_err(|e| AppError::internal(format!("Playback session serialize failed: {e}")))?;
        self.with_conn(|conn| {
            conn.execute(
                "INSERT INTO settings (key, value_json, updated_at)
                 VALUES ('playback_session_v1', ?1, strftime('%Y-%m-%dT%H:%M:%SZ', 'now'))
                 ON CONFLICT(key) DO UPDATE SET
                   value_json = excluded.value_json,
                   updated_at = excluded.updated_at;",
                params![json],
            )?;
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
