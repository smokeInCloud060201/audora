//! SQLite persistence layer with WAL mode, foreign keys, pre-migration backups, and repositories.

pub mod db;
pub mod media_repo;
pub mod notes_repo;
pub mod playlist_repo;
pub mod settings_repo;

pub use db::{Database, MigrationSpec, MIGRATION_0001_INITIAL_SQL, MIGRATION_0002_PLAYLISTS_SQL};
pub use media_repo::SqliteMediaRepository;
pub use notes_repo::SqliteNotesRepository;
pub use playlist_repo::SqlitePlaylistRepository;
pub use settings_repo::SqliteSettingsRepository;
