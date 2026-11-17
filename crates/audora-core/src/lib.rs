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
    Database, SqliteMediaRepository, SqliteNotesRepository, SqlitePlaylistRepository,
    SqliteSettingsRepository,
};
use services::AudoraAppService;

/// Builds a production `CommandDispatcher` backed by an on-disk SQLite database and `GStreamerBackend`.
pub fn build_production_dispatcher(db_path: impl AsRef<Path>) -> AppResult<CommandDispatcher> {
    let (dispatcher, _) = build_production_dispatcher_with_db(db_path)?;
    Ok(dispatcher)
}

/// Builds a production `CommandDispatcher` and returns its shared `Database` handle for SQLite state/backup operations.
pub fn build_production_dispatcher_with_db(
    db_path: impl AsRef<Path>,
) -> AppResult<(CommandDispatcher, Database)> {
    let path_ref = db_path.as_ref();
    let app_data_dir = path_ref
        .parent()
        .map_or_else(|| std::path::PathBuf::from("."), Path::to_path_buf);
    let db = Database::open_or_recover(path_ref)?;
    let media_repo = Arc::new(SqliteMediaRepository::new(db.clone()));
    let notes_repo = Arc::new(SqliteNotesRepository::new(db.clone()));
    let playlist_repo = Arc::new(SqlitePlaylistRepository::new(db.clone()));
    let settings_repo = Arc::new(SqliteSettingsRepository::new(db.clone()));
    let playback_backend = Arc::new(GStreamerBackend::new());
    let service = AudoraAppService::with_data_dir(
        media_repo,
        notes_repo,
        playlist_repo,
        settings_repo,
        playback_backend,
        app_data_dir,
    )
    .with_database(db.clone());
    Ok((CommandDispatcher::new(service), db))
}

