//! Audora Rust application core (`src-tauri`).
//!
//! Implements domain models, traits, GStreamer & fake playback adapters, SQLite WAL persistence
//! with pre-migration backups, and validated IPC command handlers.

pub mod adapters;
pub mod commands;
pub mod domain;
pub mod error;
pub mod persistence;
pub mod services;

use std::path::Path;
use std::sync::Arc;

use adapters::{FakePlaybackBackend, GStreamerBackend};
use commands::CommandDispatcher;
use error::AppResult;
use persistence::{
    Database, SqliteMediaRepository, SqliteNotesRepository, SqliteSettingsRepository,
};
use services::AudoraAppService;

/// Builds a production `CommandDispatcher` backed by an on-disk SQLite database and `GStreamerBackend`.
pub fn build_production_dispatcher(db_path: impl AsRef<Path>) -> AppResult<CommandDispatcher> {
    let db = Database::open(db_path)?;
    let media_repo = Arc::new(SqliteMediaRepository::new(db.clone()));
    let notes_repo = Arc::new(SqliteNotesRepository::new(db.clone()));
    let settings_repo = Arc::new(SqliteSettingsRepository::new(db));
    let playback_backend = Arc::new(GStreamerBackend::new());
    let service = AudoraAppService::new(media_repo, notes_repo, settings_repo, playback_backend);
    Ok(CommandDispatcher::new(service))
}

