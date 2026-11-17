//! Application services orchestrating library records, playback state, timestamped notes,
//! and user settings (`docs/FEATURES.md`).

use std::cmp::Ordering as CmpOrdering;
use std::collections::hash_map::RandomState;
use std::fs;
use std::hash::{BuildHasher, Hasher};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use sha2::{Digest, Sha256};

use crate::domain::{
    current_iso8601_utc, validate_local_media_path_str, validate_note_body, validate_playback_rate,
    validate_playlist_fields, AppSettings, AppUpdateStatusDto, Availability, BackendEvent,
    CleanupSummaryDto, CreateNoteInput, CreatePlaylistInput, DiagnosticReport, LibraryFilter,
    LibrarySearchResult, MediaId, MediaItem, MediaRepository, Note, NotesRepository,
    PlaybackBackend, PlaybackCommand, PlaybackSnapshot, PlaybackState, Playlist,
    PlaylistRepository, ReleaseManifestDto, RelinkMediaRecord, RepeatMode, SettingsRepository,
    TimestampPrecision, UpdateNoteInput, UpdatePlaylistInput, WindowMode, MAX_SEARCH_QUERY_CHARS,
};
use crate::error::{redact_path, AppError, AppResult};

static ID_COUNTER: AtomicU64 = AtomicU64::new(1);

fn generate_id(prefix: &str) -> String {
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    let seq = ID_COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("{prefix}_{millis}_{seq}")
}

fn now_timestamp_string() -> String {
    current_iso8601_utc()
}

fn canonicalize_if_exists(raw: &str) -> String {
    let p = Path::new(raw);
    let Ok(canon) = fs::canonicalize(p) else {
        return raw.to_owned();
    };
    let lossy = canon.to_string_lossy();
    if let Some(unc) = lossy.strip_prefix(r"\\?\UNC\") {
        format!(r"\\{unc}")
    } else if let Some(stripped) = lossy.strip_prefix(r"\\?\") {
        stripped.to_owned()
    } else {
        lossy.into_owned()
    }
}

/// Core application coordinator owning domain invariants across playback, library, notes, playlists, and settings.
pub struct AudoraAppService {
    media_repo: Arc<dyn MediaRepository>,
    notes_repo: Arc<dyn NotesRepository>,
    playlist_repo: Arc<dyn PlaylistRepository>,
    settings_repo: Arc<dyn SettingsRepository>,
    playback_backend: Arc<dyn PlaybackBackend>,
    snapshot: Mutex<PlaybackSnapshot>,
    app_data_dir: PathBuf,
    db: Option<crate::persistence::Database>,
}

impl AudoraAppService {
    #[must_use]
    pub fn new(
        media_repo: Arc<dyn MediaRepository>,
        notes_repo: Arc<dyn NotesRepository>,
        playlist_repo: Arc<dyn PlaylistRepository>,
        settings_repo: Arc<dyn SettingsRepository>,
        playback_backend: Arc<dyn PlaybackBackend>,
    ) -> Self {
        let seq = ID_COUNTER.fetch_add(1, Ordering::Relaxed);
        let rand_token = RandomState::new().build_hasher().finish();
        let pid = std::process::id();
        let default_dir =
            std::env::temp_dir().join(format!("audora-data-{pid}-{seq}-{rand_token:016x}"));
        Self::with_data_dir(
            media_repo,
            notes_repo,
            playlist_repo,
            settings_repo,
            playback_backend,
            default_dir,
        )
    }

    #[must_use]
    pub fn with_data_dir(
        media_repo: Arc<dyn MediaRepository>,
        notes_repo: Arc<dyn NotesRepository>,
        playlist_repo: Arc<dyn PlaylistRepository>,
        settings_repo: Arc<dyn SettingsRepository>,
        playback_backend: Arc<dyn PlaybackBackend>,
        app_data_dir: PathBuf,
    ) -> Self {
        let initial_rate = settings_repo
            .load_settings()
            .map(|s| s.default_playback_speed)
            .unwrap_or(1.0);
        let snapshot = PlaybackSnapshot {
            rate: initial_rate,
            ..PlaybackSnapshot::default()
        };
        Self {
            media_repo,
            notes_repo,
            playlist_repo,
            settings_repo,
            playback_backend,
            snapshot: Mutex::new(snapshot),
            app_data_dir,
            db: None,
        }
    }

    #[must_use]
    pub fn with_database(mut self, db: crate::persistence::Database) -> Self {
        self.db = Some(db);
        self
    }

    #[must_use]
    pub fn app_data_dir(&self) -> &Path {
        &self.app_data_dir
    }

    fn validate_local_media_path(raw_path: &str) -> AppResult<&str> {
        validate_local_media_path_str(raw_path)
    }

    /// Opens a local media file, probes its stream metadata without modifying the source file,
    /// records/updates its library entry, increments the load generation, and loads it for playback.
    pub fn open_media(&self, raw_path: &str) -> AppResult<MediaItem> {
        let trimmed = Self::validate_local_media_path(raw_path)?;
        let normalized_path = canonicalize_if_exists(trimmed);
        let path = Path::new(&normalized_path);
        let file_name = path
            .file_name()
            .and_then(|s| s.to_str())
            .filter(|s| !s.is_empty())
            .ok_or_else(|| AppError::validation("Media path must point to a valid file name"))?
            .to_owned();

        if !path.exists() {
            if let Some(existing) = self
                .media_repo
                .get_by_path(&normalized_path)?
                .or(self.media_repo.get_by_path(trimmed)?)
            {
                let now = now_timestamp_string();
                self.media_repo
                    .update_availability(&existing.id, Availability::Missing, &now)?;
            }
            return Err(AppError::media_unavailable(format!(
                "File not found at last known location ({file_name})."
            )));
        }

        let metadata = fs::metadata(path).map_err(|_| {
            AppError::media_unreadable(format!("Unable to read metadata for {file_name}."))
        })?;
        if !metadata.is_file() {
            return Err(AppError::media_unreadable(
                "Selected path is a directory, not a media file.",
            ));
        }
        if metadata.len() == 0 {
            return Err(AppError::media_corrupt(format!(
                "{file_name} is empty (0 bytes)."
            )));
        }

        let settings = self.settings_repo.load_settings()?;
        let next_gen = {
            let mut snap = self
                .snapshot
                .lock()
                .map_err(|_| AppError::internal("Playback snapshot lock poisoned"))?;
            snap.generation = snap.generation.saturating_add(1);
            snap.state = PlaybackState::Loading;
            snap.error = None;
            snap.generation
        };

        let probed = match self.playback_backend.load(path, next_gen) {
            Ok(meta) => meta,
            Err(err) => {
                let mut snap = self
                    .snapshot
                    .lock()
                    .map_err(|_| AppError::internal("Playback snapshot lock poisoned"))?;
                snap.state = PlaybackState::Failed;
                snap.error = Some(err.to_payload());
                tracing::warn!(
                    path = %redact_path(path),
                    code = err.code().as_str(),
                    "Failed to load media into backend"
                );
                return Err(err);
            }
        };

        let existing = self
            .media_repo
            .get_by_path(&normalized_path)?
            .or(self.media_repo.get_by_path(trimmed)?);
        let now = now_timestamp_string();
        let media_id = existing
            .as_ref()
            .map_or_else(|| generate_id("med"), |m| m.id.clone());
        let prior_pos = existing.as_ref().map_or(0, |m| m.last_position_ms);
        let prior_completed = existing.as_ref().is_some_and(|m| m.completed);

        let prior_folder = existing.as_ref().and_then(|m| m.source_folder_id.clone());
        let item_to_save = MediaItem {
            id: media_id,
            path: normalized_path,
            display_name: file_name,
            media_kind: probed.media_kind,
            duration_ms: probed.duration_ms,
            duration_probed: probed.duration_ms.is_some(),
            last_position_ms: prior_pos,
            completed: prior_completed,
            file_size_bytes: Some(metadata.len()),
            fingerprint: None,
            availability: Availability::Available,
            added_at: existing.map_or_else(|| now.clone(), |m| m.added_at),
            last_opened_at: Some(now.clone()),
            last_seen_at: now,
            note_count: 0,
            source_folder_id: prior_folder,
        };

        let saved = self.media_repo.upsert_media(&item_to_save)?;

        // Determine starting position (`Resume where I left off` unless previously completed).
        let resume_pos = if settings.resume_where_left_off && !saved.completed {
            saved.last_position_ms
        } else {
            0
        };
        let clamped_pos = self.playback_backend.seek(next_gen, resume_pos)?;
        self.playback_backend
            .set_rate(next_gen, settings.default_playback_speed)?;

        {
            let mut snap = self
                .snapshot
                .lock()
                .map_err(|_| AppError::internal("Playback snapshot lock poisoned"))?;
            snap.state = PlaybackState::Paused;
            snap.media_id = Some(saved.id.clone());
            snap.position_ms = clamped_pos;
            snap.duration_ms = saved.duration_ms;
            snap.rate = settings.default_playback_speed;
            if !snap.queue.contains(&saved.id) {
                snap.queue.push(saved.id.clone());
            }
        }

        Ok(saved)
    }

