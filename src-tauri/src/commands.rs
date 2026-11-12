//! Narrow validated IPC command boundary between the TypeScript UI and Rust application services.
//!
//! All commands return `Result<T, AppErrorPayload>` with stable `AV-*` error codes (`docs/ERROR_CATALOG.md`).

use crate::domain::{
    AppSettings, CreateNoteInput, DiagnosticReport, LibraryFilter, LibrarySearchResult, MediaItem,
    Note, PlaybackCommand, PlaybackSnapshot, UpdateNoteInput, WindowMode,
};
use crate::error::AppErrorPayload;
use crate::services::AudoraAppService;

pub type CommandResult<T> = Result<T, AppErrorPayload>;

/// Validated IPC command dispatcher wrapping `AudoraAppService`.
pub struct CommandDispatcher {
    service: AudoraAppService,
}

impl CommandDispatcher {
    #[must_use]
    pub const fn new(service: AudoraAppService) -> Self {
        Self { service }
    }

    #[must_use]
    pub const fn service(&self) -> &AudoraAppService {
        &self.service
    }

    pub fn open_media(&self, path: &str) -> CommandResult<MediaItem> {
        self.service
            .open_media(path)
            .map_err(|err| err.to_payload())
    }

    pub fn open_media_batch(&self, paths: &[String]) -> CommandResult<Vec<MediaItem>> {
        self.service
            .open_media_batch(paths)
            .map_err(|err| err.to_payload())
    }

    pub fn remove_from_queue(&self, media_id: &str) -> CommandResult<PlaybackSnapshot> {
        self.service
            .remove_from_queue(media_id)
            .map_err(|err| err.to_payload())
    }

    pub fn mark_media_missing(&self, media_id: &str) -> CommandResult<MediaItem> {
        self.service
            .mark_media_missing(media_id)
            .map_err(|err| err.to_payload())
    }

    pub fn get_playback_snapshot(&self) -> CommandResult<PlaybackSnapshot> {
        self.service
            .get_playback_snapshot()
            .map_err(|err| err.to_payload())
    }

    pub fn playback_command(&self, command: PlaybackCommand) -> CommandResult<PlaybackSnapshot> {
        self.service
            .playback_command(command)
            .map_err(|err| err.to_payload())
    }

    pub fn create_note(&self, input: CreateNoteInput) -> CommandResult<Note> {
        self.service
            .create_note(input)
            .map_err(|err| err.to_payload())
    }

    pub fn update_note(&self, input: UpdateNoteInput) -> CommandResult<Note> {
        self.service
            .update_note(input)
            .map_err(|err| err.to_payload())
    }

    pub fn delete_note(&self, note_id: &str) -> CommandResult<bool> {
        self.service
            .delete_note(note_id)
            .map_err(|err| err.to_payload())
    }

    pub fn list_notes(&self, media_id: &str) -> CommandResult<Vec<Note>> {
        self.service
            .list_notes(media_id)
            .map_err(|err| err.to_payload())
    }

    pub fn activate_note(&self, note_id: &str) -> CommandResult<PlaybackSnapshot> {
        self.service
            .activate_note(note_id)
            .map_err(|err| err.to_payload())
    }

    pub fn search_library(
        &self,
        query: Option<&str>,
        filter: LibraryFilter,
    ) -> CommandResult<LibrarySearchResult> {
        self.service
            .search_library(query, filter)
            .map_err(|err| err.to_payload())
    }

    pub fn relink_media(
        &self,
        media_id: &str,
        new_path: &str,
        confirm_mismatch: bool,
    ) -> CommandResult<MediaItem> {
        self.service
            .relink_media(media_id, new_path, confirm_mismatch)
            .map_err(|err| err.to_payload())
    }

    pub fn remove_media(&self, media_id: &str) -> CommandResult<bool> {
        self.service
            .remove_media(media_id)
            .map_err(|err| err.to_payload())
    }

    pub fn get_settings(&self) -> CommandResult<AppSettings> {
        self.service.get_settings().map_err(|err| err.to_payload())
    }

    pub fn update_settings(&self, settings: AppSettings) -> CommandResult<AppSettings> {
        self.service
            .update_settings(settings)
            .map_err(|err| err.to_payload())
    }

    pub fn set_window_mode(&self, mode: WindowMode) -> CommandResult<AppSettings> {
        self.service
            .set_window_mode(mode)
            .map_err(|err| err.to_payload())
    }

    pub fn generate_diagnostic_report(&self) -> CommandResult<DiagnosticReport> {
        self.service
            .generate_diagnostic_report()
            .map_err(|err| err.to_payload())
    }
}
