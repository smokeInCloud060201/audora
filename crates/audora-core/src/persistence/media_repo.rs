//! SQLite implementation of `MediaRepository`.

use rusqlite::{params, OptionalExtension, Row};

use crate::domain::{
    Availability, LibraryFilter, MediaItem, MediaKind, MediaRepository, RelinkMediaRecord,
};
use crate::error::{AppError, AppResult};
use crate::persistence::Database;

#[derive(Clone)]
pub struct SqliteMediaRepository {
    db: Database,
}

impl SqliteMediaRepository {
    #[must_use]
    pub const fn new(db: Database) -> Self {
        Self { db }
    }

    fn map_row(row: &Row<'_>) -> rusqlite::Result<MediaItem> {
        let kind_str: String = row.get("media_kind")?;
        let avail_str: String = row.get("availability")?;
        let duration_i64: Option<i64> = row.get("duration_ms")?;
        let last_pos_i64: i64 = row.get("last_position_ms")?;
        let completed_int: i64 = row.get("completed")?;
        let size_i64: Option<i64> = row.get("file_size_bytes")?;
        let note_count_i64: i64 = row.get("note_count").unwrap_or(0);

        Ok(MediaItem {
            id: row.get("id")?,
            path: row.get("path")?,
            display_name: row.get("display_name")?,
            media_kind: MediaKind::from_db_str(&kind_str).unwrap_or(MediaKind::Video),
            duration_ms: duration_i64.and_then(|v| u64::try_from(v).ok()),
            last_position_ms: u64::try_from(last_pos_i64).unwrap_or(0),
            completed: completed_int != 0,
            file_size_bytes: size_i64.and_then(|v| u64::try_from(v).ok()),
            fingerprint: row.get("fingerprint")?,
            availability: Availability::from_db_str(&avail_str).unwrap_or(Availability::Missing),
            added_at: row.get("added_at")?,
            last_opened_at: row.get("last_opened_at")?,
            last_seen_at: row.get("last_seen_at")?,
            note_count: usize::try_from(note_count_i64).unwrap_or(0),
        })
    }
}

impl MediaRepository for SqliteMediaRepository {
    fn upsert_media(&self, item: &MediaItem) -> AppResult<MediaItem> {
        self.db.with_conn(|conn| {
            let tx = conn.transaction()?;
            tx.execute(
                "INSERT INTO media (
                    id, path, display_name, media_kind, duration_ms, last_position_ms,
                    completed, file_size_bytes, fingerprint, availability, added_at,
                    last_opened_at, last_seen_at
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)
                ON CONFLICT(path) DO UPDATE SET
                    display_name = excluded.display_name,
                    media_kind = excluded.media_kind,
                    duration_ms = COALESCE(excluded.duration_ms, media.duration_ms),
                    file_size_bytes = COALESCE(excluded.file_size_bytes, media.file_size_bytes),
                    availability = excluded.availability,
                    last_opened_at = excluded.last_opened_at,
                    last_seen_at = excluded.last_seen_at",
                params![
                    item.id,
                    item.path,
                    item.display_name,
                    item.media_kind.as_str(),
                    item.duration_ms.and_then(|v| i64::try_from(v).ok()),
                    i64::try_from(item.last_position_ms).unwrap_or(0),
                    i64::from(item.completed),
                    item.file_size_bytes.and_then(|v| i64::try_from(v).ok()),
                    item.fingerprint,
                    item.availability.as_str(),
                    item.added_at,
                    item.last_opened_at,
                    item.last_seen_at,
                ],
            )?;
            tx.commit()?;
            Ok(())
        })?;