    /// Opens a batch of file or single-level directory paths (`Flow 1` in `docs/WORKFLOWS.md`),
    /// adding all valid media files to the session queue and loading the first item for playback.
    ///
    /// Per `docs/SECURITY.md`, directory inspection is strictly non-recursive (single level only).
    pub fn open_media_batch(&self, raw_paths: &[String]) -> AppResult<Vec<MediaItem>> {
        if raw_paths.is_empty() {
            return Err(AppError::validation("At least one media path is required"));
        }

        let mut expanded_paths: Vec<(String, bool)> = Vec::new();
        for raw in raw_paths {
            let trimmed = raw.trim();
            if trimmed.is_empty() {
                continue;
            }
            let p = Path::new(trimmed);
            if p.is_dir() {
                let mut dir_files: Vec<String> = fs::read_dir(p)
                    .map_err(|_| {
                        AppError::media_unreadable("Unable to read selected folder contents.")
                    })?
                    .filter_map(Result::ok)
                    .map(|entry| entry.path())
                    .filter(|entry_path| {
                        entry_path.is_file()
                            && entry_path.metadata().map(|m| m.len() > 0).unwrap_or(false)
                            && Self::is_supported_media_extension(entry_path)
                    })
                    .filter_map(|entry_path| entry_path.to_str().map(str::to_owned))
                    .collect();
                dir_files.sort();
                expanded_paths.extend(dir_files.into_iter().map(|f| (f, true)));
            } else {
                expanded_paths.push((trimmed.to_owned(), false));
            }
        }

        if expanded_paths.is_empty() {
            return Err(AppError::media_unavailable(
                "No supported audio or video files were found in the selection.",
            ));
        }

        let mut opened_items = Vec::with_capacity(expanded_paths.len());
        let mut last_err: Option<AppError> = None;
        for (path_str, from_dir) in &expanded_paths {
            match self.open_media(path_str) {
                Ok(item) => opened_items.push(item),
                Err(err) if *from_dir => {
                    last_err = Some(err);
                }
                Err(err) => return Err(err),
            }
        }
        if opened_items.is_empty() {
            return Err(last_err.unwrap_or_else(|| {
                AppError::media_unavailable(
                    "No playable audio or video files were found in the selection.",
                )
            }));
        }

        // Keep the first opened item active without double-spawning external probers (`M-5`).
        if opened_items.len() > 1 {
            let first = &opened_items[0];
            if let Ok(mut snap) = self.snapshot.lock() {
                self.load_queued_track_locked(&mut snap, &first.id, false);
                if let Ok(settings) = self.settings_repo.load_settings() {
                    if settings.resume_where_left_off && !first.completed {
                        let ack = self
                            .playback_backend
                            .seek(snap.generation, first.last_position_ms)
                            .unwrap_or(first.last_position_ms);
                        snap.position_ms = ack;
                    }
                }
            }
        }

        Ok(opened_items)
    }

    fn is_supported_media_extension(path: &Path) -> bool {
        matches!(
            path.extension()
                .and_then(|e| e.to_str())
                .map(str::to_ascii_lowercase)
                .as_deref(),
            Some(
                "mp4"
                    | "m4v"
                    | "mov"
                    | "webm"
                    | "mkv"
                    | "mp3"
                    | "wav"
                    | "flac"
                    | "ogg"
                    | "opus"
                    | "m4a"
                    | "aac"
            )
        )
    }

    fn gcd_usize(mut a: usize, mut b: usize) -> usize {
        while b != 0 {
            let t = b;
            b = a % b;
            a = t;
        }
        a
    }

    fn next_shuffle_index(idx: usize, len: usize) -> Option<usize> {
        if len <= 1 {
            return None;
        }
        if len == 2 {
            return Some((idx + 1) % 2);
        }
        let step = (2..len)
            .find(|&s| Self::gcd_usize(s, len) == 1)
            .unwrap_or(1);
        Some((idx + step) % len)
    }

    fn load_queued_track_locked(
        &self,
        snap: &mut PlaybackSnapshot,
        next_id: &str,
        auto_play: bool,
    ) {
        if let Ok(Some(item)) = self.media_repo.get_by_id(next_id) {
            let next_gen = snap.generation.saturating_add(1);
            let p = Path::new(&item.path);
            if !p.is_file() {
                let _ = self.media_repo.update_availability(
                    &item.id,
                    Availability::Missing,
                    &now_timestamp_string(),
                );
                let _ = self.playback_backend.stop();
                snap.generation = next_gen;
                snap.media_id = Some(next_id.to_owned());
                snap.position_ms = 0;
                snap.state = PlaybackState::Failed;
                snap.error = Some(
                    AppError::media_unavailable("Queued media file is missing from disk.")
                        .to_payload(),
                );
                return;
            }
            let loaded_dur = self
                .playback_backend
                .load(p, next_gen)
                .ok()
                .and_then(|m| m.duration_ms);
            snap.generation = next_gen;
            snap.media_id = Some(next_id.to_owned());
            snap.duration_ms = loaded_dur.or(item.duration_ms);
            snap.position_ms = 0;
            snap.error = None;
            if auto_play {
                let _ = self.playback_backend.play(next_gen);
                snap.state = PlaybackState::Playing;
            } else if snap.state != PlaybackState::Idle {
                snap.state = PlaybackState::Paused;
            }
        } else {
            snap.media_id = Some(next_id.to_owned());
            snap.position_ms = 0;
        }
    }

