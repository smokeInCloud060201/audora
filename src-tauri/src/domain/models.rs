//! Domain data contracts for media library items, playback state, notes, and settings.
//!
//! All persisted timestamps use non-negative integer milliseconds (`u64`).

use serde::{Deserialize, Serialize};

/// Maximum UTF-8 character count allowed for a note body.
pub const MAX_NOTE_BODY_CHARS: usize = 10_000;

/// Maximum UTF-8 character count allowed for search queries.
pub const MAX_SEARCH_QUERY_CHARS: usize = 512;

/// Minimum supported playback speed multiplier.
pub const MIN_PLAYBACK_RATE: f64 = 0.5;

/// Maximum supported playback speed multiplier.
pub const MAX_PLAYBACK_RATE: f64 = 2.0;

/// Stable application-generated identifier for a media library record.
pub type MediaId = String;

/// Stable application-generated identifier for a timestamped note.
pub type NoteId = String;

/// High-level media presentation kind (`Video` vs `Audio`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MediaKind {
    Video,
    Audio,
}

impl MediaKind {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Video => "video",
            Self::Audio => "audio",
        }
    }

    pub fn from_db_str(value: &str) -> Option<Self> {
        match value {
            "video" => Some(Self::Video),
            "audio" => Some(Self::Audio),
            _ => None,
        }
    }
}

/// Reachability status of a media item's local source file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Availability {
    Available,
    Missing,
    Unreadable,
    Unsupported,
}

impl Availability {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Available => "available",
            Self::Missing => "missing",
            Self::Unreadable => "unreadable",
            Self::Unsupported => "unsupported",
        }
    }

    pub fn from_db_str(value: &str) -> Option<Self> {
        match value {
            "available" => Some(Self::Available),
            "missing" => Some(Self::Missing),
            "unreadable" => Some(Self::Unreadable),
            "unsupported" => Some(Self::Unsupported),
            _ => None,
        }
    }
}

/// A local media item stored in the library.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MediaItem {
    pub id: MediaId,
    pub path: String,
    pub display_name: String,
    pub media_kind: MediaKind,
    pub duration_ms: Option<u64>,
    pub last_position_ms: u64,
    pub completed: bool,
    pub file_size_bytes: Option<u64>,
    pub fingerprint: Option<String>,
    pub availability: Availability,
    pub added_at: String,
    pub last_opened_at: Option<String>,
    pub last_seen_at: String,
    pub note_count: usize,
}

impl MediaItem {
    /// Computes playback completion percentage (`0..=100`) for the Library table (`4:206`).
    #[must_use]
    pub fn progress_percent(&self) -> u8 {
        if self.completed {
            return 100;
        }
        let Some(duration_ms) = self.duration_ms else {
            return 0;
        };
        if duration_ms == 0 || self.last_position_ms == 0 {
            return 0;
        }
        let clamped = self.last_position_ms.min(duration_ms);
        let pct = (clamped.saturating_mul(100)) / duration_ms;
        u8::try_from(pct.min(100)).unwrap_or(100)
    }
}

/// Explicit playback state machine states (`docs/ARCHITECTURE.md`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PlaybackState {
    Idle,
    Loading,
    Playing,
    Paused,
    Seeking,
    Ended,
    Failed,
}

/// Session queue repeat behavior.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RepeatMode {
    Off,
    All,
    One,
}

/// Authoritative snapshot of the playback service and active queue.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PlaybackSnapshot {
    pub generation: u64,
    pub state: PlaybackState,
    pub media_id: Option<MediaId>,
    pub position_ms: u64,
    pub duration_ms: Option<u64>,
    pub rate: f64,
    pub volume: f64,
    pub muted: bool,
    pub repeat_mode: RepeatMode,
    pub shuffle: bool,
    pub queue: Vec<MediaId>,
    pub error: Option<crate::error::AppErrorPayload>,
}

impl Default for PlaybackSnapshot {
    fn default() -> Self {
        Self {
            generation: 0,
            state: PlaybackState::Idle,
            media_id: None,
            position_ms: 0,
            duration_ms: None,
            rate: 1.0,
            volume: 1.0,
            muted: false,
            repeat_mode: RepeatMode::Off,
            shuffle: false,
            queue: Vec::new(),
            error: None,
        }
    }
}

/// Commands accepted by the playback service.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum PlaybackCommand {
    Play,
    Pause,
    TogglePlayPause,
    Seek { position_ms: u64 },
    SkipRelative { delta_ms: i64 },
    SetRate { rate: f64 },
    SetVolume { volume: f64 },
    SetMuted { muted: bool },
    SetRepeat { mode: RepeatMode },
    SetShuffle { shuffle: bool },
    Next,
    Previous,
    Stop,
}

/// Asynchronous events emitted by a playback backend for a specific load generation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum BackendEvent {
    Ready { duration_ms: Option<u64> },
    PositionChanged { position_ms: u64 },
    StateChanged { state: PlaybackState },
    Ended,
    Failed { detail: String },
}

/// A timestamped note or quick marker attached to a media item.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Note {
    pub id: NoteId,
    pub media_id: MediaId,
    pub position_ms: u64,
    pub is_quick_marker: bool,
    pub body: String,
    pub created_at: String,
    pub updated_at: String,
}

/// Input payload for creating a note at a captured playhead position (`Frame 4:88`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CreateNoteInput {
    pub media_id: MediaId,
    pub position_ms: u64,
    pub is_quick_marker: bool,
    pub body: String,
}

/// Input payload for updating an existing note's text.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UpdateNoteInput {
    pub id: NoteId,
    pub body: String,
}

/// Filter tabs in the Media Library (`All | Video | Audio` in `Frame 4:206`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LibraryFilter {
    All,
    Video,
    Audio,
}

/// Combined offline search results for media items and matching notes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LibrarySearchResult {
    pub media_items: Vec<MediaItem>,
    pub matching_notes: Vec<Note>,
}

/// Application visual theme preference (`Frame 4:441`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ThemeMode {
    Dark,
    Light,
    System,
}

/// Timestamp precision preference (`Nearest second` vs `Milliseconds` in `Frame 4:562`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TimestampPrecision {
    NearestSecond,
    Milliseconds,
}

/// Window layout mode (`Full mode` `2:3` vs `Minimum mode` `2:103`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum WindowMode {
    Full,
    Minimum,
}

/// Persisted user preferences (`Frame 4:441`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AppSettings {
    pub theme: ThemeMode,
    pub resume_where_left_off: bool,
    pub pause_on_headphones_disconnect: bool,
    pub default_playback_speed: f64,
    pub timestamp_precision: TimestampPrecision,
    pub window_mode: WindowMode,
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            theme: ThemeMode::Dark,
            resume_where_left_off: true,
            pause_on_headphones_disconnect: true,
            default_playback_speed: 1.0,
            timestamp_precision: TimestampPrecision::NearestSecond,
            window_mode: WindowMode::Full,
        }
    }
}

/// Redacted, user-reviewable diagnostic report (`docs/SECURITY.md`).
///
/// Never includes file paths, user home directories, or note bodies.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiagnosticReport {
    pub app_version: String,
    pub os_family: String,
    pub arch: String,
    pub schema_version: i64,
    pub media_count: usize,
    pub missing_media_count: usize,
    pub total_note_count: usize,
    pub playback_state: PlaybackState,
    pub last_error_code: Option<String>,
    pub generated_at: String,
}
