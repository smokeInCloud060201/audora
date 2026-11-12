//! Application services orchestrating library records, playback state, timestamped notes,
//! and user settings (`docs/FEATURES.md`).

use std::fs;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::domain::{
    AppSettings, Availability, BackendEvent, CreateNoteInput, DiagnosticReport, LibraryFilter,
    LibrarySearchResult, MediaItem, MediaRepository, Note, NotesRepository, PlaybackBackend,
    PlaybackCommand, PlaybackSnapshot, PlaybackState, RelinkMediaRecord, RepeatMode,
    SettingsRepository, TimestampPrecision, UpdateNoteInput, WindowMode, MAX_NOTE_BODY_CHARS,
    MAX_PLAYBACK_RATE, MAX_SEARCH_QUERY_CHARS, MIN_PLAYBACK_RATE,
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
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    format!("{secs}")
}

/// Core application coordinator owning domain invariants across playback, library, notes, and settings.
pub struct AudoraAppService {
    media_repo: Arc<dyn MediaRepository>,
    notes_repo: Arc<dyn NotesRepository>,
    settings_repo: Arc<dyn SettingsRepository>,
    playback_backend: Arc<dyn PlaybackBackend>,
    snapshot: Mutex<PlaybackSnapshot>,
}

impl AudoraAppService {
    #[must_use]
    pub fn new(
        media_repo: Arc<dyn MediaRepository>,
        notes_repo: Arc<dyn NotesRepository>,
        settings_repo: Arc<dyn SettingsRepository>,
        playback_backend: Arc<dyn PlaybackBackend>,
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
            settings_repo,
            playback_backend,
            snapshot: Mutex::new(snapshot),
        }
    }

    fn validate_local_media_path(raw_path: &str) -> AppResult<&str> {
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

    /// Opens a local media file, probes its stream metadata without modifying the source file,
    /// records/updates its library entry, increments the load generation, and loads it for playback.
    pub fn open_media(&self, raw_path: &str) -> AppResult<MediaItem> {
        let trimmed = Self::validate_local_media_path(raw_path)?;
        let path = Path::new(trimmed);
        let file_name = path
            .file_name()
            .and_then(|s| s.to_str())
            .filter(|s| !s.is_empty())
            .ok_or_else(|| AppError::validation("Media path must point to a valid file name"))?
            .to_owned();

        if !path.exists() {
            if let Some(existing) = self.media_repo.get_by_path(trimmed)? {
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

        let existing = self.media_repo.get_by_path(trimmed)?;
        let now = now_timestamp_string();
        let media_id = existing
            .as_ref()
            .map_or_else(|| generate_id("med"), |m| m.id.clone());
        let prior_pos = existing.as_ref().map_or(0, |m| m.last_position_ms);
        let prior_completed = existing.as_ref().is_some_and(|m| m.completed);

        let item_to_save = MediaItem {
            id: media_id,
            path: trimmed.to_owned(),
            display_name: file_name,
            media_kind: probed.media_kind,
            duration_ms: probed.duration_ms,
            last_position_ms: prior_pos,
            completed: prior_completed,
            file_size_bytes: Some(metadata.len()),
            fingerprint: None,
            availability: Availability::Available,
            added_at: existing.map_or_else(|| now.clone(), |m| m.added_at),
            last_opened_at: Some(now.clone()),
            last_seen_at: now,
            note_count: 0,
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

        let mut expanded_paths: Vec<String> = Vec::new();
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
                        entry_path.is_file() && Self::is_supported_media_extension(entry_path)
                    })
                    .filter_map(|entry_path| entry_path.to_str().map(str::to_owned))
                    .collect();
                dir_files.sort();
                expanded_paths.extend(dir_files);
            } else {
                expanded_paths.push(trimmed.to_owned());
            }
        }

        if expanded_paths.is_empty() {
            return Err(AppError::media_unavailable(
                "No supported audio or video files were found in the selection.",
            ));
        }

        let mut opened_items = Vec::with_capacity(expanded_paths.len());
        for path_str in &expanded_paths {
            let item = self.open_media(path_str)?;
            opened_items.push(item);
        }

        // Keep the first opened item active while leaving all opened items in the queue.
        if opened_items.len() > 1 {
            let first = &opened_items[0];
            let _ = self.open_media(&first.path)?;
        }

        Ok(opened_items)
    }

    fn is_supported_media_extension(path: &Path) -> bool {
        matches!(
            path.extension()
                .and_then(|e| e.to_str())
                .map(str::to_ascii_lowercase)
                .as_deref(),
            Some("mp4" | "mov" | "webm" | "mkv" | "mp3" | "wav" | "flac" | "ogg" | "opus" | "m4a")
        )
    }

    /// Removes a media item from the active session queue without deleting its library record.
    pub fn remove_from_queue(&self, media_id: &str) -> AppResult<PlaybackSnapshot> {
        let mut snap = self
            .snapshot
            .lock()
            .map_err(|_| AppError::internal("Playback snapshot lock poisoned"))?;
        let current_idx = snap.queue.iter().position(|id| id == media_id);
        snap.queue.retain(|id| id != media_id);

        if snap.media_id.as_deref() == Some(media_id) {
            if snap.queue.is_empty() {
                let _ = self.playback_backend.stop();
                snap.media_id = None;
                snap.state = PlaybackState::Idle;
                snap.position_ms = 0;
            } else {
                let next_idx = current_idx
                    .unwrap_or(0)
                    .min(snap.queue.len().saturating_sub(1));
                snap.media_id = Some(snap.queue[next_idx].clone());
                snap.position_ms = 0;
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
                let target = if delta_ms.is_negative() {
                    snap.position_ms.saturating_sub(delta_ms.unsigned_abs())
                } else {
                    snap.position_ms.saturating_add(delta_ms.unsigned_abs())
                };
                let acknowledged = self.playback_backend.seek(gen, target)?;
                snap.position_ms = acknowledged;
            }
            PlaybackCommand::SetRate { rate } => {
                if !(MIN_PLAYBACK_RATE..=MAX_PLAYBACK_RATE).contains(&rate) {
                    return Err(AppError::validation(format!(
                        "Playback speed must be between {MIN_PLAYBACK_RATE}x and {MAX_PLAYBACK_RATE}x"
                    )));
                }
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
                    if let Some(idx) = snap.queue.iter().position(|id| id == &current_id) {
                        let len = snap.queue.len();
                        let next_idx = if snap.shuffle && len > 2 {
                            Some((idx + 2) % len)
                        } else if idx + 1 < len {
                            Some(idx + 1)
                        } else if (snap.repeat_mode == RepeatMode::All || snap.shuffle) && len > 0 {
                            Some(0)
                        } else {
                            None
                        };
                        if let Some(i) = next_idx {
                            let next_id = snap.queue[i].clone();
                            snap.media_id = Some(next_id);
                            snap.position_ms = 0;
                        }
                    }
                }
            }
            PlaybackCommand::Previous => {
                if snap.position_ms > 3_000 {
                    let acknowledged = self.playback_backend.seek(gen, 0)?;
                    snap.position_ms = acknowledged;
                } else if let Some(current_id) = snap.media_id.clone() {
                    if let Some(idx) = snap.queue.iter().position(|id| id == &current_id) {
                        if idx > 0 {
                            snap.media_id = Some(snap.queue[idx - 1].clone());
                            snap.position_ms = 0;
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
                        let next_idx = if snap.shuffle && len > 2 {
                            Some((idx + 2) % len)
                        } else if idx + 1 < len {
                            Some(idx + 1)
                        } else if snap.repeat_mode == RepeatMode::All && len > 1 {
                            Some(0)
                        } else {
                            None
                        };
                        if let Some(i) = next_idx {
                            snap.media_id = Some(snap.queue[i].clone());
                            snap.position_ms = 0;
                            snap.state = PlaybackState::Playing;
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

        let trimmed_body = input.body.trim().to_owned();
        if trimmed_body.chars().count() > MAX_NOTE_BODY_CHARS {
            return Err(AppError::validation(format!(
                "Note text exceeds maximum length of {MAX_NOTE_BODY_CHARS} characters"
            )));
        }

        if trimmed_body.is_empty() && !input.is_quick_marker {
            return Err(AppError::validation(
                "Enter note text or check 'Save as quick marker' before saving.",
            ));
        }

        let settings = self.settings_repo.load_settings()?;
        let rounded_pos = match settings.timestamp_precision {
            TimestampPrecision::NearestSecond => ((input.position_ms + 500) / 1_000) * 1_000,
            TimestampPrecision::Milliseconds => input.position_ms,
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
        let trimmed = input.body.trim();
        if trimmed.is_empty() {
            return Err(AppError::validation(
                "Note text cannot be empty when editing",
            ));
        }
        if trimmed.chars().count() > MAX_NOTE_BODY_CHARS {
            return Err(AppError::validation(format!(
                "Note text exceeds maximum length of {MAX_NOTE_BODY_CHARS} characters"
            )));
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

    /// Activates a note ("Jump to moment"): loads the note's media if not already active and seeks
    /// directly to `note.position_ms` (overriding `Resume where I left off`).
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

        self.playback_command(PlaybackCommand::Seek {
            position_ms: note.position_ms,
        })
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

        // Prohibit silent record merges (`AV-201`).
        if let Some(existing) = self.media_repo.get_by_path(trimmed_path)? {
            if existing.id != current.id {
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

        let now = now_timestamp_string();
        self.media_repo.relink_path(&RelinkMediaRecord {
            id: media_id,
            new_path: trimmed_path,
            display_name: &display_name,
            media_kind: probed.media_kind,
            duration_ms: probed.duration_ms,
            file_size_bytes: Some(fs_meta.len()),
            updated_at: &now,
        })
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
        if !(MIN_PLAYBACK_RATE..=MAX_PLAYBACK_RATE).contains(&settings.default_playback_speed) {
            return Err(AppError::validation(format!(
                "Default playback speed must be between {MIN_PLAYBACK_RATE}x and {MAX_PLAYBACK_RATE}x"
            )));
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
        let snap = self.get_playback_snapshot()?;

        Ok(DiagnosticReport {
            app_version: env!("CARGO_PKG_VERSION").to_owned(),
            os_family: std::env::consts::OS.to_owned(),
            arch: std::env::consts::ARCH.to_owned(),
            schema_version: 1,
            media_count,
            missing_media_count,
            total_note_count,
            playback_state: snap.state,
            last_error_code: snap.error.map(|e| e.code.as_str().to_owned()),
            generated_at: now_timestamp_string(),
        })
    }
}