    /// Removes a media item from the active session queue without deleting its library record.
    pub fn remove_from_queue(&self, media_id: &str) -> AppResult<PlaybackSnapshot> {
        let mut snap = self
            .snapshot
            .lock()
            .map_err(|_| AppError::internal("Playback snapshot lock poisoned"))?;
        let current_idx = snap.queue.iter().position(|id| id == media_id);
        let was_playing = snap.state == PlaybackState::Playing;
        snap.queue.retain(|id| id != media_id);

        if snap.media_id.as_deref() == Some(media_id) {
            if snap.queue.is_empty() {
                let _ = self.playback_backend.stop();
                snap.media_id = None;
                snap.duration_ms = None;
                snap.state = PlaybackState::Idle;
                snap.position_ms = 0;
            } else {
                let next_idx = current_idx
                    .unwrap_or(0)
                    .min(snap.queue.len().saturating_sub(1));
                let next_id = snap.queue[next_idx].clone();
                self.load_queued_track_locked(&mut snap, &next_id, was_playing);
            }
        }
        Ok(snap.clone())
    }

    /// Marks a media item as `Missing` when its underlying file is moved or offline.
    pub fn mark_media_missing(&self, media_id: &str) -> AppResult<MediaItem> {
        let now = now_timestamp_string();
        self.media_repo
            .update_availability(media_id, Availability::Missing, &now)?;
        self.media_repo
            .get_by_id(media_id)?
            .ok_or_else(|| AppError::validation("Media item not found"))
    }

    /// Returns the current authoritative `PlaybackSnapshot`.
    pub fn get_playback_snapshot(&self) -> AppResult<PlaybackSnapshot> {
        let snap = self
            .snapshot
            .lock()
            .map_err(|_| AppError::internal("Playback snapshot lock poisoned"))?;
        Ok(snap.clone())
    }

    /// Executes a serialized `PlaybackCommand` and updates state/persistence accordingly.
    pub fn playback_command(&self, command: PlaybackCommand) -> AppResult<PlaybackSnapshot> {
        let mut snap = self
            .snapshot
            .lock()
            .map_err(|_| AppError::internal("Playback snapshot lock poisoned"))?;
        let gen = snap.generation;

        match command {
            PlaybackCommand::Play => {
                if snap.media_id.is_none() {
                    return Err(AppError::validation("No media is loaded for playback"));
                }
                self.playback_backend.play(gen)?;
                snap.state = PlaybackState::Playing;
            }
            PlaybackCommand::Pause => {
                if snap.media_id.is_some() {
                    self.playback_backend.pause(gen)?;
                    snap.state = PlaybackState::Paused;
                    if let Some(ref media_id) = snap.media_id {
                        let _ = self.media_repo.update_progress(
                            media_id,
                            snap.position_ms,
                            false,
                            &now_timestamp_string(),
                        );
                    }
                }
            }
            PlaybackCommand::TogglePlayPause => {
                if snap.state == PlaybackState::Playing {
                    self.playback_backend.pause(gen)?;
                    snap.state = PlaybackState::Paused;
                    if let Some(ref media_id) = snap.media_id {
                        let _ = self.media_repo.update_progress(
                            media_id,
                            snap.position_ms,
                            false,
                            &now_timestamp_string(),
                        );
                    }
                } else if snap.media_id.is_some() {
                    self.playback_backend.play(gen)?;
                    snap.state = PlaybackState::Playing;
                }
            }
            PlaybackCommand::Seek { position_ms } => {
                if snap.media_id.is_none() {
                    return Err(AppError::validation("Cannot seek when no media is loaded"));
                }
                let acknowledged = self.playback_backend.seek(gen, position_ms)?;
                snap.position_ms = acknowledged;
                if let Some(ref media_id) = snap.media_id {
                    let _ = self.media_repo.update_progress(
                        media_id,
                        acknowledged,
                        false,
                        &now_timestamp_string(),
                    );
                }
            }
            PlaybackCommand::SkipRelative { delta_ms } => {
                if snap.media_id.is_none() {
                    return Err(AppError::validation("Cannot skip when no media is loaded"));
                }
                let target = if delta_ms.is_negative() {
                    snap.position_ms.saturating_sub(delta_ms.unsigned_abs())
                } else {
                    snap.position_ms.saturating_add(delta_ms.unsigned_abs())
                };
                let acknowledged = self.playback_backend.seek(gen, target)?;
                snap.position_ms = acknowledged;
                if let Some(ref media_id) = snap.media_id {
                    let _ = self.media_repo.update_progress(
                        media_id,
                        acknowledged,
                        false,
                        &now_timestamp_string(),
                    );
                }
            }
            PlaybackCommand::SetRate { rate } => {
                validate_playback_rate(rate)?;
                self.playback_backend.set_rate(gen, rate)?;
                snap.rate = rate;
            }
            PlaybackCommand::SetVolume { volume } => {
                self.playback_backend.set_volume(volume, snap.muted)?;
                snap.volume = volume;
            }
            PlaybackCommand::SetMuted { muted } => {
                self.playback_backend.set_volume(snap.volume, muted)?;
                snap.muted = muted;
            }
            PlaybackCommand::SetRepeat { mode } => {
                snap.repeat_mode = mode;
            }
            PlaybackCommand::SetShuffle { shuffle } => {
                snap.shuffle = shuffle;
            }
            PlaybackCommand::Next => {
                if let Some(current_id) = snap.media_id.clone() {
                    let was_playing = snap.state == PlaybackState::Playing;
                    if let Some(idx) = snap.queue.iter().position(|id| id == &current_id) {
                        let len = snap.queue.len();
                        let next_idx = if snap.shuffle && len > 1 {
                            Self::next_shuffle_index(idx, len)
                        } else if idx + 1 < len {
                            Some(idx + 1)
                        } else if snap.repeat_mode == RepeatMode::All && len > 0 {
                            Some(0)
                        } else {
                            None
                        };
                        if let Some(i) = next_idx {
                            let _ = self.media_repo.update_progress(
                                &current_id,
                                snap.position_ms,
                                false,
                                &now_timestamp_string(),
                            );
                            let next_id = snap.queue[i].clone();
                            self.load_queued_track_locked(&mut snap, &next_id, was_playing);
                        }
                    }
                }
            }
            PlaybackCommand::Previous => {
                if snap.position_ms > 3_000 {
                    let acknowledged = self.playback_backend.seek(gen, 0)?;
                    snap.position_ms = acknowledged;
                    if let Some(ref media_id) = snap.media_id {
                        let _ = self.media_repo.update_progress(
                            media_id,
                            acknowledged,
                            false,
                            &now_timestamp_string(),
                        );
                    }
                } else if let Some(current_id) = snap.media_id.clone() {
                    let was_playing = snap.state == PlaybackState::Playing;
                    if let Some(idx) = snap.queue.iter().position(|id| id == &current_id) {
                        let prev_idx = if idx > 0 {
                            Some(idx - 1)
                        } else if snap.repeat_mode == RepeatMode::All && !snap.queue.is_empty() {
                            Some(snap.queue.len() - 1)
                        } else {
                            None
                        };
                        if let Some(i) = prev_idx {
                            let _ = self.media_repo.update_progress(
                                &current_id,
                                snap.position_ms,
                                false,
                                &now_timestamp_string(),
                            );
                            let prev_id = snap.queue[i].clone();
                            self.load_queued_track_locked(&mut snap, &prev_id, was_playing);
                        } else {
                            let acknowledged = self.playback_backend.seek(gen, 0)?;
                            snap.position_ms = acknowledged;
                            let _ = self.media_repo.update_progress(
                                &current_id,
                                acknowledged,
                                false,
                                &now_timestamp_string(),
                            );
                        }
                    }
                }
            }
            PlaybackCommand::Stop => {
                if let Some(ref media_id) = snap.media_id {
                    let _ = self.media_repo.update_progress(
                        media_id,
                        snap.position_ms,
                        false,
                        &now_timestamp_string(),
                    );
                }
                self.playback_backend.stop()?;
                snap.state = PlaybackState::Idle;
                snap.position_ms = 0;
            }
        }

        Ok(snap.clone())
    }

