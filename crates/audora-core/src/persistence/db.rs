//! SQLite connection manager, WAL configuration, atomic pre-migration backup, and migration runner.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

use rusqlite::{
    params, Connection, ErrorCode as SqliteErrorCode, OptionalExtension, TransactionBehavior,
};

use crate::domain::{
    current_iso8601_utc, normalize_persisted_timestamp, validate_local_media_path_str,
    validate_note_body, validate_playback_rate, validate_playlist_fields, validate_volume,
    AppSettings, Availability, MediaItem, MediaKind, Note, PersistedPlaybackSession, Playlist,
    RelationalAppStateDto,
};
use crate::error::{redact_path, AppError, AppResult};

fn set_private_dir_permissions(path: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(path, fs::Permissions::from_mode(0o700));
    }
    #[cfg(not(unix))]
    let _ = path;
}

fn set_private_file_permissions(path: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if path.is_file() {
            let _ = fs::set_permissions(path, fs::Permissions::from_mode(0o600));
        }
    }
    #[cfg(not(unix))]
    let _ = path;
}

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
        let precheck_corrupt = if db_path.is_file() {
            match fs::File::open(&db_path) {
                Ok(mut f) => {
                    use std::io::Read;
                    let len = f.metadata().map(|m| m.len()).unwrap_or(0);
                    if len == 0 {
                        false
                    } else if len < 100 {
                        true
                    } else {
                        let mut magic = [0_u8; 16];
                        f.read_exact(&mut magic).is_err() || &magic != b"SQLite format 3\0"
                    }
                }
                Err(_) => false,
            }
        } else {
            false
        };

        if precheck_corrupt {
            tracing::warn!(
                path = %redact_path(&db_path),
                "Database header corrupted; preserving corrupt db and wal before recovery"
            );
            Self::preserve_corrupt_wal_if_present(&db_path);
        } else {
            match Self::open_internal(&db_path, BUILTIN_MIGRATIONS) {
                Ok(db) => match db.quick_check_healthy() {
                    Ok(true) => return Ok(db),
                    Ok(false) => {
                        tracing::warn!(
                            path = %redact_path(&db_path),
                            "Database quick_check failed; preserving corrupt file before recovery"
                        );
                        // Preserve `-wal` BEFORE dropping `db`, because `sqlite3_close` can unlink `-wal`.
                        Self::preserve_corrupt_wal_if_present(&db_path);
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
                set_private_file_permissions(&corrupt_bak);
            }
            let wal_path = PathBuf::from(format!("{}-wal", db_path.display()));
            let shm_path = PathBuf::from(format!("{}-shm", db_path.display()));
            Self::preserve_corrupt_wal_if_present(&db_path);
            let _ = fs::remove_file(&wal_path);
            let _ = fs::remove_file(&shm_path);

            let candidates = Self::discover_backup_candidates(&db_path);
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
                let _ = fs::remove_file(&db_path);
            }
            let _ = fs::remove_file(&db_path);
        }

        let mut recovered = Self::open(&db_path)?;
        if corrupt_bak.exists() {
            recovered.last_backup_path = Some(corrupt_bak);
        }
        Ok(recovered)
    }

    fn preserve_corrupt_wal_if_present(db_path: &Path) {
        let wal_path = PathBuf::from(format!("{}-wal", db_path.display()));
        if wal_path.exists() {
            let wal_len = fs::metadata(&wal_path).map(|m| m.len()).unwrap_or(0);
            if wal_len > 0 {
                let wal_corrupt_bak =
                    PathBuf::from(format!("{}-wal.corrupt.bak", db_path.display()));
                if !wal_corrupt_bak.exists() && fs::copy(&wal_path, &wal_corrupt_bak).is_ok() {
                    set_private_file_permissions(&wal_corrupt_bak);
                }
            }
        }
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

    /// Runs `PRAGMA integrity_check` to verify SQLite B-tree and page consistency,
    /// treating `SQLITE_CORRUPT` / `SQLITE_NOTADB` errors as `Ok(false)` (`H-4`).
    pub fn verify_integrity(&self) -> AppResult<bool> {
        self.with_conn(|conn| {
            match conn.query_row("PRAGMA integrity_check;", [], |row| row.get::<_, String>(0)) {
                Ok(status) => Ok(status.eq_ignore_ascii_case("ok")),
                Err(e) if Self::is_sqlite_corruption_error(&e) => Ok(false),
                Err(e) => Err(AppError::from(e)),
            }
        })
    }

    /// Runs fast `PRAGMA quick_check(1)` on startup so 50,000-row databases boot rapidly while still
    /// catching interior-page corruption (`H-4`).
    fn quick_check_healthy(&self) -> AppResult<bool> {
        self.with_conn(|conn| {
            match conn.query_row("PRAGMA quick_check(1);", [], |row| row.get::<_, String>(0)) {
                Ok(status) => Ok(status.eq_ignore_ascii_case("ok")),
                Err(e) if Self::is_sqlite_corruption_error(&e) => Ok(false),
                Err(e) => Err(AppError::from(e)),
            }
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
            let checkpoint_complete = conn
                .query_row("PRAGMA wal_checkpoint(TRUNCATE);", [], |row| {
                    let busy: i64 = row.get(0)?;
                    Ok(busy == 0)
                })
                .unwrap_or(false);
            let _ = fs::remove_file(&tmp_backup);
            let vacuum_ok = tmp_backup
                .to_str()
                .is_some_and(|s| conn.execute("VACUUM INTO ?1;", [s]).is_ok());
            if !vacuum_ok {
                let _ = fs::remove_file(&tmp_backup);
                if !checkpoint_complete {
                    return Err(AppError::persistence(
                        "Failed to checkpoint WAL before manual database backup.",
                    ));
                }
                fs::copy(db_path, &tmp_backup).map_err(|io_err| {
                    let _ = fs::remove_file(&tmp_backup);
                    tracing::error!(
                        path = %redact_path(&backup_path),
                        "Failed to stage manual database backup copy: {io_err}"
                    );
                    AppError::persistence("Failed to create manual database backup.")
                })?;
            }
            set_private_file_permissions(&tmp_backup);
            fs::rename(&tmp_backup, &backup_path).map_err(|io_err| {
                let _ = fs::remove_file(&tmp_backup);
                tracing::error!(
                    path = %redact_path(&backup_path),
                    "Failed to finalize manual database backup: {io_err}"
                );
                AppError::persistence("Failed to create manual database backup.")
            })?;
            set_private_file_permissions(&backup_path);
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
                set_private_dir_permissions(parent);
            }
        }

        let conn = Connection::open(&db_path).map_err(|e| {
            let corrupt = Self::is_sqlite_corruption_error(&e);
            if corrupt {
                Self::preserve_corrupt_wal_if_present(&db_path);
            }
            (AppError::from(e), corrupt)
        })?;
        set_private_file_permissions(&db_path);
        if let Err(e) = Self::configure_base_pragmas(&conn) {
            let corrupt = Self::is_sqlite_corruption_error(&e);
            if corrupt {
                Self::preserve_corrupt_wal_if_present(&db_path);
            }
            return Err((AppError::from(e), corrupt));
        }
        set_private_file_permissions(&PathBuf::from(format!("{}-wal", db_path.display())));
        set_private_file_permissions(&PathBuf::from(format!("{}-shm", db_path.display())));
        if let Err(e) = Self::ensure_migrations_table(&conn) {
            let corrupt = Self::is_sqlite_corruption_error(&e);
            if corrupt {
                Self::preserve_corrupt_wal_if_present(&db_path);
            }
            return Err((AppError::from(e), corrupt));
        }

        let current_version = match Self::current_schema_version(&conn) {
            Ok(v) => v,
            Err(e) => {
                let corrupt = Self::is_sqlite_corruption_error(&e);
                if corrupt {
                    Self::preserve_corrupt_wal_if_present(&db_path);
                }
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
            set_private_file_permissions(&backup_path);
            last_backup_path = Some(backup_path);
        }

        let mut db = Self {
            conn: Arc::new(Mutex::new(conn)),
            db_path: Some(db_path),
            last_backup_path,
        };
        db.apply_migrations(migrations)?;
        db.enable_foreign_keys()?;
        if db.schema_version().unwrap_or(0) >= 4 {
            if let Err(err) = db.migrate_legacy_ui_state_blob() {
                tracing::warn!(
                    code = err.code().as_str(),
                    "Deferred legacy app_ui_state migration; preserving blob for retry"
                );
            }
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
        db.apply_migrations(BUILTIN_MIGRATIONS)
            .map_err(|(e, _)| e)?;
        db.enable_foreign_keys().map_err(|(e, _)| e)?;
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

    fn parse_lenient_state_object(
        obj: &serde_json::Map<String, serde_json::Value>,
    ) -> (
        AppSettings,
        Vec<MediaItem>,
        Vec<Note>,
        Vec<Playlist>,
        Option<PersistedPlaybackSession>,
    ) {
        let mut base_settings_val =
            serde_json::to_value(AppSettings::default()).unwrap_or(serde_json::Value::Null);
        if let (Some(target_map), Some(src_map)) = (
            base_settings_val.as_object_mut(),
            obj.get("settings").and_then(serde_json::Value::as_object),
        ) {
            for (k, v) in src_map {
                target_map.insert(k.clone(), v.clone());
            }
        }
        let mut settings: AppSettings =
            serde_json::from_value(base_settings_val).unwrap_or_default();
        if !(crate::domain::MIN_PLAYBACK_RATE..=crate::domain::MAX_PLAYBACK_RATE)
            .contains(&settings.default_playback_speed)
            || !settings.default_playback_speed.is_finite()
        {
            settings.default_playback_speed = 1.0;
        }
        let now_iso = current_iso8601_utc();
        settings.source_folders.retain_mut(|f| {
            let Ok(clean) = validate_local_media_path_str(&f.path) else {
                return false;
            };
            let clean_owned = clean.to_owned();
            if f.label.trim().is_empty() {
                f.label = Path::new(&clean_owned)
                    .file_name()
                    .and_then(|s| s.to_str())
                    .unwrap_or(&clean_owned)
                    .to_owned();
            }
            f.path = clean_owned;
            f.added_at = normalize_persisted_timestamp(&f.added_at, &now_iso);
            f.last_scanned_at = normalize_persisted_timestamp(&f.last_scanned_at, &now_iso);
            true
        });

        let mut media = Vec::new();
        if let Some(arr) = obj.get("media").and_then(serde_json::Value::as_array) {
            for elem in arr {
                match serde_json::from_value::<MediaItem>(elem.clone()) {
                    Ok(mut m) => {
                        if m.id.trim().is_empty() || validate_local_media_path_str(&m.path).is_err()
                        {
                            continue;
                        }
                        if m.display_name.trim().is_empty() {
                            m.display_name = Path::new(&m.path)
                                .file_name()
                                .and_then(|s| s.to_str())
                                .unwrap_or(&m.path)
                                .to_owned();
                        }
                        m.added_at = normalize_persisted_timestamp(&m.added_at, &now_iso);
                        m.last_seen_at = normalize_persisted_timestamp(&m.last_seen_at, &now_iso);
                        m.last_opened_at = m
                            .last_opened_at
                            .as_deref()
                            .filter(|s| !s.trim().is_empty())
                            .map(|s| normalize_persisted_timestamp(s, &now_iso));
                        media.push(m);
                    }
                    Err(e) => {
                        tracing::warn!("Skipping malformed media item during state import: {e}");
                    }
                }
            }
        }

        let mut notes = Vec::new();
        if let Some(arr) = obj.get("notes").and_then(serde_json::Value::as_array) {
            for elem in arr {
                match serde_json::from_value::<Note>(elem.clone()) {
                    Ok(mut n) => {
                        if n.id.trim().is_empty() || n.media_id.trim().is_empty() {
                            continue;
                        }
                        let Ok(clean_body) = validate_note_body(&n.body, n.is_quick_marker) else {
                            continue;
                        };
                        let clean_owned = clean_body.to_owned();
                        n.is_quick_marker = clean_owned.is_empty() && n.is_quick_marker;
                        n.body = clean_owned;
                        n.created_at = normalize_persisted_timestamp(&n.created_at, &now_iso);
                        n.updated_at = normalize_persisted_timestamp(&n.updated_at, &now_iso);
                        notes.push(n);
                    }
                    Err(e) => {
                        tracing::warn!("Skipping malformed note during state import: {e}");
                    }
                }
            }
        }

        let mut playlists = Vec::new();
        if let Some(arr) = obj.get("playlists").and_then(serde_json::Value::as_array) {
            for elem in arr {
                match serde_json::from_value::<Playlist>(elem.clone()) {
                    Ok(mut pl) => {
                        if pl.id.trim().is_empty() {
                            continue;
                        }
                        let Ok((clean_name, clean_desc)) =
                            validate_playlist_fields(&pl.name, &pl.description)
                        else {
                            continue;
                        };
                        let clean_name = clean_name.to_owned();
                        let clean_desc = clean_desc.to_owned();
                        pl.name = clean_name;
                        pl.description = clean_desc;
                        pl.created_at = normalize_persisted_timestamp(&pl.created_at, &now_iso);
                        pl.updated_at = normalize_persisted_timestamp(&pl.updated_at, &now_iso);
                        playlists.push(pl);
                    }
                    Err(e) => {
                        tracing::warn!("Skipping malformed playlist during state import: {e}");
                    }
                }
            }
        }

        let playback_session: Option<PersistedPlaybackSession> = obj
            .get("playback_session")
            .and_then(|v| serde_json::from_value::<PersistedPlaybackSession>(v.clone()).ok())
            .map(|mut s| {
                if !s.volume.is_finite() {
                    s.volume = 1.0;
                } else {
                    s.volume = s.volume.clamp(0.0, 1.0);
                }
                if !s.rate.is_finite() {
                    s.rate = 1.0;
                } else {
                    s.rate = s.rate.clamp(
                        crate::domain::MIN_PLAYBACK_RATE,
                        crate::domain::MAX_PLAYBACK_RATE,
                    );
                }
                s
            });

        (settings, media, notes, playlists, playback_session)
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
        let trimmed = raw.trim();
        if trimmed.is_empty() || trimmed == "{}" {
            self.with_conn(|conn| {
                conn.execute("DELETE FROM app_ui_state;", [])?;
                Ok(())
            })?;
            return Ok(true);
        }

        let raw_val: serde_json::Value = serde_json::from_str(trimmed).map_err(|e| {
            AppError::validation(format!("Legacy app_ui_state JSON parse failed: {e}"))
        })?;
        let Some(obj) = raw_val.as_object() else {
            return Err(AppError::validation(
                "Legacy app_ui_state root is not a JSON object",
            ));
        };

        let (settings, media, notes, playlists, playback_session) =
            Self::parse_lenient_state_object(obj);
        let settings_json = serde_json::to_string(&settings)
            .map_err(|e| AppError::internal(format!("Settings serialize failed: {e}")))?;
        let session_json = playback_session
            .as_ref()
            .and_then(|s| serde_json::to_string(s).ok());

        // Non-destructively merge legacy `v2` blob onto any existing relational rows (e.g. CLI opens)
        // and purge `app_ui_state` inside the exact same `IMMEDIATE` transaction (`C-1`).
        self.with_conn(|conn| {
            let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
            for m in &media {
                Self::reconcile_upsert_media_in_tx(&tx, m, false)?;
            }
            for n in &notes {
                let exists: Option<i64> = tx
                    .query_row(
                        "SELECT 1 FROM media WHERE id = ?1",
                        params![n.media_id],
                        |r| r.get(0),
                    )
                    .optional()?;
                if exists.is_some() {
                    tx.execute(
                        "INSERT INTO notes (id, media_id, position_ms, is_quick_marker, body, created_at, updated_at)
                         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
                         ON CONFLICT(id) DO UPDATE SET
                           media_id = excluded.media_id,
                           position_ms = excluded.position_ms,
                           is_quick_marker = excluded.is_quick_marker,
                           body = excluded.body,
                           updated_at = excluded.updated_at;",
                        params![
                            n.id,
                            n.media_id,
                            i64::try_from(n.position_ms).unwrap_or(i64::MAX),
                            i64::from(n.is_quick_marker),
                            n.body,
                            n.created_at,
                            n.updated_at,
                        ],
                    )?;
                }
            }
            for pl in &playlists {
                tx.execute(
                    "INSERT INTO playlists (id, name, description, created_at, updated_at)
                     VALUES (?1, ?2, ?3, ?4, ?5)
                     ON CONFLICT(id) DO UPDATE SET
                       name = excluded.name,
                       description = excluded.description,
                       updated_at = excluded.updated_at;",
                    params![pl.id, pl.name, pl.description, pl.created_at, pl.updated_at],
                )?;
                tx.execute(
                    "DELETE FROM playlist_items WHERE playlist_id = ?1",
                    params![pl.id],
                )?;
                let mut seen_pl_items = std::collections::HashSet::new();
                let mut pos = 0_i64;
                for mid in &pl.item_ids {
                    if !seen_pl_items.insert(mid.clone()) {
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
                            params![pl.id, mid, pos, pl.updated_at],
                        )?;
                        pos = pos.saturating_add(1);
                    }
                }
            }
            if obj.contains_key("settings") {
                tx.execute(
                    "INSERT INTO settings (key, value_json, updated_at)
                     VALUES ('app_preferences_v1', ?1, strftime('%Y-%m-%dT%H:%M:%SZ', 'now'))
                     ON CONFLICT(key) DO UPDATE SET
                       value_json = excluded.value_json,
                       updated_at = excluded.updated_at;",
                    params![settings_json],
                )?;
            }
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
            tx.execute("DELETE FROM app_ui_state;", [])?;
            tx.commit()?;
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
                 ORDER BY COALESCE(m.last_opened_at, m.added_at) DESC, m.rowid DESC, m.display_name ASC",
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
                 ORDER BY position_ms ASC, created_at ASC, id ASC",
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
                 ORDER BY updated_at DESC, created_at DESC, rowid DESC, id ASC",
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
            let mut playlists = Vec::new();
            let mut pl_index_by_id = std::collections::HashMap::new();
            for pl_res in pl_rows {
                let (id, name, description, created_at, updated_at) = pl_res?;
                pl_index_by_id.insert(id.clone(), playlists.len());
                playlists.push(Playlist {
                    id,
                    name,
                    description,
                    item_ids: Vec::new(),
                    created_at,
                    updated_at,
                });
            }
            if !playlists.is_empty() {
                let mut item_stmt = conn.prepare(
                    "SELECT playlist_id, media_id
                     FROM playlist_items
                     ORDER BY playlist_id ASC, position ASC",
                )?;
                let item_rows = item_stmt.query_map([], |r| {
                    Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
                })?;
                for ir in item_rows {
                    let (pid, mid) = ir?;
                    if let Some(&idx) = pl_index_by_id.get(&pid) {
                        playlists[idx].item_ids.push(mid);
                    }
                }
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

    /// Lightweight query returning standalone `media.path`s (`WHERE source_folder_id IS NULL`)
    /// and configured `TrustedSourceFolder`s without reading notes or playlists.
    pub fn query_allowlist_sources(
        &self,
    ) -> AppResult<(Vec<String>, Vec<crate::domain::TrustedSourceFolder>)> {
        self.with_conn(|conn| {
            let mut stmt = conn.prepare("SELECT path FROM media WHERE source_folder_id IS NULL")?;
            let rows = stmt.query_map([], |r| r.get::<_, String>(0))?;
            let mut standalone_paths = Vec::new();
            for r in rows {
                standalone_paths.push(r?);
            }
            let raw_settings: Option<String> = conn
                .query_row(
                    "SELECT value_json FROM settings WHERE key = 'app_preferences_v1'",
                    [],
                    |r| r.get(0),
                )
                .optional()?;
            let folders = raw_settings
                .as_deref()
                .and_then(|s| serde_json::from_str::<AppSettings>(s).ok())
                .map(|s| s.source_folders)
                .unwrap_or_default();
            Ok((standalone_paths, folders))
        })
    }

    /// Transactionally imports a JSON state document into normalized relational tables.
    pub fn import_relational_state_json(&self, payload_json: &str) -> AppResult<()> {
        let raw_val: serde_json::Value = serde_json::from_str(payload_json)
            .map_err(|e| AppError::validation(format!("Invalid state JSON payload: {e}")))?;
        let Some(obj) = raw_val.as_object() else {
            return Err(AppError::validation("State JSON root must be an object"));
        };

        let (settings, media, notes, playlists, playback_session) =
            Self::parse_lenient_state_object(obj);

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
                if !seen_paths.insert(m.path.clone()) || !valid_media_ids.insert(m.id.clone()) {
                    continue;
                }
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
                        m.duration_ms.map(|v| i64::try_from(v).unwrap_or(i64::MAX)),
                        i64::from(m.duration_probed),
                        i64::try_from(m.last_position_ms).unwrap_or(i64::MAX),
                        i64::from(m.completed),
                        m.file_size_bytes
                            .map(|v| i64::try_from(v).unwrap_or(i64::MAX)),
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
                        i64::try_from(n.position_ms).unwrap_or(i64::MAX),
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
            } else {
                tx.execute("DELETE FROM settings WHERE key = 'playback_session_v1';", [])?;
            }

            tx.commit()?;
            Ok(())
        })
    }

    // =========================================================================
    // O(1) Indexed Relational RPC Operations (Scales to 50,000+ Tracks & Notes)
    // =========================================================================

    pub(crate) fn reconcile_upsert_media_in_tx(
        tx: &rusqlite::Transaction<'_>,
        item: &MediaItem,
        authoritative_folder: bool,
    ) -> AppResult<()> {
        if item.id.trim().is_empty() {
            return Err(AppError::validation("Media item ID must not be empty"));
        }
        let clean_path = validate_local_media_path_str(&item.path)?.to_owned();
        let display_name = if item.display_name.trim().is_empty() {
            Path::new(&clean_path)
                .file_name()
                .and_then(|s| s.to_str())
                .unwrap_or(&clean_path)
                .to_owned()
        } else {
            item.display_name.trim().to_owned()
        };
        let now_iso = current_iso8601_utc();
        let added_at = normalize_persisted_timestamp(&item.added_at, &now_iso);
        let last_seen_at = normalize_persisted_timestamp(&item.last_seen_at, &now_iso);
        let last_opened_at = item
            .last_opened_at
            .as_deref()
            .filter(|s| !s.trim().is_empty())
            .map(|s| normalize_persisted_timestamp(s, &now_iso));

        let existing_by_id: Option<String> = tx
            .query_row(
                "SELECT path FROM media WHERE id = ?1",
                params![item.id],
                |r| r.get(0),
            )
            .optional()?;
        struct ExistingMediaByPath {
            id: String,
            duration_ms: Option<i64>,
            duration_probed: i64,
            file_size_bytes: Option<i64>,
            added_at: String,
            source_folder_id: Option<String>,
            last_position_ms: i64,
            completed: bool,
            last_opened_at: Option<String>,
            fingerprint: Option<String>,
        }
        let existing_by_path: Option<ExistingMediaByPath> = tx
            .query_row(
                "SELECT id, duration_ms, duration_probed, file_size_bytes, added_at,
                        source_folder_id, last_position_ms, completed, last_opened_at, fingerprint
                 FROM media WHERE path = ?1",
                params![clean_path],
                |r| {
                    Ok(ExistingMediaByPath {
                        id: r.get(0)?,
                        duration_ms: r.get(1)?,
                        duration_probed: r.get::<_, i64>(2).unwrap_or(0),
                        file_size_bytes: r.get(3)?,
                        added_at: r.get(4)?,
                        source_folder_id: r.get::<_, Option<String>>(5).unwrap_or(None),
                        last_position_ms: r.get::<_, i64>(6).unwrap_or(0),
                        completed: r.get::<_, i64>(7).unwrap_or(0) != 0,
                        last_opened_at: r.get::<_, Option<String>>(8).unwrap_or(None),
                        fingerprint: r.get::<_, Option<String>>(9).unwrap_or(None),
                    })
                },
            )
            .optional()?;

        let dur_param = item
            .duration_ms
            .map(|v| i64::try_from(v).unwrap_or(i64::MAX));
        let size_param = item
            .file_size_bytes
            .map(|v| i64::try_from(v).unwrap_or(i64::MAX));
        let pos_param = i64::try_from(item.last_position_ms).unwrap_or(i64::MAX);

        if let Some(other) = existing_by_path {
            if other.id != item.id {
                if let Some(ref old_path) = existing_by_id {
                    if old_path != &clean_path {
                        return Err(AppError::relink_conflict(
                            "Target file path already belongs to another item in your library.",
                        ));
                    }
                } else {
                    // `other.id` held `clean_path` (e.g. CLI open or v3 migration) while UI generated `item.id` ->
                    // move `other.id` path aside, insert `item.id` preserving prior progress/metadata, re-home children, and remove `other.id`.
                    let temp_path = format!("__rehome__{}__{}", other.id, item.id);
                    tx.execute(
                        "UPDATE media SET path = ?2 WHERE id = ?1",
                        params![other.id, temp_path],
                    )?;
                    let merged_dur = dur_param.or(other.duration_ms);
                    let merged_probed =
                        i64::from(item.duration_probed || other.duration_probed != 0);
                    let merged_size = size_param.or(other.file_size_bytes);
                    let merged_pos = pos_param.max(other.last_position_ms.max(0));
                    let merged_completed = i64::from(item.completed || other.completed);
                    let merged_last_opened = last_opened_at.or(other.last_opened_at);
                    let merged_fingerprint = item.fingerprint.clone().or(other.fingerprint);
                    let merged_folder = if authoritative_folder {
                        item.source_folder_id.clone()
                    } else {
                        item.source_folder_id.clone().or(other.source_folder_id)
                    };
                    tx.execute(
                        "INSERT INTO media (
                            id, path, display_name, media_kind, duration_ms, duration_probed,
                            last_position_ms, completed, file_size_bytes, fingerprint, availability,
                            added_at, last_opened_at, last_seen_at, source_folder_id
                        ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)",
                        params![
                            item.id,
                            clean_path,
                            display_name,
                            item.media_kind.as_str(),
                            merged_dur,
                            merged_probed,
                            merged_pos,
                            merged_completed,
                            merged_size,
                            merged_fingerprint,
                            item.availability.as_str(),
                            other.added_at,
                            merged_last_opened,
                            last_seen_at,
                            merged_folder,
                        ],
                    )?;
                    tx.execute(
                        "UPDATE notes SET media_id = ?1 WHERE media_id = ?2",
                        params![item.id, other.id],
                    )?;
                    tx.execute(
                        "UPDATE OR IGNORE playlist_items SET media_id = ?1 WHERE media_id = ?2",
                        params![item.id, other.id],
                    )?;
                    tx.execute("DELETE FROM media WHERE id = ?1", params![other.id])?;
                    return Ok(());
                }
            }
        }

        if existing_by_id.is_some() {
            if authoritative_folder {
                // `H-2`: Authoritative RPC updates allow clearing `source_folder_id` to `NULL`.
                tx.execute(
                    "UPDATE media
                     SET path = ?2,
                         display_name = ?3,
                         media_kind = ?4,
                         duration_ms = COALESCE(?5, duration_ms),
                         duration_probed = CASE WHEN ?6 != 0 THEN 1 ELSE duration_probed END,
                         last_position_ms = ?7,
                         completed = ?8,
                         file_size_bytes = COALESCE(?9, file_size_bytes),
                         availability = ?10,
                         last_opened_at = COALESCE(?11, last_opened_at),
                         last_seen_at = ?12,
                         source_folder_id = ?13,
                         fingerprint = COALESCE(?14, fingerprint)
                     WHERE id = ?1",
                    params![
                        item.id,
                        clean_path,
                        display_name,
                        item.media_kind.as_str(),
                        dur_param,
                        i64::from(item.duration_probed),
                        pos_param,
                        i64::from(item.completed),
                        size_param,
                        item.availability.as_str(),
                        last_opened_at,
                        last_seen_at,
                        item.source_folder_id,
                        item.fingerprint,
                    ],
                )?;
            } else {
                tx.execute(
                    "UPDATE media
                     SET path = ?2,
                         display_name = ?3,
                         media_kind = ?4,
                         duration_ms = COALESCE(?5, duration_ms),
                         duration_probed = CASE WHEN ?6 != 0 THEN 1 ELSE duration_probed END,
                         last_position_ms = ?7,
                         completed = ?8,
                         file_size_bytes = COALESCE(?9, file_size_bytes),
                         availability = ?10,
                         last_opened_at = COALESCE(?11, last_opened_at),
                         last_seen_at = ?12,
                         source_folder_id = COALESCE(?13, source_folder_id),
                         fingerprint = COALESCE(?14, fingerprint)
                     WHERE id = ?1",
                    params![
                        item.id,
                        clean_path,
                        display_name,
                        item.media_kind.as_str(),
                        dur_param,
                        i64::from(item.duration_probed),
                        pos_param,
                        i64::from(item.completed),
                        size_param,
                        item.availability.as_str(),
                        last_opened_at,
                        last_seen_at,
                        item.source_folder_id,
                        item.fingerprint,
                    ],
                )?;
            }
            return Ok(());
        }

        tx.execute(
            "INSERT INTO media (
                id, path, display_name, media_kind, duration_ms, duration_probed,
                last_position_ms, completed, file_size_bytes, fingerprint, availability,
                added_at, last_opened_at, last_seen_at, source_folder_id
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)",
            params![
                item.id,
                clean_path,
                display_name,
                item.media_kind.as_str(),
                dur_param,
                i64::from(item.duration_probed),
                pos_param,
                i64::from(item.completed),
                size_param,
                item.fingerprint,
                item.availability.as_str(),
                added_at,
                last_opened_at,
                last_seen_at,
                item.source_folder_id,
            ],
        )?;
        Ok(())
    }

    /// Upserts one or more media records transactionally without touching notes or playlists.
    pub fn rpc_upsert_media_batch(&self, items: &[MediaItem]) -> AppResult<()> {
        self.with_conn(|conn| {
            let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
            for item in items {
                Self::reconcile_upsert_media_in_tx(&tx, item, true)?;
            }
            tx.commit()?;
            Ok(())
        })
    }

    fn rpc_update_progress_in_tx(
        tx: &rusqlite::Transaction<'_>,
        media_id: &str,
        last_position_ms: u64,
        completed: bool,
        session: Option<&PersistedPlaybackSession>,
    ) -> AppResult<()> {
        if media_id.trim().is_empty() {
            return Err(AppError::validation("media_id must not be empty"));
        }
        let session_json =
            match session {
                Some(s) => {
                    validate_playback_rate(s.rate)?;
                    validate_volume(s.volume)?;
                    Some(serde_json::to_string(s).map_err(|e| {
                        AppError::internal(format!("Session serialize failed: {e}"))
                    })?)
                }
                None => None,
            };
        let now_iso = current_iso8601_utc();
        tx.execute(
            "UPDATE media
             SET last_position_ms = ?2,
                 completed = ?3,
                 last_seen_at = ?4
             WHERE id = ?1",
            params![
                media_id,
                i64::try_from(last_position_ms).unwrap_or(i64::MAX),
                i64::from(completed),
                now_iso,
            ],
        )?;
        if let Some(ref s_json) = session_json {
            tx.execute(
                "INSERT INTO settings (key, value_json, updated_at)
                 VALUES ('playback_session_v1', ?1, ?2)
                 ON CONFLICT(key) DO UPDATE SET
                   value_json = excluded.value_json,
                   updated_at = excluded.updated_at;",
                params![s_json, now_iso],
            )?;
        }
        Ok(())
    }

    /// O(1) indexed playhead progress + optional session snapshot update.
    pub fn rpc_update_progress(
        &self,
        media_id: &str,
        last_position_ms: u64,
        completed: bool,
        session: Option<&PersistedPlaybackSession>,
    ) -> AppResult<()> {
        self.with_conn(|conn| {
            let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
            Self::rpc_update_progress_in_tx(&tx, media_id, last_position_ms, completed, session)?;
            tx.commit()?;
            Ok(())
        })
    }

    fn rpc_delete_media_ids_in_tx(
        tx: &rusqlite::Transaction<'_>,
        media_ids: &[String],
    ) -> AppResult<usize> {
        let mut total = 0_usize;
        for id in media_ids {
            total =
                total.saturating_add(tx.execute("DELETE FROM media WHERE id = ?1", params![id])?);
        }
        Ok(total)
    }

    /// Deletes one or more media records (`ON DELETE CASCADE` removes associated notes and playlist entries).
    pub fn rpc_delete_media_ids(&self, media_ids: &[String]) -> AppResult<usize> {
        self.with_conn(|conn| {
            let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let total = Self::rpc_delete_media_ids_in_tx(&tx, media_ids)?;
            tx.commit()?;
            Ok(total)
        })
    }

    fn rpc_upsert_note_in_tx(tx: &rusqlite::Transaction<'_>, note: &Note) -> AppResult<()> {
        if note.id.trim().is_empty() || note.media_id.trim().is_empty() {
            return Err(AppError::validation(
                "Note ID and media_id must not be empty",
            ));
        }
        let clean_body = validate_note_body(&note.body, note.is_quick_marker)?.to_owned();
        let now_iso = current_iso8601_utc();
        let created_at = normalize_persisted_timestamp(&note.created_at, &now_iso);
        let updated_at = normalize_persisted_timestamp(&note.updated_at, &now_iso);
        let is_qm = clean_body.is_empty() && note.is_quick_marker;

        let media_exists: Option<i64> = tx
            .query_row(
                "SELECT 1 FROM media WHERE id = ?1",
                params![note.media_id],
                |r| r.get(0),
            )
            .optional()?;
        if media_exists.is_none() {
            return Err(AppError::validation("Referenced media item does not exist"));
        }
        tx.execute(
            "INSERT INTO notes (id, media_id, position_ms, is_quick_marker, body, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
             ON CONFLICT(id) DO UPDATE SET
               media_id = excluded.media_id,
               position_ms = excluded.position_ms,
               is_quick_marker = excluded.is_quick_marker,
               body = excluded.body,
               updated_at = excluded.updated_at;",
            params![
                note.id,
                note.media_id,
                i64::try_from(note.position_ms).unwrap_or(i64::MAX),
                i64::from(is_qm),
                clean_body,
                created_at,
                updated_at,
            ],
        )?;
        Ok(())
    }

    /// O(1) indexed note upsert with domain validation and foreign-key pre-check (`C-2` / `H-6`).
    pub fn rpc_upsert_note(&self, note: &Note) -> AppResult<()> {
        self.with_conn(|conn| {
            let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
            Self::rpc_upsert_note_in_tx(&tx, note)?;
            tx.commit()?;
            Ok(())
        })
    }

    fn rpc_delete_note_in_tx(tx: &rusqlite::Transaction<'_>, note_id: &str) -> AppResult<bool> {
        if note_id.trim().is_empty() {
            return Err(AppError::validation("Note ID must not be empty"));
        }
        let n = tx.execute("DELETE FROM notes WHERE id = ?1", params![note_id])?;
        Ok(n > 0)
    }

    /// O(1) indexed note deletion.
    pub fn rpc_delete_note(&self, note_id: &str) -> AppResult<bool> {
        self.with_conn(|conn| {
            let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let deleted = Self::rpc_delete_note_in_tx(&tx, note_id)?;
            tx.commit()?;
            Ok(deleted)
        })
    }

    fn rpc_upsert_playlist_in_tx(
        tx: &rusqlite::Transaction<'_>,
        playlist: &Playlist,
    ) -> AppResult<()> {
        if playlist.id.trim().is_empty() {
            return Err(AppError::validation("Playlist ID must not be empty"));
        }
        let (clean_name, clean_desc) =
            validate_playlist_fields(&playlist.name, &playlist.description)?;
        let clean_name = clean_name.to_owned();
        let clean_desc = clean_desc.to_owned();
        let now_iso = current_iso8601_utc();
        let created_at = normalize_persisted_timestamp(&playlist.created_at, &now_iso);
        let updated_at = normalize_persisted_timestamp(&playlist.updated_at, &now_iso);

        tx.execute(
            "INSERT INTO playlists (id, name, description, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(id) DO UPDATE SET
               name = excluded.name,
               description = excluded.description,
               updated_at = excluded.updated_at;",
            params![playlist.id, clean_name, clean_desc, created_at, updated_at],
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
                    params![playlist.id, mid, pos, updated_at],
                )?;
                pos = pos.saturating_add(1);
            }
        }
        Ok(())
    }

    /// Single-playlist transactional upsert (`playlists` + ordered `playlist_items`) with domain validation.
    pub fn rpc_upsert_playlist(&self, playlist: &Playlist) -> AppResult<()> {
        self.with_conn(|conn| {
            let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
            Self::rpc_upsert_playlist_in_tx(&tx, playlist)?;
            tx.commit()?;
            Ok(())
        })
    }

    fn rpc_delete_playlist_in_tx(
        tx: &rusqlite::Transaction<'_>,
        playlist_id: &str,
    ) -> AppResult<bool> {
        if playlist_id.trim().is_empty() {
            return Err(AppError::validation("Playlist ID must not be empty"));
        }
        let n = tx.execute("DELETE FROM playlists WHERE id = ?1", params![playlist_id])?;
        Ok(n > 0)
    }

    /// O(1) indexed playlist deletion (`ON DELETE CASCADE` cleans up `playlist_items`).
    pub fn rpc_delete_playlist(&self, playlist_id: &str) -> AppResult<bool> {
        self.with_conn(|conn| {
            let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let deleted = Self::rpc_delete_playlist_in_tx(&tx, playlist_id)?;
            tx.commit()?;
            Ok(deleted)
        })
    }

    fn rpc_save_settings_in_tx(
        tx: &rusqlite::Transaction<'_>,
        settings: &AppSettings,
    ) -> AppResult<()> {
        validate_playback_rate(settings.default_playback_speed)?;
        let now_iso = current_iso8601_utc();
        let mut normalized = settings.clone();
        for folder in &mut normalized.source_folders {
            let clean = validate_local_media_path_str(&folder.path)?.to_owned();
            folder.path = clean;
            folder.added_at = normalize_persisted_timestamp(&folder.added_at, &now_iso);
            folder.last_scanned_at =
                normalize_persisted_timestamp(&folder.last_scanned_at, &now_iso);
        }
        let json = serde_json::to_string(&normalized)
            .map_err(|e| AppError::internal(format!("Settings serialize failed: {e}")))?;
        tx.execute(
            "INSERT INTO settings (key, value_json, updated_at)
             VALUES ('app_preferences_v1', ?1, ?2)
             ON CONFLICT(key) DO UPDATE SET
               value_json = excluded.value_json,
               updated_at = excluded.updated_at;",
            params![json, now_iso],
        )?;
        Ok(())
    }

    /// O(1) settings upsert with playback-rate and folder-path validation (`H-6`).
    pub fn rpc_save_settings(&self, settings: &AppSettings) -> AppResult<()> {
        self.with_conn(|conn| {
            let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
            Self::rpc_save_settings_in_tx(&tx, settings)?;
            tx.commit()?;
            Ok(())
        })
    }

    fn rpc_save_playback_session_in_tx(
        tx: &rusqlite::Transaction<'_>,
        session: &PersistedPlaybackSession,
    ) -> AppResult<()> {
        validate_playback_rate(session.rate)?;
        validate_volume(session.volume)?;
        let now_iso = current_iso8601_utc();
        let json = serde_json::to_string(session)
            .map_err(|e| AppError::internal(format!("Playback session serialize failed: {e}")))?;
        tx.execute(
            "INSERT INTO settings (key, value_json, updated_at)
             VALUES ('playback_session_v1', ?1, ?2)
             ON CONFLICT(key) DO UPDATE SET
               value_json = excluded.value_json,
               updated_at = excluded.updated_at;",
            params![json, now_iso],
        )?;
        Ok(())
    }

    /// O(1) playback session upsert with rate and volume validation (`H-6`).
    pub fn rpc_save_playback_session(&self, session: &PersistedPlaybackSession) -> AppResult<()> {
        self.with_conn(|conn| {
            let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
            Self::rpc_save_playback_session_in_tx(&tx, session)?;
            tx.commit()?;
            Ok(())
        })
    }

    /// Executes an ordered batch of relational RPC operations atomically inside a single
    /// `IMMEDIATE` transaction (`C-8` unload / shutdown flush), rejecting malformed payloads.
    pub fn rpc_execute_batch(&self, ops: &[serde_json::Value]) -> AppResult<()> {
        if ops.is_empty() {
            return Ok(());
        }
        self.with_conn(|conn| {
            let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
            for op_val in ops {
                let Some(op) = op_val.get("op").and_then(serde_json::Value::as_str) else {
                    return Err(AppError::validation(
                        "Batch operation missing string 'op' field",
                    ));
                };
                match op {
                    "update_progress" => {
                        let mid = op_val
                            .get("media_id")
                            .and_then(serde_json::Value::as_str)
                            .ok_or_else(|| {
                                AppError::validation(
                                    "Batch update_progress missing string 'media_id'",
                                )
                            })?;
                        let pos = match op_val.get("last_position_ms") {
                            Some(v) => v
                                .as_u64()
                                .map(|u| u.min(crate::domain::MAX_SAFE_DURATION_MS))
                                .or_else(|| {
                                    v.as_f64().filter(|f| f.is_finite() && *f >= 0.0).map(|f| {
                                        f.round().min(crate::domain::MAX_SAFE_DURATION_MS as f64)
                                            as u64
                                    })
                                })
                                .ok_or_else(|| {
                                    AppError::validation(
                                        "Batch update_progress has invalid 'last_position_ms'",
                                    )
                                })?,
                            None => {
                                return Err(AppError::validation(
                                    "Batch update_progress missing 'last_position_ms'",
                                ));
                            }
                        };
                        let comp = match op_val.get("completed") {
                            Some(serde_json::Value::Bool(b)) => *b,
                            None => false,
                            Some(_) => {
                                return Err(AppError::validation(
                                    "Batch update_progress has invalid 'completed' boolean",
                                ));
                            }
                        };
                        let sess: Option<PersistedPlaybackSession> = match op_val.get("session") {
                            None | Some(serde_json::Value::Null) => None,
                            Some(v) => Some(serde_json::from_value(v.clone()).map_err(|e| {
                                AppError::validation(format!(
                                    "Batch update_progress invalid 'session': {e}"
                                ))
                            })?),
                        };
                        Self::rpc_update_progress_in_tx(&tx, mid, pos, comp, sess.as_ref())?;
                    }
                    "upsert_media_batch" => {
                        let items_val = op_val.get("items").ok_or_else(|| {
                            AppError::validation("Batch upsert_media_batch missing 'items'")
                        })?;
                        let items: Vec<MediaItem> = serde_json::from_value(items_val.clone())
                            .map_err(|e| {
                                AppError::validation(format!(
                                    "Batch upsert_media_batch invalid 'items': {e}"
                                ))
                            })?;
                        for item in &items {
                            Self::reconcile_upsert_media_in_tx(&tx, item, true)?;
                        }
                    }
                    "delete_media_ids" => {
                        let ids_val = op_val.get("ids").ok_or_else(|| {
                            AppError::validation("Batch delete_media_ids missing 'ids'")
                        })?;
                        let ids: Vec<String> =
                            serde_json::from_value(ids_val.clone()).map_err(|e| {
                                AppError::validation(format!(
                                    "Batch delete_media_ids invalid 'ids': {e}"
                                ))
                            })?;
                        Self::rpc_delete_media_ids_in_tx(&tx, &ids)?;
                    }
                    "upsert_note" => {
                        let note_val = op_val.get("note").ok_or_else(|| {
                            AppError::validation("Batch upsert_note missing 'note'")
                        })?;
                        let note: Note = serde_json::from_value(note_val.clone()).map_err(|e| {
                            AppError::validation(format!("Batch upsert_note invalid 'note': {e}"))
                        })?;
                        Self::rpc_upsert_note_in_tx(&tx, &note)?;
                    }
                    "delete_note" => {
                        let id = op_val
                            .get("id")
                            .and_then(serde_json::Value::as_str)
                            .ok_or_else(|| {
                                AppError::validation("Batch delete_note missing string 'id'")
                            })?;
                        Self::rpc_delete_note_in_tx(&tx, id)?;
                    }
                    "upsert_playlist" => {
                        let pl_val = op_val.get("playlist").ok_or_else(|| {
                            AppError::validation("Batch upsert_playlist missing 'playlist'")
                        })?;
                        let pl: Playlist = serde_json::from_value(pl_val.clone()).map_err(|e| {
                            AppError::validation(format!(
                                "Batch upsert_playlist invalid 'playlist': {e}"
                            ))
                        })?;
                        Self::rpc_upsert_playlist_in_tx(&tx, &pl)?;
                    }
                    "delete_playlist" => {
                        let id = op_val
                            .get("id")
                            .and_then(serde_json::Value::as_str)
                            .ok_or_else(|| {
                                AppError::validation("Batch delete_playlist missing string 'id'")
                            })?;
                        Self::rpc_delete_playlist_in_tx(&tx, id)?;
                    }
                    "save_settings" => {
                        let s_val = op_val.get("settings").ok_or_else(|| {
                            AppError::validation("Batch save_settings missing 'settings'")
                        })?;
                        let s: AppSettings =
                            serde_json::from_value(s_val.clone()).map_err(|e| {
                                AppError::validation(format!(
                                    "Batch save_settings invalid 'settings': {e}"
                                ))
                            })?;
                        Self::rpc_save_settings_in_tx(&tx, &s)?;
                    }
                    "save_session" => {
                        let s_val = op_val.get("session").ok_or_else(|| {
                            AppError::validation("Batch save_session missing 'session'")
                        })?;
                        let s: PersistedPlaybackSession = serde_json::from_value(s_val.clone())
                            .map_err(|e| {
                                AppError::validation(format!(
                                    "Batch save_session invalid 'session': {e}"
                                ))
                            })?;
                        Self::rpc_save_playback_session_in_tx(&tx, &s)?;
                    }
                    "revoke_source_folder" => {
                        let p = op_val
                            .get("path")
                            .and_then(serde_json::Value::as_str)
                            .ok_or_else(|| {
                                AppError::validation(
                                    "Batch revoke_source_folder missing string 'path'",
                                )
                            })?;
                        validate_local_media_path_str(p)?;
                    }
                    other => {
                        return Err(AppError::validation(format!(
                            "Unsupported batch RPC operation '{other}'"
                        )));
                    }
                }
            }
            tx.commit()?;
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
        if let Err(err) = conn.pragma_update(None, "journal_mode", "WAL") {
            if Self::is_sqlite_corruption_error(&err) {
                return Err(err);
            }
        }
        conn.pragma_update(None, "synchronous", "NORMAL")?;
        conn.pragma_update(None, "foreign_keys", "OFF")?;
        Ok(())
    }

    fn enable_foreign_keys(&self) -> Result<(), (AppError, bool)> {
        let guard = self.conn.lock().map_err(|_| {
            (
                AppError::internal("Database connection mutex poisoned"),
                false,
            )
        })?;
        guard
            .pragma_update(None, "foreign_keys", "ON")
            .map_err(|e| {
                let corrupt = Self::is_sqlite_corruption_error(&e);
                (AppError::from(e), corrupt)
            })?;
        let has_fk_violation = match guard
            .query_row("PRAGMA foreign_key_check;", [], |_| Ok(true))
            .optional()
        {
            Ok(Some(true)) => true,
            Ok(_) => false,
            Err(e) => {
                let corrupt = Self::is_sqlite_corruption_error(&e);
                return Err((AppError::from(e), corrupt));
            }
        };
        if has_fk_violation {
            return Err((
                AppError::persistence("Database foreign key integrity check failed."),
                false,
            ));
        }
        Ok(())
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

    fn apply_migrations(&mut self, migrations: &[MigrationSpec]) -> Result<(), (AppError, bool)> {
        let mut guard = self.conn.lock().map_err(|_| {
            (
                AppError::internal("Database connection mutex poisoned"),
                false,
            )
        })?;
        let current = Self::current_schema_version(&guard).map_err(|e| {
            let corrupt = Self::is_sqlite_corruption_error(&e);
            (AppError::from(e), corrupt)
        })?;
        for migration in migrations {
            if migration.version <= current {
                continue;
            }
            let tx = guard
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(|e| {
                    let corrupt = Self::is_sqlite_corruption_error(&e);
                    (AppError::from(e), corrupt)
                })?;
            if let Err(err) = tx.execute_batch(migration.sql) {
                let is_corrupt = Self::is_sqlite_corruption_error(&err);
                tracing::error!(
                    version = migration.version,
                    name = migration.name,
                    "Schema migration failed; rolling back transaction: {err}"
                );
                return Err((
                    AppError::persistence(format!(
                        "Schema migration {} ({}) failed; changes were rolled back and backup preserved.",
                        migration.version, migration.name
                    )),
                    is_corrupt,
                ));
            }
            tx.execute(
                "INSERT INTO schema_migrations (version, name, applied_at) VALUES (?1, ?2, strftime('%Y-%m-%dT%H:%M:%SZ', 'now'))",
                rusqlite::params![migration.version, migration.name],
            )
            .map_err(|e| {
                let corrupt = Self::is_sqlite_corruption_error(&e);
                (AppError::from(e), corrupt)
            })?;
            tx.commit().map_err(|e| {
                let corrupt = Self::is_sqlite_corruption_error(&e);
                (AppError::from(e), corrupt)
            })?;
        }
        Ok(())
    }
}
