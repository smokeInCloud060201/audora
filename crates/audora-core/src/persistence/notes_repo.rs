//! SQLite implementation of `NotesRepository`.

use rusqlite::{params, OptionalExtension, Row};

use crate::domain::{
    CreateNoteInput, LibraryFilter, Note, NoteId, NotesRepository, UpdateNoteInput,
};
use crate::error::{AppError, AppResult};
use crate::persistence::Database;

#[derive(Clone)]
pub struct SqliteNotesRepository {
    db: Database,
}

impl SqliteNotesRepository {
    #[must_use]
    pub const fn new(db: Database) -> Self {
        Self { db }
    }

    fn map_row(row: &Row<'_>) -> rusqlite::Result<Note> {
        let pos_i64: i64 = row.get("position_ms")?;
        let quick_marker_i64: i64 = row.get("is_quick_marker")?;
        Ok(Note {
            id: row.get("id")?,
            media_id: row.get("media_id")?,
            position_ms: u64::try_from(pos_i64).unwrap_or(0),
            is_quick_marker: quick_marker_i64 != 0,
            body: row.get("body")?,
            created_at: row.get("created_at")?,
            updated_at: row.get("updated_at")?,
        })
    }
}

impl NotesRepository for SqliteNotesRepository {
    fn create_note(&self, id: NoteId, input: &CreateNoteInput, now_iso: &str) -> AppResult<Note> {
        self.db.with_conn(|conn| {
            let tx = conn.transaction()?;
            tx.execute(
                "INSERT INTO notes (id, media_id, position_ms, is_quick_marker, body, created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![
                    id,
                    input.media_id,
                    i64::try_from(input.position_ms).unwrap_or(0),
                    i64::from(input.is_quick_marker),
                    input.body,
                    now_iso,
                    now_iso,
                ],
            )?;
            tx.commit()?;
            Ok(())
        })?;

        self.get_note(&id)?
            .ok_or_else(|| AppError::persistence("Created note could not be reloaded"))
    }

    fn get_note(&self, id: &str) -> AppResult<Option<Note>> {
        self.db.with_conn(|conn| {
            let note = conn
                .query_row(
                    "SELECT id, media_id, position_ms, is_quick_marker, body, created_at, updated_at
                     FROM notes WHERE id = ?1",
                    params![id],
                    Self::map_row,
                )
                .optional()?;
            Ok(note)
        })
    }

    fn update_note(&self, input: &UpdateNoteInput, now_iso: &str) -> AppResult<Note> {
        let trimmed = input.body.trim();
        let is_quick_marker = trimmed.is_empty();
        self.db.with_conn(|conn| {
            let tx = conn.transaction()?;
            let affected = tx.execute(
                "UPDATE notes
                 SET body = ?2, is_quick_marker = ?3, updated_at = ?4
                 WHERE id = ?1",
                params![input.id, trimmed, i64::from(is_quick_marker), now_iso],
            )?;
            if affected == 0 {
                return Err(AppError::validation(format!(
                    "Note '{}' does not exist",
                    input.id
                )));
            }
            tx.commit()?;
            Ok(())
        })?;

        self.get_note(&input.id)?
            .ok_or_else(|| AppError::persistence("Updated note could not be reloaded"))
    }

    fn delete_note(&self, id: &str) -> AppResult<bool> {
        self.db.with_conn(|conn| {
            let tx = conn.transaction()?;
            let affected = tx.execute("DELETE FROM notes WHERE id = ?1", params![id])?;
            tx.commit()?;
            Ok(affected > 0)
        })
    }

    fn list_notes_for_media(&self, media_id: &str) -> AppResult<Vec<Note>> {
        self.db.with_conn(|conn| {
            let mut stmt = conn.prepare(
                "SELECT id, media_id, position_ms, is_quick_marker, body, created_at, updated_at
                 FROM notes
                 WHERE media_id = ?1
                 ORDER BY position_ms ASC, created_at ASC",
            )?;
            let rows = stmt.query_map(params![media_id], Self::map_row)?;
            let mut notes = Vec::new();
            for row in rows {
                notes.push(row?);
            }
            Ok(notes)
        })
    }

    fn search_notes(&self, query: &str, filter: LibraryFilter) -> AppResult<Vec<Note>> {
        self.db.with_conn(|conn| {
            let trimmed = query.trim();
            if trimmed.is_empty() {
                return Ok(Vec::new());
            }
            let kind_filter = match filter {
                LibraryFilter::All => None,
                LibraryFilter::Video => Some("video"),
                LibraryFilter::Audio => Some("audio"),
            };

            if !trimmed.is_ascii() {
                let needle = trimmed.to_lowercase();
                let mut stmt = conn.prepare(
                    "SELECT n.id, n.media_id, n.position_ms, n.is_quick_marker, n.body, n.created_at, n.updated_at, m.display_name
                     FROM notes n
                     INNER JOIN media m ON m.id = n.media_id
                     WHERE (?1 IS NULL OR m.media_kind = ?1)
                     ORDER BY n.updated_at DESC, n.position_ms ASC",
                )?;
                let rows = stmt.query_map(params![kind_filter], |row| {
                    let note = Self::map_row(row)?;
                    let display_name: String = row.get("display_name")?;
                    Ok((note, display_name))
                })?;
                let mut notes = Vec::new();
                for row in rows {
                    let (note, display_name) = row?;
                    if note.body.to_lowercase().contains(&needle)
                        || display_name.to_lowercase().contains(&needle)
                    {
                        notes.push(note);
                    }
                }
                return Ok(notes);
            }

            let mut escaped = String::with_capacity(trimmed.len() + 4);
            escaped.push('%');
            for ch in trimmed.to_lowercase().chars() {
                match ch {
                    '\\' | '%' | '_' => {
                        escaped.push('\\');
                        escaped.push(ch);
                    }
                    c => escaped.push(c),
                }
            }
            escaped.push('%');

            let mut stmt = conn.prepare(
                "SELECT n.id, n.media_id, n.position_ms, n.is_quick_marker, n.body, n.created_at, n.updated_at
                 FROM notes n
                 INNER JOIN media m ON m.id = n.media_id
                 WHERE (LOWER(n.body) LIKE ?1 ESCAPE '\\' OR LOWER(m.display_name) LIKE ?1 ESCAPE '\\')
                   AND (?2 IS NULL OR m.media_kind = ?2)
                 ORDER BY n.updated_at DESC, n.position_ms ASC",
            )?;
            let rows = stmt.query_map(params![escaped, kind_filter], Self::map_row)?;
            let mut notes = Vec::new();
            for row in rows {
                notes.push(row?);
            }
            Ok(notes)
        })
    }
}