    /// Processes an asynchronous callback from the playback backend, ignoring stale load generations.
    pub fn apply_backend_event(
        &self,
        event_generation: u64,
        event: BackendEvent,
    ) -> AppResult<PlaybackSnapshot> {
        let mut snap = self
            .snapshot
            .lock()
            .map_err(|_| AppError::internal("Playback snapshot lock poisoned"))?;

        // Suppress stale events from an older media load generation (`docs/ARCHITECTURE.md`).
        if event_generation != snap.generation {
            tracing::debug!(
                stale_generation = event_generation,
                active_generation = snap.generation,
                "Ignored stale backend event"
            );
            return Ok(snap.clone());
        }

        match event {
            BackendEvent::Ready { duration_ms } => {
                snap.duration_ms = duration_ms;
                if snap.state == PlaybackState::Loading {
                    snap.state = PlaybackState::Paused;
                }
            }
            BackendEvent::PositionChanged { position_ms } => {
                snap.position_ms = match snap.duration_ms {
                    Some(dur) => position_ms.min(dur),
                    None => position_ms,
                };
            }
            BackendEvent::StateChanged { state } => {
                snap.state = state;
            }
            BackendEvent::Ended => {
                snap.state = PlaybackState::Ended;
                if let Some(dur) = snap.duration_ms {
                    snap.position_ms = dur;
                }
                if let Some(ref media_id) = snap.media_id {
                    let _ = self.media_repo.update_progress(
                        media_id,
                        snap.position_ms,
                        true,
                        &now_timestamp_string(),
                    );
                }
                if snap.repeat_mode == RepeatMode::One {
                    let _ = self.playback_backend.seek(snap.generation, 0);
                    let _ = self.playback_backend.play(snap.generation);
                    snap.position_ms = 0;
                    snap.state = PlaybackState::Playing;
                } else if let Some(current_id) = snap.media_id.clone() {
                    if let Some(idx) = snap.queue.iter().position(|id| id == &current_id) {
                        let len = snap.queue.len();
                        let next_idx = if snap.shuffle && len > 1 {
                            Self::next_shuffle_index(idx, len)
                        } else if idx + 1 < len {
                            Some(idx + 1)
                        } else if snap.repeat_mode == RepeatMode::All && len > 0 {
                            Some(0)
                        } else {
                            None
                        };
                        if let Some(i) = next_idx {
                            let next_id = snap.queue[i].clone();
                            self.load_queued_track_locked(&mut snap, &next_id, true);
                        }
                    }
                }
            }
            BackendEvent::Failed { detail } => {
                snap.state = PlaybackState::Failed;
                snap.error = Some(AppError::media_corrupt(detail).to_payload());
            }
        }

        Ok(snap.clone())
    }

    /// Creates a timestamped note or quick marker (`Flow 3` in `docs/WORKFLOWS.md`).
    pub fn create_note(&self, input: CreateNoteInput) -> AppResult<Note> {
        let media = self
            .media_repo
            .get_by_id(&input.media_id)?
            .ok_or_else(|| AppError::validation("Referenced media item does not exist"))?;

        let trimmed_body = validate_note_body(&input.body, input.is_quick_marker)?.to_owned();

        let settings = self.settings_repo.load_settings()?;
        let pre_clamped = match media.duration_ms {
            Some(dur) => input.position_ms.min(dur),
            None => input.position_ms,
        };
        let rounded_pos = match settings.timestamp_precision {
            TimestampPrecision::NearestSecond => {
                (pre_clamped.saturating_add(500) / 1_000).saturating_mul(1_000)
            }
            TimestampPrecision::Milliseconds => pre_clamped,
        };
        let clamped_pos = match media.duration_ms {
            Some(dur) => rounded_pos.min(dur),
            None => rounded_pos,
        };

        let normalized_input = CreateNoteInput {
            media_id: input.media_id,
            position_ms: clamped_pos,
            is_quick_marker: trimmed_body.is_empty() && input.is_quick_marker,
            body: trimmed_body,
        };

        let id = generate_id("note");
        self.notes_repo
            .create_note(id, &normalized_input, &now_timestamp_string())
    }

    /// Updates an existing note's text.
    pub fn update_note(&self, input: UpdateNoteInput) -> AppResult<Note> {
        let trimmed = validate_note_body(&input.body, false)?;
        if trimmed.is_empty() {
            return Err(AppError::validation(
                "Note text cannot be empty when editing",
            ));
        }
        self.notes_repo.update_note(
            &UpdateNoteInput {
                id: input.id,
                body: trimmed.to_owned(),
            },
            &now_timestamp_string(),
        )
    }

    /// Deletes a note by ID.
    pub fn delete_note(&self, note_id: &str) -> AppResult<bool> {
        if note_id.trim().is_empty() {
            return Err(AppError::validation("Note ID must not be empty"));
        }
        self.notes_repo.delete_note(note_id)
    }

    /// Lists notes for a given media item in chronological order (`position_ms ASC`).
    pub fn list_notes(&self, media_id: &str) -> AppResult<Vec<Note>> {
        self.notes_repo.list_notes_for_media(media_id)
    }

    /// Activates a note ("Jump to moment"): loads the note's media if not already active, seeks
    /// directly to `note.position_ms` (overriding `Resume where I left off`), and starts playback.
    pub fn activate_note(&self, note_id: &str) -> AppResult<PlaybackSnapshot> {
        let note = self
            .notes_repo
            .get_note(note_id)?
            .ok_or_else(|| AppError::validation("Selected note does not exist"))?;

        let media = self
            .media_repo
            .get_by_id(&note.media_id)?
            .ok_or_else(|| AppError::validation("Media item for note does not exist"))?;

        if media.availability == Availability::Missing || !Path::new(&media.path).exists() {
            self.media_repo.update_availability(
                &media.id,
                Availability::Missing,
                &now_timestamp_string(),
            )?;
            return Err(AppError::media_unavailable(
                "This media file is offline. Relink the file to jump to this moment.",
            ));
        }

        let needs_load = {
            let snap = self
                .snapshot
                .lock()
                .map_err(|_| AppError::internal("Playback snapshot lock poisoned"))?;
            snap.media_id.as_deref() != Some(media.id.as_str())
        };

        if needs_load {
            self.open_media(&media.path)?;
        }

        {
            let mut snap = self
                .snapshot
                .lock()
                .map_err(|_| AppError::internal("Playback snapshot lock poisoned"))?;
            snap.error = None;
            let gen = snap.generation;
            let acknowledged = self.playback_backend.seek(gen, note.position_ms)?;
            snap.position_ms = acknowledged;
            let _ = self.media_repo.update_progress(
                &media.id,
                acknowledged,
                media.completed,
                &now_timestamp_string(),
            );
        }
        self.playback_command(PlaybackCommand::Play)
    }

