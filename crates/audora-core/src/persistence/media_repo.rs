//! SQLite implementation of `MediaRepository`.

use rusqlite::{params, Connection, OptionalExtension, Row, TransactionBehavior};

use crate::domain::{
    validate_local_media_path_str, Availability, LibraryFilter, MediaItem, MediaKind,
    MediaRepository, RelinkMediaRecord,
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

    fn load_by_id_conn(conn: &Connection, id: &str) -> AppResult<Option<MediaItem>> {
        let item = conn
            .query_row(
                "SELECT m.*, (SELECT COUNT(*) FROM notes n WHERE n.media_id = m.id) AS note_count
                 FROM media m WHERE m.id = ?1",
                params![id],
                Self::map_row,
            )
            .optional()?;
        Ok(item)
    }

    fn load_by_path_conn(conn: &Connection, path: &str) -> AppResult<Option<MediaItem>> {
        let item = conn
            .query_row(
                "SELECT m.*, (SELECT COUNT(*) FROM notes n WHERE n.media_id = m.id) AS note_count
                 FROM media m WHERE m.path = ?1",
                params![path],
                Self::map_row,
            )
            .optional()?;
        Ok(item)
    }

    fn map_row(row: &Row<'_>) -> rusqlite::Result<MediaItem> {
        let kind_str: String = row.get("media_kind")?;
        let avail_str: String = row.get("availability")?;
        let duration_i64: Option<i64> = row.get("duration_ms")?;
        let duration_probed_int: i64 = row.get("duration_probed").unwrap_or(0);
        let last_pos_i64: i64 = row.get("last_position_ms")?;
        let completed_int: i64 = row.get("completed")?;
        let size_i64: Option<i64> = row.get("file_size_bytes")?;
        let note_count_i64: i64 = row.get("note_count").unwrap_or(0);
        let source_folder_id: Option<String> = row.get("source_folder_id").unwrap_or(None);

        Ok(MediaItem {
            id: row.get("id")?,
            path: row.get("path")?,
            display_name: row.get("display_name")?,
            media_kind: MediaKind::from_db_str(&kind_str).unwrap_or(MediaKind::Video),
            duration_ms: duration_i64.and_then(|v| u64::try_from(v).ok()),
            duration_probed: duration_probed_int != 0,
            last_position_ms: u64::try_from(last_pos_i64).unwrap_or(0),
            completed: completed_int != 0,
            file_size_bytes: size_i64.and_then(|v| u64::try_from(v).ok()),
            fingerprint: row.get("fingerprint")?,
            availability: Availability::from_db_str(&avail_str).unwrap_or(Availability::Missing),
            added_at: row.get("added_at")?,
            last_opened_at: row.get("last_opened_at")?,
            last_seen_at: row.get("last_seen_at")?,
            note_count: usize::try_from(note_count_i64).unwrap_or(0),
            source_folder_id,
        })
    }
    fn escape_like_pattern(raw: &str) -> String {
        let mut out = String::with_capacity(raw.len() + 4);
        out.push('%');
        for ch in raw.chars() {
            match ch {
                '\\' | '%' | '_' => {
                    out.push('\\');
                    out.push(ch);
                }
                c => out.push(c),
            }
        }
        out.push('%');
        out
    }
}

impl MediaRepository for SqliteMediaRepository {
    fn upsert_media(&self, item: &MediaItem) -> AppResult<MediaItem> {
        let clean_path = validate_local_media_path_str(&item.path)?.to_owned();
        self.db.with_conn(|conn| {
            let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let _ = Database::reconcile_upsert_media_in_tx(&tx, item, false)?;
            tx.commit()?;
            Self::load_by_path_conn(conn, &clean_path)?
                .ok_or_else(|| AppError::persistence("Upserted media record could not be reloaded"))
        })
    }

    fn get_by_id(&self, id: &str) -> AppResult<Option<MediaItem>> {
        self.db.with_conn(|conn| Self::load_by_id_conn(conn, id))
    }

    fn get_by_path(&self, path: &str) -> AppResult<Option<MediaItem>> {
        self.db
            .with_conn(|conn| Self::load_by_path_conn(conn, path))
    }

