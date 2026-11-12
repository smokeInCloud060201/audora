//! SQLite connection manager, WAL configuration, atomic pre-migration backup, and migration runner.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use rusqlite::Connection;

use crate::error::{AppError, AppResult};

/// Embedded SQL for migration `0001_initial.sql`.
pub const MIGRATION_0001_INITIAL_SQL: &str = include_str!("../../migrations/0001_initial.sql");

/// A versioned SQL migration specification.
#[derive(Debug, Clone, Copy)]
pub struct MigrationSpec {
    pub version: i64,
    pub name: &'static str,
    pub sql: &'static str,
}

/// Built-in ordered schema migrations.
pub const BUILTIN_MIGRATIONS: &[MigrationSpec] = &[MigrationSpec {
    version: 1,
    name: "0001_initial",
    sql: MIGRATION_0001_INITIAL_SQL,
}];

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

    /// Opens an on-disk SQLite database with a custom migration slice (used for testing recovery).
    pub fn open_with_migrations(
        path: impl AsRef<Path>,
        migrations: &[MigrationSpec],
    ) -> AppResult<Self> {
        let db_path = path.as_ref().to_path_buf();
        if let Some(parent) = db_path.parent() {
            if !parent.as_os_str().is_empty() {
                fs::create_dir_all(parent).map_err(|e| {
                    AppError::persistence(format!("Failed to create database directory: {e}"))
                })?;
            }
        }

        let conn = Connection::open(&db_path)?;
        Self::configure_pragmas(&conn)?;
        Self::ensure_migrations_table(&conn)?;

        let current_version = Self::current_schema_version(&conn)?;
        let has_pending = migrations.iter().any(|m| m.version > current_version);

        // If the database file already has tables/data and new migrations are pending, create an
        // atomic pre-migration backup before running migrations (`docs/ARCHITECTURE.md`).
        let mut last_backup_path = None;
        if has_pending && current_version >= 1 && db_path.exists() {
            let backup_path = db_path.with_extension(format!("sqlite.pre-v{current_version}.bak"));
            fs::copy(&db_path, &backup_path).map_err(|e| {
                AppError::persistence(format!(
                    "Failed to create pre-migration backup before upgrading schema: {e}"
                ))
            })?;
            last_backup_path = Some(backup_path);
        }

        let mut db = Self {
            conn: Arc::new(Mutex::new(conn)),
            db_path: Some(db_path),
            last_backup_path,
        };
        db.apply_migrations(migrations)?;
        Ok(db)
    }

    /// Opens an in-memory SQLite database with built-in migrations applied (for fast unit tests).
    pub fn open_in_memory() -> AppResult<Self> {
        let conn = Connection::open_in_memory()?;
        Self::configure_pragmas(&conn)?;
        Self::ensure_migrations_table(&conn)?;
        let mut db = Self {
            conn: Arc::new(Mutex::new(conn)),
            db_path: None,
            last_backup_path: None,
        };
        db.apply_migrations(BUILTIN_MIGRATIONS)?;
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
        self.with_conn(|conn| Self::current_schema_version(conn))
    }

    fn configure_pragmas(conn: &Connection) -> AppResult<()> {
        conn.pragma_update(None, "foreign_keys", "ON")?;
        let _ = conn.pragma_update(None, "journal_mode", "WAL");
        conn.pragma_update(None, "synchronous", "NORMAL")?;
        Ok(())
    }

    fn ensure_migrations_table(conn: &Connection) -> AppResult<()> {
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS schema_migrations (
                version INTEGER PRIMARY KEY NOT NULL,
                name TEXT NOT NULL,
                applied_at TEXT NOT NULL
            );",
        )?;
        Ok(())
    }

    fn current_schema_version(conn: &Connection) -> AppResult<i64> {
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
                let tx = conn.transaction()?;
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
                    "INSERT INTO schema_migrations (version, name, applied_at) VALUES (?1, ?2, datetime('now'))",
                    rusqlite::params![migration.version, migration.name],
                )?;
                tx.commit()?;
            }
            Ok(())
        })
    }
}
