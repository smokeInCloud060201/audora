-- Audora Initial Database Schema (Migration 0001)
-- Foreign keys and WAL mode are enforced by the connection manager before migration execution.

CREATE TABLE IF NOT EXISTS media (
    id TEXT PRIMARY KEY NOT NULL,
    path TEXT NOT NULL UNIQUE,
    display_name TEXT NOT NULL,
    media_kind TEXT NOT NULL CHECK (media_kind IN ('video', 'audio')),
    duration_ms INTEGER CHECK (duration_ms IS NULL OR duration_ms >= 0),
    last_position_ms INTEGER NOT NULL DEFAULT 0 CHECK (last_position_ms >= 0),
    completed INTEGER NOT NULL DEFAULT 0 CHECK (completed IN (0, 1)),
    file_size_bytes INTEGER CHECK (file_size_bytes IS NULL OR file_size_bytes >= 0),
    fingerprint TEXT,
    availability TEXT NOT NULL CHECK (availability IN ('available', 'missing', 'unreadable', 'unsupported')),
    added_at TEXT NOT NULL,
    last_opened_at TEXT,
    last_seen_at TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_media_kind ON media(media_kind);
CREATE INDEX IF NOT EXISTS idx_media_last_opened ON media(last_opened_at DESC, added_at DESC);

CREATE TABLE IF NOT EXISTS notes (
    id TEXT PRIMARY KEY NOT NULL,
    media_id TEXT NOT NULL REFERENCES media(id) ON DELETE CASCADE,
    position_ms INTEGER NOT NULL CHECK (position_ms >= 0),
    is_quick_marker INTEGER NOT NULL DEFAULT 0 CHECK (is_quick_marker IN (0, 1)),
    body TEXT NOT NULL DEFAULT '',
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_notes_media_position ON notes(media_id, position_ms ASC, created_at ASC);
CREATE INDEX IF NOT EXISTS idx_notes_updated_at ON notes(updated_at DESC);

CREATE TABLE IF NOT EXISTS settings (
    key TEXT PRIMARY KEY NOT NULL,
    value_json TEXT NOT NULL,
    updated_at TEXT NOT NULL
);
