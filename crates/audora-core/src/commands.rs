//! Narrow validated IPC command boundary between the TypeScript UI and Rust application services.
//!
//! All commands return `Result<T, AppErrorPayload>` with stable `AV-*` error codes (`docs/ERROR_CATALOG.md`).

use crate::domain::{
    AppSettings, AppUpdateStatusDto, CleanupSummaryDto, CreateNoteInput, CreatePlaylistInput,
    DiagnosticReport, LibraryFilter, LibrarySearchResult, MediaId, MediaItem, Note,
    PlaybackCommand, PlaybackSnapshot, Playlist, ReleaseManifestDto, UpdateNoteInput,
    UpdatePlaylistInput, WindowMode,
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

    pub fn check_for_updates(&self) -> CommandResult<AppUpdateStatusDto> {
        self.service
            .check_for_updates()
            .map_err(|err| err.to_payload())
    }

    pub fn check_for_updates_with_options(
        &self,
        include_dismissed: bool,
    ) -> CommandResult<AppUpdateStatusDto> {
        self.service
            .check_for_updates_with_options(include_dismissed)
            .map_err(|err| err.to_payload())
    }

    pub fn stage_release_manifest(&self, manifest: &ReleaseManifestDto) -> CommandResult<String> {
        self.service
            .stage_release_manifest(manifest)
            .map(|p| p.display().to_string())
            .map_err(|err| err.to_payload())
    }

    pub fn stage_github_release_json(&self, raw_json: &str) -> CommandResult<AppUpdateStatusDto> {
        self.service
            .stage_github_release_json(raw_json)
            .map_err(|err| err.to_payload())
    }

    pub fn install_update(&self, target_version: &str) -> CommandResult<AppUpdateStatusDto> {
        self.service
            .install_update(target_version)
            .map_err(|err| err.to_payload())
    }

    pub fn dismiss_update(&self, version: &str) -> CommandResult<AppUpdateStatusDto> {
        self.service
            .dismiss_update(version)
            .map_err(|err| err.to_payload())
    }

    pub fn cleanup_application_data(&self) -> CommandResult<CleanupSummaryDto> {
        self.service
            .cleanup_application_data()
            .map_err(|err| err.to_payload())
    }

    pub fn create_playlist(&self, input: CreatePlaylistInput) -> CommandResult<Playlist> {
        self.service
            .create_playlist(input)
            .map_err(|err| err.to_payload())
    }

    pub fn update_playlist(&self, input: UpdatePlaylistInput) -> CommandResult<Playlist> {
        self.service
            .update_playlist(input)
            .map_err(|err| err.to_payload())
    }

    pub fn delete_playlist(&self, playlist_id: &str) -> CommandResult<bool> {
        self.service
            .delete_playlist(playlist_id)
            .map_err(|err| err.to_payload())
    }

    pub fn list_playlists(&self) -> CommandResult<Vec<Playlist>> {
        self.service
            .list_playlists()
            .map_err(|err| err.to_payload())
    }

    pub fn get_playlist(&self, playlist_id: &str) -> CommandResult<Playlist> {
        self.service
            .get_playlist(playlist_id)
            .map_err(|err| err.to_payload())
    }

    pub fn add_items_to_playlist(
        &self,
        playlist_id: &str,
        media_ids: &[MediaId],
    ) -> CommandResult<Playlist> {
        self.service
            .add_items_to_playlist(playlist_id, media_ids)
            .map_err(|err| err.to_payload())
    }

    pub fn remove_item_from_playlist(
        &self,
        playlist_id: &str,
        media_id: &str,
    ) -> CommandResult<Playlist> {
        self.service
            .remove_item_from_playlist(playlist_id, media_id)
            .map_err(|err| err.to_payload())
    }

    pub fn move_playlist_item(
        &self,
        playlist_id: &str,
        media_id: &str,
        target_index: usize,
    ) -> CommandResult<Playlist> {
        self.service
            .move_playlist_item(playlist_id, media_id, target_index)
            .map_err(|err| err.to_payload())
    }

    pub fn play_playlist(
        &self,
        playlist_id: &str,
        start_media_id: Option<&str>,
        shuffle: bool,
    ) -> CommandResult<PlaybackSnapshot> {
        self.service
            .play_playlist(playlist_id, start_media_id, shuffle)
            .map_err(|err| err.to_payload())
    }
}