    fn list_media(&self, query: Option<&str>, filter: LibraryFilter) -> AppResult<Vec<MediaItem>> {
        self.db.with_conn(|conn| {
            let clean_query = query.map(str::trim).filter(|q| !q.is_empty());
            let kind_filter = match filter {
                LibraryFilter::All => None,
                LibraryFilter::Video => Some("video"),
                LibraryFilter::Audio => Some("audio"),
            };

            if let Some(q) = clean_query.filter(|q| !q.is_ascii()) {
                let needle = q.to_lowercase();
                let mut stmt = conn.prepare(
                    "SELECT m.*, (SELECT COUNT(*) FROM notes n WHERE n.media_id = m.id) AS note_count
                     FROM media m
                     WHERE (?1 IS NULL OR m.media_kind = ?1)
                     ORDER BY COALESCE(m.last_opened_at, m.added_at) DESC, m.display_name ASC",
                )?;
                let rows = stmt.query_map(params![kind_filter], Self::map_row)?;
                let mut candidates = Vec::new();
                for row in rows {
                    candidates.push(row?);
                }

                let mut matched_note_media_ids = std::collections::HashSet::new();
                if candidates
                    .iter()
                    .any(|item| !item.display_name.to_lowercase().contains(&needle))
                {
                    let mut note_stmt = conn.prepare(
                        "SELECT n.media_id, n.body
                         FROM notes n
                         INNER JOIN media m ON m.id = n.media_id
                         WHERE (?1 IS NULL OR m.media_kind = ?1)",
                    )?;
                    let note_rows = note_stmt.query_map(params![kind_filter], |r| {
                        Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
                    })?;
                    for note_res in note_rows {
                        let (mid, body) = note_res?;
                        if !matched_note_media_ids.contains(&mid)
                            && body.to_lowercase().contains(&needle)
                        {
                            matched_note_media_ids.insert(mid);
                        }
                    }
                }

                candidates.retain(|item| {
                    item.display_name.to_lowercase().contains(&needle)
                        || matched_note_media_ids.contains(&item.id)
                });
                return Ok(candidates);
            }

            let search_pattern = clean_query.map(|q| Self::escape_like_pattern(&q.to_lowercase()));
            let mut stmt = conn.prepare(
                "SELECT m.*, (SELECT COUNT(*) FROM notes n WHERE n.media_id = m.id) AS note_count
                 FROM media m
                 WHERE (?1 IS NULL OR LOWER(m.display_name) LIKE ?1 ESCAPE '\\' OR EXISTS (
                     SELECT 1 FROM notes n WHERE n.media_id = m.id AND LOWER(n.body) LIKE ?1 ESCAPE '\\'
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
            let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
            tx.execute(
                "UPDATE media
                 SET last_position_ms = ?2, completed = ?3, last_seen_at = ?4
                 WHERE id = ?1",
                params![
                    id,
                    i64::try_from(last_position_ms).unwrap_or(i64::MAX),
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
            let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
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
            let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let conflict: Option<i64> = tx
                .query_row(
                    "SELECT 1 FROM media WHERE path = ?1 AND id != ?2",
                    params![record.new_path, record.id],
                    |r| r.get(0),
                )
                .optional()?;
            if conflict.is_some() {
                return Err(AppError::relink_conflict(
                    "Target file path already belongs to another item in your library.",
                ));
            }
            let dur_param = record
                .duration_ms
                .map(|v| i64::try_from(v).unwrap_or(i64::MAX));
            let updated = tx.execute(
                "UPDATE media
                 SET path = ?2,
                     display_name = ?3,
                     media_kind = ?4,
                     duration_ms = ?5,
                     duration_probed = ?8,
                     last_position_ms = CASE
                         WHEN ?5 IS NOT NULL AND last_position_ms > ?5 THEN ?5
                         ELSE last_position_ms
                     END,
                     file_size_bytes = COALESCE(?6, file_size_bytes),
                     availability = 'available',
                     last_seen_at = ?7
                 WHERE id = ?1",
                params![
                    record.id,
                    record.new_path,
                    record.display_name,
                    record.media_kind.as_str(),
                    dur_param,
                    record
                        .file_size_bytes
                        .map(|v| i64::try_from(v).unwrap_or(i64::MAX)),
                    record.updated_at,
                    i64::from(record.duration_probed),
                ],
            )?;
            if updated == 0 {
                return Err(AppError::validation(format!(
                    "Media item '{}' not found",
                    record.id
                )));
            }
            tx.commit()?;
            Self::load_by_id_conn(conn, record.id)?
                .ok_or_else(|| AppError::persistence("Relinked media record could not be loaded"))
        })
    }

    fn delete_media(&self, id: &str) -> AppResult<bool> {
        self.db.with_conn(|conn| {
            let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let affected = tx.execute("DELETE FROM media WHERE id = ?1", params![id])?;
            tx.commit()?;
            Ok(affected > 0)
        })
    }
}
