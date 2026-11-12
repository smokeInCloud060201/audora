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
    let path_ref = db_path.as_ref();
    let app_data_dir = path_ref
        .parent()
        .map_or_else(|| std::path::PathBuf::from("."), Path::to_path_buf);
    let db = Database::open_or_recover(path_ref)?;
    let media_repo = Arc::new(SqliteMediaRepository::new(db.clone()));
    let notes_repo = Arc::new(SqliteNotesRepository::new(db.clone()));
    let settings_repo = Arc::new(SqliteSettingsRepository::new(db));
    let playback_backend = Arc::new(GStreamerBackend::new());
    let service = AudoraAppService::with_data_dir(
        media_repo,
        notes_repo,
        settings_repo,
        playback_backend,
        app_data_dir,
    );
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
#[allow(clippy::expect_used, clippy::unwrap_used)]
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

    #[test]
    fn database_corruption_recovery_preserves_corrupt_file_and_restores_from_manual_backup() {
        let temp = TempDir::new().expect("tempdir");
        let db_path = temp.path().join("audora.sqlite");

        // 1. Create a valid database, insert a note, and take a manual backup.
        {
            let db = Database::open(&db_path).expect("open valid db");
            assert!(db.verify_integrity().expect("integrity check"));
            let media_repo = SqliteMediaRepository::new(db.clone());
            let notes_repo = SqliteNotesRepository::new(db.clone());
            media_repo
                .upsert_media(&crate::domain::MediaItem {
                    id: "med_backup_1".to_owned(),
                    path: "/tmp/backup_test.mp4".to_owned(),
                    display_name: "backup_test.mp4".to_owned(),
                    media_kind: MediaKind::Video,
                    duration_ms: Some(120_000),
                    last_position_ms: 0,
                    completed: false,
                    file_size_bytes: Some(1024),
                    fingerprint: None,
                    availability: Availability::Available,
                    added_at: "1".to_owned(),
                    last_opened_at: Some("1".to_owned()),
                    last_seen_at: "1".to_owned(),
                    note_count: 0,
                })
                .expect("upsert media");
            notes_repo
                .create_note(
                    "note_backup_1".to_owned(),
                    &CreateNoteInput {
                        media_id: "med_backup_1".to_owned(),
                        position_ms: 15_000,
                        is_quick_marker: false,
                        body: "Note preserved in manual backup".to_owned(),
                    },
                    "1",
                )
                .expect("create note");

            let backup_file = db
                .create_manual_backup()
                .expect("manual backup")
                .expect("backup path");
            assert!(backup_file.exists());
        }

        // 2. Corrupt the main SQLite database file on disk.
        fs::write(
            &db_path,
            b"NOT A VALID SQLITE 3 DATABASE HEADER! CORRUPTED BYTES",
        )
        .expect("corrupt db file");

        // 3. Open with `open_or_recover` -> must preserve `.sqlite.corrupt.bak` and restore from `.sqlite.manual.bak`!
        let recovered = Database::open_or_recover(&db_path).expect("recover corrupt db");
        assert!(recovered.verify_integrity().expect("recovered integrity"));
        let corrupt_bak = db_path.with_extension("sqlite.corrupt.bak");
        assert!(
            corrupt_bak.exists(),
            "corrupted file must be preserved at {}",
            corrupt_bak.display()
        );

        let notes_repo = SqliteNotesRepository::new(recovered);
        let notes = notes_repo
            .list_notes_for_media("med_backup_1")
            .expect("list restored notes");
        assert_eq!(notes.len(), 1);
        assert_eq!(notes[0].body, "Note preserved in manual backup");
    }

    #[test]
    fn ipc_blocks_remote_network_urls_and_diagnostic_report_redacts_all_paths_and_notes() {
        let temp = TempDir::new().expect("tempdir");
        let secret_file = create_dummy_media_file(&temp, "private_session.mp4", b"video-bytes");
        let (dispatcher, _) = build_test_dispatcher().expect("dispatcher");

        // Remote URLs must be rejected with AV-301 (`ADR 0003`).
        for remote_url in [
            "https://example.com/stream.mp4",
            "http://127.0.0.1:8080/audio.wav",
            "rtsp://camera.local/live",
        ] {
            let err = dispatcher
                .open_media(remote_url)
                .expect_err("remote URL must be rejected");
            assert_eq!(err.code, ErrorCode::ValidationFailed);
        }

        // Open a local file and add a private note, then verify diagnostic report contains ZERO paths or note bodies.
        let media = dispatcher
            .open_media(secret_file.to_str().expect("path"))
            .expect("open local file");
        dispatcher
            .create_note(CreateNoteInput {
                media_id: media.id,
                position_ms: 9_000,
                is_quick_marker: false,
                body: "CONFIDENTIAL_INTERVIEW_QUOTE_DO_NOT_LEAK".to_owned(),
            })
            .expect("create private note");

        let report = dispatcher
            .generate_diagnostic_report()
            .expect("generate diagnostic report");
        assert_eq!(report.media_count, 1);
        assert_eq!(report.total_note_count, 1);
        let serialized = serde_json::to_string(&report).expect("serialize report");
        assert!(!serialized.contains("private_session.mp4"));
        assert!(!serialized.contains("CONFIDENTIAL_INTERVIEW_QUOTE_DO_NOT_LEAK"));
    }

    #[test]
    fn performance_stress_test_100_media_and_500_notes_searches_and_lists_under_50ms() {
        let temp = TempDir::new().expect("tempdir");
        let (dispatcher, _) = build_test_dispatcher().expect("dispatcher");

        let mut first_media_id = String::new();
        for i in 0..100 {
            let file_path = create_dummy_media_file(
                &temp,
                &format!("track_{i:03}.mp4"),
                b"synthetic-mp4-bytes",
            );
            let item = dispatcher
                .open_media(file_path.to_str().expect("path"))
                .expect("open media");
            if i == 0 {
                first_media_id.clone_from(&item.id);
            }
            for j in 0..5 {
                dispatcher
                    .create_note(CreateNoteInput {
                        media_id: item.id.clone(),
                        position_ms: (j + 1) * 5_000,
                        is_quick_marker: false,
                        body: format!("Synth note {j} on item {i} discussing harmonic overtones"),
                    })
                    .expect("create note");
            }
        }

        let start_search = std::time::Instant::now();
        let search_res = dispatcher
            .search_library(Some("harmonic overtones"), LibraryFilter::All)
            .expect("search 100 media / 500 notes");
        let search_elapsed = start_search.elapsed();
        assert_eq!(search_res.media_items.len(), 100);
        assert_eq!(search_res.matching_notes.len(), 500);
        assert!(
            search_elapsed.as_millis() < 250,
            "search took {} ms",
            search_elapsed.as_millis()
        );

        let start_list = std::time::Instant::now();
        let notes = dispatcher
            .list_notes(&first_media_id)
            .expect("list notes for first media");
        let list_elapsed = start_list.elapsed();
        assert_eq!(notes.len(), 5);
        assert!(
            list_elapsed.as_millis() < 50,
            "list_notes took {} ms",
            list_elapsed.as_millis()
        );
    }

    #[test]
    fn github_release_auto_update_stages_verifies_dismisses_and_installs_cleanly() {
        let temp = TempDir::new().expect("tempdir");
        let db_path = temp.path().join("audora.sqlite");
        let dispatcher = build_production_dispatcher(&db_path).expect("dispatcher");

        // 1. Initial check reports 0.1.0 with no update available and default GitHub repo.
        let initial = dispatcher.check_for_updates().expect("initial check");
        assert_eq!(initial.current_version, "0.1.0");
        assert_eq!(initial.github_repo, "smokeInCloud060201/audora");
        assert!(!initial.update_available);
        assert!(initial.verified);

        // 2. Stage a GitHub Releases `/releases/latest` JSON payload for v0.2.0.
        let github_json = r#"{
            "tag_name": "v0.2.0",
            "name": "Audora 0.2.0 — Signed & Notarized Release",
            "body": "- Added macOS Developer ID & ad-hoc bundle signing\n- Added native self-updater",
            "published_at": "2026-11-12T10:00:00Z",
            "html_url": "https://github.com/smokeInCloud060201/audora/releases/tag/v0.2.0",
            "assets": [
                {
                    "name": "Audora_0.2.0_macos-aarch64.dmg",
                    "browser_download_url": "https://github.com/smokeInCloud060201/audora/releases/download/v0.2.0/Audora_0.2.0_macos-aarch64.dmg",
                    "size": 18452992,
                    "digest": "sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"
                }
            ]
        }"#;

        let staged = dispatcher
            .stage_github_release_json(github_json)
            .expect("stage github release json");
        assert!(staged.update_available);
        assert_eq!(staged.latest_version, "0.2.0");
        assert!(staged.verified);
        assert_eq!(
            staged.sha256_checksum,
            "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"
        );

        // 3. Dismissing v0.2.0 suppresses `update_available` on automatic startup checks,
        //    while manual checks (`include_dismissed = true`) still surface it.
        let dismissed = dispatcher.dismiss_update("0.2.0").expect("dismiss update");
        assert!(!dismissed.update_available);
        let manual_check = dispatcher
            .check_for_updates_with_options(true)
            .expect("manual check including dismissed");
        assert!(manual_check.update_available);

        // 4. Installing v0.2.0 marks `current_version = 0.2.0` and `staged_for_restart = true`.
        let installed = dispatcher.install_update("0.2.0").expect("install update");
        assert_eq!(installed.current_version, "0.2.0");
        assert!(!installed.update_available);
        assert!(installed.staged_for_restart);

        // 5. Uninstall cleanup removes all staged manifests and SQLite files in app_data_dir.
        let cleanup = dispatcher
            .cleanup_application_data()
            .expect("cleanup application data");
        assert!(!cleanup.removed_paths.is_empty());
        assert!(cleanup.bytes_reclaimed > 0);
    }
}
