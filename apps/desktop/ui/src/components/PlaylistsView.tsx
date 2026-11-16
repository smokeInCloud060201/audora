import React, { useEffect, useMemo, useState } from "react";
import { ipcClient } from "../api/ipc";
import type {
  AppErrorPayload,
  LibraryFilter,
  MediaItem,
  Playlist,
} from "../types/domain";
import { formatDurationMs, progressPercent } from "../utils/format";

interface PlaylistsViewProps {
  playlists: Playlist[];
  allMedia: MediaItem[];
  activeMediaId: string | null;
  selectedPlaylistId: string | null;
  onSelectPlaylist: (playlistId: string | null) => void;
  onPlayPlaylist: (
    playlistId: string,
    options?: { startMediaId?: string; shuffle?: boolean },
  ) => void;
  onAddToQueue: (mediaId: string) => void;
  onRelinkMissingMedia: (item: MediaItem) => void;
  onOpenLocalFiles: () => void;
  onPlaylistsMutated: (toastMessage?: string) => void;
  onPlaylistDeleted: (deleted: Playlist) => void;
}

export function PlaylistsView({
  playlists,
  allMedia,
  activeMediaId,
  selectedPlaylistId,
  onSelectPlaylist,
  onPlayPlaylist,
  onAddToQueue,
  onRelinkMissingMedia,
  onOpenLocalFiles,
  onPlaylistsMutated,
  onPlaylistDeleted,
}: PlaylistsViewProps): React.ReactElement {
  const [sidebarFilter, setSidebarFilter] = useState<string>("");

  // Create / Edit Playlist modal state
  const [composerMode, setComposerMode] = useState<"create" | "edit" | null>(null);
  const [nameDraft, setNameDraft] = useState<string>("");
  const [descDraft, setDescDraft] = useState<string>("");
  const [composerError, setComposerError] = useState<string | null>(null);

  // Add Media from Library modal state
  const [isAddMediaOpen, setIsAddMediaOpen] = useState<boolean>(false);
  const [pickerSearch, setPickerSearch] = useState<string>("");
  const [pickerFilter, setPickerFilter] = useState<LibraryFilter>("all");
  const [selectedMediaIds, setSelectedMediaIds] = useState<string[]>([]);

  useEffect(() => {
    if (playlists.length === 0) {
      if (selectedPlaylistId !== null) {
        onSelectPlaylist(null);
      }
      return;
    }
    if (!selectedPlaylistId || !playlists.some((p) => p.id === selectedPlaylistId)) {
      onSelectPlaylist(playlists[0]!.id);
    }
  }, [playlists, selectedPlaylistId, onSelectPlaylist]);

  const mediaMap = useMemo(() => {
    const map = new Map<string, MediaItem>();
    for (const m of allMedia) {
      map.set(m.id, m);
    }
    return map;
  }, [allMedia]);

  const filteredPlaylists = useMemo(() => {
    const q = sidebarFilter.trim().toLowerCase();
    if (!q) return playlists;
    return playlists.filter(
      (p) =>
        p.name.toLowerCase().includes(q) ||
        p.description.toLowerCase().includes(q),
    );
  }, [playlists, sidebarFilter]);

  const activePlaylist = useMemo(
    () =>
      playlists.find((p) => p.id === selectedPlaylistId) ??
      playlists[0] ??
      null,
    [playlists, selectedPlaylistId],
  );

  const activePlaylistItems = useMemo(() => {
    if (!activePlaylist) return [];
    return activePlaylist.item_ids
      .map((id) => mediaMap.get(id))
      .filter((m): m is MediaItem => Boolean(m));
  }, [activePlaylist, mediaMap]);

  const totalDurationMs = useMemo(
    () =>
      activePlaylistItems.reduce(
        (acc, item) => acc + (item.duration_ms ?? 0),
        0,
      ),
    [activePlaylistItems],
  );

  const openCreateModal = () => {
    setComposerMode("create");
    setNameDraft("");
    setDescDraft("");
    setComposerError(null);
  };

  const openEditModal = () => {
    if (!activePlaylist) return;
    setComposerMode("edit");
    setNameDraft(activePlaylist.name);
    setDescDraft(activePlaylist.description);
    setComposerError(null);
  };

  const handleSaveComposer = (e: React.FormEvent) => {
    e.preventDefault();
    setComposerError(null);
    try {
      if (composerMode === "create") {
        const created = ipcClient.createPlaylist({
          name: nameDraft,
          description: descDraft,
        });
        setComposerMode(null);
        onSelectPlaylist(created.id);
        onPlaylistsMutated(`Created playlist "${created.name}".`);
      } else if (composerMode === "edit" && activePlaylist) {
        const updated = ipcClient.updatePlaylist({
          id: activePlaylist.id,
          name: nameDraft,
          description: descDraft,
        });
        setComposerMode(null);
        onPlaylistsMutated(`Updated playlist "${updated.name}".`);
      }
    } catch (err) {
      const payload = err as AppErrorPayload;
      setComposerError(payload.message ?? "Unable to save playlist.");
    }
  };

  const handleDeleteActivePlaylist = () => {
    if (!activePlaylist) return;
    const removed = ipcClient.deletePlaylist(activePlaylist.id);
    if (removed) {
      onPlaylistDeleted(removed);
    }
  };

  const openAddMediaModal = () => {
    setPickerSearch("");
    setPickerFilter("all");
    setSelectedMediaIds([]);
    setIsAddMediaOpen(true);
  };

  const candidateLibraryItems = useMemo(() => {
    if (!activePlaylist) return [];
    const existingSet = new Set(activePlaylist.item_ids);
    const q = pickerSearch.trim().toLowerCase();
    return allMedia.filter((m) => {
      if (existingSet.has(m.id)) return false;
      if (pickerFilter !== "all" && m.media_kind !== pickerFilter) return false;
      if (q && !m.display_name.toLowerCase().includes(q)) return false;
      return true;
    });
  }, [activePlaylist, allMedia, pickerSearch, pickerFilter]);

  const toggleCandidateSelection = (mediaId: string) => {
    setSelectedMediaIds((prev) =>
      prev.includes(mediaId)
        ? prev.filter((id) => id !== mediaId)
        : [...prev, mediaId],
    );
  };

  const handleConfirmAddMedia = () => {
    if (!activePlaylist || selectedMediaIds.length === 0) {
      setIsAddMediaOpen(false);
      return;
    }
    ipcClient.addMediaToPlaylist(activePlaylist.id, selectedMediaIds);
    const count = selectedMediaIds.length;
    setIsAddMediaOpen(false);
    onPlaylistsMutated(
      `Added ${count} ${count === 1 ? "item" : "items"} to "${activePlaylist.name}".`,
    );
  };

  const handleMoveItem = (mediaId: string, nextIndex: number) => {
    if (!activePlaylist) return;
    ipcClient.movePlaylistItem(activePlaylist.id, mediaId, nextIndex);
    onPlaylistsMutated();
  };

  const handleRemoveTrack = (mediaId: string, displayName: string) => {
    if (!activePlaylist) return;
    ipcClient.removeMediaFromPlaylist(activePlaylist.id, mediaId);
    onPlaylistsMutated(`Removed "${displayName}" from "${activePlaylist.name}".`);
  };

  return (
    <div className="playlists-workspace" data-testid="playlists-workspace">
      {/* Left Sidebar: Playlists List */}
      <aside className="playlists-sidebar" aria-label="Playlists list">
        <div className="playlists-sidebar-header">
          <div>
            <h1 style={{ margin: 0, fontSize: 18 }}>Playlists</h1>
            <span style={{ fontSize: 11.5, color: "var(--text-tertiary)" }}>
              {playlists.length} {playlists.length === 1 ? "playlist" : "playlists"}
            </span>
          </div>
          <button
            type="button"
            className="btn-primary"
            data-testid="new-playlist-btn"
            onClick={openCreateModal}
          >
            ＋ New
          </button>
        </div>

        {playlists.length > 3 && (
          <div style={{ padding: "0 14px 10px" }}>
            <input
              type="search"
              aria-label="Filter playlists"
              placeholder="Filter playlists…"
              value={sidebarFilter}
              onChange={(e) => setSidebarFilter(e.target.value)}
              className="source-picker-filter"
            />
          </div>
        )}

        <div className="playlists-sidebar-list" role="listbox" aria-label="Saved playlists">
          {filteredPlaylists.length === 0 ? (
            <div
              style={{
                padding: "28px 14px",
                textAlign: "center",
                fontSize: 12.5,
                color: "var(--text-tertiary)",
              }}
            >
              {playlists.length === 0
                ? "No playlists yet."
                : "No playlists match your filter."}
            </div>
          ) : (
            filteredPlaylists.map((pl) => {
              const isSelected = activePlaylist?.id === pl.id;
              const plDuration = pl.item_ids.reduce(
                (sum, id) => sum + (mediaMap.get(id)?.duration_ms ?? 0),
                0,
              );
              return (
                <button
                  key={pl.id}
                  type="button"
                  role="option"
                  aria-selected={isSelected}
                  className="playlist-nav-card"
                  onClick={() => onSelectPlaylist(pl.id)}
                >
                  <div className="playlist-nav-glyph" aria-hidden="true">
                    ☰
                  </div>
                  <div style={{ minWidth: 0, flex: 1 }}>
                    <div className="playlist-nav-title">{pl.name}</div>
                    <div className="playlist-nav-meta tabular-nums">
                      {pl.item_ids.length} {pl.item_ids.length === 1 ? "item" : "items"}
                      {plDuration > 0 ? ` · ${formatDurationMs(plDuration)}` : ""}
                    </div>
                  </div>
                </button>
              );
            })
          )}
        </div>
      </aside>

      {/* Right Stage: Selected Playlist Detail or Empty State */}
      <div className="playlists-detail">
        {!activePlaylist ? (
          <div className="library-empty-box" data-testid="playlists-empty-state">
            <div style={{ fontSize: 32 }}>☰</div>
            <h2 style={{ margin: 0, fontSize: 20 }}>Create your first playlist</h2>
            <p
              style={{
                margin: 0,
                maxWidth: 440,
                color: "var(--text-secondary)",
                fontSize: 13,
              }}
            >
              Organize lectures, interviews, sound reels, or music into as many local playlists
              as you like. Deleting a playlist never touches your library or notes.
            </p>
            <button type="button" className="btn-primary" onClick={openCreateModal}>
              ＋ Create playlist
            </button>
          </div>
        ) : (
          <>
            <header className="playlist-hero">
              <div className="playlist-hero-left">
                <div className="playlist-hero-artwork" aria-hidden="true">
                  ☰
                </div>
                <div style={{ minWidth: 0 }}>
                  <span className="canvas-eyebrow" style={{ display: "block", marginBottom: 4 }}>
                    LOCAL PLAYLIST
                  </span>
                  <h2 className="playlist-hero-title">{activePlaylist.name}</h2>
                  {activePlaylist.description && (
                    <p className="playlist-hero-desc">{activePlaylist.description}</p>
                  )}
                  <div className="playlist-hero-stats tabular-nums">
                    {activePlaylistItems.length}{" "}
                    {activePlaylistItems.length === 1 ? "item" : "items"}
                    {totalDurationMs > 0 ? ` · ${formatDurationMs(totalDurationMs)}` : ""}
                  </div>
                </div>
              </div>

              <div className="playlist-hero-actions">
                <button
                  type="button"
                  className="btn-primary"
                  disabled={activePlaylistItems.every((m) => m.availability !== "available")}
                  onClick={() => onPlayPlaylist(activePlaylist.id, { shuffle: false })}
                >
                  ▶ Play
                </button>
                <button
                  type="button"
                  className="btn-secondary"
                  disabled={activePlaylistItems.every((m) => m.availability !== "available")}
                  onClick={() => onPlayPlaylist(activePlaylist.id, { shuffle: true })}
                >
                  ⇄ Shuffle
                </button>
                <button
                  type="button"
                  className="btn-secondary"
                  onClick={openAddMediaModal}
                >
                  ＋ Add media
                </button>
                <button
                  type="button"
                  className="btn-secondary"
                  onClick={openEditModal}
                >
                  Edit
                </button>
                <button
                  type="button"
                  className="btn-danger-subtle"
                  style={{ height: 34 }}
                  onClick={handleDeleteActivePlaylist}
                >
                  Delete
                </button>
              </div>
            </header>

            {activePlaylistItems.length === 0 ? (
              <div className="library-empty-box" style={{ height: 320 }}>
                <div style={{ fontSize: 26 }}>▣</div>
                <h3 style={{ margin: 0, fontSize: 17 }}>This playlist is empty</h3>
                <p
                  style={{
                    margin: 0,
                    maxWidth: 400,
                    color: "var(--text-secondary)",
                    fontSize: 13,
                  }}
                >
                  Add audio or video files from your library, or open local files from disk.
                </p>
                <div style={{ display: "flex", gap: 10, marginTop: 4 }}>
                  <button
                    type="button"
                    className="btn-primary"
                    onClick={openAddMediaModal}
                  >
                    ＋ Add from library
                  </button>
                  {allMedia.length === 0 && (
                    <button
                      type="button"
                      className="btn-secondary"
                      onClick={onOpenLocalFiles}
                    >
                      Choose local files…
                    </button>
                  )}
                </div>
              </div>
            ) : (
              <table className="library-table">
                <thead>
                  <tr>
                    <th style={{ width: 44 }}>#</th>
                    <th>Title</th>
                    <th>Progress</th>
                    <th>Duration</th>
                    <th style={{ textAlign: "right" }}>Actions</th>
                  </tr>
                </thead>
                <tbody>
                  {activePlaylistItems.map((item, idx) => {
                    const isMissing = item.availability === "missing";
                    const isPlayingItem = item.id === activeMediaId;
                    const pct = progressPercent(
                      item.last_position_ms,
                      item.duration_ms,
                    );
                    return (
                      <tr key={item.id} className="library-row">
                        <td className="tabular-nums" style={{ color: "var(--text-muted)" }}>
                          {isPlayingItem ? "▶" : idx + 1}
                        </td>
                        <td
                          onClick={() => {
                            if (isMissing) {
                              onRelinkMissingMedia(item);
                            } else {
                              onPlayPlaylist(activePlaylist.id, {
                                startMediaId: item.id,
                                shuffle: false,
                              });
                            }
                          }}
                        >
                          <div style={{ display: "flex", alignItems: "center", gap: 10 }}>
                            <span style={{ fontWeight: 600 }}>{item.display_name}</span>
                            <span
                              style={{
                                fontSize: 11,
                                color: "var(--text-tertiary)",
                                textTransform: "uppercase",
                              }}
                            >
                              {item.media_kind}
                            </span>
                            {item.note_count > 0 && (
                              <span className="timestamp-pill tabular-nums">
                                {item.note_count} {item.note_count === 1 ? "note" : "notes"}
                              </span>
                            )}
                            {isMissing && (
                              <span
                                style={{
                                  padding: "2px 8px",
                                  borderRadius: 999,
                                  background: "var(--status-error-bg)",
                                  color: "var(--status-error-text)",
                                  fontSize: 11,
                                  fontWeight: 600,
                                }}
                              >
                                Missing
                              </span>
                            )}
                          </div>
                        </td>
                        <td className="tabular-nums">
                          {item.completed ? "Completed" : pct > 0 ? `${pct}%` : "Not started"}
                        </td>
                        <td
                          className="tabular-nums"
                          style={{ color: "var(--text-tertiary)" }}
                        >
                          {formatDurationMs(item.duration_ms)}
                        </td>
                        <td style={{ textAlign: "right" }}>
                          <div
                            style={{
                              display: "inline-flex",
                              alignItems: "center",
                              gap: 6,
                            }}
                          >
                            <button
                              type="button"
                              className="btn-secondary"
                              style={{ height: 28, padding: "0 8px", fontSize: 11 }}
                              disabled={idx === 0}
                              aria-label={`Move ${item.display_name} up`}
                              title="Move up"
                              onClick={() => handleMoveItem(item.id, idx - 1)}
                            >
                              ↑
                            </button>
                            <button
                              type="button"
                              className="btn-secondary"
                              style={{ height: 28, padding: "0 8px", fontSize: 11 }}
                              disabled={idx === activePlaylistItems.length - 1}
                              aria-label={`Move ${item.display_name} down`}
                              title="Move down"
                              onClick={() => handleMoveItem(item.id, idx + 1)}
                            >
                              ↓
                            </button>
                            {!isMissing && (
                              <button
                                type="button"
                                className="btn-secondary"
                                style={{ height: 28, padding: "0 9px", fontSize: 11 }}
                                onClick={() => onAddToQueue(item.id)}
                              >
                                + Queue
                              </button>
                            )}
                            <button
                              type="button"
                              className="btn-danger-subtle"
                              style={{ height: 28, padding: "0 9px", fontSize: 11 }}
                              aria-label={`Remove ${item.display_name} from playlist`}
                              onClick={() => handleRemoveTrack(item.id, item.display_name)}
                            >
                              Remove
                            </button>
                          </div>
                        </td>
                      </tr>
                    );
                  })}
                </tbody>
              </table>
            )}
          </>
        )}
      </div>

      {/* Create / Edit Playlist Modal */}
      {composerMode && (
        <div
          className="modal-backdrop"
          role="dialog"
          aria-modal="true"
          aria-label={composerMode === "create" ? "Create playlist" : "Edit playlist"}
          data-testid="playlist-composer-modal"
        >
          <form className="modal-dialog" onSubmit={handleSaveComposer}>
            <div style={{ display: "flex", justifyContent: "space-between", alignItems: "center" }}>
              <h2 style={{ margin: 0, fontSize: 17 }}>
                {composerMode === "create" ? "New playlist" : "Edit playlist details"}
              </h2>
              <button
                type="button"
                className="btn-secondary"
                style={{ width: 30, height: 30, padding: 0 }}
                onClick={() => setComposerMode(null)}
              >
                ✕
              </button>
            </div>

            <div style={{ display: "flex", flexDirection: "column", gap: 6 }}>
              <label
                htmlFor="playlist-name-input"
                style={{ fontSize: 12, fontWeight: 600, color: "var(--text-secondary)" }}
              >
                Name
              </label>
              <input
                id="playlist-name-input"
                type="text"
                autoFocus
                placeholder="e.g. Sound Design Masterclass"
                value={nameDraft}
                onChange={(e) => setNameDraft(e.target.value)}
                style={{
                  width: "100%",
                  padding: "10px 12px",
                  borderRadius: 8,
                  border: "1px solid var(--border-control)",
                  background: "var(--bg-inset)",
                  color: "var(--text-primary)",
                  fontSize: 13,
                }}
              />
            </div>

            <div style={{ display: "flex", flexDirection: "column", gap: 6 }}>
              <label
                htmlFor="playlist-desc-input"
                style={{ fontSize: 12, fontWeight: 600, color: "var(--text-secondary)" }}
              >
                Description (optional)
              </label>
              <textarea
                id="playlist-desc-input"
                className="note-textarea"
                style={{ minHeight: 74 }}
                placeholder="Notes or context about this collection…"
                value={descDraft}
                onChange={(e) => setDescDraft(e.target.value)}
                onKeyDown={(e) => {
                  if ((e.metaKey || e.ctrlKey) && e.key === "Enter") {
                    handleSaveComposer(e);
                  }
                }}
              />
            </div>

            {composerError && (
              <div role="alert" style={{ fontSize: 12, color: "var(--status-error-text)" }}>
                {composerError}
              </div>
            )}

            <div style={{ display: "flex", justifyContent: "flex-end", gap: 10 }}>
              <button
                type="button"
                className="btn-secondary"
                onClick={() => setComposerMode(null)}
              >
                Cancel
              </button>
              <button type="submit" className="btn-primary">
                {composerMode === "create" ? "Create playlist" : "Save changes"}
              </button>
            </div>
          </form>
        </div>
      )}

      {/* Add Media from Library Multi-Select Modal */}
      {isAddMediaOpen && activePlaylist && (
        <div
          className="modal-backdrop"
          role="dialog"
          aria-modal="true"
          aria-label="Add media from library"
          data-testid="playlist-add-media-modal"
        >
          <div className="source-picker-dialog" style={{ width: 540 }}>
            <div className="source-picker-header">
              <div>
                <h2 style={{ margin: 0, fontSize: 16 }}>
                  Add media to “{activePlaylist.name}”
                </h2>
                <p style={{ margin: "2px 0 0 0", fontSize: 12, color: "var(--text-tertiary)" }}>
                  Select one or more items from your local library
                </p>
              </div>
              <button
                type="button"
                className="btn-secondary"
                style={{ width: 30, height: 30, padding: 0 }}
                onClick={() => setIsAddMediaOpen(false)}
              >
                ✕
              </button>
            </div>

            <div style={{ display: "flex", gap: 8, alignItems: "center" }}>
              <input
                type="search"
                aria-label="Search library items"
                placeholder="Search library…"
                value={pickerSearch}
                onChange={(e) => setPickerSearch(e.target.value)}
                className="source-picker-filter"
                style={{ flex: 1 }}
              />
              {(["all", "video", "audio"] as LibraryFilter[]).map((f) => (
                <button
                  key={f}
                  type="button"
                  className="filter-pill"
                  aria-pressed={pickerFilter === f}
                  onClick={() => setPickerFilter(f)}
                >
                  {f.charAt(0).toUpperCase() + f.slice(1)}
                </button>
              ))}
            </div>

            <div className="playlist-picker-list">
              {candidateLibraryItems.length === 0 ? (
                <div className="source-picker-empty" style={{ padding: "24px 12px" }}>
                  {allMedia.length === 0
                    ? "Your library has no media files yet."
                    : "All matching library items are already in this playlist."}
                </div>
              ) : (
                candidateLibraryItems.map((item) => {
                  const checked = selectedMediaIds.includes(item.id);
                  return (
                    <button
                      key={item.id}
                      type="button"
                      role="checkbox"
                      aria-checked={checked}
                      className="playlist-picker-row"
                      onClick={() => toggleCandidateSelection(item.id)}
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
                          {item.display_name}
                        </div>
                        <div
                          className="tabular-nums"
                          style={{ fontSize: 11, color: "var(--text-tertiary)" }}
                        >
                          {item.media_kind.toUpperCase()} ·{" "}
                          {formatDurationMs(item.duration_ms)}
                        </div>
                      </div>
                      <span
                        className={`playlist-check-indicator ${checked ? "checked" : ""}`}
                        aria-hidden="true"
                      >
                        {checked ? "✓" : "＋"}
                      </span>
                    </button>
                  );
                })
              )}
            </div>

            <div className="source-picker-footer">
              <span style={{ fontSize: 12, color: "var(--text-tertiary)" }}>
                {selectedMediaIds.length} selected
              </span>
              <div style={{ display: "flex", gap: 10 }}>
                <button
                  type="button"
                  className="btn-secondary"
                  onClick={() => setIsAddMediaOpen(false)}
                >
                  Cancel
                </button>
                <button
                  type="button"
                  className="btn-primary"
                  disabled={selectedMediaIds.length === 0}
                  onClick={handleConfirmAddMedia}
                >
                  Add {selectedMediaIds.length > 0 ? `(${selectedMediaIds.length})` : ""}
                </button>
              </div>
            </div>
          </div>
        </div>
      )}
    </div>
  );
}