    /// Searches media titles and note text locally (`Flow 5`).
    pub fn search_library(
        &self,
        query: Option<&str>,
        filter: LibraryFilter,
    ) -> AppResult<LibrarySearchResult> {
        if let Some(q) = query {
            if q.chars().count() > MAX_SEARCH_QUERY_CHARS {
                return Err(AppError::validation("Search query is too long"));
            }
        }
        let media_items = self.media_repo.list_media(query, filter)?;
        let matching_notes = match query.map(str::trim).filter(|q| !q.is_empty()) {
            Some(q) => self.notes_repo.search_notes(q, filter)?,
            None => Vec::new(),
        };
        Ok(LibrarySearchResult {
            media_items,
            matching_notes,
        })
    }

    /// Relinks a missing media record to a relocated file path (`Flow 6` in `docs/WORKFLOWS.md`).
    ///
    /// Never silently merges two library records (`AV-201`) and prompts for confirmation (`AV-202`)
    /// if media kind or duration differs by more than `500 ms` unless `confirm_mismatch` is `true`.
    pub fn relink_media(
        &self,
        media_id: &str,
        new_path: &str,
        confirm_mismatch: bool,
    ) -> AppResult<MediaItem> {
        let trimmed_path = Self::validate_local_media_path(new_path)?;

        let current = self
            .media_repo
            .get_by_id(media_id)?
            .ok_or_else(|| AppError::validation("Media item to relink was not found"))?;

        // Prohibit silent record merges (`AV-201`), checking both raw and canonical paths (`M-2`).
        let candidate_canonical = fs::canonicalize(trimmed_path).ok();
        let all_existing = self.media_repo.list_media(None, LibraryFilter::All)?;
        for existing in all_existing {
            if existing.id == current.id {
                continue;
            }
            let same_raw = existing.path == trimmed_path;
            let same_canon = match (&candidate_canonical, fs::canonicalize(&existing.path).ok()) {
                (Some(a), Some(b)) => a == &b,
                _ => false,
            };
            if same_raw || same_canon {
                return Err(AppError::relink_conflict(format!(
                    "'{}' is already tracked as another item in your library.",
                    existing.display_name
                )));
            }
        }

        let path = Path::new(trimmed_path);
        if !path.is_file() {
            return Err(AppError::media_unavailable(
                "Selected replacement file does not exist.",
            ));
        }

        let fs_meta = fs::metadata(path)
            .map_err(|_| AppError::media_unreadable("Unable to read replacement file metadata."))?;
        if fs_meta.len() == 0 {
            return Err(AppError::media_corrupt(
                "Replacement media file is empty (0 bytes).",
            ));
        }
        let probed = self.playback_backend.probe_media(path)?;

        let kind_changed = probed.media_kind != current.media_kind;
        let duration_differs = match (current.duration_ms, probed.duration_ms) {
            (Some(old_dur), Some(new_dur)) => old_dur.abs_diff(new_dur) > 500,
            _ => false,
        };

        if (kind_changed || duration_differs) && !confirm_mismatch {
            return Err(AppError::relink_mismatch(
                "The selected file's duration or media type differs from the original record. Confirm to relink anyway.",
            ));
        }

        let display_name = path
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or(&current.display_name)
            .to_owned();

        let canon_new = canonicalize_if_exists(trimmed_path);
        let now = now_timestamp_string();
        let updated_item = self.media_repo.relink_path(&RelinkMediaRecord {
            id: media_id,
            new_path: &canon_new,
            display_name: &display_name,
            media_kind: probed.media_kind,
            duration_ms: probed.duration_ms,
            duration_probed: probed.duration_ms.is_some(),
            file_size_bytes: Some(fs_meta.len()),
            updated_at: &now,
        })?;

        if let Ok(mut snap) = self.snapshot.lock() {
            if snap.media_id.as_deref() == Some(media_id) {
                snap.duration_ms = updated_item.duration_ms;
                if let Some(dur) = snap.duration_ms {
                    snap.position_ms = snap.position_ms.min(dur);
                }
                snap.error = None;
            }
        }

        Ok(updated_item)
    }

    /// Removes a media item and its associated notes from the library.
    pub fn remove_media(&self, media_id: &str) -> AppResult<bool> {
        {
            let mut snap = self
                .snapshot
                .lock()
                .map_err(|_| AppError::internal("Playback snapshot lock poisoned"))?;
            snap.queue.retain(|id| id != media_id);
            if snap.media_id.as_deref() == Some(media_id) {
                let _ = self.playback_backend.stop();
                snap.media_id = None;
                snap.duration_ms = None;
                snap.error = None;
                snap.state = PlaybackState::Idle;
                snap.position_ms = 0;
            }
        }
        self.media_repo.delete_media(media_id)
    }

    /// Loads persisted application settings.
    pub fn get_settings(&self) -> AppResult<AppSettings> {
        self.settings_repo.load_settings()
    }

    /// Validates and saves application settings.
    pub fn update_settings(&self, settings: AppSettings) -> AppResult<AppSettings> {
        validate_playback_rate(settings.default_playback_speed)?;
        for folder in &settings.source_folders {
            validate_local_media_path_str(&folder.path)?;
        }
        self.settings_repo
            .save_settings(&settings, &now_timestamp_string())?;
        Ok(settings)
    }

    /// Switches between `Full` and `Minimum` window modes while preserving playback state.
    pub fn set_window_mode(&self, mode: WindowMode) -> AppResult<AppSettings> {
        let mut settings = self.settings_repo.load_settings()?;
        settings.window_mode = mode;
        self.settings_repo
            .save_settings(&settings, &now_timestamp_string())?;
        Ok(settings)
    }

    /// Pauses active playback when headphones disconnect if the user preference is enabled.
    pub fn handle_audio_route_disconnected(&self) -> AppResult<PlaybackSnapshot> {
        let settings = self.settings_repo.load_settings()?;
        if settings.pause_on_headphones_disconnect {
            let is_playing = {
                let snap = self
                    .snapshot
                    .lock()
                    .map_err(|_| AppError::internal("Playback snapshot lock poisoned"))?;
                snap.state == PlaybackState::Playing
            };
            if is_playing {
                return self.playback_command(PlaybackCommand::Pause);
            }
        }
        self.get_playback_snapshot()
    }

    /// Generates a redacted, user-reviewable diagnostic report (`docs/SECURITY.md`).
    ///
    /// Guarantees that zero file paths, filenames, or note contents appear in the report.
    pub fn generate_diagnostic_report(&self) -> AppResult<DiagnosticReport> {
        let all_media = self.media_repo.list_media(None, LibraryFilter::All)?;
        let media_count = all_media.len();
        let missing_media_count = all_media
            .iter()
            .filter(|m| m.availability == Availability::Missing)
            .count();
        let total_note_count: usize = all_media.iter().map(|m| m.note_count).sum();
        let playlist_count = self.playlist_repo.list_playlists()?.len();
        let snap = self.get_playback_snapshot()?;

        let schema_version = self.settings_repo.schema_version().unwrap_or(4);

        Ok(DiagnosticReport {
            app_version: env!("CARGO_PKG_VERSION").to_owned(),
            os_family: std::env::consts::OS.to_owned(),
            arch: std::env::consts::ARCH.to_owned(),
            schema_version,
            media_count,
            missing_media_count,
            total_note_count,
            playlist_count,
            playback_state: snap.state,
            last_error_code: snap.error.map(|e| e.code.as_str().to_owned()),
            generated_at: now_timestamp_string(),
        })
    }

