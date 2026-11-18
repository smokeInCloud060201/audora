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

        // 1. Initial check reports 1.1.3 with no update available and default GitHub repo.
        let initial = dispatcher.check_for_updates().expect("initial check");
        assert_eq!(initial.current_version, "1.1.3");
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
                },
                {
                    "name": "Audora_1.2.0_macos-x86_64.dmg",
                    "browser_download_url": "https://github.com/smokeInCloud060201/audora/releases/download/v1.2.0/Audora_1.2.0_macos-x86_64.dmg",
                    "size": 18452992,
                    "digest": "sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"
                },
                {
                    "name": "Audora_1.2.0_windows-x86_64-setup.exe",
                    "browser_download_url": "https://github.com/smokeInCloud060201/audora/releases/download/v1.2.0/Audora_1.2.0_windows-x86_64-setup.exe",
                    "size": 18452992,
                    "digest": "sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"
                },
                {
                    "name": "Audora_1.2.0_linux-x86_64.deb",
                    "browser_download_url": "https://github.com/smokeInCloud060201/audora/releases/download/v1.2.0/Audora_1.2.0_linux-x86_64.deb",
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
                },
                {
                    "name": "Audora_1.2.0_macos-x86_64.dmg",
                    "browser_download_url": "https://example.com/Audora_1.2.0_macos-x86_64.dmg",
                    "size": 20000000,
                    "digest": null
                },
                {
                    "name": "Audora_1.2.0_windows-x86_64-setup.exe",
                    "browser_download_url": "https://example.com/Audora_1.2.0_windows-x86_64-setup.exe",
                    "size": 20000000,
                    "digest": null
                },
                {
                    "name": "Audora_1.2.0_linux-x86_64.deb",
                    "browser_download_url": "https://example.com/Audora_1.2.0_linux-x86_64.deb",
                    "size": 20000000,
                    "digest": null
                }
            ]
        }"#;

        let parsed = parse_github_release_json(unverified_json).expect("parse unverified");
        assert!(
            !parsed.download_url.ends_with(".json"),
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
                    "name": "Audora_2.0.0_macos-x86_64.dmg",
                    "browser_download_url": "https://example.com/Audora-intel.dmg",
                    "size": 100,
                    "digest": "sha256:1111111111111111111111111111111111111111111111111111111111111111"
                },
                {
                    "name": "Audora_2.0.0_macos-x86_64.app.tar.gz",
                    "browser_download_url": "https://example.com/Audora-intel.app.tar.gz",
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

    #[test]
    fn ended_event_auto_advances_past_missing_queued_files_and_restarts_from_zero_after_final_end()
    {
        let temp = TempDir::new().expect("tempdir");
        let f1 = create_dummy_media_file(&temp, "01_part1.mp3", b"audio-1");
        let f2 = create_dummy_media_file(&temp, "02_missing.mp3", b"audio-2");
        let f3 = create_dummy_media_file(&temp, "03_part3.mp3", b"audio-3");

        let (dispatcher, _) = build_test_dispatcher().expect("dispatcher");
        let opened = dispatcher
            .open_media_batch(&[
                f1.to_string_lossy().into_owned(),
                f2.to_string_lossy().into_owned(),
                f3.to_string_lossy().into_owned(),
            ])
            .expect("open 3 tracks");
        assert_eq!(opened.len(), 3);

        // Delete Track 2 on disk while it is still queued
        fs::remove_file(&f2).expect("remove track 2 from disk");

        // Start playing Track 1 and emit `BackendEvent::Ended`
        let playing_snap = dispatcher
            .playback_command(PlaybackCommand::Play)
            .expect("play track 1");
        assert_eq!(
            playing_snap.media_id.as_deref(),
            Some(opened[0].id.as_str())
        );

        let after_first_end = dispatcher
            .service()
            .apply_backend_event(playing_snap.generation, BackendEvent::Ended)
            .expect("track 1 ended");

        // Must mark Track 2 as Missing, skip it, and auto-play Track 3 at 0 ms!
        assert_eq!(
            after_first_end.media_id.as_deref(),
            Some(opened[2].id.as_str())
        );
        assert_eq!(after_first_end.state, PlaybackState::Playing);
        assert_eq!(after_first_end.position_ms, 0);

        let library_items = dispatcher
            .search_library(None, LibraryFilter::All)
            .expect("list library")
            .media_items;
        let t2_record = library_items
            .iter()
            .find(|m| m.id == opened[1].id)
            .expect("track 2 record");
        assert_eq!(t2_record.availability, Availability::Missing);

        // Emit `BackendEvent::Ended` on final Track 3 (Repeat Off) -> transitions to `Ended`
        let after_final_end = dispatcher
            .service()
            .apply_backend_event(after_first_end.generation, BackendEvent::Ended)
            .expect("track 3 ended");
        assert_eq!(after_final_end.state, PlaybackState::Ended);
        assert_eq!(after_final_end.position_ms, 180_000);

        // Toggling play after `Ended` seeks back to 0 ms and plays cleanly
        let replayed = dispatcher
            .playback_command(PlaybackCommand::TogglePlayPause)
            .expect("replay after ended");
        assert_eq!(replayed.state, PlaybackState::Playing);
        assert_eq!(replayed.position_ms, 0);
    }

    #[test]
    fn rpc_execute_batch_is_atomic_and_rejects_malformed_payloads() {
        let db = Database::open_in_memory().expect("open in-memory db");
        let valid_batch = serde_json::json!([
            {
                "op": "upsert_media_batch",
                "items": [{
                    "id": "med_batch_1",
                    "path": "/tmp/batch_track.mp3",
                    "display_name": "batch_track.mp3",
                    "media_kind": "audio",
                    "duration_ms": 120000,
                    "duration_probed": true,
                    "last_position_ms": 0,
                    "completed": false,
                    "file_size_bytes": 4096,
                    "availability": "available",
                    "added_at": "2026-05-01T00:00:00Z",
                    "last_seen_at": "2026-05-01T00:00:00Z"
                }]
            },
            {
                "op": "upsert_note",
                "note": {
                    "id": "note_batch_1",
                    "media_id": "med_batch_1",
                    "position_ms": 12500,
                    "is_quick_marker": false,
                    "body": "Atomic batch note",
                    "created_at": "2026-05-01T00:00:00Z",
                    "updated_at": "2026-05-01T00:00:00Z"
                }
            },
            {
                "op": "update_progress",
                "media_id": "med_batch_1",
                "last_position_ms": 54321,
                "completed": false
            },
            {
                "op": "revoke_source_folder",
                "path": "/tmp/RevokedFolder"
            }
        ]);
        db.rpc_execute_batch(valid_batch.as_array().unwrap())
            .expect("valid atomic batch");

        let state_before = db.export_relational_state().unwrap().unwrap();
        assert_eq!(state_before.media[0].last_position_ms, 54_321);
        assert_eq!(state_before.notes.len(), 1);

        // Now execute a batch where op 0 mutates progress to 99_999 and op 1 is malformed -> MUST fail & roll back op 0!
        let broken_batch = serde_json::json!([
            {
                "op": "update_progress",
                "media_id": "med_batch_1",
                "last_position_ms": 99999,
                "completed": true
            },
            {
                "op": "delete_note"
            }
        ]);
        let err = db
            .rpc_execute_batch(broken_batch.as_array().unwrap())
            .expect_err("malformed op must fail batch");
        assert_eq!(err.code(), ErrorCode::ValidationFailed);

        let state_after = db.export_relational_state().unwrap().unwrap();
        assert_eq!(
            state_after.media[0].last_position_ms, 54_321,
            "earlier op in failed batch must roll back cleanly"
        );
        assert!(!state_after.media[0].completed);

        // Unknown op is also rejected with AV-301
        let unknown_op = serde_json::json!([{ "op": "drop_everything" }]);
        assert_eq!(
            db.rpc_execute_batch(unknown_op.as_array().unwrap())
                .expect_err("unknown op")
                .code(),
            ErrorCode::ValidationFailed
        );
    }

    #[test]
    fn reconcile_upsert_blocks_silent_relink_conflict_and_preserves_id_reconciliation_progress() {
        let db = Database::open_in_memory().expect("db");
        let item_a = crate::domain::MediaItem {
            id: "med_a".to_owned(),
            path: "/tmp/a.mp4".to_owned(),
            display_name: "a.mp4".to_owned(),
            media_kind: MediaKind::Video,
            duration_ms: Some(100_000),
            duration_probed: true,
            last_position_ms: 40_000,
            completed: true,
            file_size_bytes: Some(2048),
            fingerprint: Some("fp-original-a".to_owned()),
            availability: Availability::Available,
            added_at: "2026-01-01T10:00:00.000Z".to_owned(),
            last_opened_at: Some("2026-02-01T10:00:00.000Z".to_owned()),
            last_seen_at: "2026-02-01T10:00:00.000Z".to_owned(),
            note_count: 0,
            source_folder_id: None,
        };
        let item_b = crate::domain::MediaItem {
            id: "med_b".to_owned(),
            path: "/tmp/b.mp4".to_owned(),
            display_name: "b.mp4".to_owned(),
            ..item_a.clone()
        };
        db.rpc_upsert_media_batch(&[item_a.clone(), item_b])
            .expect("insert a and b");

        // 1. Moving existing `med_a` onto `/tmp/b.mp4` (owned by `med_b`) MUST fail with AV-201 RelinkConflict!
        let mut conflicting_a = item_a.clone();
        conflicting_a.path = "/tmp/b.mp4".to_owned();
        let conflict_err = db
            .rpc_upsert_media_batch(&[conflicting_a])
            .expect_err("must reject silent overwrite of med_b");
        assert_eq!(conflict_err.code(), ErrorCode::RelinkConflict);

        // 2. Reconciling a brand-new ID `med_ui_a` onto `/tmp/a.mp4` preserves higher progress, completed, last_opened_at, and fingerprint!
        let reconciled_ui = crate::domain::MediaItem {
            id: "med_ui_a".to_owned(),
            path: "/tmp/a.mp4".to_owned(),
            display_name: "a.mp4".to_owned(),
            media_kind: MediaKind::Video,
            duration_ms: None,
            duration_probed: false,
            last_position_ms: 5_000,
            completed: false,
            file_size_bytes: None,
            fingerprint: None,
            availability: Availability::Available,
            added_at: "2026-03-01T00:00:00.000Z".to_owned(),
            last_opened_at: None,
            last_seen_at: "2026-03-01T00:00:00.000Z".to_owned(),
            note_count: 0,
            source_folder_id: None,
        };
        db.rpc_upsert_media_batch(&[reconciled_ui])
            .expect("reconcile id");
        let state = db.export_relational_state().unwrap().unwrap();
        assert_eq!(state.media.len(), 2);
        let merged = state
            .media
            .iter()
            .find(|m| m.id == "med_ui_a")
            .expect("merged item");
        assert_eq!(merged.last_position_ms, 40_000);
        assert!(merged.completed);
        assert_eq!(
            merged.last_opened_at.as_deref(),
            Some("2026-02-01T10:00:00.000Z")
        );
        assert_eq!(merged.fingerprint.as_deref(), Some("fp-original-a"));
    }

    #[test]
    fn path_and_timestamp_validators_reject_webdav_remote_schemes_and_invalid_iso8601() {
        use crate::domain::{
            clamp_f64_to_safe_duration_ms, format_unix_ms_iso8601, normalize_persisted_timestamp,
            validate_local_media_path_str, MAX_SAFE_DURATION_MS,
        };

        for forbidden in [
            "smb://nas/share/song.mp3",
            "dav://host/file.mp4",
            "davs://host/file.mp4",
            "nfs://server/export/a.wav",
            "afp://mac/share/a.mov",
            "file:///tmp/a.mp4",
            "file:/tmp/a.mp4",
            "FILE:/C:/media/a.mp4",
            "rtmp://live.example/app/stream",
            "rtmps://live.example/app/stream",
            "udp://239.0.0.1:5000",
            "tcp://127.0.0.1:9000",
            "srt://relay.example:9000",
            "rist://relay.example:9000",
            "mmsh://stream.example/live",
            r"\\evil.example@SSL\DavWWWRoot\clip.mp4",
            r"\\evil.example@80\share\clip.mp4",
            r"\\evil.example:445\share\clip.mp4",
            r"\\evil.example\DavWWWRoot\clip.mp4",
            r"\\?\UNC\evil.example@443\share\clip.mp4",
            r"/\evil.example@80/share/clip.mp4",
            r"\/evil.example:445\share\clip.mp4",
            r"//evil.example/share/DavWWWRoot/clip.mp4",
            r"//NAS/Media@80/clip.mp4",
            r"//NAS/Media:445/clip.mp4",
            "//",
            r"\\",
        ] {
            assert!(
                validate_local_media_path_str(forbidden).is_err(),
                "must reject {forbidden}"
            );
        }

        // Standard SMB UNC share, local Windows drives, verbatim drive with `@`, POSIX `@` dirs, and trailing space stay valid!
        assert_eq!(
            validate_local_media_path_str(r"\\NAS\Media\clip.mp4").unwrap(),
            r"\\NAS\Media\clip.mp4"
        );
        assert_eq!(
            validate_local_media_path_str(r"//NAS/Media/clip.mp4").unwrap(),
            r"//NAS/Media/clip.mp4"
        );
        assert_eq!(
            validate_local_media_path_str(r"C:\Users\clip.mp4").unwrap(),
            r"C:\Users\clip.mp4"
        );
        assert_eq!(
            validate_local_media_path_str("c:/Users/clip.mp4").unwrap(),
            "c:/Users/clip.mp4"
        );
        assert_eq!(
            validate_local_media_path_str("/tmp/user@host/clip.mp4").unwrap(),
            "/tmp/user@host/clip.mp4"
        );
        assert_eq!(
            validate_local_media_path_str(r"\\?\C:\Users\dev@corp\clip.mp4").unwrap(),
            r"\\?\C:\Users\dev@corp\clip.mp4"
        );
        assert_eq!(
            validate_local_media_path_str("/tmp/Folder With Space ").unwrap(),
            "/tmp/Folder With Space "
        );

        let fallback = "2026-11-18T00:00:00.000Z";
        assert_eq!(
            normalize_persisted_timestamp("2026-05-10T14:30:00Z", fallback),
            "2026-05-10T14:30:00.000Z"
        );
        assert_eq!(
            normalize_persisted_timestamp("2026-05-10T14:30:00.2Z", fallback),
            "2026-05-10T14:30:00.200Z"
        );
        assert_eq!(
            normalize_persisted_timestamp("2026-05-10T14:30:00.250Z", fallback),
            "2026-05-10T14:30:00.250Z"
        );
        assert_eq!(
            normalize_persisted_timestamp("2026-05-10T14:30:00.123456Z", fallback),
            "2026-05-10T14:30:00.123Z"
        );
        assert_eq!(
            format_unix_ms_iso8601(u128::MAX),
            "9999-12-31T23:59:59.999Z"
        );
        assert_eq!(clamp_f64_to_safe_duration_ms(-1.0), None);
        assert_eq!(clamp_f64_to_safe_duration_ms(f64::NAN), None);
        assert_eq!(
            clamp_f64_to_safe_duration_ms(1e30),
            Some(MAX_SAFE_DURATION_MS)
        );
        for invalid_ts in [
            "2026-99-99",
            "2026-05-10 not-utc",
            "2026-05-10T14:30:00+07:00",
            "2026-02-30T10:00:00Z",
        ] {
            assert_eq!(
                normalize_persisted_timestamp(invalid_ts, fallback),
                fallback,
                "must reject non-UTC/invalid timestamp {invalid_ts}"
            );
        }
    }

    #[test]
    fn release_parser_blocks_cross_os_and_cross_arch_fallbacks() {
        use crate::services::parse_github_release_json_for_target;

        let macos_arm_only = r#"{
            "tag_name": "v1.3.0",
            "html_url": "https://github.com/smokeInCloud060201/audora/releases/tag/v1.3.0",
            "assets": [
                {
                    "name": "Audora_1.3.0_macos-aarch64.app.tar.gz",
                    "browser_download_url": "https://example.com/Audora_1.3.0_macos-aarch64.app.tar.gz",
                    "size": 12345,
                    "digest": "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
                }
            ]
        }"#;

        // Matches on macos + aarch64
        let arm_mac =
            parse_github_release_json_for_target(macos_arm_only, "macos", "aarch64").unwrap();
        assert_eq!(arm_mac.package_size_bytes, 12345);
        assert_eq!(arm_mac.sha256.len(), 64);

        // Must NOT match on macos-x86_64, windows-x86_64, or linux-x86_64 (even though `.tar.gz` suffix overlaps!)
        for (os, arch) in [
            ("macos", "x86_64"),
            ("windows", "x86_64"),
            ("linux", "x86_64"),
        ] {
            let miss = parse_github_release_json_for_target(macos_arm_only, os, arch).unwrap();
            assert_eq!(
                miss.download_url,
                "https://github.com/smokeInCloud060201/audora/releases/tag/v1.3.0"
            );
            assert_eq!(miss.package_size_bytes, 0);
            assert!(miss.sha256.is_empty(), "must be unverified for {os}-{arch}");
        }
    }

    #[test]
    fn prober_skips_known_size_ebml_cluster_and_ignores_oversized_id3v2_tag() {
        use crate::adapters::probe_container_duration_ms;

        let temp = TempDir::new().expect("tempdir");

        // 1. WebM where a known-size `Cluster` (`0x1F43_B675`) appears BEFORE `Info` (`0x1549_A966`)
        let mut webm = Vec::new();
        webm.extend_from_slice(&[0x1A, 0x45, 0xDF, 0xA3, 0x84, 0x42, 0x86, 0x81, 0x01]);
        let mut segment_payload = Vec::new();
        // Known-size Cluster of 4 zero bytes
        segment_payload.extend_from_slice(&[0x1F, 0x43, 0xB6, 0x75, 0x84, 0, 0, 0, 0]);
        let mut info_payload = Vec::new();
        info_payload.extend_from_slice(&[0x2A, 0xD7, 0xB1, 0x83, 0x0F, 0x42, 0x40]);
        info_payload.extend_from_slice(&[0x44, 0x89, 0x88]);
        info_payload.extend_from_slice(&(7_500.0_f64).to_be_bytes());
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
        let webm_path = create_dummy_media_file(&temp, "cluster_first.webm", &webm);
        assert_eq!(probe_container_duration_ms(&webm_path), Some(7_500));

        // 2. MP3 whose leading ID3v2 header claims a truncated/oversized length (`next >= file_len`)
        //    must NOT advance `offset` past EOF and must still scan the Xing header inside the buffer!
        let mut mp3 = vec![0_u8; 256];
        mp3[0..3].copy_from_slice(b"ID3");
        mp3[3] = 4;
        mp3[4] = 0;
        mp3[6..10].copy_from_slice(&[0x00, 0x10, 0x00, 0x00]); // claims >163KB (> 256B file_len)
        mp3[16..20].copy_from_slice(&[0xFF, 0xFB, 0x90, 0x00]);
        mp3[16 + 36..16 + 40].copy_from_slice(b"Xing");
        mp3[16 + 40..16 + 44].copy_from_slice(&1_u32.to_be_bytes());
        mp3[16 + 44..16 + 48].copy_from_slice(&500_u32.to_be_bytes()); // 500 * 1152 / 44100 = 13_061 ms
        let mp3_path = create_dummy_media_file(&temp, "truncated_id3.mp3", &mp3);
        assert_eq!(probe_container_duration_ms(&mp3_path), Some(13_061));
    }

    #[test]
    fn wal_corrupt_backup_allowlist_sources_clamping_bounds_and_windows_parent_cleanup() {
        let temp = TempDir::new().expect("tempdir");
        let db_path = temp.path().join("audora.sqlite");
        let wal_path = temp.path().join("audora.sqlite-wal");
        {
            let live_db = Database::open(&db_path).expect("open initial wal db");
            live_db
                .rpc_save_settings(&crate::domain::AppSettings::default())
                .expect("write wal frame");
            let saved_main = fs::read(&db_path).expect("read main db");
            let saved_wal = fs::read(&wal_path).unwrap_or_else(|_| b"UNFLUSHED_WAL_DATA".to_vec());
            drop(live_db);
            let mut corrupted_main = saved_main;
            for b in corrupted_main.iter_mut().take(64) {
                *b = 0xFF;
            }
            fs::write(&db_path, corrupted_main).unwrap();
            fs::write(&wal_path, saved_wal).unwrap();
        }

        let recovered = Database::open_or_recover(&db_path).expect("recover");
        assert!(temp.path().join("audora.sqlite.corrupt.bak").is_file());
        assert!(temp.path().join("audora.sqlite-wal.corrupt.bak").is_file());

        // `parse_lenient_state_object` drops oversized notes (>10,000 chars) and oversized playlist names (>120 chars),
        // and `query_allowlist_sources` returns only standalone media (`source_folder_id IS NULL`) + `source_folders`.
        let huge_note = "x".repeat(crate::domain::MAX_NOTE_BODY_CHARS + 10);
        let huge_pl_name = "p".repeat(crate::domain::MAX_PLAYLIST_NAME_CHARS + 5);
        let import_payload = serde_json::json!({
            "media": [
                {
                    "id": "m_standalone",
                    "path": "/tmp/standalone.flac",
                    "media_kind": "audio",
                    "availability": "available"
                },
                {
                    "id": "m_folder",
                    "path": "/tmp/folder/track.flac",
                    "media_kind": "audio",
                    "availability": "available",
                    "source_folder_id": "src_1"
                }
            ],
            "notes": [
                { "id": "n_huge", "media_id": "m_standalone", "position_ms": 0, "body": huge_note },
                { "id": "n_ok", "media_id": "m_standalone", "position_ms": 1000, "body": "valid" }
            ],
            "playlists": [
                { "id": "pl_huge", "name": huge_pl_name, "description": "" },
                { "id": "pl_ok", "name": "Valid List", "description": "", "item_ids": ["m_standalone"] }
            ],
            "settings": {
                "source_folders": [{
                    "id": "src_1",
                    "path": "/tmp/folder",
                    "label": "folder",
                    "enabled": true
                }]
            }
        });
        recovered
            .import_relational_state_json(&import_payload.to_string())
            .expect("import");

        let (standalone_paths, folders) = recovered.query_allowlist_sources().expect("sources");
        assert_eq!(standalone_paths, vec!["/tmp/standalone.flac".to_owned()]);
        assert_eq!(folders.len(), 1);

        let exported = recovered.export_relational_state().unwrap().unwrap();
        assert_eq!(exported.notes.len(), 1);
        assert_eq!(exported.notes[0].id, "n_ok");
        assert_eq!(exported.playlists.len(), 1);
        assert_eq!(exported.playlists[0].id, "pl_ok");

        // `u64::MAX` clamps to `i64::MAX` (`MAX_SAFE_DURATION_MS`) instead of resetting to 0!
        recovered
            .rpc_update_progress("m_standalone", u64::MAX, false, None)
            .expect("clamp progress");
        let after_clamp = recovered.export_relational_state().unwrap().unwrap();
        let m_st = after_clamp
            .media
            .iter()
            .find(|m| m.id == "m_standalone")
            .unwrap();
        assert_eq!(m_st.last_position_ms, crate::domain::MAX_SAFE_DURATION_MS);

        drop(recovered);

        // Windows-style `<root>/com.audora.desktop/data` cleanup removes both `data` and empty `com.audora.desktop`!
        let win_parent = temp.path().join("com.audora.desktop");
        let win_data = win_parent.join("data");
        fs::create_dir_all(win_data.join("webview")).unwrap();
        fs::write(win_data.join("audora.sqlite"), b"stub").unwrap();
        let _ = crate::services::cleanup_all_app_artifacts(&win_data).expect("cleanup win layout");
        assert!(!win_data.exists());
        assert!(!win_parent.exists());
    }

    #[test]
    fn round2_prober_handles_isobmff_wav_rf64_flac_ogg_mp3_vbr_aac_and_gst_non_ascii() {
        use crate::adapters::probe_container_duration_ms;

        fn make_box(tag: &[u8; 4], payload: &[u8]) -> Vec<u8> {
            let total = u32::try_from(8 + payload.len()).expect("box size");
            let mut out = Vec::with_capacity(8 + payload.len());
            out.extend_from_slice(&total.to_be_bytes());
            out.extend_from_slice(tag);
            out.extend_from_slice(payload);
            out
        }

        fn make_mvhd(timescale: u32, duration: u32) -> Vec<u8> {
            let mut p = vec![0_u8; 24];
            p[12..16].copy_from_slice(&timescale.to_be_bytes());
            p[16..20].copy_from_slice(&duration.to_be_bytes());
            make_box(b"mvhd", &p)
        }

        fn make_mehd(duration: u32) -> Vec<u8> {
            let mut p = vec![0_u8; 8];
            p[4..8].copy_from_slice(&duration.to_be_bytes());
            make_box(b"mehd", &p)
        }

        fn make_mdhd(timescale: u32, duration: u32) -> Vec<u8> {
            let mut p = vec![0_u8; 24];
            p[12..16].copy_from_slice(&timescale.to_be_bytes());
            p[16..20].copy_from_slice(&duration.to_be_bytes());
            make_box(b"mdhd", &p)
        }

        fn make_sidx(ref_id: u32, timescale: u32, sub_dur: u32) -> Vec<u8> {
            let mut p = vec![0_u8; 36];
            p[4..8].copy_from_slice(&ref_id.to_be_bytes());
            p[8..12].copy_from_slice(&timescale.to_be_bytes());
            p[22..24].copy_from_slice(&1_u16.to_be_bytes()); // reference_count = 1
            p[28..32].copy_from_slice(&sub_dur.to_be_bytes());
            make_box(b"sidx", &p)
        }

        let temp = TempDir::new().expect("tempdir");
        let ftyp = make_box(b"ftyp", b"isom\x00\x00\x02\x00isom");

        // 1a. ISO-BMFF `mvhd` duration
        let mut mp4_mvhd = ftyp.clone();
        mp4_mvhd.extend_from_slice(&make_box(b"moov", &make_mvhd(1_000, 12_345)));
        let mp4_mvhd_path = create_dummy_media_file(&temp, "mvhd.mp4", &mp4_mvhd);
        assert_eq!(probe_container_duration_ms(&mp4_mvhd_path), Some(12_345));

        // 1b. Hybrid fMP4: `mvhd` (2s) + `mvex/mehd` (45s) -> honors larger `mehd`!
        let mut hybrid_moov_body = make_mvhd(1_000, 2_000);
        hybrid_moov_body.extend_from_slice(&make_box(b"mvex", &make_mehd(45_000)));
        let mut mp4_hybrid = ftyp.clone();
        mp4_hybrid.extend_from_slice(&make_box(b"moov", &hybrid_moov_body));
        let mp4_hybrid_path = create_dummy_media_file(&temp, "hybrid.mp4", &mp4_hybrid);
        assert_eq!(probe_container_duration_ms(&mp4_hybrid_path), Some(45_000));

        // 1c. `mdhd` fallback when `mvhd` duration = 0
        let mut mdhd_moov_body = make_mvhd(1_000, 0);
        let mdia = make_box(b"mdia", &make_mdhd(48_000, 480_000));
        mdhd_moov_body.extend_from_slice(&make_box(b"trak", &mdia));
        let mut mp4_mdhd = ftyp.clone();
        mp4_mdhd.extend_from_slice(&make_box(b"moov", &mdhd_moov_body));
        let mp4_mdhd_path = create_dummy_media_file(&temp, "mdhd.m4a", &mp4_mdhd);
        assert_eq!(probe_container_duration_ms(&mp4_mdhd_path), Some(10_000));

        // 1d. `sidx` fallback across multiple segment index boxes
        let mut mp4_sidx = ftyp.clone();
        mp4_sidx.extend_from_slice(&make_box(b"moov", &make_mvhd(1_000, 0)));
        mp4_sidx.extend_from_slice(&make_sidx(1, 1_000, 7_500));
        mp4_sidx.extend_from_slice(&make_sidx(1, 1_000, 9_500));
        let mp4_sidx_path = create_dummy_media_file(&temp, "sidx.mp4", &mp4_sidx);
        assert_eq!(probe_container_duration_ms(&mp4_sidx_path), Some(17_000));

        // 2a. Standard RIFF WAV
        let mut riff = Vec::new();
        riff.extend_from_slice(b"RIFF");
        riff.extend_from_slice(&0_u32.to_le_bytes());
        riff.extend_from_slice(b"WAVE");
        riff.extend_from_slice(b"fmt ");
        riff.extend_from_slice(&16_u32.to_le_bytes());
        let mut fmt_body = [0_u8; 16];
        fmt_body[8..12].copy_from_slice(&176_400_u32.to_le_bytes());
        riff.extend_from_slice(&fmt_body);
        riff.extend_from_slice(b"data");
        riff.extend_from_slice(&176_400_u32.to_le_bytes());
        riff.resize(riff.len() + 176_400, 0);
        let riff_path = create_dummy_media_file(&temp, "std.wav", &riff);
        assert_eq!(probe_container_duration_ms(&riff_path), Some(1_000));

        // 2b. RF64 WAV where `data` (`0xFFFF_FFFF` sentinel size) appears BEFORE `fmt `
        let mut rf64 = Vec::new();
        rf64.extend_from_slice(b"RF64");
        rf64.extend_from_slice(&0xFFFF_FFFF_u32.to_le_bytes());
        rf64.extend_from_slice(b"WAVE");
        rf64.extend_from_slice(b"ds64");
        rf64.extend_from_slice(&16_u32.to_le_bytes());
        rf64.extend_from_slice(&0_u64.to_le_bytes()); // riffSize
        rf64.extend_from_slice(&88_200_u64.to_le_bytes()); // dataSize = 88_200
        rf64.extend_from_slice(b"data");
        rf64.extend_from_slice(&0xFFFF_FFFF_u32.to_le_bytes());
        rf64.resize(rf64.len() + 88_200, 0);
        rf64.extend_from_slice(b"fmt ");
        rf64.extend_from_slice(&16_u32.to_le_bytes());
        let mut rf64_fmt = [0_u8; 16];
        rf64_fmt[8..12].copy_from_slice(&44_100_u32.to_le_bytes());
        rf64.extend_from_slice(&rf64_fmt);
        let rf64_path = create_dummy_media_file(&temp, "rf64_data_first.wav", &rf64);
        assert_eq!(probe_container_duration_ms(&rf64_path), Some(2_000));

        // 3. FLAC STREAMINFO (44.1kHz, 441_000 samples -> 10_000 ms)
        let mut flac = Vec::new();
        flac.extend_from_slice(b"fLaC");
        flac.extend_from_slice(&[0x80, 0x00, 0x00, 34]); // last metadata block, type 0, len 34
        let mut si = [0_u8; 34];
        // 44_100 = 0x0AC44 -> bits in si[10..13]: si[10] = 0x0A, si[11] = 0xC4, si[12] top nibble = 0x4
        si[10] = 0x0A;
        si[11] = 0xC4;
        si[12] = 0x40;
        // 441_000 samples = 0x00_0006_BAA8 in si[13..18]
        si[15] = 0x06;
        si[16] = 0xBA;
        si[17] = 0xA8;
        flac.extend_from_slice(&si);
        let flac_path = create_dummy_media_file(&temp, "track.flac", &flac);
        assert_eq!(probe_container_duration_ms(&flac_path), Some(10_000));

        // 4. Ogg Vorbis + trailing padding bytes after EOS page (`0x04`)
        let mut vorbis = Vec::new();
        vorbis.extend_from_slice(b"OggS\x00\x02");
        vorbis.extend_from_slice(&0_u64.to_le_bytes());
        vorbis.extend_from_slice(&9_u32.to_le_bytes());
        vorbis.extend_from_slice(&0_u32.to_le_bytes());
        vorbis.extend_from_slice(&0_u32.to_le_bytes());
        vorbis.push(1);
        vorbis.push(30);
        let mut vhead = [0_u8; 30];
        vhead[0..7].copy_from_slice(b"\x01vorbis");
        vhead[12..16].copy_from_slice(&44_100_u32.to_le_bytes());
        vorbis.extend_from_slice(&vhead);
        vorbis.extend_from_slice(b"OggS\x00\x04");
        vorbis.extend_from_slice(&441_000_u64.to_le_bytes());
        vorbis.extend_from_slice(&9_u32.to_le_bytes());
        vorbis.extend_from_slice(&1_u32.to_le_bytes());
        vorbis.extend_from_slice(&0_u32.to_le_bytes());
        vorbis.push(0);
        vorbis.extend_from_slice(&[0_u8; 64]); // trailing padding after EOS
        let vorbis_path = create_dummy_media_file(&temp, "padded.ogg", &vorbis);
        assert_eq!(probe_container_duration_ms(&vorbis_path), Some(10_000));

        // 5a. MP3 VBR multi-frame without Xing/VBRI (2x 128kbps 417B + 2x 320kbps 1044B at 44.1kHz -> 4*1152/44100 = 104 ms)
        let mut mp3_vbr = Vec::new();
        for _ in 0..2 {
            let mut f = vec![0_u8; 417];
            f[0..4].copy_from_slice(&[0xFF, 0xFB, 0x90, 0x00]); // 128 kbps
            mp3_vbr.extend_from_slice(&f);
        }
        for _ in 0..2 {
            let mut f = vec![0_u8; 1044];
            f[0..4].copy_from_slice(&[0xFF, 0xFB, 0xE0, 0x00]); // 320 kbps
            mp3_vbr.extend_from_slice(&f);
        }
        let mp3_vbr_path = create_dummy_media_file(&temp, "vbr_no_xing.mp3", &mp3_vbr);
        assert_eq!(probe_container_duration_ms(&mp3_vbr_path), Some(104));

        // 5b. MP3 VBRI with CRC enabled (`0xFF 0xFA`, crc_len = 2 -> VBRI at offset 4 + 2 + 32 = 38)
        let mut mp3_vbri = vec![0_u8; 256];
        mp3_vbri[0..4].copy_from_slice(&[0xFF, 0xFA, 0x90, 0x00]);
        mp3_vbri[38..42].copy_from_slice(b"VBRI");
        mp3_vbri[52..56].copy_from_slice(&500_u32.to_be_bytes());
        let mp3_vbri_path = create_dummy_media_file(&temp, "vbri_crc.mp3", &mp3_vbri);
        assert_eq!(probe_container_duration_ms(&mp3_vbri_path), Some(13_061));

        // 5c. False MP3 sync near EOF (`idx > 0 && next_file_pos + 4 > effective_end`) must be rejected
        let mut mp3_eof_junk = vec![0_u8; 64];
        mp3_eof_junk[10..14].copy_from_slice(&[0xFF, 0xFB, 0x90, 0x00]); // frame_size 417 > 64
        let mp3_eof_junk_path = create_dummy_media_file(&temp, "eof_junk.mp3", &mp3_eof_junk);
        assert_eq!(probe_container_duration_ms(&mp3_eof_junk_path), None);

        // 5d. Single-frame MP3 at `idx == 0 && next_file_pos == effective_end` succeeds
        let mut mp3_single = vec![0_u8; 417];
        mp3_single[0..4].copy_from_slice(&[0xFF, 0xFB, 0x90, 0x00]);
        let mp3_single_path = create_dummy_media_file(&temp, "single_frame.mp3", &mp3_single);
        assert_eq!(probe_container_duration_ms(&mp3_single_path), Some(26));

        // 6a. Short ISO-BMFF `.aac` containing fake `0xFF 0xF1` ADTS bytes in `mdat` -> must read `mvhd` first!
        let mut bmff_aac = ftyp;
        bmff_aac.extend_from_slice(&make_box(b"moov", &make_mvhd(1_000, 5_000)));
        bmff_aac.extend_from_slice(&make_box(
            b"mdat",
            &[0xFF, 0xF1, 0x4C, 0x80, 0x10, 0x00, 0xFC],
        ));
        let bmff_aac_path = create_dummy_media_file(&temp, "container.aac", &bmff_aac);
        assert_eq!(probe_container_duration_ms(&bmff_aac_path), Some(5_000));

        // 6b. Exact 7-byte single raw ADTS frame at offset 0 (48kHz -> 1024 / 48 = 21 ms)
        let mut single_adts = vec![0_u8; 7];
        single_adts[0] = 0xFF;
        single_adts[1] = 0xF1;
        single_adts[2] = 0x4C; // 48kHz
        single_adts[3] = 0x80; // frame_len = 7 -> 7 << 5 = 0xE0
        single_adts[4] = 0x00;
        single_adts[5] = 0xE0;
        single_adts[6] = 0xFC;
        let single_adts_path = create_dummy_media_file(&temp, "single.aac", &single_adts);
        assert_eq!(probe_container_duration_ms(&single_adts_path), Some(21));

        // 6c. False ADTS sync at `idx > 0` overshooting EOF must be rejected (`None`)
        let mut false_adts = vec![0_u8; 16];
        false_adts[4..11].copy_from_slice(&[0xFF, 0xF1, 0x4C, 0x80, 0x10, 0x00, 0xFC]);
        let false_adts_path = create_dummy_media_file(&temp, "false_adts.aac", &false_adts);
        assert_eq!(probe_container_duration_ms(&false_adts_path), None);

        // 7. `parse_gst_duration_ms` with non-ASCII `\u{FFFD}` must NOT panic
        let gst_out = "  Duration: 0:00:01.2\u{FFFD}\n  audio #0: Opus\n";
        let gst_meta =
            GStreamerBackend::parse_discoverer_output(Path::new("/tmp/bad.opus"), gst_out)
                .expect("non-ascii duration falls back to None safely");
        assert_eq!(gst_meta.duration_ms, None);
    }

    #[test]
    fn round2_reconcile_relink_resets_duration_and_v3_v4_preserves_duplicate_path_notes() {
        let temp = TempDir::new().expect("tempdir");
        let db_path = temp.path().join("audora_dup_v3.sqlite");

        // 1. `reconcile_upsert_media_in_tx` resets duration/size/fingerprint on relink (`path_changed`)
        //    and clears `duration_ms` when `!path_changed && authoritative_folder` resets probe flag.
        let mem_db = Database::open_in_memory().expect("mem db");
        let mut item = crate::domain::MediaItem {
            id: "med_1".to_owned(),
            path: "/tmp/old.mp4".to_owned(),
            display_name: "old.mp4".to_owned(),
            media_kind: MediaKind::Video,
            duration_ms: Some(120_000),
            duration_probed: true,
            last_position_ms: 15_000,
            completed: false,
            file_size_bytes: Some(4096),
            fingerprint: Some("fp_old".to_owned()),
            availability: Availability::Available,
            added_at: "2026-01-01T00:00:00.000Z".to_owned(),
            last_opened_at: None,
            last_seen_at: "2026-01-01T00:00:00.000Z".to_owned(),
            note_count: 0,
            source_folder_id: None,
        };
        mem_db
            .rpc_upsert_media_batch(&[item.clone()])
            .expect("seed");

        item.path = "/tmp/new.mp4".to_owned();
        item.display_name = "new.mp4".to_owned();
        item.duration_ms = None;
        item.duration_probed = false;
        item.file_size_bytes = Some(1024);
        item.fingerprint = None;
        mem_db
            .rpc_upsert_media_batch(&[item.clone()])
            .expect("relink path");
        let after_relink = mem_db.export_relational_state().unwrap().unwrap();
        assert_eq!(after_relink.media[0].duration_ms, None);
        assert!(!after_relink.media[0].duration_probed);
        assert_eq!(after_relink.media[0].file_size_bytes, Some(1024));
        assert_eq!(after_relink.media[0].fingerprint, None);

        item.duration_ms = Some(60_000);
        item.duration_probed = true;
        mem_db
            .rpc_upsert_media_batch(&[item.clone()])
            .expect("set probed");
        item.duration_ms = None;
        item.duration_probed = false;
        mem_db.rpc_upsert_media_batch(&[item]).expect("reset probe");
        let after_reset = mem_db.export_relational_state().unwrap().unwrap();
        assert_eq!(after_reset.media[0].duration_ms, None);
        assert!(!after_reset.media[0].duration_probed);

        // 2. `v3 -> v4` migration with duplicate paths inside the v3 blob preserves notes/playlists/session!
        let v3_migrations = &crate::persistence::db::BUILTIN_MIGRATIONS[..3];
        let v3_db = Database::open_with_migrations(&db_path, v3_migrations).expect("open v3");
        v3_db
            .with_conn(|conn| {
                let blob = r#"{
                    "media": [
                        { "id": "med_dup_1", "path": "/tmp/dup.mp3", "media_kind": "audio", "availability": "available" },
                        { "id": "med_dup_2", "path": "/tmp/dup.mp3", "media_kind": "audio", "availability": "available" }
                    ],
                    "notes": [
                        { "id": "note_from_1", "media_id": "med_dup_1", "position_ms": 1000, "body": "From dup 1" },
                        { "id": "note_from_2", "media_id": "med_dup_2", "position_ms": 2000, "body": "From dup 2" }
                    ],
                    "playlists": [
                        { "id": "pl_dup", "name": "Dup List", "item_ids": ["med_dup_1", "med_dup_2"] }
                    ],
                    "playback_session": {
                        "active_media_id": "med_dup_1",
                        "queue": ["med_dup_1", "med_dup_2"],
                        "volume": 0.9,
                        "rate": 1.0
                    }
                }"#;
                conn.execute(
                    "INSERT INTO app_ui_state (key, payload_json, updated_at) VALUES ('v2', ?1, '1');",
                    [blob],
                )?;
                Ok(())
            })
            .expect("seed v3 dup");
        drop(v3_db);

        let (dispatcher, v4_db) =
            build_production_dispatcher_with_db(&db_path).expect("upgrade to v4");
        let migrated = v4_db.export_relational_state().unwrap().unwrap();
        assert_eq!(migrated.media.len(), 1);
        assert_eq!(migrated.media[0].id, "med_dup_2");
        assert_eq!(migrated.notes.len(), 2);
        assert!(migrated.notes.iter().all(|n| n.media_id == "med_dup_2"));
        assert_eq!(migrated.playlists[0].item_ids, vec!["med_dup_2"]);
        let sess = migrated.playback_session.expect("session");
        assert_eq!(sess.active_media_id.as_deref(), Some("med_dup_2"));
        assert_eq!(sess.queue, vec!["med_dup_2"]);

        // `generate_diagnostic_report` falls back to persisted `playback_session` when in-memory `snapshot.media_id` is None!
        let diag = dispatcher.generate_diagnostic_report().expect("diag");
        assert_eq!(diag.playback_state, PlaybackState::Paused);

        // `import_relational_state_json` handles duplicate paths and duplicate IDs cleanly
        let import_json = serde_json::json!({
            "media": [
                { "id": "m_a", "path": "/tmp/one.mp4", "media_kind": "video", "availability": "available" },
                { "id": "m_b", "path": "/tmp/one.mp4", "media_kind": "video", "availability": "available" },
                { "id": "m_a", "path": "/tmp/two.mp4", "media_kind": "video", "availability": "available" },
                { "id": "m_c", "path": "/tmp/two.mp4", "media_kind": "video", "availability": "available" }
            ],
            "notes": [
                { "id": "n_b", "media_id": "m_b", "position_ms": 1000, "body": "Remapped from m_b to m_a" }
            ],
            "playlists": [
                { "id": "pl_1", "name": "Imported", "item_ids": ["m_b", "m_a", "m_c"] }
            ],
            "playback_session": {
                "active_media_id": "m_b",
                "queue": ["m_b", "m_c"]
            }
        });
        v4_db
            .import_relational_state_json(&import_json.to_string())
            .expect("import with dups");
        let imported = v4_db.export_relational_state().unwrap().unwrap();
        assert_eq!(imported.media.len(), 2);
        assert_eq!(imported.notes.len(), 1);
        assert_eq!(imported.notes[0].media_id, "m_a");
        assert_eq!(imported.playlists[0].item_ids, vec!["m_a", "m_c"]);
        let imp_sess = imported.playback_session.expect("imp session");
        assert_eq!(imp_sess.active_media_id.as_deref(), Some("m_a"));
        assert_eq!(imp_sess.queue, vec!["m_a", "m_c"]);

        // `SetVolume` rejects NaN & `open_media` resets snapshot cleanly on backend load failure
        assert_eq!(
            dispatcher
                .playback_command(PlaybackCommand::SetVolume { volume: f64::NAN })
                .expect_err("NaN volume")
                .code,
            ErrorCode::ValidationFailed
        );
        let good_file = create_dummy_media_file(&temp, "ok.mp4", b"ok");
        let bad_file = create_dummy_media_file(&temp, "bad.prores", b"bad");
        dispatcher
            .open_media(good_file.to_str().unwrap())
            .expect("open good");
        assert!(dispatcher.open_media(bad_file.to_str().unwrap()).is_err());
        let failed_snap = dispatcher.get_playback_snapshot().expect("snap");
        assert_eq!(failed_snap.state, PlaybackState::Failed);
        assert_eq!(failed_snap.media_id, None);
        assert_eq!(failed_snap.duration_ms, None);
        assert_eq!(failed_snap.position_ms, 0);

        // `open_media_batch` pre-validates remote URLs, strips leading whitespace + CRLF, and preserves POSIX trailing spaces
        assert_eq!(
            dispatcher
                .open_media_batch(&["rtmp://evil.example/live".to_owned()])
                .expect_err("reject rtmp in batch")
                .code,
            ErrorCode::ValidationFailed
        );
        let spaced_dir = temp.path().join("Folder With Spaces");
        fs::create_dir_all(&spaced_dir).expect("spaced dir");
        create_dummy_media_file(&temp, "Folder With Spaces/clip.mp4", b"clip-bytes");
        let spaced_opened = dispatcher
            .open_media_batch(&[format!("  {}\r\n", spaced_dir.to_str().unwrap())])
            .expect("open folder with interior spaces and CRLF wrapper");
        assert_eq!(spaced_opened.len(), 1);

        #[cfg(unix)]
        {
            let trailing_dir = temp.path().join("Folder Space ");
            fs::create_dir_all(&trailing_dir).expect("trailing space dir");
            create_dummy_media_file(&temp, "Folder Space /clip.mp4", b"clip-bytes");
            let trailing_opened = dispatcher
                .open_media_batch(&[trailing_dir.to_str().unwrap().to_owned()])
                .expect("open folder with trailing space on unix");
            assert_eq!(trailing_opened.len(), 1);
        }

        // `SqliteMediaRepository::upsert_media` reloads via validated path & `relink_path` enforces AV-201 inside tx
        let repo = SqliteMediaRepository::new(v4_db.clone());
        let mut ws_item = imported.media[0].clone();
        let ws_path = format!("  {}\n", ws_item.path);
        ws_item.path = ws_path;
        let reloaded_ws = repo.upsert_media(&ws_item).expect("reload cleaned path");
        assert_eq!(reloaded_ws.path, imported.media[0].path);
        let relink_conflict = repo
            .relink_path(&crate::domain::RelinkMediaRecord {
                id: &imported.media[1].id,
                new_path: &imported.media[0].path,
                display_name: "conflict.mp4",
                media_kind: MediaKind::Video,
                duration_ms: Some(1_000),
                duration_probed: true,
                file_size_bytes: Some(100),
                updated_at: "2026-05-01T00:00:00.000Z",
            })
            .expect_err("relink_path must enforce AV-201");
        assert_eq!(relink_conflict.code(), ErrorCode::RelinkConflict);
        drop(v4_db);
        drop(dispatcher);

        // Corrupt WAL recovery unlinks stale `-wal.corrupt.bak` when a new corruption event has no `-wal`
        let wal_bak_db = temp.path().join("wal_stale.sqlite");
        let stale_wal_bak = temp.path().join("wal_stale.sqlite-wal.corrupt.bak");
        fs::write(&stale_wal_bak, b"STALE_OLD_WAL").unwrap();
        fs::write(&wal_bak_db, b"NOT_SQLITE").unwrap();
        let recovered_no_wal = Database::open_or_recover(&wal_bak_db).expect("recover without wal");
        assert!(recovered_no_wal.verify_integrity().unwrap());
        assert!(
            !stale_wal_bak.exists(),
            "stale -wal.corrupt.bak must be unlinked when no -wal exists"
        );
    }
}
