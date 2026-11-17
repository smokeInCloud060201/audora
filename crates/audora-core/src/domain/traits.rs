//! Domain traits decoupling application services from concrete playback decoders and SQL storage.

use std::path::Path;

use crate::domain::models::{
    AppSettings, Availability, CreateNoteInput, CreatePlaylistInput, LibraryFilter, MediaId,
    MediaItem, MediaKind, Note, NoteId, PlaybackState, Playlist, PlaylistId, UpdateNoteInput,
    UpdatePlaylistInput,
};
use crate::error::AppResult;

/// Metadata probed from a local media file by the playback backend.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProbedMediaMetadata {
    pub media_kind: MediaKind,
    pub duration_ms: Option<u64>,
    pub container_format: Option<String>,
    pub video_codec: Option<String>,
    pub audio_codec: Option<String>,
}

/// Abstraction over the media playback and stream-probing engine (`ADR 0001`).
pub trait PlaybackBackend: Send + Sync {
    /// Probes container, codec, and duration metadata without modifying the source file.
    fn probe_media(&self, path: &Path) -> AppResult<ProbedMediaMetadata>;

    /// Loads a media file under a monotonically increasing load `generation`.
    fn load(&self, path: &Path, generation: u64) -> AppResult<ProbedMediaMetadata>;

    /// Transitions playback to `Playing`.
    fn play(&self, generation: u64) -> AppResult<()>;

    /// Transitions playback to `Paused`.
    fn pause(&self, generation: u64) -> AppResult<()>;

    /// Seeks to `position_ms` (clamped to stream duration when known).
    fn seek(&self, generation: u64, position_ms: u64) -> AppResult<u64>;

    /// Adjusts playback speed multiplier (`0.5..=2.0`).
    fn set_rate(&self, generation: u64, rate: f64) -> AppResult<()>;

    /// Adjusts output volume (`0.0..=1.0`) and mute flag.
    fn set_volume(&self, volume: f64, muted: bool) -> AppResult<()>;

    /// Stops playback and releases active decoder resources cleanly.
    fn stop(&self) -> AppResult<()>;

    /// Returns the backend's current presentation state and position.
    fn current_state(&self) -> (u64, PlaybackState, u64);
}

/// Parameters for updating a media record's locator and probed metadata during relinking.
#[derive(Debug, Clone)]
pub struct RelinkMediaRecord<'a> {
    pub id: &'a str,
    pub new_path: &'a str,
    pub display_name: &'a str,
    pub media_kind: MediaKind,
    pub duration_ms: Option<u64>,
    pub duration_probed: bool,
    pub file_size_bytes: Option<u64>,
    pub updated_at: &'a str,
}

/// Persistence contract for media library records.
pub trait MediaRepository: Send + Sync {
    fn upsert_media(&self, item: &MediaItem) -> AppResult<MediaItem>;
    fn get_by_id(&self, id: &str) -> AppResult<Option<MediaItem>>;
    fn get_by_path(&self, path: &str) -> AppResult<Option<MediaItem>>;
    fn list_media(&self, query: Option<&str>, filter: LibraryFilter) -> AppResult<Vec<MediaItem>>;
    fn update_progress(
        &self,
        id: &str,
        last_position_ms: u64,
        completed: bool,
        updated_at: &str,
    ) -> AppResult<()>;
    fn update_availability(
        &self,
        id: &str,
        availability: Availability,
        updated_at: &str,
    ) -> AppResult<()>;
    fn relink_path(&self, record: &RelinkMediaRecord<'_>) -> AppResult<MediaItem>;
    fn delete_media(&self, id: &str) -> AppResult<bool>;
}

/// Persistence contract for timestamped notes and quick markers.
pub trait NotesRepository: Send + Sync {
    fn create_note(&self, id: NoteId, input: &CreateNoteInput, now_iso: &str) -> AppResult<Note>;
    fn get_note(&self, id: &str) -> AppResult<Option<Note>>;
    fn update_note(&self, input: &UpdateNoteInput, now_iso: &str) -> AppResult<Note>;
    fn delete_note(&self, id: &str) -> AppResult<bool>;
    fn list_notes_for_media(&self, media_id: &str) -> AppResult<Vec<Note>>;
    fn search_notes(&self, query: &str, filter: LibraryFilter) -> AppResult<Vec<Note>>;
}

/// Persistence contract for typed application settings.
pub trait SettingsRepository: Send + Sync {
    fn load_settings(&self) -> AppResult<AppSettings>;
    fn save_settings(&self, settings: &AppSettings, now_iso: &str) -> AppResult<()>;
    fn schema_version(&self) -> AppResult<i64> {
        Ok(4)
    }
}

/// Persistence contract for user playlists and ordered playlist items.
pub trait PlaylistRepository: Send + Sync {
    fn create_playlist(
        &self,
        id: PlaylistId,
        input: &CreatePlaylistInput,
        now_iso: &str,
    ) -> AppResult<Playlist>;
    fn get_playlist(&self, id: &str) -> AppResult<Option<Playlist>>;
    fn list_playlists(&self) -> AppResult<Vec<Playlist>>;
    fn update_playlist(&self, input: &UpdatePlaylistInput, now_iso: &str) -> AppResult<Playlist>;
    fn delete_playlist(&self, id: &str) -> AppResult<bool>;
    fn add_items_to_playlist(
        &self,
        playlist_id: &str,
        media_ids: &[MediaId],
        now_iso: &str,
    ) -> AppResult<Playlist>;
    fn remove_item_from_playlist(
        &self,
        playlist_id: &str,
        media_id: &str,
        now_iso: &str,
    ) -> AppResult<Playlist>;
    fn move_playlist_item(
        &self,
        playlist_id: &str,
        media_id: &str,
        target_index: usize,
        now_iso: &str,
    ) -> AppResult<Playlist>;
}