/// Builds an in-memory `CommandDispatcher` with a `FakePlaybackBackend` for deterministic testing.
pub fn build_test_dispatcher() -> AppResult<(CommandDispatcher, Arc<FakePlaybackBackend>)> {
    let db = Database::open_in_memory()?;
    let media_repo = Arc::new(SqliteMediaRepository::new(db.clone()));
    let notes_repo = Arc::new(SqliteNotesRepository::new(db.clone()));
    let playlist_repo = Arc::new(SqlitePlaylistRepository::new(db.clone()));
    let settings_repo = Arc::new(SqliteSettingsRepository::new(db));
    let backend = Arc::new(FakePlaybackBackend::new());
    let service = AudoraAppService::new(
        media_repo,
        notes_repo,
        playlist_repo,
        settings_repo,
        backend.clone(),
    );
    Ok((CommandDispatcher::new(service), backend))
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    use crate::domain::{
        Availability, BackendEvent, CreateNoteInput, CreatePlaylistInput, LibraryFilter, MediaKind,
        MediaRepository, NotesRepository, PlaybackCommand, PlaybackState, ProbedMediaMetadata,
        TimestampPrecision, UpdateNoteInput, UpdatePlaylistInput, WindowMode,
    };
    use crate::error::ErrorCode;
    use crate::persistence::{
        MigrationSpec, MIGRATION_0001_INITIAL_SQL, MIGRATION_0002_PLAYLISTS_SQL,
        MIGRATION_0003_UI_STATE_SQL, MIGRATION_0004_RELATIONAL_UNIFICATION_SQL,
    };

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

        // 2. Attempt to upgrade with a faulty v5 migration on top of v1..v4.
        let faulty_migrations = [
            MigrationSpec {
                version: 1,
                name: "0001_initial",
                sql: MIGRATION_0001_INITIAL_SQL,
            },
            MigrationSpec {
                version: 2,
                name: "0002_playlists",
                sql: MIGRATION_0002_PLAYLISTS_SQL,
            },
            MigrationSpec {
                version: 3,
                name: "0003_ui_state",
                sql: MIGRATION_0003_UI_STATE_SQL,
            },
            MigrationSpec {
                version: 4,
                name: "0004_relational_unification",
                sql: MIGRATION_0004_RELATIONAL_UNIFICATION_SQL,
            },
            MigrationSpec {
                version: 5,
                name: "0005_broken",
                sql: "ALTER TABLE notes ADD COLUMN valid_col TEXT; INVALID SQL SYNTAX HERE;",
            },
        ];

        let result = Database::open_with_migrations(&db_path, &faulty_migrations);
        assert!(result.is_err(), "broken migration must fail");
        let err = result.err().expect("error");
        assert_eq!(err.code(), ErrorCode::PersistenceFailure);

        // 3. Verify that the pre-migration backup file was created before v5 ran.
        let expected_backup = db_path.with_extension("sqlite.pre-v4.bak");
        assert!(
            expected_backup.exists(),
            "pre-migration backup file must exist at {}",
            expected_backup.display()
        );

        // 4. Verify the original database rolled back cleanly to v4 and kept the note!
        let recovered_db = Database::open(&db_path).expect("reopen rolled-back v4 db");
        assert_eq!(recovered_db.schema_version().expect("version"), 4);
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

        // 7. Activate note seeks directly to 3_425 ms and auto-plays.
        let snap = dispatcher
            .activate_note(&early_note.id)
            .expect("activate note");
        assert_eq!(snap.position_ms, 3_425);
        assert_eq!(snap.state, PlaybackState::Playing);
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
                    duration_probed: true,
                    last_position_ms: 0,
                    completed: false,
                    file_size_bytes: Some(1024),
                    fingerprint: None,
                    availability: Availability::Available,
                    added_at: "1".to_owned(),
                    last_opened_at: Some("1".to_owned()),
                    last_seen_at: "1".to_owned(),
                    note_count: 0,
                    source_folder_id: None,
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
            search_elapsed.as_millis() < 500,
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
            list_elapsed.as_millis() < 250,
            "list_notes took {} ms",
            list_elapsed.as_millis()
        );
    }

    #[test]
    fn github_release_auto_update_stages_verifies_dismisses_and_installs_cleanly() {
        let temp = TempDir::new().expect("tempdir");
        let db_path = temp.path().join("audora.sqlite");
        let dispatcher = build_production_dispatcher(&db_path).expect("dispatcher");

        // 1. Initial check reports 1.1.0 with no update available and default GitHub repo.
        let initial = dispatcher.check_for_updates().expect("initial check");
        assert_eq!(initial.current_version, "1.1.0");
        assert_eq!(initial.github_repo, "smokeInCloud060201/audora");
        assert!(!initial.update_available);
        assert!(initial.verified);

        // 2. Stage a GitHub Releases `/releases/latest` JSON payload for v1.2.0.
        let github_json = r#"{
            "tag_name": "v1.2.0",
            "name": "Audora 1.2.0 — Signed & Notarized Release",
            "body": "- Added macOS Developer ID & ad-hoc bundle signing\n- Added native self-updater",
            "published_at": "2026-11-12T10:00:00Z",
            "html_url": "https://github.com/smokeInCloud060201/audora/releases/tag/v1.2.0",
            "assets": [
                {
                    "name": "Audora_1.2.0_macos-aarch64.dmg",
                    "browser_download_url": "https://github.com/smokeInCloud060201/audora/releases/download/v1.2.0/Audora_1.2.0_macos-aarch64.dmg",
                    "size": 18452992,
                    "digest": "sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"
                }
            ]
        }"#;

        let staged = dispatcher
            .stage_github_release_json(github_json)
            .expect("stage github release json");
        assert!(staged.update_available);
        assert_eq!(staged.latest_version, "1.2.0");
        assert!(staged.verified);
        assert_eq!(
            staged.sha256_checksum,
            "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"
        );

        // 3. Dismissing v1.2.0 suppresses `update_available` on automatic startup checks,
        //    while manual checks (`include_dismissed = true`) still surface it.
        let dismissed = dispatcher.dismiss_update("1.2.0").expect("dismiss update");
        assert!(!dismissed.update_available);
        let manual_check = dispatcher
            .check_for_updates_with_options(true)
            .expect("manual check including dismissed");
        assert!(manual_check.update_available);

        // 4. Installing v1.2.0 marks `current_version = 1.2.0` and `staged_for_restart = true`.
        let installed = dispatcher.install_update("1.2.0").expect("install update");
        assert_eq!(installed.current_version, "1.2.0");
        assert!(!installed.update_available);
        assert!(installed.staged_for_restart);

        // 5. Uninstall cleanup removes all staged manifests and SQLite files in app_data_dir.
        let _ = dispatcher
            .stage_github_release_json(github_json)
            .expect("re-stage manifest before cleanup");
        let cleanup = dispatcher
            .cleanup_application_data()
            .expect("cleanup application data");
        assert!(!cleanup.removed_paths.is_empty());
        assert!(cleanup.bytes_reclaimed > 0);
        drop(dispatcher);
        let _ = crate::services::cleanup_all_app_artifacts(temp.path())
            .expect("standalone uninstall cleanup");
        assert!(!db_path.exists());
    }

    #[test]
    fn multi_playlist_crud_reorder_cascade_and_play_skips_missing() {
        let temp = TempDir::new().expect("tempdir");
        let track1_path = create_dummy_media_file(&temp, "track1.mp3", b"audio-1");
        let track2_path = create_dummy_media_file(&temp, "track2.mp4", b"video-2");
        let track3_path = create_dummy_media_file(&temp, "track3.wav", b"audio-3");

        let (dispatcher, _) = build_test_dispatcher().expect("dispatcher");
        let t1 = dispatcher
            .open_media(track1_path.to_str().expect("p1"))
            .expect("t1");
        let t2 = dispatcher
            .open_media(track2_path.to_str().expect("p2"))
            .expect("t2");
        let t3 = dispatcher
            .open_media(track3_path.to_str().expect("p3"))
            .expect("t3");

        // Create multiple playlists; a media item can belong to several playlists
        let study = dispatcher
            .create_playlist(CreatePlaylistInput {
                name: "Deep Study".to_owned(),
                description: "Focus tracks and lectures".to_owned(),
                initial_item_ids: vec![t1.id.clone(), t2.id.clone()],
            })
            .expect("create study playlist");
        assert_eq!(study.item_ids, vec![t1.id.clone(), t2.id.clone()]);

        let commute = dispatcher
            .create_playlist(CreatePlaylistInput {
                name: "Commute Mix".to_owned(),
                description: String::new(),
                initial_item_ids: vec![t2.id.clone()],
            })
            .expect("create commute playlist");
        assert_eq!(dispatcher.list_playlists().expect("list").len(), 2);

        // Append items (ignoring duplicates) and reorder
        let study = dispatcher
            .add_items_to_playlist(&study.id, &[t2.id.clone(), t3.id.clone()])
            .expect("add items");
        assert_eq!(
            study.item_ids,
            vec![t1.id.clone(), t2.id.clone(), t3.id.clone()]
        );

        let reordered = dispatcher
            .move_playlist_item(&study.id, &t3.id, 0)
            .expect("move t3 to front");
        assert_eq!(
            reordered.item_ids,
            vec![t3.id.clone(), t1.id.clone(), t2.id.clone()]
        );

        // Rename playlist
        let renamed = dispatcher
            .update_playlist(UpdatePlaylistInput {
                id: commute.id.clone(),
                name: "Morning Commute".to_owned(),
                description: "Podcasts".to_owned(),
            })
            .expect("rename");
        assert_eq!(renamed.name, "Morning Commute");

        // Mark t3 missing -> play_playlist skips t3 and plays [t1, t2] starting at t1
        dispatcher
            .mark_media_missing(&t3.id)
            .expect("mark t3 missing");
        let snap = dispatcher
            .play_playlist(&study.id, None, true)
            .expect("play playlist");
        assert_eq!(snap.state, PlaybackState::Playing);
        assert_eq!(snap.media_id.as_deref(), Some(t1.id.as_str()));
        assert_eq!(snap.queue, vec![t1.id.clone(), t2.id.clone()]);
        assert!(snap.shuffle);

        // Removing t2 from the library cascades cleanly out of both playlists
        dispatcher.remove_media(&t2.id).expect("remove t2");
        let study_after = dispatcher.get_playlist(&study.id).expect("get study");
        assert_eq!(study_after.item_ids, vec![t3.id.clone(), t1.id.clone()]);
        let commute_after = dispatcher.get_playlist(&commute.id).expect("get commute");
        assert!(commute_after.item_ids.is_empty());

        // Deleting a playlist never deletes the underlying media items in the library
        assert!(dispatcher.delete_playlist(&study.id).expect("delete study"));
        assert_eq!(dispatcher.list_playlists().expect("list").len(), 1);
        assert_eq!(
            dispatcher
                .search_library(None, LibraryFilter::All)
                .expect("search")
                .media_items
                .len(),
            2
        );
    }

    #[test]
    fn sqlite_ui_state_snapshot_persists_backs_up_and_clears_atomically() {
        let temp = TempDir::new().expect("tempdir");
        let db_path = temp.path().join("audora.sqlite");
        let (dispatcher, db) = build_production_dispatcher_with_db(&db_path).expect("open db");

        assert_eq!(db.schema_version().expect("schema v4"), 4);
        assert_eq!(db.get_ui_state_json().expect("empty state"), None);

        let sample = r#"{
            "media": [{
                "id": "med_rel_1",
                "path": "/tmp/rel_track.mp3",
                "display_name": "rel_track.mp3",
                "media_kind": "audio",
                "duration_ms": 90000,
                "duration_probed": true,
                "last_position_ms": 12000,
                "completed": false,
                "file_size_bytes": 4096,
                "fingerprint": null,
                "availability": "available",
                "added_at": "1",
                "last_opened_at": "1",
                "last_seen_at": "1",
                "note_count": 1,
                "source_folder_id": "src_1"
            }],
            "notes": [{
                "id": "note_rel_1",
                "media_id": "med_rel_1",
                "position_ms": 5000,
                "is_quick_marker": false,
                "body": "Stored in normalized relational notes table",
                "created_at": "1",
                "updated_at": "1"
            }],
            "playlists": [{
                "id": "pl_rel_1",
                "name": "Relational Playlist",
                "description": "",
                "item_ids": ["med_rel_1"],
                "created_at": "1",
                "updated_at": "1"
            }],
            "settings": { "theme": "light" }
        }"#;
        db.set_ui_state_json(sample)
            .expect("persist relational state");

        // Verify diagnostic report in Rust sees real relational rows directly (`C-6`)!
        let diag = dispatcher
            .generate_diagnostic_report()
            .expect("diagnostic report");
        assert_eq!(diag.schema_version, 4);
        assert_eq!(diag.media_count, 1);
        assert_eq!(diag.total_note_count, 1);
        assert_eq!(diag.playlist_count, 1);

        // Test O(1) indexed RPC progress update
        db.rpc_update_progress("med_rel_1", 45_000, true, None)
            .expect("rpc update progress");
        let exported = db
            .export_relational_state()
            .expect("export")
            .expect("non-empty");
        assert_eq!(exported.media[0].last_position_ms, 45_000);
        assert!(exported.media[0].completed);
        assert_eq!(exported.media[0].source_folder_id.as_deref(), Some("src_1"));

        let backup_path = db
            .create_manual_backup()
            .expect("create backup")
            .expect("backup path");
        assert!(backup_path.is_file());

        db.clear_ui_state().expect("clear ui state");
        assert_eq!(db.get_ui_state_json().expect("cleared"), None);
    }

    #[test]
    fn pure_rust_container_prober_parses_webm_ebml_mp3_xing_and_ogg_opus() {
        use crate::adapters::probe_container_duration_ms;

        let temp = TempDir::new().expect("tempdir");

        // 1. Synthetic WebM / Matroska EBML header + Segment + Info (TimestampScale=1_000_000, Duration=12500.0f64 -> 12_500ms)
        let mut webm = Vec::new();
        webm.extend_from_slice(&[0x1A, 0x45, 0xDF, 0xA3, 0x84, 0x42, 0x86, 0x81, 0x01]); // EBML Header (4 bytes payload)
        let mut info_payload = Vec::new();
        info_payload.extend_from_slice(&[0x2A, 0xD7, 0xB1, 0x83, 0x0F, 0x42, 0x40]); // TimestampScale = 1_000_000
        info_payload.extend_from_slice(&[0x44, 0x89, 0x88]); // Duration (8-byte float)
        info_payload.extend_from_slice(&(12_500.0_f64).to_be_bytes());
        let mut segment_payload = Vec::new();
        segment_payload.extend_from_slice(&[
            0x15,
            0x49,
            0xA9,
            0x66,
            0x80 | (info_payload.len() as u8),
        ]);
        segment_payload.extend_from_slice(&info_payload);
        webm.extend_from_slice(&[0x18, 0x53, 0x80, 0x67, 0x80 | (segment_payload.len() as u8)]);
        webm.extend_from_slice(&segment_payload);
        webm.resize(64, 0);
        let webm_path = create_dummy_media_file(&temp, "clip.webm", &webm);
        assert_eq!(probe_container_duration_ms(&webm_path), Some(12_500));

        // 2. Synthetic MP3 MPEG-1 Layer III 44.1kHz Stereo frame with Xing VBR header (1000 frames -> 26_122 ms)
        let mut mp3 = vec![0_u8; 256];
        // Frame sync: MPEG-1 (11), Layer III (01), no CRC (1) -> 0xFF 0xFB; 128kbps (1001), 44.1kHz (00) -> 0x90; Stereo (00) -> 0x00
        mp3[0] = 0xFF;
        mp3[1] = 0xFB;
        mp3[2] = 0x90;
        mp3[3] = 0x00;
        // Side info for MPEG-1 stereo = 32 bytes -> Xing at offset 36
        mp3[36..40].copy_from_slice(b"Xing");
        mp3[40..44].copy_from_slice(&1_u32.to_be_bytes()); // frames flag
        mp3[44..48].copy_from_slice(&1_000_u32.to_be_bytes()); // 1000 frames * 1152 / 44100 = 26_122 ms
        let mp3_path = create_dummy_media_file(&temp, "song.mp3", &mp3);
        assert_eq!(probe_container_duration_ms(&mp3_path), Some(26_122));

        // 3. Synthetic Ogg Opus (OpusHead pre_skip = 312, final OggS granule = 480_312 -> 480_000 / 48_000 = 10_000 ms)
        let mut opus = Vec::new();
        // Page 1: OggS header (27 bytes + 1 segment of 19 bytes)
        opus.extend_from_slice(b"OggS\x00\x02");
        opus.extend_from_slice(&0_u64.to_le_bytes());
        opus.extend_from_slice(&1_u32.to_le_bytes());
        opus.extend_from_slice(&0_u32.to_le_bytes());
        opus.extend_from_slice(&0_u32.to_le_bytes());
        opus.push(1);
        opus.push(19);
        opus.extend_from_slice(b"OpusHead\x01\x02");
        opus.extend_from_slice(&312_u16.to_le_bytes());
        opus.extend_from_slice(&48_000_u32.to_le_bytes());
        opus.extend_from_slice(&[0, 0, 0]);
        // Page 2: Final OggS page with granule_position = 480_312
        opus.extend_from_slice(b"OggS\x00\x04");
        opus.extend_from_slice(&480_312_u64.to_le_bytes());
        opus.extend_from_slice(&1_u32.to_le_bytes());
        opus.extend_from_slice(&1_u32.to_le_bytes());
        opus.extend_from_slice(&0_u32.to_le_bytes());
        opus.push(0);
        let opus_path = create_dummy_media_file(&temp, "voice.opus", &opus);
        assert_eq!(probe_container_duration_ms(&opus_path), Some(10_000));
    }

    #[test]
    fn release_verification_rejects_missing_digest_sidecars_and_arbitrary_versions() {
        use crate::services::{compare_semver, parse_github_release_json};
        use std::cmp::Ordering;

        assert_eq!(compare_semver("0.2.0-rc.1", "0.2.0"), Ordering::Less);
        assert_eq!(compare_semver("0.2.0", "0.2.0-rc.1"), Ordering::Greater);
        assert_eq!(compare_semver("0.2.0", "0.1.9"), Ordering::Greater);

        // Missing digest must NOT fabricate a fake SHA-256 (`verified == false`).
        let unverified_json = r#"{
            "tag_name": "v1.2.0",
            "name": "Audora 1.2.0",
            "body": "Unsigned test payload",
            "published_at": "2026-11-16T00:00:00Z",
            "html_url": "https://github.com/smokeInCloud060201/audora/releases/tag/v1.2.0",
            "assets": [
                {
                    "name": "release-manifest-macos-aarch64.json",
                    "browser_download_url": "https://example.com/manifest.json",
                    "size": 512,
                    "digest": "sha256:ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff"
                },
                {
                    "name": "Audora_1.2.0_macos-aarch64.dmg",
                    "browser_download_url": "https://example.com/Audora_1.2.0_macos-aarch64.dmg",
                    "size": 20000000,
                    "digest": null
                }
            ]
        }"#;

        let parsed = parse_github_release_json(unverified_json).expect("parse unverified");
        assert!(
            parsed.download_url.ends_with(".dmg"),
            "must skip .json sidecar and pick installer"
        );
        assert!(
            parsed.sha256.is_empty(),
            "must not bleed sidecar digest or fabricate SHA-256"
        );

        let temp = TempDir::new().expect("tempdir");
        let db_path = temp.path().join("audora.sqlite");
        let dispatcher = build_production_dispatcher(&db_path).expect("dispatcher");

        // Arbitrary unstaged version install must fail.
        assert!(dispatcher.install_update("99.0.0").is_err());

        let status = dispatcher
            .stage_github_release_json(unverified_json)
            .expect("stage unverified");
        assert!(!status.verified);
        assert!(!status.update_available);
        assert!(dispatcher.install_update("1.2.0").is_err());
    }

    #[test]
    fn unicode_search_wildcard_escaping_even_shuffle_and_safe_cleanup() {
        let temp = TempDir::new().expect("tempdir");
        let user_doc = temp.path().join("Important_User_Document.pdf");
        fs::write(&user_doc, b"never-delete-user-files").expect("write user doc");

        let db_path = temp.path().join("audora.sqlite");
        let dispatcher = build_production_dispatcher(&db_path).expect("dispatcher");

        let f1 = create_dummy_media_file(&temp, "01_Über_Konzert.mp3", b"a1");
        let f2 = create_dummy_media_file(&temp, "02_Mix_100%_Final.wav", b"a2");
        let f3 = create_dummy_media_file(&temp, "03_Mix_1000_Draft.wav", b"a3");
        let f4 = create_dummy_media_file(&temp, "04_Outro.flac", b"a4");

        let opened = dispatcher
            .open_media_batch(&[
                f1.to_string_lossy().into_owned(),
                f2.to_string_lossy().into_owned(),
                f3.to_string_lossy().into_owned(),
                f4.to_string_lossy().into_owned(),
            ])
            .expect("open 4 tracks");
        assert_eq!(opened.len(), 4);

        // Non-ASCII uppercase 'Ü' matches lowercase 'über'
        let unicode_res = dispatcher
            .search_library(Some("über"), LibraryFilter::All)
            .expect("unicode search");
        assert_eq!(unicode_res.media_items.len(), 1);
        assert_eq!(unicode_res.media_items[0].id, opened[0].id);

        // Escaped '%' matches literal '100%' and NOT '1000'
        let pct_res = dispatcher
            .search_library(Some("100%"), LibraryFilter::All)
            .expect("escaped wildcard search");
        assert_eq!(pct_res.media_items.len(), 1);
        assert_eq!(pct_res.media_items[0].id, opened[1].id);

        // Even-length queue (4 items) shuffle visits all 4 items without starving odd indices
        dispatcher
            .playback_command(PlaybackCommand::SetShuffle { shuffle: true })
            .expect("enable shuffle");
        let mut visited = std::collections::HashSet::new();
        visited.insert(
            dispatcher
                .get_playback_snapshot()
                .expect("snap")
                .media_id
                .expect("active id"),
        );
        for _ in 0..3 {
            let next_snap = dispatcher
                .playback_command(PlaybackCommand::Next)
                .expect("shuffle next");
            visited.insert(next_snap.media_id.expect("next media id"));
        }
        assert_eq!(
            visited.len(),
            4,
            "even-length shuffle must visit all 4 tracks"
        );

        // Cleanup removes Audora DB files while preserving unrelated user files in the same folder
        let _ = dispatcher
            .cleanup_application_data()
            .expect("cleanup app data");
        assert!(
            user_doc.is_file(),
            "unrelated user files must never be deleted by cleanup"
        );
    }

    #[test]
    fn v3_ui_state_blob_automatically_migrates_into_v4_relational_tables_with_zero_data_loss() {
        let temp = TempDir::new().expect("tempdir");
        let db_path = temp.path().join("audora_legacy_v3.sqlite");

        // 1. Open database at v3 schema (`0001`..`0003`) and write a legacy `app_ui_state` (`v2`) JSON blob.
        let v3_migrations = &crate::persistence::db::BUILTIN_MIGRATIONS[..3];
        let v3_db = Database::open_with_migrations(&db_path, v3_migrations).expect("open v3 db");
        assert_eq!(v3_db.schema_version().expect("v3 schema"), 3);

        let legacy_blob = r#"{
            "media": [{
                "id": "med_legacy_1",
                "path": "/tmp/Legacy_Session.flac",
                "display_name": "Legacy_Session.flac",
                "media_kind": "audio",
                "duration_ms": 240000,
                "last_position_ms": 95000,
                "completed": false,
                "file_size_bytes": 8192,
                "fingerprint": null,
                "availability": "available",
                "added_at": "Yesterday",
                "last_opened_at": "Yesterday",
                "last_seen_at": "Yesterday",
                "note_count": 1,
                "source_folder_id": "src_legacy"
            }],
            "notes": [{
                "id": "note_legacy_1",
                "media_id": "med_legacy_1",
                "position_ms": 95000,
                "is_quick_marker": false,
                "body": "Migrated seamlessly from v3 JSON blob to v4 relational row",
                "created_at": "Yesterday",
                "updated_at": "Yesterday"
            }],
            "playlists": [{
                "id": "pl_legacy_1",
                "name": "Legacy Favorites",
                "description": "Upgraded playlist",
                "item_ids": ["med_legacy_1"],
                "created_at": "Yesterday",
                "updated_at": "Yesterday"
            }],
            "settings": {
                "theme": "dark",
                "default_playback_speed": 1.25,
                "source_folders": [{
                    "id": "src_legacy",
                    "path": "/tmp/Music",
                    "label": "Music",
                    "enabled": true,
                    "added_at": "Yesterday",
                    "last_scanned_at": "Yesterday",
                    "media_count": 1
                }]
            },
            "playback_session": {
                "active_media_id": "med_legacy_1",
                "queue": ["med_legacy_1"],
                "volume": 0.8,
                "muted": false,
                "rate": 1.25,
                "repeat_mode": "all",
                "shuffle": true
            }
        }"#;

        v3_db
            .with_conn(|conn| {
                conn.execute(
                    "INSERT INTO app_ui_state (key, payload_json, updated_at) VALUES ('v2', ?1, '100');",
                    [legacy_blob],
                )?;
                Ok(())
            })
            .expect("insert v3 blob");
        drop(v3_db);

        // 2. Open with production `Database::open` -> upgrades to v4, creates `.pre-v3.bak`, and migrates blob!
        let (dispatcher, v4_db) =
            build_production_dispatcher_with_db(&db_path).expect("upgrade to v4");
        assert_eq!(v4_db.schema_version().expect("v4 schema"), 4);
        assert!(
            db_path.with_extension("sqlite.pre-v3.bak").is_file(),
            "pre-v3 backup must exist"
        );

        // Legacy `app_ui_state` table row must be purged after relational migration.
        let remaining_blobs: i64 = v4_db
            .with_conn(|conn| {
                conn.query_row("SELECT COUNT(*) FROM app_ui_state;", [], |r| r.get(0))
                    .map_err(crate::error::AppError::from)
            })
            .expect("count blobs");
        assert_eq!(remaining_blobs, 0);

        // Rust repositories & diagnostic report must see all migrated relational records!
        let diag = dispatcher.generate_diagnostic_report().expect("diag");
        assert_eq!(diag.schema_version, 4);
        assert_eq!(diag.media_count, 1);
        assert_eq!(diag.total_note_count, 1);
        assert_eq!(diag.playlist_count, 1);

        let exported = v4_db
            .export_relational_state()
            .expect("export")
            .expect("some state");
        assert_eq!(exported.media[0].id, "med_legacy_1");
        assert_eq!(exported.media[0].last_position_ms, 95_000);
        assert_eq!(
            exported.media[0].source_folder_id.as_deref(),
            Some("src_legacy")
        );
        assert_eq!(
            exported.notes[0].body,
            "Migrated seamlessly from v3 JSON blob to v4 relational row"
        );
        assert_eq!(exported.playlists[0].item_ids, vec!["med_legacy_1"]);
        assert_eq!(exported.settings.source_folders.len(), 1);
        let session = exported.playback_session.expect("session");
        assert_eq!(session.active_media_id.as_deref(), Some("med_legacy_1"));
        assert!(session.shuffle);
    }

    #[test]
    fn v3_blob_merges_when_media_table_populated_reconciles_ids_and_clears_folder_id() {
        let temp = TempDir::new().expect("tempdir");
        let db_path = temp.path().join("audora_cli_plus_v3.sqlite");

        // 1. Simulate v3 DB where CLI already inserted `med_cli_1` into `media` AND UI saved a v2 blob
        //    containing another item, a float timestamp, a missing `note_count`, AND a malformed entry!
        let v3_migrations = &crate::persistence::db::BUILTIN_MIGRATIONS[..3];
        let v3_db = Database::open_with_migrations(&db_path, v3_migrations).expect("open v3");
        v3_db
            .with_conn(|conn| {
                conn.execute(
                    "INSERT INTO media (id, path, display_name, media_kind, last_position_ms, completed, availability, added_at, last_seen_at)
                     VALUES ('med_cli_1', '/tmp/shared_talk.mp4', 'shared_talk.mp4', 'video', 0, 0, 'available', '1700000000', '1700000000');",
                    [],
                )?;
                conn.execute(
                    "INSERT INTO notes (id, media_id, position_ms, is_quick_marker, body, created_at, updated_at)
                     VALUES ('note_cli_1', 'med_cli_1', 3000, 0, 'CLI note preserved across ID reconciliation', '1700000000', '1700000000');",
                    [],
                )?;
                let blob = r#"{
                    "media": [
                        {
                            "id": "med_ui_shared",
                            "path": "/tmp/shared_talk.mp4",
                            "display_name": "shared_talk.mp4",
                            "media_kind": "video",
                            "duration_ms": 120000.4,
                            "last_position_ms": 42000.7,
                            "availability": "available",
                            "source_folder_id": "src_keep"
                        },
                        { "malformed": true },
                        {
                            "id": "med_ui_2",
                            "path": "/tmp/second.mp3",
                            "media_kind": "audio",
                            "availability": "available"
                        }
                    ],
                    "notes": [{
                        "id": "note_ui_1",
                        "media_id": "med_ui_shared",
                        "position_ms": 15500.2,
                        "body": "UI note on reconciled ID"
                    }],
                    "playlists": [{
                        "id": "pl_ui_1",
                        "name": "Study Queue",
                        "item_ids": ["med_ui_shared", "med_ui_2"]
                    }]
                }"#;
                conn.execute(
                    "INSERT INTO app_ui_state (key, payload_json, updated_at) VALUES ('v2', ?1, '1');",
                    [blob],
                )?;
                Ok(())
            })
            .expect("seed v3");
        drop(v3_db);

        // 2. Upgrade to v4 -> both CLI note and UI note survive under `med_ui_shared`, `med_ui_2` survives!
        let (_, v4_db) = build_production_dispatcher_with_db(&db_path).expect("upgrade v4");
        let state = v4_db
            .export_relational_state()
            .expect("export")
            .expect("state");
        assert_eq!(state.media.len(), 2);
        assert_eq!(state.notes.len(), 2);
        assert!(state.notes.iter().all(|n| n.media_id == "med_ui_shared"));
        assert_eq!(state.playlists[0].item_ids.len(), 2);

        // 3. Clearing `source_folder_id = None` via `rpc_upsert_media_batch` sets SQLite column to NULL (`H-2`)!
        let mut detached = state
            .media
            .iter()
            .find(|m| m.id == "med_ui_shared")
            .expect("shared")
            .clone();
        assert_eq!(detached.source_folder_id.as_deref(), Some("src_keep"));
        detached.source_folder_id = None;
        v4_db
            .rpc_upsert_media_batch(&[detached])
            .expect("clear folder id");

        let after_clear = v4_db
            .export_relational_state()
            .expect("export after clear")
            .expect("state");
        let reloaded = after_clear
            .media
            .iter()
            .find(|m| m.id == "med_ui_shared")
            .expect("reloaded");
        assert_eq!(reloaded.source_folder_id, None);

        // 4. Domain validation rejects remote URLs and oversized notes in `rpc_*` (`H-6`).
        let mut bad_item = reloaded.clone();
        bad_item.path = "https://evil.example/stream.mp4".to_owned();
        assert!(v4_db.rpc_upsert_media_batch(&[bad_item]).is_err());
    }

    #[test]
    fn prober_handles_aac_adts_mp3_cover_art_ogg_payload_rf64_wav_and_semver_rc10() {
        use crate::adapters::probe_container_duration_ms;
        use crate::services::{compare_semver, parse_github_release_json};
        use std::cmp::Ordering;

        assert_eq!(
            compare_semver("1.0.3-rc.10", "1.0.3-rc.2"),
            Ordering::Greater
        );

        // Higher-priority installer suffix must win on every OS even when fallback appears first (`C-5`).
        let release_json = r#"{
            "tag_name": "v2.0.0",
            "assets": [
                {
                    "name": "Audora_2.0.0_macos-aarch64.dmg",
                    "browser_download_url": "https://example.com/Audora.dmg",
                    "size": 100,
                    "digest": "sha256:1111111111111111111111111111111111111111111111111111111111111111"
                },
                {
                    "name": "Audora_2.0.0_macos-aarch64.app.tar.gz",
                    "browser_download_url": "https://example.com/Audora.app.tar.gz",
                    "size": 90,
                    "digest": "sha256:2222222222222222222222222222222222222222222222222222222222222222"
                },
                {
                    "name": "Audora_2.0.0_windows-x86_64-portable.zip",
                    "browser_download_url": "https://example.com/Audora-portable.zip",
                    "size": 110,
                    "digest": "sha256:3333333333333333333333333333333333333333333333333333333333333333"
                },
                {
                    "name": "Audora_2.0.0_windows-x86_64-setup.exe",
                    "browser_download_url": "https://example.com/Audora-setup.exe",
                    "size": 120,
                    "digest": "sha256:4444444444444444444444444444444444444444444444444444444444444444"
                },
                {
                    "name": "Audora_2.0.0_linux-x86_64.tar.gz",
                    "browser_download_url": "https://example.com/Audora-linux.tar.gz",
                    "size": 130,
                    "digest": "sha256:5555555555555555555555555555555555555555555555555555555555555555"
                },
                {
                    "name": "Audora_2.0.0_linux-x86_64.deb",
                    "browser_download_url": "https://example.com/Audora-linux.deb",
                    "size": 140,
                    "digest": "sha256:6666666666666666666666666666666666666666666666666666666666666666"
                }
            ]
        }"#;
        let manifest = parse_github_release_json(release_json).expect("parse");
        if cfg!(target_os = "macos") {
            assert!(manifest.download_url.ends_with(".app.tar.gz"));
            assert_eq!(
                manifest.sha256,
                "2222222222222222222222222222222222222222222222222222222222222222"
            );
        } else if cfg!(target_os = "windows") {
            assert!(manifest.download_url.ends_with("-setup.exe"));
            assert_eq!(
                manifest.sha256,
                "4444444444444444444444444444444444444444444444444444444444444444"
            );
        } else {
            assert!(manifest.download_url.ends_with(".deb"));
            assert_eq!(
                manifest.sha256,
                "6666666666666666666666666666666666666666666666666666666666666666"
            );
        }

        let temp = TempDir::new().expect("tempdir");

        // 1. AAC ADTS stream (3 frames of 128 bytes at 48kHz -> 3 * 1024 / 48000 = 64 ms)
        let mut aac = Vec::new();
        for _ in 0..3 {
            let mut frame = vec![0_u8; 128];
            frame[0] = 0xFF;
            frame[1] = 0xF1; // MPEG-4, Layer 00, no CRC
            frame[2] = 0x4C; // LC profile, sf_idx = 3 (48000 Hz)
            frame[3] = 0x80; // stereo, frame_len = 128 -> bits: 128 << 5 = 0x1000 -> byte4 = 0x10, byte5 = 0x00
            frame[4] = 0x10;
            frame[5] = 0x00;
            frame[6] = 0xFC; // 1 raw data block (0)
            aac.extend_from_slice(&frame);
        }
        let aac_path = create_dummy_media_file(&temp, "clip.aac", &aac);
        assert_eq!(probe_container_duration_ms(&aac_path), Some(64));

        // 2. MP3 with embedded JPEG APP2 false sync `0xFF 0xE2 0x12 0x00` followed by 2 valid CBR 128kbps 44.1kHz frames (417 bytes each)
        let mut mp3 = vec![0_u8; 16 + 417 * 2];
        mp3[0..4].copy_from_slice(&[0xFF, 0xE2, 0x12, 0x00]); // false sync must be skipped!
        mp3[16..20].copy_from_slice(&[0xFF, 0xFB, 0x90, 0x00]); // valid frame 1 (frame_size = 417)
        mp3[16 + 417..16 + 421].copy_from_slice(&[0xFF, 0xFB, 0x90, 0x00]); // valid frame 2
        let mp3_path = create_dummy_media_file(&temp, "cover_sync.mp3", &mp3);
        // 834 audio bytes at 128 kbps -> (834 * 8) / 128 = 52 ms (NOT 2+ hours from 8kbps false sync!)
        assert_eq!(probe_container_duration_ms(&mp3_path), Some(52));

        // 3. Ogg Opus with fake `"OggS\x00"` bytes inside compressed payload must still read real page header
        let mut opus = Vec::new();
        opus.extend_from_slice(b"OggS\x00\x02");
        opus.extend_from_slice(&0_u64.to_le_bytes());
        opus.extend_from_slice(&42_u32.to_le_bytes()); // serial = 42
        opus.extend_from_slice(&0_u32.to_le_bytes());
        opus.extend_from_slice(&0_u32.to_le_bytes());
        opus.push(1);
        opus.push(19);
        opus.extend_from_slice(b"OpusHead\x01\x02");
        opus.extend_from_slice(&0_u16.to_le_bytes());
        opus.extend_from_slice(&48_000_u32.to_le_bytes());
        opus.extend_from_slice(&[0, 0, 0]);
        // Page 2 with serial 42, granule = 240_000 (5_000 ms), and fake `OggS\x00` inside its 32-byte payload!
        opus.extend_from_slice(b"OggS\x00\x04");
        opus.extend_from_slice(&240_000_u64.to_le_bytes());
        opus.extend_from_slice(&42_u32.to_le_bytes());
        opus.extend_from_slice(&1_u32.to_le_bytes());
        opus.extend_from_slice(&0_u32.to_le_bytes());
        opus.push(1);
        opus.push(32);
        let mut fake_payload = [0_u8; 32];
        fake_payload[0..5].copy_from_slice(b"OggS\x00");
        fake_payload[6..14].copy_from_slice(&999_999_999_u64.to_le_bytes());
        fake_payload[14..18].copy_from_slice(&42_u32.to_le_bytes());
        opus.extend_from_slice(&fake_payload);
        let opus_path = create_dummy_media_file(&temp, "tricky.opus", &opus);
        assert_eq!(probe_container_duration_ms(&opus_path), Some(5_000));
    }
}
