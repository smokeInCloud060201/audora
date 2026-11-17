//! Domain data contracts for media library items, playback state, notes, and settings.
//!
//! All persisted timestamps use non-negative integer milliseconds (`u64`).

use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Deserializer, Serialize};

use crate::error::{AppError, AppResult};

/// Maximum safe millisecond timestamp stored in SQLite (`i64::MAX`).
pub const MAX_SAFE_DURATION_MS: u64 = i64::MAX as u64;

fn deserialize_u64_lenient<'de, D>(deserializer: D) -> Result<u64, D::Error>
where
    D: Deserializer<'de>,
{
    let opt = Option::<serde_json::Value>::deserialize(deserializer)?;
    let Some(val) = opt else {
        return Ok(0);
    };
    match val {
        serde_json::Value::Number(n) => {
            if let Some(u) = n.as_u64() {
                Ok(u.min(MAX_SAFE_DURATION_MS))
            } else if let Some(i) = n.as_i64() {
                Ok(u64::try_from(i.max(0))
                    .unwrap_or(0)
                    .min(MAX_SAFE_DURATION_MS))
            } else if let Some(f) = n.as_f64() {
                if f.is_finite() && f > 0.0 {
                    let clamped = f.round().min(MAX_SAFE_DURATION_MS as f64);
                    Ok(clamped as u64)
                } else {
                    Ok(0)
                }
            } else {
                Ok(0)
            }
        }
        _ => Ok(0),
    }
}

fn deserialize_opt_u64_lenient<'de, D>(deserializer: D) -> Result<Option<u64>, D::Error>
where
    D: Deserializer<'de>,
{
    let opt = Option::<serde_json::Value>::deserialize(deserializer)?;
    let Some(val) = opt else {
        return Ok(None);
    };
    match val {
        serde_json::Value::Null => Ok(None),
        serde_json::Value::Number(n) => {
            if let Some(u) = n.as_u64() {
                Ok(Some(u.min(MAX_SAFE_DURATION_MS)))
            } else if let Some(i) = n.as_i64() {
                if i < 0 {
                    Ok(None)
                } else {
                    Ok(Some((i as u64).min(MAX_SAFE_DURATION_MS)))
                }
            } else if let Some(f) = n.as_f64() {
                if f.is_finite() && f >= 0.0 {
                    let clamped = f.round().min(MAX_SAFE_DURATION_MS as f64);
                    Ok(Some(clamped as u64))
                } else {
                    Ok(None)
                }
            } else {
                Ok(None)
            }
        }
        _ => Ok(None),
    }
}

/// Formats a Unix millisecond timestamp as an ISO-8601 UTC string (`YYYY-MM-DDTHH:MM:SS.sssZ`).
#[must_use]
pub fn format_unix_ms_iso8601(unix_ms: u128) -> String {
    let total_secs = (unix_ms / 1_000) as i64;
    let millis = (unix_ms % 1_000) as u32;
    let days = total_secs.div_euclid(86_400);
    let secs_of_day = total_secs.rem_euclid(86_400) as u32;
    let hour = secs_of_day / 3_600;
    let minute = (secs_of_day % 3_600) / 60;
    let second = secs_of_day % 60;

    // Civil calendar conversion (Howard Hinnant's algorithm)
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let y = (yoe as i64) + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = if month <= 2 { y + 1 } else { y };

    format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}.{millis:03}Z")
}

/// Returns the current wall-clock time as an ISO-8601 UTC string (`YYYY-MM-DDTHH:MM:SS.sssZ`).
#[must_use]
pub fn current_iso8601_utc() -> String {
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    format_unix_ms_iso8601(millis)
}

/// Normalizes persisted timestamp strings (`""`, `"Just now"`, `"Yesterday"`, or raw epoch seconds)
/// into sortable UTC ISO-8601 strings so SQLite `ORDER BY ... DESC` is always chronological (`H-3`).
#[must_use]
pub fn normalize_persisted_timestamp(raw: &str, fallback_iso: &str) -> String {
    let trimmed = raw.trim();
    if trimmed.is_empty() || trimmed.eq_ignore_ascii_case("just now") {
        return fallback_iso.to_owned();
    }
    if trimmed.eq_ignore_ascii_case("yesterday") {
        let yesterday_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis().saturating_sub(86_400_000))
            .unwrap_or(0);
        return format_unix_ms_iso8601(yesterday_ms);
    }
    if trimmed.len() >= 9 && trimmed.len() <= 13 && trimmed.bytes().all(|b| b.is_ascii_digit()) {
        if let Ok(num) = trimmed.parse::<u128>() {
            let ms = if trimmed.len() <= 10 {
                num.saturating_mul(1_000)
            } else {
                num
            };
            return format_unix_ms_iso8601(ms);
        }
    }
    if trimmed.len() >= 10
        && trimmed.as_bytes().get(4) == Some(&b'-')
        && trimmed.as_bytes().get(7) == Some(&b'-')
    {
        return trimmed.to_owned();
    }
    fallback_iso.to_owned()
}

