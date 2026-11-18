import React, { useEffect, useMemo, useRef, useState } from "react";
import { createPortal } from "react-dom";
import { ipcClient } from "../api/ipc";
import type {
  AppErrorPayload,
  LibraryFilter,
  MediaItem,
  Playlist,
  TimestampPrecision,
} from "../types/domain";
import { formatDurationMs } from "../utils/format";
import { useModalFocusTrap } from "../utils/useModalFocusTrap";
import {
  IconArrowDown,
  IconArrowUp,
  IconCheck,
  IconChevronLeft,
  IconClose,
  IconCollapseLeft,
  IconEdit,
  IconPlay,
  IconPlaylists,
  IconPlus,
  IconShuffle,
  IconTrash,
} from "./Icons";

function activePlaylistSafe(
  playlists: Playlist[],
  selectedPlaylistId: string | null,
): Playlist | null {
  return selectedPlaylistId
    ? (playlists.find((p) => p.id === selectedPlaylistId) ?? null)
    : null;
}

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
  onRelinkMissingMedia: (item: MediaItem) => void;
  onOpenLocalFiles: () => void;
  onPlaylistsMutated: (toastMessage?: string) => void;
  onPlaylistDeleted: (deleted: Playlist) => void;
  onCollapse: () => void;
  timestampPrecision: TimestampPrecision;
  onModalOpenChange?: (open: boolean) => void;
}