    /// Creates a new user playlist (`0002_playlists`).
    pub fn create_playlist(&self, input: CreatePlaylistInput) -> AppResult<Playlist> {
        let (trimmed_name, trimmed_desc) =
            validate_playlist_fields(&input.name, &input.description)?;
        let sanitized = CreatePlaylistInput {
            name: trimmed_name.to_owned(),
            description: trimmed_desc.to_owned(),
            initial_item_ids: input.initial_item_ids,
        };
        let id = generate_id("pl");
        let now = now_timestamp_string();
        self.playlist_repo.create_playlist(id, &sanitized, &now)
    }

    /// Updates a playlist's name and description.
    pub fn update_playlist(&self, input: UpdatePlaylistInput) -> AppResult<Playlist> {
        let (trimmed_name, trimmed_desc) =
            validate_playlist_fields(&input.name, &input.description)?;
        let sanitized = UpdatePlaylistInput {
            id: input.id,
            name: trimmed_name.to_owned(),
            description: trimmed_desc.to_owned(),
        };
        self.playlist_repo
            .update_playlist(&sanitized, &now_timestamp_string())
    }

    /// Deletes a playlist container without removing media items or notes from the library.
    pub fn delete_playlist(&self, playlist_id: &str) -> AppResult<bool> {
        self.playlist_repo.delete_playlist(playlist_id)
    }

    /// Lists all playlists ordered by most recently updated.
    pub fn list_playlists(&self) -> AppResult<Vec<Playlist>> {
        self.playlist_repo.list_playlists()
    }

    /// Returns a single playlist by ID.
    pub fn get_playlist(&self, playlist_id: &str) -> AppResult<Playlist> {
        self.playlist_repo
            .get_playlist(playlist_id)?
            .ok_or_else(|| AppError::validation("Selected playlist does not exist"))
    }

    /// Appends one or more existing library items to a playlist (deduplicating already-present items).
    pub fn add_items_to_playlist(
        &self,
        playlist_id: &str,
        media_ids: &[MediaId],
    ) -> AppResult<Playlist> {
        self.playlist_repo
            .add_items_to_playlist(playlist_id, media_ids, &now_timestamp_string())
    }

    /// Removes a single media item from a playlist without removing it from the library.
    pub fn remove_item_from_playlist(
        &self,
        playlist_id: &str,
        media_id: &str,
    ) -> AppResult<Playlist> {
        self.playlist_repo
            .remove_item_from_playlist(playlist_id, media_id, &now_timestamp_string())
    }

    /// Moves a media item to `target_index` (`0-based`) within a playlist.
    pub fn move_playlist_item(
        &self,
        playlist_id: &str,
        media_id: &str,
        target_index: usize,
    ) -> AppResult<Playlist> {
        self.playlist_repo.move_playlist_item(
            playlist_id,
            media_id,
            target_index,
            &now_timestamp_string(),
        )
    }

    /// Loads a playlist's available items into the session queue (skipping offline items) and starts playback.
    pub fn play_playlist(
        &self,
        playlist_id: &str,
        start_media_id: Option<&str>,
        shuffle: bool,
    ) -> AppResult<PlaybackSnapshot> {
        let playlist = self.get_playlist(playlist_id)?;
        let mut available_items: Vec<MediaItem> = Vec::new();
        for mid in &playlist.item_ids {
            if let Some(item) = self.media_repo.get_by_id(mid)? {
                if item.availability == Availability::Available && Path::new(&item.path).exists() {
                    available_items.push(item);
                } else if !Path::new(&item.path).exists() {
                    let _ = self.media_repo.update_availability(
                        &item.id,
                        Availability::Missing,
                        &now_timestamp_string(),
                    );
                }
            }
        }

        if available_items.is_empty() {
            return Err(AppError::media_unavailable(
                "No playable media files are currently available in this playlist.",
            ));
        }

        let target = if let Some(sid) = start_media_id {
            available_items
                .iter()
                .find(|m| m.id == sid)
                .ok_or_else(|| {
                    AppError::media_unavailable(
                        "Selected playlist item is offline or missing. Relink the file to play it.",
                    )
                })?
        } else {
            &available_items[0]
        };

        self.open_media(&target.path)?;

        let queue_ids: Vec<MediaId> = available_items.iter().map(|m| m.id.clone()).collect();
        {
            let mut snap = self
                .snapshot
                .lock()
                .map_err(|_| AppError::internal("Playback snapshot lock poisoned"))?;
            snap.queue = queue_ids;
            snap.shuffle = shuffle;
        }

        self.playback_command(PlaybackCommand::Play)
    }

    fn updates_dir(&self) -> PathBuf {
        self.app_data_dir.join("updates")
    }

    fn staged_manifest_path(&self) -> PathBuf {
        self.updates_dir().join("release-manifest.json")
    }

    /// Stages a release manifest into `<app_data_dir>/updates/release-manifest.json`.
    pub fn stage_release_manifest(&self, manifest: &ReleaseManifestDto) -> AppResult<PathBuf> {
        let dir = self.updates_dir();
        fs::create_dir_all(&dir).map_err(|e| {
            tracing::error!(path = %redact_path(&dir), "Failed to create updates dir: {e}");
            AppError::persistence("Failed to create local update staging directory.")
        })?;
        let path = self.staged_manifest_path();
        let bytes = serde_json::to_vec_pretty(manifest).map_err(|e| {
            AppError::internal(format!("Failed to serialize release manifest: {e}"))
        })?;
        fs::write(&path, bytes).map_err(|e| {
            tracing::error!(path = %redact_path(&path), "Failed to write staged manifest: {e}");
            AppError::persistence("Failed to write staged release manifest.")
        })?;
        Ok(path)
    }

    /// Parses a GitHub Releases `/repos/{owner}/{repo}/releases/latest` JSON payload,
    /// stages the manifest on disk, and evaluates the update status.
    pub fn stage_github_release_json(&self, raw_json: &str) -> AppResult<AppUpdateStatusDto> {
        let manifest = parse_github_release_json(raw_json)?;
        let _ = self.stage_release_manifest(&manifest)?;
        self.check_for_updates_with_options(false)
    }

    /// Checks whether a newer signed/verified release is available relative to the current version.
    pub fn check_for_updates(&self) -> AppResult<AppUpdateStatusDto> {
        self.check_for_updates_with_options(false)
    }

    /// Checks whether a newer signed/verified release is available, optionally including a
    /// previously dismissed version (used when the user manually clicks "Check for updates").
    pub fn check_for_updates_with_options(
        &self,
        include_dismissed: bool,
    ) -> AppResult<AppUpdateStatusDto> {
        let settings = self.settings_repo.load_settings()?;
        let cargo_ver = env!("CARGO_PKG_VERSION").to_owned();
        let current_version = match settings.installed_version {
            Some(ref v) if compare_semver(v, &cargo_ver) == CmpOrdering::Greater => v.clone(),
            _ => cargo_ver,
        };
        let github_repo = crate::domain::DEFAULT_GITHUB_RELEASE_REPO.to_owned();

        let now_ms = i64::try_from(
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_millis())
                .unwrap_or(0),
        )
        .unwrap_or(0);

