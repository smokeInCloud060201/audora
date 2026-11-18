import React, { useRef, useState } from "react";
import { ipcClient } from "../api/ipc";
import type { AppErrorPayload, MediaItem, Playlist } from "../types/domain";
import { useModalFocusTrap } from "../utils/useModalFocusTrap";
import { IconCheck, IconClose, IconPlus } from "./Icons";

interface AddToPlaylistModalProps {
  mediaItem: MediaItem;
  playlists: Playlist[];
  onChanged: (message?: string) => void;
  onClose: () => void;
}

export function AddToPlaylistModal({
  mediaItem,
  playlists,
  onChanged,
  onClose,
}: AddToPlaylistModalProps): React.ReactElement {
  const [newPlaylistName, setNewPlaylistName] = useState<string>("");
  const [errorMessage, setErrorMessage] = useState<string | null>(null);
  const dialogRef = useRef<HTMLDivElement | null>(null);
  const nameInputRef = useRef<HTMLInputElement | null>(null);
  useModalFocusTrap(true, dialogRef, nameInputRef);

  const handleToggleMembership = (playlist: Playlist) => {
    setErrorMessage(null);
    const isMember = playlist.item_ids.includes(mediaItem.id);
    try {
      if (isMember) {
        ipcClient.removeMediaFromPlaylist(playlist.id, mediaItem.id);
        onChanged(`Removed from "${playlist.name}"`);
      } else {
        ipcClient.addMediaToPlaylist(playlist.id, [mediaItem.id]);
        onChanged(`Added to "${playlist.name}"`);
      }
    } catch (err) {
      const payload = err as AppErrorPayload;
      setErrorMessage(payload.message ?? "Unable to update playlist.");
    }
  };

  const handleCreateWithItem = (e: React.FormEvent) => {
    e.preventDefault();
    setErrorMessage(null);
    try {
      const created = ipcClient.createPlaylist({
        name: newPlaylistName,
        initial_item_ids: [mediaItem.id],
      });
      setNewPlaylistName("");
      onChanged(`Created playlist "${created.name}" with "${mediaItem.display_name}"`);
    } catch (err) {
      const payload = err as AppErrorPayload;
      setErrorMessage(payload.message ?? "Unable to create playlist.");
    }
  };

  return (
    <dialog
      className="modal-backdrop"
      aria-modal="true"
      aria-label="Add media to playlist"
      data-testid="add-to-playlist-modal"
    >
      <div ref={dialogRef} className="modal-dialog" style={{ width: 460 }}>
        <div
          style={{
            display: "flex",
            justifyContent: "space-between",
            alignItems: "flex-start",
            gap: 12,
          }}
        >
          <div style={{ minWidth: 0 }}>
            <h2 style={{ margin: 0, fontSize: 17 }}>Add to playlist</h2>
            <p
              style={{
                margin: "3px 0 0 0",
                fontSize: 12,
                color: "var(--text-tertiary)",
                overflow: "hidden",
                textOverflow: "ellipsis",
                whiteSpace: "nowrap",
              }}
            >
              {mediaItem.display_name}
            </p>
          </div>
          <button
            type="button"
            className="btn-secondary"
            style={{ width: 30, height: 30, padding: 0 }}
            aria-label="Close playlist modal"
            onClick={onClose}
          >
            <IconClose size={14} />
          </button>
        </div>

        <form onSubmit={handleCreateWithItem} style={{ display: "flex", gap: 8 }}>
          <input
            ref={nameInputRef}
            type="text"
            aria-label="New playlist name"
            placeholder="New playlist name…"
            value={newPlaylistName}
            onChange={(e) => setNewPlaylistName(e.target.value)}
            style={{
              flex: 1,
              padding: "8px 12px",
              borderRadius: 8,
              border: "1px solid var(--border-control)",
              background: "var(--bg-inset)",
              color: "var(--text-primary)",
              fontSize: 12.5,
            }}
          />
          <button type="submit" className="btn-primary">
            <IconPlus size={14} />
            <span>Create &amp; add</span>
          </button>
        </form>

        {errorMessage && (
          <div role="alert" style={{ fontSize: 12, color: "var(--status-error-text)" }}>
            {errorMessage}
          </div>
        )}

        <div className="playlist-picker-list" role="group" aria-label="Existing playlists">
          {playlists.length === 0 ? (
            <div className="source-picker-empty" style={{ padding: "22px 12px" }}>
              No playlists yet. Enter a name above to create your first playlist.
            </div>
          ) : (
            playlists.map((pl) => {
              const checked = pl.item_ids.includes(mediaItem.id);
              return (
                <button
                  key={pl.id}
                  type="button"
                  role="checkbox"
                  aria-checked={checked}
                  className="playlist-picker-row"
                  onClick={() => handleToggleMembership(pl)}
                >
                  <div style={{ minWidth: 0, flex: 1 }}>
                    <div
                      style={{
                        fontSize: 13,
                        fontWeight: 600,
                        color: "var(--text-primary)",
                        overflow: "hidden",
                        textOverflow: "ellipsis",
                        whiteSpace: "nowrap",
                      }}
                    >
                      {pl.name}
                    </div>
                    <div style={{ fontSize: 11, color: "var(--text-tertiary)" }}>
                      {pl.item_ids.length} {pl.item_ids.length === 1 ? "item" : "items"}
                    </div>
                  </div>
                  <span
                    className={`playlist-check-indicator ${checked ? "checked" : ""}`}
                    aria-hidden="true"
                  >
                    {checked ? <IconCheck size={13} /> : <IconPlus size={13} />}
                  </span>
                </button>
              );
            })
          )}
        </div>

        <div style={{ display: "flex", justifyContent: "flex-end" }}>
          <button type="button" className="btn-secondary" onClick={onClose}>
            Done
          </button>
        </div>
      </div>
    </dialog>
  );
}