/// Validates a local media or folder path against empty strings, control characters, and remote schemes (`H-6`).
pub fn validate_local_media_path_str(raw_path: &str) -> AppResult<&str> {
    let trimmed = raw_path.trim();
    if trimmed.is_empty() {
        return Err(AppError::validation("Media file path must not be empty"));
    }
    if trimmed.chars().any(char::is_control) {
        return Err(AppError::validation(
            "Media file path contains invalid control characters",
        ));
    }
    let lower = trimmed.to_ascii_lowercase();
    for forbidden_scheme in [
        "http://", "https://", "rtsp://", "rtsps://", "ftp://", "sftp://", "ws://", "wss://",
        "mms://",
    ] {
        if lower.starts_with(forbidden_scheme) {
            return Err(AppError::validation(
                "Audora only opens local files on your device; remote network URLs are not permitted.",
            ));
        }
    }
    Ok(trimmed)
}

/// Validates note text and quick-marker rules (`MAX_NOTE_BODY_CHARS`).
pub fn validate_note_body(body: &str, is_quick_marker: bool) -> AppResult<&str> {
    let trimmed = body.trim();
    if trimmed.chars().count() > MAX_NOTE_BODY_CHARS {
        return Err(AppError::validation(format!(
            "Note text exceeds maximum length of {MAX_NOTE_BODY_CHARS} characters"
        )));
    }
    if trimmed.is_empty() && !is_quick_marker {
        return Err(AppError::validation(
            "Enter note text or check 'Save as quick marker' before saving.",
        ));
    }
    Ok(trimmed)
}

/// Validates playlist name and description character bounds.
pub fn validate_playlist_fields<'a>(name: &'a str, desc: &'a str) -> AppResult<(&'a str, &'a str)> {
    let trimmed_name = name.trim();
    if trimmed_name.is_empty() {
        return Err(AppError::validation("Playlist name must not be empty"));
    }
    if trimmed_name.chars().count() > MAX_PLAYLIST_NAME_CHARS {
        return Err(AppError::validation(format!(
            "Playlist name must not exceed {MAX_PLAYLIST_NAME_CHARS} characters"
        )));
    }
    let trimmed_desc = desc.trim();
    if trimmed_desc.chars().count() > MAX_PLAYLIST_DESCRIPTION_CHARS {
        return Err(AppError::validation(format!(
            "Playlist description must not exceed {MAX_PLAYLIST_DESCRIPTION_CHARS} characters"
        )));
    }
    Ok((trimmed_name, trimmed_desc))
}

/// Validates playback speed bounds (`0.5x..=2.0x`).
pub fn validate_playback_rate(rate: f64) -> AppResult<f64> {
    if !rate.is_finite() || !(MIN_PLAYBACK_RATE..=MAX_PLAYBACK_RATE).contains(&rate) {
        return Err(AppError::validation(format!(
            "Playback speed must be between {MIN_PLAYBACK_RATE}x and {MAX_PLAYBACK_RATE}x"
        )));
    }
    Ok(rate)
}

/// Validates audio volume bounds (`0.0..=1.0`).
pub fn validate_volume(volume: f64) -> AppResult<f64> {
    if !volume.is_finite() || !(0.0..=1.0).contains(&volume) {
        return Err(AppError::validation("Volume must be between 0.0 and 1.0"));
    }
    Ok(volume)
}

/// Maximum UTF-8 character count allowed for a note body.
pub const MAX_NOTE_BODY_CHARS: usize = 10_000;

/// Maximum UTF-8 character count allowed for search queries.
pub const MAX_SEARCH_QUERY_CHARS: usize = 512;

/// Minimum supported playback speed multiplier.
pub const MIN_PLAYBACK_RATE: f64 = 0.5;

/// Maximum supported playback speed multiplier.
pub const MAX_PLAYBACK_RATE: f64 = 2.0;

/// Maximum UTF-8 character count allowed for playlist names.
pub const MAX_PLAYLIST_NAME_CHARS: usize = 120;

/// Maximum UTF-8 character count allowed for playlist descriptions.
pub const MAX_PLAYLIST_DESCRIPTION_CHARS: usize = 500;

/// Stable application-generated identifier for a media library record.
pub type MediaId = String;

/// Stable application-generated identifier for a timestamped note.
pub type NoteId = String;

/// Stable application-generated identifier for a user playlist.
pub type PlaylistId = String;

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
    #[serde(default)]
    pub display_name: String,
    pub media_kind: MediaKind,
    #[serde(default, deserialize_with = "deserialize_opt_u64_lenient")]
    pub duration_ms: Option<u64>,
    #[serde(default)]
    pub duration_probed: bool,
    #[serde(default, deserialize_with = "deserialize_u64_lenient")]
    pub last_position_ms: u64,
    #[serde(default)]
    pub completed: bool,
    #[serde(default, deserialize_with = "deserialize_opt_u64_lenient")]
    pub file_size_bytes: Option<u64>,
    #[serde(default)]
    pub fingerprint: Option<String>,
    pub availability: Availability,
    #[serde(default)]
    pub added_at: String,
    #[serde(default)]
    pub last_opened_at: Option<String>,
    #[serde(default)]
    pub last_seen_at: String,
    #[serde(default)]
    pub note_count: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_folder_id: Option<String>,
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
    #[serde(default, deserialize_with = "deserialize_u64_lenient")]
    pub position_ms: u64,
    #[serde(default)]
    pub is_quick_marker: bool,
    #[serde(default)]
    pub body: String,
    #[serde(default)]
    pub created_at: String,
    #[serde(default)]
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