        let manifest_path = self.staged_manifest_path();
        let manifest: Option<ReleaseManifestDto> = if manifest_path.is_file() {
            fs::read_to_string(&manifest_path)
                .ok()
                .and_then(|raw| serde_json::from_str::<ReleaseManifestDto>(&raw).ok())
        } else {
            None
        };

        if let Some(m) = manifest {
            let is_newer = compare_semver(&m.version, &current_version) == CmpOrdering::Greater;
            let is_dismissed = !include_dismissed
                && settings.dismissed_update_version.as_deref() == Some(&m.version);
            let verified = m.sha256.len() == 64 && m.sha256.chars().all(|c| c.is_ascii_hexdigit());
            let download_url = if m.download_url.is_empty() {
                format!(
                    "https://github.com/{github_repo}/releases/tag/v{}",
                    m.version
                )
            } else {
                m.download_url
            };

            return Ok(AppUpdateStatusDto {
                current_version,
                update_available: is_newer && !is_dismissed && verified,
                latest_version: m.version,
                release_title: m.title,
                release_notes: m.notes,
                package_size_bytes: m.package_size_bytes,
                sha256_checksum: m.sha256,
                download_url,
                github_repo,
                verified,
                staged_for_restart: false,
                checked_at_unix_ms: now_ms,
            });
        }

        let mut hasher = Sha256::new();
        hasher.update(format!("audora-release-{}", current_version).as_bytes());
        let sha256_checksum = format!("{:x}", hasher.finalize());

        Ok(AppUpdateStatusDto {
            current_version: current_version.clone(),
            update_available: false,
            latest_version: current_version.clone(),
            release_title: format!("Audora {current_version}"),
            release_notes: "You are running the latest verified release of Audora.".to_owned(),
            package_size_bytes: 0,
            sha256_checksum,
            download_url: format!("https://github.com/{github_repo}/releases"),
            github_repo,
            verified: true,
            staged_for_restart: false,
            checked_at_unix_ms: now_ms,
        })
    }

    /// Marks the target update version as installed and staged for restart after verifying its checksum.
    pub fn install_update(&self, target_version: &str) -> AppResult<AppUpdateStatusDto> {
        let trimmed = target_version.trim().trim_start_matches(['v', 'V']);
        if trimmed.is_empty() {
            return Err(AppError::validation(
                "Target update version must not be empty",
            ));
        }

        let status = self.check_for_updates_with_options(true)?;
        if !status.verified {
            return Err(AppError::validation(
                "Cannot install an unverified release package (SHA-256 signature check failed).",
            ));
        }
        if !status.update_available || status.latest_version != trimmed {
            return Err(AppError::validation(
                "Requested update version does not match a verified staged release.",
            ));
        }

        let mut settings = self.settings_repo.load_settings()?;
        settings.installed_version = Some(trimmed.to_owned());
        settings.dismissed_update_version = None;
        self.settings_repo
            .save_settings(&settings, &now_timestamp_string())?;
        let _ = fs::remove_file(self.staged_manifest_path());

        Ok(AppUpdateStatusDto {
            current_version: trimmed.to_owned(),
            update_available: false,
            latest_version: trimmed.to_owned(),
            staged_for_restart: true,
            ..status
        })
    }

    /// Dismisses notifications for a specific update version until a newer release is published.
    pub fn dismiss_update(&self, version: &str) -> AppResult<AppUpdateStatusDto> {
        let trimmed = version.trim().trim_start_matches(['v', 'V']);
        if trimmed.is_empty() {
            return Err(AppError::validation(
                "Update version to dismiss must not be empty",
            ));
        }
        let mut settings = self.settings_repo.load_settings()?;
        settings.dismissed_update_version = Some(trimmed.to_owned());
        self.settings_repo
            .save_settings(&settings, &now_timestamp_string())?;
        self.check_for_updates()
    }

    /// Cleans all local application caches, update staging files, and database artifacts in `app_data_dir`.
    pub fn cleanup_application_data(&self) -> AppResult<CleanupSummaryDto> {
        let mut vacuum_reclaimed: u64 = 0;
        let mut active_db_file: Option<PathBuf> = None;
        if let Some(ref db) = self.db {
            if let Some(db_path) = db.db_path().map(Path::to_path_buf) {
                let wal_path = PathBuf::from(format!("{}-wal", db_path.display()));
                let before_bytes = fs::metadata(&db_path).map(|m| m.len()).unwrap_or(0)
                    + fs::metadata(&wal_path).map(|m| m.len()).unwrap_or(0);
                let _ = db.reset_all_user_data();
                let _ = db.with_conn(|conn| {
                    let _ = conn.execute_batch("VACUUM; PRAGMA wal_checkpoint(TRUNCATE);");
                    Ok(())
                });
                let after_bytes = fs::metadata(&db_path).map(|m| m.len()).unwrap_or(0)
                    + fs::metadata(&wal_path).map(|m| m.len()).unwrap_or(0);
                vacuum_reclaimed = before_bytes.saturating_sub(after_bytes);
                if before_bytes > 0 {
                    active_db_file = Some(db_path);
                }
            }
        }
        let mut summary = cleanup_all_app_artifacts(&self.app_data_dir)?;
        summary.bytes_reclaimed = summary.bytes_reclaimed.saturating_add(vacuum_reclaimed);
        if let Some(db_file) = active_db_file {
            let db_display = db_file.display().to_string();
            if !summary.removed_paths.iter().any(|p| p == &db_display) {
                summary.removed_paths.push(db_display);
            }
        }
        Ok(summary)
    }
}

/// Compares two semantic version strings (`major.minor.patch[-prerelease]`, ignoring optional leading `v`).
///
/// Follows SemVer 2.0.0 §11.4 precedence: pre-release versions sort strictly below stable releases,
/// and dot-separated numeric pre-release identifiers are compared numerically (`rc.10 > rc.2`) (`L-3`).
#[must_use]
pub fn compare_semver(a: &str, b: &str) -> CmpOrdering {
    fn parse_semver(v: &str) -> ((u64, u64, u64), Option<&str>) {
        let clean = v.trim().trim_start_matches(['v', 'V']);
        let no_build = clean.split('+').next().unwrap_or(clean);
        let (core, pre) = match no_build.split_once('-') {
            Some((c, p)) if !p.is_empty() => (c, Some(p)),
            _ => (no_build, None),
        };
        let mut iter = core.split('.').map(|s| s.parse::<u64>().unwrap_or(0));
        (
            (
                iter.next().unwrap_or(0),
                iter.next().unwrap_or(0),
                iter.next().unwrap_or(0),
            ),
            pre,
        )
    }

    fn cmp_prerelease(pa: &str, pb: &str) -> CmpOrdering {
        let mut ia = pa.split('.');
        let mut ib = pb.split('.');
        loop {
            match (ia.next(), ib.next()) {
                (None, None) => return CmpOrdering::Equal,
                (None, Some(_)) => return CmpOrdering::Less,
                (Some(_), None) => return CmpOrdering::Greater,
                (Some(sa), Some(sb)) => {
                    let ord = match (sa.parse::<u64>(), sb.parse::<u64>()) {
                        (Ok(na), Ok(nb)) => na.cmp(&nb),
                        (Ok(_), Err(_)) => CmpOrdering::Less,
                        (Err(_), Ok(_)) => CmpOrdering::Greater,
                        (Err(_), Err(_)) => sa.cmp(sb),
                    };
                    if ord != CmpOrdering::Equal {
                        return ord;
                    }
                }
            }
        }
    }

    let (core_a, pre_a) = parse_semver(a);
    let (core_b, pre_b) = parse_semver(b);
    match core_a.cmp(&core_b) {
        CmpOrdering::Equal => match (pre_a, pre_b) {
            (None, None) => CmpOrdering::Equal,
            (Some(_), None) => CmpOrdering::Less,
            (None, Some(_)) => CmpOrdering::Greater,
            (Some(pa), Some(pb)) => cmp_prerelease(pa, pb),
        },
        ord => ord,
    }
}

