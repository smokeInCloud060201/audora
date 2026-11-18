//! SQLite implementation of `PlaylistRepository`.

use rusqlite::{params, Connection, OptionalExtension, TransactionBehavior};

use crate::domain::{
    CreatePlaylistInput, MediaId, Playlist, PlaylistId, PlaylistRepository, UpdatePlaylistInput,
};
use crate::error::{AppError, AppResult};
use crate::persistence::Database;

#[derive(Clone)]
pub struct SqlitePlaylistRepository {
    db: Database,
}

impl SqlitePlaylistRepository {
    #[must_use]
    pub const fn new(db: Database) -> Self {
        Self { db }
    }
}

fn load_playlist_items(conn: &Connection, playlist_id: &str) -> rusqlite::Result<Vec<MediaId>> {
    let mut stmt = conn.prepare(
        "SELECT media_id FROM playlist_items WHERE playlist_id = ?1 ORDER BY position ASC",
    )?;
    let rows = stmt.query_map(params![playlist_id], |row| row.get::<_, String>(0))?;
    let mut ids = Vec::new();
    for r in rows {
        ids.push(r?);
    }
    Ok(ids)
}

fn read_playlist_row(conn: &Connection, id: &str) -> AppResult<Option<Playlist>> {
    let row_opt: Option<(String, String, String, String, String)> = conn
        .query_row(
            "SELECT id, name, description, created_at, updated_at FROM playlists WHERE id = ?1",
            params![id],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                ))
            },
        )
        .optional()?;

    let Some((pid, name, description, created_at, updated_at)) = row_opt else {
        return Ok(None);
    };
    let item_ids = load_playlist_items(conn, &pid)?;
    Ok(Some(Playlist {
        id: pid,
        name,
        description,
        item_ids,
        created_at,
        updated_at,
    }))
}

impl PlaylistRepository for SqlitePlaylistRepository {
    fn create_playlist(
        &self,
        id: PlaylistId,
        input: &CreatePlaylistInput,
        now_iso: &str,
    ) -> AppResult<Playlist> {
        self.db.with_conn(|conn| {
            let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
            tx.execute(
                "INSERT INTO playlists (id, name, description, created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                params![id, input.name, input.description, now_iso, now_iso],
            )?;

            let mut seen = std::collections::HashSet::new();
            let mut pos = 0_i64;
            for media_id in &input.initial_item_ids {
                if !seen.insert(media_id.clone()) {
                    continue;
                }
                let exists: Option<i64> = tx
                    .query_row(
                        "SELECT 1 FROM media WHERE id = ?1",
                        params![media_id],
                        |r| r.get(0),
                    )
                    .optional()?;
                if exists.is_some() {
                    tx.execute(
                        "INSERT INTO playlist_items (playlist_id, media_id, position, added_at)
                         VALUES (?1, ?2, ?3, ?4)",
                        params![id, media_id, pos, now_iso],
                    )?;
                    pos = pos.saturating_add(1);
                }
            }

            tx.commit()?;
            read_playlist_row(conn, &id)?
                .ok_or_else(|| AppError::persistence("Created playlist could not be reloaded"))
        })
    }

    fn get_playlist(&self, id: &str) -> AppResult<Option<Playlist>> {
        self.db.with_conn(|conn| read_playlist_row(conn, id))
    }

