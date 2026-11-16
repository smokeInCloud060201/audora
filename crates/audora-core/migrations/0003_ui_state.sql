-- Audora v3 Schema Migration: Unified SQLite UI State Snapshot Store
-- Consolidates persisted UI library, playlists, notes, and settings inside `audora.sqlite`
-- so WAL checkpoints, pre-migration backups, and manual `.sqlite.manual.bak` snapshots
-- capture 100% of application state in a single database file.

CREATE TABLE IF NOT EXISTS app_ui_state (
    key          TEXT PRIMARY KEY NOT NULL,
    payload_json TEXT NOT NULL,
    updated_at   TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now'))
);