export const PlaylistsView = React.memo(function PlaylistsView({
  playlists,
  allMedia,
  activeMediaId,
  selectedPlaylistId,
  onSelectPlaylist,
  onPlayPlaylist,
  onRelinkMissingMedia,
  onOpenLocalFiles,
  onPlaylistsMutated,
  onPlaylistDeleted,
  onCollapse,
  timestampPrecision,
  onModalOpenChange,
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

  const composerDialogRef = useRef<HTMLFormElement | null>(null);
  const composerNameRef = useRef<HTMLInputElement | null>(null);
  const addMediaDialogRef = useRef<HTMLDivElement | null>(null);
  const addMediaSearchRef = useRef<HTMLInputElement | null>(null);
  const hasOpenPlaylistModal =
    Boolean(composerMode) ||
    (isAddMediaOpen && Boolean(activePlaylistSafe(playlists, selectedPlaylistId)));
  useModalFocusTrap(Boolean(composerMode), composerDialogRef, composerNameRef);
  useModalFocusTrap(isAddMediaOpen && Boolean(activePlaylistSafe(playlists, selectedPlaylistId)), addMediaDialogRef, addMediaSearchRef);

  useEffect(() => {
    onModalOpenChange?.(hasOpenPlaylistModal);
    return () => {
      onModalOpenChange?.(false);
    };
  }, [hasOpenPlaylistModal, onModalOpenChange]);

  useEffect(() => {
    if (!composerMode && !isAddMediaOpen) return undefined;
    const onKeyDown = (e: KeyboardEvent) => {
      if (e.key === "Escape") {
        e.stopPropagation();
        if (isAddMediaOpen) {
          setIsAddMediaOpen(false);
        } else if (composerMode) {
          setComposerMode(null);
        }
      }
    };
    window.addEventListener("keydown", onKeyDown, true);
    return () => window.removeEventListener("keydown", onKeyDown, true);
  }, [composerMode, isAddMediaOpen]);

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
      selectedPlaylistId
        ? (playlists.find((p) => p.id === selectedPlaylistId) ?? null)
        : null,
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
      onSelectPlaylist(null);
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
    <>
      <aside
        className="playlists-drawer"
        aria-label="Playlists sidebar"
        data-testid="playlists-drawer"
      >
        {/* 49px Top Bar aligned with Right Queue/Notes Bar */}
        <div className="playlists-drawer-topbar">
          {activePlaylist ? (
            <button
              type="button"
              className="jump-link-btn"
              style={{ fontSize: 12 }}
              onClick={() => onSelectPlaylist(null)}
            >
              <IconChevronLeft size={14} />
              <span>All playlists</span>
            </button>
          ) : (
            <span
              className="playlists-drawer-heading"
              style={{ display: "inline-flex", alignItems: "center", gap: 6 }}
            >
              <IconPlaylists size={14} />
              <span>Playlists ({playlists.length})</span>
            </span>
          )}

          <div style={{ display: "flex", alignItems: "center", gap: 6 }}>
            {!activePlaylist && (
              <button
                type="button"
                className="panel-icon-btn"
                data-testid="new-playlist-btn"
                aria-label="Create new playlist"
                title="Create new playlist"
                onClick={openCreateModal}
              >
                <IconPlus size={14} />
              </button>
            )}
            <button
              type="button"
              className="panel-icon-btn"
              aria-label="Hide playlists bar"
              title="Hide playlists bar ([)"
              onClick={onCollapse}
            >
              <IconCollapseLeft size={14} />
            </button>
          </div>
        </div>

        {!activePlaylist ? (
          /* Mode 1: All Playlists List */
          <>
            {playlists.length > 3 && (
              <div style={{ padding: "10px 10px 4px" }}>
                <input
                  type="search"
                  aria-label="Filter playlists"
                  placeholder="Filter playlists…"
                  value={sidebarFilter}
                  onChange={(e) => setSidebarFilter(e.target.value)}
                  className="source-picker-filter"
                  style={{ padding: "6px 10px", fontSize: 12 }}
                />
              </div>
            )}

            <div className="panel-scroll-list" style={{ paddingTop: 10 }}>
              {filteredPlaylists.length === 0 ? (
                <div className="playlists-drawer-empty">
                  <div style={{ color: "var(--accent-primary)" }}>
                    <IconPlaylists size={24} />
                  </div>
                  <div style={{ fontSize: 13, fontWeight: 600, color: "var(--text-primary)" }}>
                    {playlists.length === 0 ? "No playlists yet" : "No matches"}
                  </div>
                  <p style={{ margin: 0, fontSize: 12, color: "var(--text-tertiary)" }}>
                    {playlists.length === 0
                      ? "Create playlists to organize your audio and video files."
                      : "Try a different filter."}
                  </p>
                  {playlists.length === 0 && (
                    <button
                      type="button"
                      className="btn-primary"
                      style={{ height: 30, fontSize: 12, marginTop: 4 }}
                      onClick={openCreateModal}
                    >
                      <IconPlus size={13} />
                      <span>New playlist</span>
                    </button>
                  )}
                </div>
              ) : (
                filteredPlaylists.map((pl) => {
                  const plDuration = pl.item_ids.reduce(
                    (sum, id) => sum + (mediaMap.get(id)?.duration_ms ?? 0),
                    0,
                  );
                  const hasPlayable = pl.item_ids.some(
                    (id) => mediaMap.get(id)?.availability === "available",
                  );
                  const containsActive =
                    Boolean(activeMediaId) && pl.item_ids.includes(activeMediaId!);
                  return (
                    <div
                      key={pl.id}
                      className={`playlist-nav-card ${containsActive ? "active-context" : ""}`}
                    >
                      <button
                        type="button"
                        className="playlist-nav-main-btn"
                        onClick={() => onSelectPlaylist(pl.id)}
                      >
                        <div className="playlist-nav-glyph" aria-hidden="true">
                          <IconPlaylists size={16} />
                        </div>
                        <div style={{ minWidth: 0, flex: 1 }}>
                          <div className="playlist-nav-title">{pl.name}</div>
                          <div className="playlist-nav-meta tabular-nums">
                            {pl.item_ids.length}{" "}
                            {pl.item_ids.length === 1 ? "item" : "items"}
                            {plDuration > 0
                              ? ` · ${formatDurationMs(plDuration, timestampPrecision)}`
                              : ""}
                          </div>
                        </div>
                      </button>

                      {hasPlayable && (
                        <button
                          type="button"
                          className="playlist-quick-play-btn"
                          aria-label={`Play ${pl.name}`}
                          title={`Play ${pl.name}`}
                          onClick={(e) => {
                            e.stopPropagation();
                            onPlayPlaylist(pl.id, { shuffle: false });
                          }}
                        >
                          <IconPlay size={13} />
                        </button>
                      )}
                    </div>
                  );
                })
              )}
            </div>
          </>
        ) : (
          /* Mode 2: Selected Playlist Detail & Track List */
          <>
            <div className="playlist-drawer-hero">
              <div className="playlist-drawer-title-row">
                <div className="playlist-drawer-title" title={activePlaylist.name}>
                  {activePlaylist.name}
                </div>
                <div className="playlist-drawer-title-actions">
                  <button
                    type="button"
                    className="panel-icon-btn"
                    aria-label="Edit playlist"
                    title="Edit playlist"
                    onClick={openEditModal}
                  >
                    <IconEdit size={14} />
                  </button>
                  <button
                    type="button"
                    className="panel-icon-btn danger"
                    aria-label="Delete playlist"
                    title="Delete playlist"
                    onClick={handleDeleteActivePlaylist}
                  >
                    <IconTrash size={14} />
                  </button>
                </div>
              </div>
              {activePlaylist.description && (
                <p className="playlist-drawer-desc">{activePlaylist.description}</p>
              )}
              <div className="playlist-nav-meta tabular-nums">
                {activePlaylistItems.length}{" "}
                {activePlaylistItems.length === 1 ? "item" : "items"}
                {totalDurationMs > 0
                  ? ` · ${formatDurationMs(totalDurationMs, timestampPrecision)}`
                  : ""}
              </div>

              <div className="playlist-drawer-actions">
                <button
                  type="button"
                  className="btn-primary"
                  style={{ height: 30, padding: "0 11px", fontSize: 12 }}
                  disabled={activePlaylistItems.every(
                    (m) => m.availability !== "available",
                  )}
                  onClick={() => onPlayPlaylist(activePlaylist.id, { shuffle: false })}
                >
                  <IconPlay size={13} />
                  <span>Play</span>
                </button>
                <button
                  type="button"
                  className="btn-secondary"
                  style={{ height: 30, padding: "0 9px", fontSize: 11.5 }}
                  disabled={activePlaylistItems.every(
                    (m) => m.availability !== "available",
                  )}
                  onClick={() => onPlayPlaylist(activePlaylist.id, { shuffle: true })}
                >
                  <IconShuffle size={13} />
                  <span>Shuffle</span>
                </button>
                <button
                  type="button"
                  className="btn-secondary"
                  style={{ height: 30, padding: "0 9px", fontSize: 11.5 }}
                  onClick={openAddMediaModal}
                >
                  <IconPlus size={13} />
                  <span>Add</span>
                </button>
              </div>
            </div>

            <div className="panel-scroll-list">
              {activePlaylistItems.length === 0 ? (
                <div className="playlists-drawer-empty">
                  <div style={{ fontSize: 12.5, color: "var(--text-tertiary)" }}>
                    This playlist is empty.
                  </div>
                  <button
                    type="button"
                    className="btn-primary"
                    style={{ height: 30, fontSize: 12 }}
                    onClick={openAddMediaModal}
                  >
                    <IconPlus size={13} />
                    <span>Add from library</span>
                  </button>
                  {allMedia.length === 0 && (
                    <button
                      type="button"
                      className="btn-secondary"
                      style={{ height: 28, fontSize: 11.5 }}
                      onClick={onOpenLocalFiles}
                    >
                      Choose local files…
                    </button>
                  )}
                </div>
              ) : (
                activePlaylistItems.map((item, idx) => {
                  const isMissing = item.availability === "missing";
                  const isPlayingItem = item.id === activeMediaId;
                  return (
                    <div
                      key={item.id}
                      className="note-card"
                      style={{
                        padding: "9px 10px",
                        gap: 6,
                        borderColor: isPlayingItem
                          ? "var(--accent-primary)"
                          : "var(--border-control)",
                        background: isPlayingItem
                          ? "var(--accent-surface)"
                          : "var(--bg-surface-2)",
                      }}
                    >
                      <div
                        style={{
                          display: "flex",
                          alignItems: "center",
                          justifyContent: "space-between",
                          gap: 6,
                        }}
                      >
                        <button
                          type="button"
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
                          style={{
                            border: "none",
                            background: "transparent",
                            color: "inherit",
                            textAlign: "left",
                            padding: 0,
                            cursor: "pointer",
                            minWidth: 0,
                            flex: 1,
                          }}
                        >
                          <div
                            style={{
                              display: "flex",
                              alignItems: "center",
                              gap: 5,
                              fontSize: 12.5,
                              fontWeight: 600,
                              overflow: "hidden",
                              textOverflow: "ellipsis",
                              whiteSpace: "nowrap",
                            }}
                          >
                            {isPlayingItem ? <IconPlay size={12} /> : <span>{idx + 1}.</span>}
                            <span
                              style={{
                                overflow: "hidden",
                                textOverflow: "ellipsis",
                                whiteSpace: "nowrap",
                              }}
                            >
                              {item.display_name}
                            </span>
                          </div>
                          <div
                            className="tabular-nums"
                            style={{ fontSize: 11, color: "var(--text-tertiary)" }}
                          >
                            {item.media_kind === "video" ? "Video" : "Audio"} ·{" "}
                            {formatDurationMs(item.duration_ms, timestampPrecision)}
                            {isMissing ? " · Missing" : ""}
                          </div>
                        </button>

                        <div style={{ display: "flex", alignItems: "center", gap: 3 }}>
                          <button
                            type="button"
                            className="panel-icon-btn"
                            style={{ width: 22, height: 22 }}
                            disabled={idx === 0}
                            aria-label={`Move ${item.display_name} up`}
                            title="Move up"
                            onClick={() => handleMoveItem(item.id, idx - 1)}
                          >
                            <IconArrowUp size={12} />
                          </button>
                          <button
                            type="button"
                            className="panel-icon-btn"
                            style={{ width: 22, height: 22 }}
                            disabled={idx === activePlaylistItems.length - 1}
                            aria-label={`Move ${item.display_name} down`}
                            title="Move down"
                            onClick={() => handleMoveItem(item.id, idx + 1)}
                          >
                            <IconArrowDown size={12} />
                          </button>
                          <button
                            type="button"
                            className="panel-icon-btn"
                            style={{ width: 22, height: 22 }}
                            aria-label={`Remove ${item.display_name} from playlist`}
                            title="Remove from playlist"
                            onClick={() =>
                              handleRemoveTrack(item.id, item.display_name)
                            }
                          >
                            <IconClose size={12} />
                          </button>
                        </div>
                      </div>
                    </div>
                  );
                })
              )}
            </div>
          </>
        )}
      </aside>

      {/* Create / Edit Playlist Modal */}
      {composerMode &&
        typeof document !== "undefined" &&
        createPortal(
          <div
            className="modal-backdrop"
            role="dialog"
            aria-modal="true"
            aria-label={composerMode === "create" ? "Create playlist" : "Edit playlist"}
            data-testid="playlist-composer-modal"
          >
            <form ref={composerDialogRef} className="modal-dialog" onSubmit={handleSaveComposer}>
              <div style={{ display: "flex", justifyContent: "space-between", alignItems: "center" }}>
                <h2 style={{ margin: 0, fontSize: 17 }}>
                  {composerMode === "create" ? "New playlist" : "Edit playlist details"}
                </h2>
                <button
                  type="button"
                  className="btn-secondary"
                  style={{ width: 30, height: 30, padding: 0 }}
                  aria-label="Close modal"
                  onClick={() => setComposerMode(null)}
                >
                  <IconClose size={14} />
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
                  ref={composerNameRef}
                  id="playlist-name-input"
                  type="text"
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
          </div>,
          document.body,
        )}

      {/* Add Media from Library Multi-Select Modal */}
      {isAddMediaOpen &&
        activePlaylist &&
        typeof document !== "undefined" &&
        createPortal(
          <div
            className="modal-backdrop"
            role="dialog"
            aria-modal="true"
            aria-label="Add media from library"
            data-testid="playlist-add-media-modal"
          >
            <div ref={addMediaDialogRef} className="source-picker-dialog" style={{ width: 540 }}>
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
                  aria-label="Close modal"
                  onClick={() => setIsAddMediaOpen(false)}
                >
                  <IconClose size={14} />
                </button>
              </div>

              <div style={{ display: "flex", gap: 8, alignItems: "center" }}>
                <input
                  ref={addMediaSearchRef}
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

              <div className="playlist-picker-list" role="group" aria-label="Library media items">
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
                            {formatDurationMs(item.duration_ms, timestampPrecision)}
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
          </div>,
          document.body,
        )}
    </>
  );
});