    fn list_playlists(&self) -> AppResult<Vec<Playlist>> {
        self.db.with_conn(|conn| {
            let mut stmt = conn.prepare(
                "SELECT id, name, description, created_at, updated_at
                 FROM playlists
                 ORDER BY updated_at DESC, created_at DESC, id ASC",
            )?;
            let rows = stmt.query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                ))
            })?;

            let mut playlists = Vec::new();
            let mut index_by_id = std::collections::HashMap::new();
            for r in rows {
                let (pid, name, description, created_at, updated_at) = r?;
                index_by_id.insert(pid.clone(), playlists.len());
                playlists.push(Playlist {
                    id: pid,
                    name,
                    description,
                    item_ids: Vec::new(),
                    created_at,
                    updated_at,
                });
            }
            if !playlists.is_empty() {
                let mut item_stmt = conn.prepare(
                    "SELECT playlist_id, media_id
                     FROM playlist_items
                     ORDER BY playlist_id ASC, position ASC",
                )?;
                let item_rows = item_stmt.query_map([], |row| {
                    Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
                })?;
                for ir in item_rows {
                    let (pid, mid) = ir?;
                    if let Some(&idx) = index_by_id.get(&pid) {
                        playlists[idx].item_ids.push(mid);
                    }
                }
            }
            Ok(playlists)
        })
    }

    fn update_playlist(&self, input: &UpdatePlaylistInput, now_iso: &str) -> AppResult<Playlist> {
        self.db.with_conn(|conn| {
            let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let affected = tx.execute(
                "UPDATE playlists SET name = ?1, description = ?2, updated_at = ?3 WHERE id = ?4",
                params![input.name, input.description, now_iso, input.id],
            )?;
            if affected == 0 {
                return Err(AppError::validation("Selected playlist does not exist"));
            }
            tx.commit()?;
            read_playlist_row(conn, &input.id)?
                .ok_or_else(|| AppError::persistence("Updated playlist could not be reloaded"))
        })
    }

    fn delete_playlist(&self, id: &str) -> AppResult<bool> {
        self.db.with_conn(|conn| {
            let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let affected = tx.execute("DELETE FROM playlists WHERE id = ?1", params![id])?;
            tx.commit()?;
            Ok(affected > 0)
        })
    }

    fn add_items_to_playlist(
        &self,
        playlist_id: &str,
        media_ids: &[MediaId],
        now_iso: &str,
    ) -> AppResult<Playlist> {
        self.db.with_conn(|conn| {
            let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let exists: Option<i64> = tx
                .query_row(
                    "SELECT 1 FROM playlists WHERE id = ?1",
                    params![playlist_id],
                    |r| r.get(0),
                )
                .optional()?;
            if exists.is_none() {
                return Err(AppError::validation("Selected playlist does not exist"));
            }

            let max_pos: Option<i64> = tx.query_row(
                "SELECT MAX(position) FROM playlist_items WHERE playlist_id = ?1",
                params![playlist_id],
                |r| r.get(0),
            )?;
            let mut next_pos = max_pos.map_or(0_i64, |p| p.saturating_add(1));

            for media_id in media_ids {
                let media_exists: Option<i64> = tx
                    .query_row(
                        "SELECT 1 FROM media WHERE id = ?1",
                        params![media_id],
                        |r| r.get(0),
                    )
                    .optional()?;
                if media_exists.is_none() {
                    continue;
                }
                let inserted = tx.execute(
                    "INSERT OR IGNORE INTO playlist_items (playlist_id, media_id, position, added_at)
                     VALUES (?1, ?2, ?3, ?4)",
                    params![playlist_id, media_id, next_pos, now_iso],
                )?;
                if inserted > 0 {
                    next_pos = next_pos.saturating_add(1);
                }
            }

            tx.execute(
                "UPDATE playlists SET updated_at = ?1 WHERE id = ?2",
                params![now_iso, playlist_id],
            )?;
            tx.commit()?;
            read_playlist_row(conn, playlist_id)?
                .ok_or_else(|| AppError::persistence("Playlist could not be reloaded"))
        })
    }

    fn remove_item_from_playlist(
        &self,
        playlist_id: &str,
        media_id: &str,
        now_iso: &str,
    ) -> AppResult<Playlist> {
        self.db.with_conn(|conn| {
            let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
            tx.execute(
                "DELETE FROM playlist_items WHERE playlist_id = ?1 AND media_id = ?2",
                params![playlist_id, media_id],
            )?;
            let remaining = load_playlist_items(&tx, playlist_id)?;
            for (idx, mid) in remaining.iter().enumerate() {
                let pos = i64::try_from(idx).unwrap_or(i64::MAX);
                tx.execute(
                    "UPDATE playlist_items SET position = ?1 WHERE playlist_id = ?2 AND media_id = ?3",
                    params![pos, playlist_id, mid],
                )?;
            }
            tx.execute(
                "UPDATE playlists SET updated_at = ?1 WHERE id = ?2",
                params![now_iso, playlist_id],
            )?;
            tx.commit()?;
            read_playlist_row(conn, playlist_id)?
                .ok_or_else(|| AppError::validation("Selected playlist does not exist"))
        })
    }

    fn move_playlist_item(
        &self,
        playlist_id: &str,
        media_id: &str,
        target_index: usize,
        now_iso: &str,
    ) -> AppResult<Playlist> {
        self.db.with_conn(|conn| {
            let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let mut current = load_playlist_items(&tx, playlist_id)?;
            if let Some(curr_idx) = current.iter().position(|id| id == media_id) {
                let item = current.remove(curr_idx);
                let dest = target_index.min(current.len());
                current.insert(dest, item);
                for (idx, mid) in current.iter().enumerate() {
                    let pos = i64::try_from(idx).unwrap_or(i64::MAX);
                    tx.execute(
                        "UPDATE playlist_items SET position = ?1 WHERE playlist_id = ?2 AND media_id = ?3",
                        params![pos, playlist_id, mid],
                    )?;
                }
                tx.execute(
                    "UPDATE playlists SET updated_at = ?1 WHERE id = ?2",
                    params![now_iso, playlist_id],
                )?;
            }
            tx.commit()?;
            read_playlist_row(conn, playlist_id)?
                .ok_or_else(|| AppError::validation("Selected playlist does not exist"))
        })
    }
}
