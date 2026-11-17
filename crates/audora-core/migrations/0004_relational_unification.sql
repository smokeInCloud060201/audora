-- Audora v4 Schema Migration: Full Relational Unification
-- Adds trusted source-folder foreign linkage and authoritative container-probe flags
-- directly to `media` so the UI and `audora-core` share 100% normalized relational tables
-- (`media`, `notes`, `playlists`, `playlist_items`, `settings`) with O(1) indexed updates.

ALTER TABLE media ADD COLUMN source_folder_id TEXT;
ALTER TABLE media ADD COLUMN duration_probed INTEGER NOT NULL DEFAULT 0 CHECK (duration_probed IN (0, 1));

CREATE INDEX IF NOT EXISTS idx_media_source_folder ON media(source_folder_id);