        self.get_by_path(&item.path)?
            .ok_or_else(|| AppError::persistence("Upserted media record could not be reloaded"))
    }

    fn get_by_id(&self, id: &str) -> AppResult<Option<MediaItem>> {
        self.db.with_conn(|conn| {
            let item = conn
                .query_row(
                    "SELECT m.*, (SELECT COUNT(*) FROM notes n WHERE n.media_id = m.id) AS note_count
                     FROM media m WHERE m.id = ?1",
                    params![id],
                    Self::map_row,
                )
                .optional()?;
            Ok(item)
        })
    }

    fn get_by_path(&self, path: &str) -> AppResult<Option<MediaItem>> {
        self.db.with_conn(|conn| {
            let item = conn
                .query_row(
                    "SELECT m.*, (SELECT COUNT(*) FROM notes n WHERE n.media_id = m.id) AS note_count
                     FROM media m WHERE m.path = ?1",
                    params![path],
                    Self::map_row,
                )
                .optional()?;
            Ok(item)
        })
    }

    fn list_media(&self, query: Option<&str>, filter: LibraryFilter) -> AppResult<Vec<MediaItem>> {
        self.db.with_conn(|conn| {
            let search_pattern = query
                .map(str::trim)
                .filter(|q| !q.is_empty())
                .map(|q| format!("%{}%", q.to_lowercase()));

            let kind_filter = match filter {
                LibraryFilter::All => None,
                LibraryFilter::Video => Some("video"),
                LibraryFilter::Audio => Some("audio"),
            };

            let mut stmt = conn.prepare(
                "SELECT m.*, (SELECT COUNT(*) FROM notes n WHERE n.media_id = m.id) AS note_count
                 FROM media m
                 WHERE (?1 IS NULL OR LOWER(m.display_name) LIKE ?1 OR EXISTS (
                     SELECT 1 FROM notes n WHERE n.media_id = m.id AND LOWER(n.body) LIKE ?1
                 ))
                 AND (?2 IS NULL OR m.media_kind = ?2)
                 ORDER BY COALESCE(m.last_opened_at, m.added_at) DESC, m.display_name ASC",
            )?;

            let rows = stmt.query_map(params![search_pattern, kind_filter], Self::map_row)?;
            let mut items = Vec::new();
            for row in rows {
                items.push(row?);
            }
            Ok(items)
        })
    }

    fn update_progress(
        &self,
        id: &str,
        last_position_ms: u64,
        completed: bool,
        updated_at: &str,
    ) -> AppResult<()> {
        self.db.with_conn(|conn| {
            let tx = conn.transaction()?;
            tx.execute(
                "UPDATE media
                 SET last_position_ms = ?2, completed = ?3, last_seen_at = ?4
                 WHERE id = ?1",
                params![
                    id,
                    i64::try_from(last_position_ms).unwrap_or(0),
                    i64::from(completed),
                    updated_at
                ],
            )?;
            tx.commit()?;
            Ok(())
        })
    }

    fn update_availability(
        &self,
        id: &str,
        availability: Availability,
        updated_at: &str,
    ) -> AppResult<()> {
        self.db.with_conn(|conn| {
            let tx = conn.transaction()?;
            tx.execute(
                "UPDATE media SET availability = ?2, last_seen_at = ?3 WHERE id = ?1",
                params![id, availability.as_str(), updated_at],
            )?;
            tx.commit()?;
            Ok(())
        })
    }

    fn relink_path(&self, record: &RelinkMediaRecord<'_>) -> AppResult<MediaItem> {
        self.db.with_conn(|conn| {
            let tx = conn.transaction()?;
            let updated = tx.execute(
                "UPDATE media
                 SET path = ?2,
                     display_name = ?3,
                     media_kind = ?4,
                     duration_ms = COALESCE(?5, duration_ms),
                     file_size_bytes = COALESCE(?6, file_size_bytes),
                     availability = 'available',
                     last_seen_at = ?7
                 WHERE id = ?1",
                params![
                    record.id,
                    record.new_path,
                    record.display_name,
                    record.media_kind.as_str(),
                    record.duration_ms.and_then(|v| i64::try_from(v).ok()),
                    record.file_size_bytes.and_then(|v| i64::try_from(v).ok()),
                    record.updated_at,
                ],
            )?;
            if updated == 0 {
                return Err(AppError::validation(format!(
                    "Media item '{}' not found",
                    record.id
                )));
            }
            tx.commit()?;
            Ok(())
        })?;

        self.get_by_id(record.id)?
            .ok_or_else(|| AppError::persistence("Relinked media record could not be loaded"))
    }

    fn delete_media(&self, id: &str) -> AppResult<bool> {
        self.db.with_conn(|conn| {
            let tx = conn.transaction()?;
            let affected = tx.execute("DELETE FROM media WHERE id = ?1", params![id])?;
            tx.commit()?;
            Ok(affected > 0)
        })
    }
}