/// Default GitHub repository (`owner/repo`) queried for signed Audora releases.
pub const DEFAULT_GITHUB_RELEASE_REPO: &str = "smokeInCloud060201/audora";

fn default_true() -> bool {
    true
}

fn default_one_f64() -> f64 {
    1.0
}

fn default_off_repeat() -> RepeatMode {
    RepeatMode::Off
}

/// A local folder configured by the user as a trusted source of audio/video files.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TrustedSourceFolder {
    pub id: String,
    pub path: String,
    #[serde(default)]
    pub label: String,
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default)]
    pub added_at: String,
    #[serde(default)]
    pub last_scanned_at: String,
    #[serde(default)]
    pub media_count: usize,
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
    #[serde(default = "default_true")]
    pub auto_check_updates: bool,
    #[serde(default)]
    pub installed_version: Option<String>,
    #[serde(default)]
    pub dismissed_update_version: Option<String>,
    #[serde(default)]
    pub source_folders: Vec<TrustedSourceFolder>,
    #[serde(default = "default_true")]
    pub show_playlists_bar: bool,
    #[serde(default = "default_true")]
    pub show_side_panel: bool,
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
            auto_check_updates: true,
            installed_version: None,
            dismissed_update_version: None,
            source_folders: Vec::new(),
            show_playlists_bar: true,
            show_side_panel: true,
        }
    }
}

/// Persisted playback queue and transport session state stored in `settings` (`playback_session_v1`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PersistedPlaybackSession {
    #[serde(default)]
    pub active_media_id: Option<MediaId>,
    #[serde(default)]
    pub queue: Vec<MediaId>,
    #[serde(default = "default_one_f64")]
    pub volume: f64,
    #[serde(default)]
    pub muted: bool,
    #[serde(default = "default_one_f64")]
    pub rate: f64,
    #[serde(default = "default_off_repeat")]
    pub repeat_mode: RepeatMode,
    #[serde(default)]
    pub shuffle: bool,
}

impl Default for PersistedPlaybackSession {
    fn default() -> Self {
        Self {
            active_media_id: None,
            queue: Vec::new(),
            volume: 1.0,
            muted: false,
            rate: 1.0,
            repeat_mode: RepeatMode::Off,
            shuffle: false,
        }
    }
}

/// Full relational state snapshot exported/imported across `media`, `notes`, `playlists`, and `settings`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RelationalAppStateDto {
    #[serde(default)]
    pub media: Vec<MediaItem>,
    #[serde(default)]
    pub notes: Vec<Note>,
    #[serde(default)]
    pub playlists: Vec<Playlist>,
    #[serde(default)]
    pub settings: AppSettings,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub playback_session: Option<PersistedPlaybackSession>,
}

/// Release manifest describing an available Audora release asset.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReleaseManifestDto {
    pub version: String,
    pub title: String,
    pub notes: String,
    pub package_size_bytes: u64,
    pub sha256: String,
    pub published_at: String,
    #[serde(default)]
    pub download_url: String,
}

/// Application update status returned by `check_for_updates`, `install_update`, and `dismiss_update`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AppUpdateStatusDto {
    pub current_version: String,
    pub update_available: bool,
    pub latest_version: String,
    pub release_title: String,
    pub release_notes: String,
    pub package_size_bytes: u64,
    pub sha256_checksum: String,
    pub download_url: String,
    pub github_repo: String,
    pub verified: bool,
    pub staged_for_restart: bool,
    pub checked_at_unix_ms: i64,
}

/// Summary of paths and bytes removed during `--uninstall-cleanup` or application reset.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CleanupSummaryDto {
    pub removed_paths: Vec<String>,
    pub bytes_reclaimed: u64,
}

/// A user-curated ordered playlist of local media items.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Playlist {
    pub id: PlaylistId,
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub item_ids: Vec<MediaId>,
    #[serde(default)]
    pub created_at: String,
    #[serde(default)]
    pub updated_at: String,
}

/// Input payload for creating a new playlist (`0002_playlists`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CreatePlaylistInput {
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub initial_item_ids: Vec<MediaId>,
}

/// Input payload for updating a playlist's name and description.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UpdatePlaylistInput {
    pub id: PlaylistId,
    pub name: String,
    pub description: String,
}

/// Redacted, user-reviewable diagnostic report (`docs/SECURITY.md`).
///
/// Never includes file paths, user home directories, playlist titles, or note bodies.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiagnosticReport {
    pub app_version: String,
    pub os_family: String,
    pub arch: String,
    pub schema_version: i64,
    pub media_count: usize,
    pub missing_media_count: usize,
    pub total_note_count: usize,
    #[serde(default)]
    pub playlist_count: usize,
    pub playback_state: PlaybackState,
    pub last_error_code: Option<String>,
    pub generated_at: String,
}