/// Builds an in-memory `CommandDispatcher` with a `FakePlaybackBackend` for deterministic testing.
pub fn build_test_dispatcher() -> AppResult<(CommandDispatcher, Arc<FakePlaybackBackend>)> {
    let db = Database::open_in_memory()?;
    let media_repo = Arc::new(SqliteMediaRepository::new(db.clone()));
    let notes_repo = Arc::new(SqliteNotesRepository::new(db.clone()));
    let settings_repo = Arc::new(SqliteSettingsRepository::new(db));
    let backend = Arc::new(FakePlaybackBackend::new());
    let service = AudoraAppService::new(media_repo, notes_repo, settings_repo, backend.clone());
    Ok((CommandDispatcher::new(service), backend))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    use crate::domain::{
        Availability, BackendEvent, CreateNoteInput, LibraryFilter, MediaKind, MediaRepository,
        NotesRepository, PlaybackCommand, PlaybackState, ProbedMediaMetadata, TimestampPrecision,
        UpdateNoteInput, WindowMode,
    };
    use crate::error::ErrorCode;
    use crate::persistence::{MigrationSpec, MIGRATION_0001_INITIAL_SQL};

    fn create_dummy_media_file(dir: &TempDir, name: &str, contents: &[u8]) -> std::path::PathBuf {
        let path = dir.path().join(name);
        fs::write(&path, contents).expect("write dummy media fixture");
        path
    }

    #[test]
    fn migration_runner_creates_pre_migration_backup_and_rolls_back_on_failure() {
        let temp = TempDir::new().expect("tempdir");
        let db_path = temp.path().join("audora.sqlite");

        // 1. Open initial v1 database and insert a media record + note.
        {
            let dispatcher = build_production_dispatcher(&db_path).expect("open v1 db");
            let media_file = create_dummy_media_file(&temp, "lecture.mp4", b"fake-mp4-bytes");
            let item = dispatcher
                .open_media(media_file.to_str().expect("utf8 path"))
                .expect("open media");
            dispatcher
                .create_note(CreateNoteInput {
                    media_id: item.id,
                    position_ms: 42_000,
                    is_quick_marker: false,
                    body: "Important moment that must survive migration failure".to_owned(),
                })
                .expect("create note");
        }

        // 2. Attempt to upgrade with a faulty v2 migration.
        let faulty_migrations = [
            MigrationSpec {
                version: 1,
                name: "0001_initial",
                sql: MIGRATION_0001_INITIAL_SQL,
            },
            MigrationSpec {
                version: 2,
                name: "0002_broken",
                sql: "ALTER TABLE notes ADD COLUMN valid_col TEXT; INVALID SQL SYNTAX HERE;",
            },
        ];

        let result = Database::open_with_migrations(&db_path, &faulty_migrations);
        assert!(result.is_err(), "broken migration must fail");
        let err = result.err().expect("error");
        assert_eq!(err.code(), ErrorCode::PersistenceFailure);

        // 3. Verify that the pre-migration backup file was created before v2 ran.
        let expected_backup = db_path.with_extension("sqlite.pre-v1.bak");
        assert!(
            expected_backup.exists(),
            "pre-migration backup file must exist at {}",
            expected_backup.display()
        );

        // 4. Verify the original database rolled back cleanly to v1 and kept the note!
        let recovered_db = Database::open(&db_path).expect("reopen rolled-back v1 db");
        assert_eq!(recovered_db.schema_version().expect("version"), 1);
        let notes_repo = SqliteNotesRepository::new(recovered_db.clone());
        let media_repo = SqliteMediaRepository::new(recovered_db);
        let items = media_repo
            .list_media(None, LibraryFilter::All)
            .expect("list media");
        assert_eq!(items.len(), 1);
        let notes = notes_repo
            .list_notes_for_media(&items[0].id)
            .expect("list notes");
        assert_eq!(notes.len(), 1);
        assert_eq!(
            notes[0].body,
            "Important moment that must survive migration failure"
        );
    }

    #[test]
    fn open_media_rejects_unsupported_format_with_av_104_and_preserves_source_file() {
        let temp = TempDir::new().expect("tempdir");
        let prores_path =
            create_dummy_media_file(&temp, "Camera_Original.prores", b"original-prores-bytes");
        let (dispatcher, _) = build_test_dispatcher().expect("test dispatcher");

        let err = dispatcher
            .open_media(prores_path.to_str().expect("path"))
            .expect_err("prores must be rejected with AV-104");
        assert_eq!(err.code, ErrorCode::MediaUnsupported);
        assert!(err.message.contains("Camera_Original.prores"));

        // Source media bytes must remain untouched (Product principle #2).
        let bytes_after = fs::read(&prores_path).expect("read source file");
        assert_eq!(bytes_after, b"original-prores-bytes");
    }

    #[test]
    fn playback_suppresses_stale_load_generations_and_tracks_resume_progress() {
        let temp = TempDir::new().expect("tempdir");
        let file_a = create_dummy_media_file(&temp, "first.mp4", b"video-a");
        let file_b = create_dummy_media_file(&temp, "second.wav", b"audio-b");
        let (dispatcher, _) = build_test_dispatcher().expect("dispatcher");

        let item_a = dispatcher
            .open_media(file_a.to_str().expect("path a"))
            .expect("open a");
        let snap_a = dispatcher.get_playback_snapshot().expect("snap a");
        assert_eq!(snap_a.generation, 1);

        // Seek item A to 60_000 ms (out of 180_000 ms = 33%) and pause.
        dispatcher
            .playback_command(PlaybackCommand::Seek {
                position_ms: 60_000,
            })
            .expect("seek a");
        dispatcher
            .playback_command(PlaybackCommand::Pause)
            .expect("pause a");

        // Open item B -> generation increments to 2.
        let item_b = dispatcher
            .open_media(file_b.to_str().expect("path b"))
            .expect("open b");
        let snap_b = dispatcher.get_playback_snapshot().expect("snap b");
        assert_eq!(snap_b.generation, 2);
        assert_eq!(snap_b.media_id.as_deref(), Some(item_b.id.as_str()));

        // Emit a stale event from generation 1 -> must be ignored!
        let after_stale = dispatcher
            .service()
            .apply_backend_event(
                1,
                BackendEvent::PositionChanged {
                    position_ms: 123_456,
                },
            )
            .expect("apply stale event");
        assert_eq!(after_stale.position_ms, 0);
        assert_eq!(after_stale.media_id.as_deref(), Some(item_b.id.as_str()));

        // Re-open item A -> Resume where I left off restores position 60_000 ms!
        let reopened_a = dispatcher
            .open_media(file_a.to_str().expect("path a"))
            .expect("reopen a");
        assert_eq!(reopened_a.id, item_a.id);
        assert_eq!(reopened_a.progress_percent(), 33);
        let snap_reopened = dispatcher.get_playback_snapshot().expect("snap reopened");
        assert_eq!(snap_reopened.position_ms, 60_000);
    }

    #[test]
    fn notes_and_quick_markers_enforce_validation_precision_and_jump_to_moment() {
        let temp = TempDir::new().expect("tempdir");
        let video_path = create_dummy_media_file(&temp, "art_of_listening.mp4", b"video-data");
        let (dispatcher, _) = build_test_dispatcher().expect("dispatcher");

        let media = dispatcher
            .open_media(video_path.to_str().expect("path"))
            .expect("open video");

        // 1. Empty text without `is_quick_marker` must fail with AV-301.
        let invalid = dispatcher.create_note(CreateNoteInput {
            media_id: media.id.clone(),
            position_ms: 222_400,
            is_quick_marker: false,
            body: "   ".to_owned(),
        });
        assert_eq!(
            invalid.expect_err("must reject").code,
            ErrorCode::ValidationFailed
        );

        // 2. Quick marker with empty text succeeds and clamps/rounds to duration (180_000 ms).
        let marker = dispatcher
            .create_note(CreateNoteInput {
                media_id: media.id.clone(),
                position_ms: 14_600, // rounds to nearest second 15_000 ms by default
                is_quick_marker: true,
                body: String::new(),
            })
            .expect("create quick marker");
        assert!(marker.is_quick_marker);
        assert_eq!(marker.position_ms, 15_000);

        // 3. Switch precision to Milliseconds and create an earlier note at 3_425 ms.
        let mut settings = dispatcher.get_settings().expect("get settings");
        settings.timestamp_precision = TimestampPrecision::Milliseconds;
        dispatcher
            .update_settings(settings)
            .expect("update settings");

        let early_note = dispatcher
            .create_note(CreateNoteInput {
                media_id: media.id.clone(),
                position_ms: 3_425,
                is_quick_marker: false,
                body: "Great explanation of room tone.".to_owned(),
            })
            .expect("create text note");
        assert!(!early_note.is_quick_marker);
        assert_eq!(early_note.position_ms, 3_425);

        // 4. Listing notes returns chronological order (3_425 ms before 15_000 ms).
        let list = dispatcher.list_notes(&media.id).expect("list notes");
        assert_eq!(list.len(), 2);
        assert_eq!(list[0].id, early_note.id);
        assert_eq!(list[1].id, marker.id);

        // 5. Editing the quick marker converts it to a regular text note.
        let updated_marker = dispatcher
            .update_note(UpdateNoteInput {
                id: marker.id.clone(),
                body: "Try this approach on the interview sequence.".to_owned(),
            })
            .expect("update marker");
        assert!(!updated_marker.is_quick_marker);

        // 6. Search library & notes offline.
        let search = dispatcher
            .search_library(Some("room tone"), LibraryFilter::Video)
            .expect("search");
        assert_eq!(search.media_items.len(), 1);
        assert_eq!(search.matching_notes.len(), 1);
        assert_eq!(search.matching_notes[0].id, early_note.id);

        // 7. Activate note seeks directly to 3_425 ms.
        let snap = dispatcher
            .activate_note(&early_note.id)
            .expect("activate note");
        assert_eq!(snap.position_ms, 3_425);
    }

    #[test]
    fn relink_media_prevents_silent_merge_and_preserves_notes_after_relocation() {
        let temp = TempDir::new().expect("tempdir");
        let original_path = create_dummy_media_file(&temp, "reel.mp4", b"reel-bytes");
        let other_library_path = create_dummy_media_file(&temp, "other.mp4", b"other-bytes");
        let relocated_path = create_dummy_media_file(&temp, "reel_moved.mp4", b"reel-bytes");

        let (dispatcher, backend) = build_test_dispatcher().expect("dispatcher");
        let reel = dispatcher
            .open_media(original_path.to_str().expect("path"))
            .expect("open reel");
        let _other = dispatcher
            .open_media(other_library_path.to_str().expect("other path"))
            .expect("open other");

        dispatcher
            .create_note(CreateNoteInput {
                media_id: reel.id.clone(),
                position_ms: 12_000,
                is_quick_marker: false,
                body: "Preserved across file move".to_owned(),
            })
            .expect("add note");

        // Simulate file disappearing from original path.
        fs::remove_file(&original_path).expect("remove original");
        let missing_err = dispatcher
            .open_media(original_path.to_str().expect("path"))
            .expect_err("missing file must error");
        assert_eq!(missing_err.code, ErrorCode::MediaUnavailable);

        // Attempting to relink `reel` to `other_library_path` must be blocked with AV-201!
        let conflict = dispatcher
            .relink_media(
                &reel.id,
                other_library_path.to_str().expect("other path"),
                false,
            )
            .expect_err("must block silent merge");
        assert_eq!(conflict.code, ErrorCode::RelinkConflict);

        // Register a different duration on `relocated_path` to verify AV-202 mismatch confirmation.
        backend.register_probe(
            &relocated_path,
            ProbedMediaMetadata {
                media_kind: MediaKind::Video,
                duration_ms: Some(240_000),
                container_format: Some("MP4".to_owned()),
                video_codec: Some("H.264".to_owned()),
                audio_codec: Some("AAC".to_owned()),
            },
        );

        let mismatch = dispatcher
            .relink_media(&reel.id, relocated_path.to_str().expect("relocated"), false)
            .expect_err("must ask for confirmation when duration differs");
        assert_eq!(mismatch.code, ErrorCode::RelinkMismatch);

        // Confirming the relink updates path, marks Available, and preserves all notes!
        let relinked = dispatcher
            .relink_media(&reel.id, relocated_path.to_str().expect("relocated"), true)
            .expect("relink with confirmation");
        assert_eq!(relinked.availability, Availability::Available);
        assert_eq!(relinked.note_count, 1);

        let preserved_notes = dispatcher.list_notes(&reel.id).expect("list notes");
        assert_eq!(preserved_notes.len(), 1);
        assert_eq!(preserved_notes[0].body, "Preserved across file move");
    }

    #[test]
    fn window_mode_and_headphone_disconnect_respect_user_settings() {
        let temp = TempDir::new().expect("tempdir");
        let audio_path = create_dummy_media_file(&temp, "field_notes.wav", b"wav-bytes");
        let (dispatcher, _) = build_test_dispatcher().expect("dispatcher");

        dispatcher
            .open_media(audio_path.to_str().expect("path"))
            .expect("open audio");
        dispatcher
            .playback_command(PlaybackCommand::Play)
            .expect("play");

        let updated = dispatcher
            .set_window_mode(WindowMode::Minimum)
            .expect("switch to minimum mode");
        assert_eq!(updated.window_mode, WindowMode::Minimum);

        let snap_after_disconnect = dispatcher
            .service()
            .handle_audio_route_disconnected()
            .expect("handle headphone disconnect");
        assert_eq!(snap_after_disconnect.state, PlaybackState::Paused);
    }

    #[test]
    fn gstreamer_discoverer_parser_extracts_streams_and_duration() {
        let sample_output = "\
Analyzing file:///tmp/test_video.mp4
Done discovering file:///tmp/test_video.mp4

Properties:
  Duration: 0:00:02.321995465
  Seekable: yes
  Live: no
  container #0: Quicktime
    video #1: H.264 (Main Profile)
    audio #2: Opus
";
        let meta = GStreamerBackend::parse_discoverer_output(
            Path::new("/tmp/test_video.mp4"),
            sample_output,
        )
        .expect("parse discoverer output");
        assert_eq!(meta.media_kind, MediaKind::Video);
        assert_eq!(meta.duration_ms, Some(2_321));
        assert_eq!(meta.container_format.as_deref(), Some("Quicktime"));
        assert_eq!(meta.video_codec.as_deref(), Some("H.264 (Main Profile)"));
        assert_eq!(meta.audio_codec.as_deref(), Some("Opus"));
    }

    #[test]
    fn batch_folder_open_is_non_recursive_and_queue_respects_repeat_and_shuffle() {
        let temp = TempDir::new().expect("tempdir");
        let _a = create_dummy_media_file(&temp, "01_intro.mp4", b"video-1");
        let _b = create_dummy_media_file(&temp, "02_field.wav", b"audio-2");
        let _c = create_dummy_media_file(&temp, "03_outro.mp3", b"audio-3");
        let _ignored = create_dummy_media_file(&temp, "notes.txt", b"ignore-text");

        // Nested subfolder must NOT be scanned recursively (per `docs/SECURITY.md`).
        let nested_dir = temp.path().join("nested");
        fs::create_dir_all(&nested_dir).expect("create nested dir");
        fs::write(nested_dir.join("hidden.mp4"), b"nested-video").expect("write nested");

        let (dispatcher, _) = build_test_dispatcher().expect("dispatcher");
        let folder_str = temp.path().to_str().expect("folder str").to_owned();
        let opened = dispatcher
            .open_media_batch(&[folder_str])
            .expect("open folder batch");
        assert_eq!(opened.len(), 3);
        assert_eq!(opened[0].display_name, "01_intro.mp4");
        assert_eq!(opened[1].display_name, "02_field.wav");
        assert_eq!(opened[2].display_name, "03_outro.mp3");

        let snap = dispatcher.get_playback_snapshot().expect("snap");
        assert_eq!(snap.queue.len(), 3);
        assert_eq!(snap.media_id.as_deref(), Some(opened[0].id.as_str()));

        // Test Shuffle skips to index (0 + 2) % 3 = 2.
        dispatcher
            .playback_command(PlaybackCommand::SetShuffle { shuffle: true })
            .expect("enable shuffle");
        let shuffled_snap = dispatcher
            .playback_command(PlaybackCommand::Next)
            .expect("next with shuffle");
        assert_eq!(
            shuffled_snap.media_id.as_deref(),
            Some(opened[2].id.as_str())
        );

        // Disable shuffle, enable Repeat All, and verify Next wraps from index 2 -> 0.
        dispatcher
            .playback_command(PlaybackCommand::SetShuffle { shuffle: false })
            .expect("disable shuffle");
        dispatcher
            .playback_command(PlaybackCommand::SetRepeat {
                mode: crate::domain::RepeatMode::All,
            })
            .expect("repeat all");
        let wrapped_snap = dispatcher
            .playback_command(PlaybackCommand::Next)
            .expect("next wrap");
        assert_eq!(
            wrapped_snap.media_id.as_deref(),
            Some(opened[0].id.as_str())
        );

        // Remove first item from queue -> active item advances to index 0 of remaining queue (`opened[1]`).
        let after_remove = dispatcher
            .remove_from_queue(&opened[0].id)
            .expect("remove from queue");
        assert_eq!(after_remove.queue.len(), 2);
        assert_eq!(
            after_remove.media_id.as_deref(),
            Some(opened[1].id.as_str())
        );

        // Mark `opened[1]` missing explicitly -> availability becomes Missing while preserving library item.
        let marked = dispatcher
            .mark_media_missing(&opened[1].id)
            .expect("mark missing");
        assert_eq!(marked.availability, Availability::Missing);
    }
}