/// Parses a GitHub Releases `/repos/{owner}/{repo}/releases/latest` JSON payload into a
/// `ReleaseManifestDto`, selecting the highest-priority platform installer asset (`C-5`) and extracting
/// its explicit 64-hex `sha256:` digest (leaving `sha256` empty so `verified == false` when absent).
pub fn parse_github_release_json(raw_json: &str) -> AppResult<ReleaseManifestDto> {
    let v: serde_json::Value = serde_json::from_str(raw_json)
        .map_err(|e| AppError::validation(format!("Invalid GitHub release JSON payload: {e}")))?;

    let tag_name = v
        .get("tag_name")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| AppError::validation("GitHub release JSON is missing 'tag_name'"))?;
    let version = tag_name.trim().trim_start_matches(['v', 'V']).to_owned();
    if version.is_empty() {
        return Err(AppError::validation(
            "GitHub release tag_name resolved to an empty version",
        ));
    }

    let title = v
        .get("name")
        .and_then(serde_json::Value::as_str)
        .filter(|s| !s.trim().is_empty())
        .map_or_else(|| format!("Audora {version}"), str::to_owned);
    let notes = v
        .get("body")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("Release published on GitHub.")
        .to_owned();
    let published_at = v
        .get("published_at")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("")
        .to_owned();
    let html_url = v
        .get("html_url")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("")
        .to_owned();

    // `(priority_rank, url, size, sha256)` — lower `priority_rank` wins so `.app.tar.gz` beats `.dmg`
    // regardless of GitHub asset upload order (`C-5`).
    let mut best_arch_match: Option<(usize, String, u64, String)> = None;
    let mut best_os_match: Option<(usize, String, u64, String)> = None;
    let mut fallback: Option<(String, u64, String)> = None;

    if let Some(assets) = v.get("assets").and_then(serde_json::Value::as_array) {
        let os_suffixes: &[&str] = if cfg!(target_os = "macos") {
            &[".app.tar.gz", ".dmg"]
        } else if cfg!(target_os = "windows") {
            &["-setup.exe", ".msi", "-portable.zip", ".exe"]
        } else {
            &[".deb", ".appimage", ".tar.gz"]
        };
        let arch_hint = if cfg!(target_arch = "aarch64") {
            "aarch64"
        } else {
            "x86_64"
        };

        for asset in assets {
            let name = asset
                .get("name")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("")
                .to_ascii_lowercase();
            if name.ends_with(".json") || name.ends_with(".sha256") || name.ends_with(".sig") {
                continue;
            }
            let url = asset
                .get("browser_download_url")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("");
            if url.is_empty() {
                continue;
            }
            let size = asset
                .get("size")
                .and_then(serde_json::Value::as_u64)
                .unwrap_or(0);
            let asset_sha = asset
                .get("digest")
                .and_then(serde_json::Value::as_str)
                .and_then(|d| d.strip_prefix("sha256:"))
                .filter(|hex| hex.len() == 64 && hex.chars().all(|c| c.is_ascii_hexdigit()))
                .map_or_else(String::new, str::to_ascii_lowercase);

            if fallback.is_none() {
                fallback = Some((url.to_owned(), size, asset_sha.clone()));
            }

            if let Some(rank) = os_suffixes.iter().position(|ext| name.ends_with(ext)) {
                let matches_arch = name.contains(arch_hint)
                    || (!name.contains("aarch64") && !name.contains("x86_64"));
                if matches_arch {
                    if best_arch_match
                        .as_ref()
                        .is_none_or(|(r, _, _, _)| rank < *r)
                    {
                        best_arch_match = Some((rank, url.to_owned(), size, asset_sha));
                    }
                } else if best_os_match.as_ref().is_none_or(|(r, _, _, _)| rank < *r) {
                    best_os_match = Some((rank, url.to_owned(), size, asset_sha));
                }
            }
        }
    }

    let (download_url, package_size_bytes, sha256) = best_arch_match
        .or(best_os_match)
        .map(|(_, u, s, h)| (u, s, h))
        .or(fallback)
        .unwrap_or((html_url, 0, String::new()));

    Ok(ReleaseManifestDto {
        version,
        title,
        notes,
        package_size_bytes,
        sha256,
        published_at,
        download_url,
    })
}

/// Removes Audora's own application artifacts inside `app_data_dir` (`--uninstall-cleanup` / Settings reset)
/// without following symlinks or touching unrelated files in shared directories (`H-3`).
pub fn cleanup_all_app_artifacts(app_data_dir: &Path) -> AppResult<CleanupSummaryDto> {
    let mut removed_paths = Vec::new();
    let mut bytes_reclaimed: u64 = 0;

    if !app_data_dir.exists() {
        return Ok(CleanupSummaryDto {
            removed_paths,
            bytes_reclaimed,
        });
    }

    fn dir_size(path: &Path) -> u64 {
        let Ok(meta) = fs::symlink_metadata(path) else {
            return 0;
        };
        if meta.file_type().is_symlink() {
            return 0;
        }
        if meta.is_file() {
            return meta.len();
        }
        let Ok(entries) = fs::read_dir(path) else {
            return 0;
        };
        entries
            .filter_map(Result::ok)
            .map(|e| dir_size(&e.path()))
            .sum()
    }

    fn is_owned_audora_artifact(name: &str) -> bool {
        matches!(
            name,
            "audora.sqlite"
                | "audora.sqlite-wal"
                | "audora.sqlite-shm"
                | "ui-state.json"
                | "updates"
                | "webview"
                | "logs"
                | "cache"
        ) || (name.starts_with("audora.sqlite.")
            && (name.ends_with(".bak") || name.ends_with(".bak.tmp")))
    }

    if let Ok(entries) = fs::read_dir(app_data_dir) {
        for entry in entries.filter_map(Result::ok) {
            let p = entry.path();
            let Some(name) = p.file_name().and_then(|s| s.to_str()) else {
                continue;
            };
            if !is_owned_audora_artifact(name) {
                continue;
            }
            let Ok(meta) = fs::symlink_metadata(&p) else {
                continue;
            };
            let sz = dir_size(&p);
            let remove_res = if meta.file_type().is_symlink() || meta.is_file() {
                fs::remove_file(&p)
            } else if meta.is_dir() {
                fs::remove_dir_all(&p)
            } else {
                continue;
            };
            if remove_res.is_ok() {
                bytes_reclaimed = bytes_reclaimed.saturating_add(sz);
                removed_paths.push(p.display().to_string());
            }
        }
    }

    // Only removes `app_data_dir` itself if it is now completely empty.
    let _ = fs::remove_dir(app_data_dir);
    Ok(CleanupSummaryDto {
        removed_paths,
        bytes_reclaimed,
    })
}
