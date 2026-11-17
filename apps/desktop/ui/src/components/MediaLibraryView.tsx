import React, { useMemo } from "react";
import { ipcClient } from "../api/ipc";
import type {
  AppErrorPayload,
  LibraryFilter,
  LibrarySearchResult,
  MediaItem,
  Note,
  PlaybackSnapshot,
  TimestampPrecision,
} from "../types/domain";
import {
  computeProgressPercent,
  formatDurationMs,
  formatHumanDate,
} from "../utils/format";
import {
  IconArrowRight,
  IconFolderOpen,
  IconMoreHorizontal,
  IconPlay,
  IconPlus,
} from "./Icons";

interface MediaLibraryViewProps {
  allMedia: MediaItem[];
  libraryResult: LibrarySearchResult;
  libraryFilter: LibraryFilter;
  onSetLibraryFilter: (filter: LibraryFilter) => void;
  searchQuery: string;
  onSetSearchQuery: (query: string) => void;
  searchInputRef: React.RefObject<HTMLInputElement | null>;
  sourceFolderCount: number;
  onOpenLocalSourcesSettings: () => void;
  onOpenMediaPicker: () => void;
  onSelectLibraryItem: (item: MediaItem, autoPlay?: boolean) => void;
  onJumpToNote: (noteId: string) => void;
  onAddToPlaylist: (item: MediaItem) => void;
  onUpdateSnapshot: (next: PlaybackSnapshot) => void;
  onBumpLibraryVersion: () => void;
  openLibraryMenuId: string | null;
  onSetOpenLibraryMenuId: React.Dispatch<React.SetStateAction<string | null>>;
  libraryMenuPlacement: "down" | "up";
  onSetLibraryMenuPlacement: (placement: "down" | "up") => void;
  missingMediaTarget: MediaItem | null;
  missingTargetNotes: Note[];
  relinkPathDraft: string;
  onSetRelinkPathDraft: (draft: string) => void;
  relinkError: AppErrorPayload | null;
  onAttemptRelink: (mediaId: string, newPath: string, confirmMismatch?: boolean) => void;
  onBrowseRelinkFile: (mediaId: string) => void;
  onClearMissingTarget: () => void;
  timestampPrecision: TimestampPrecision;
}

export const MediaLibraryView = React.memo(function MediaLibraryView({
  allMedia,
  libraryResult,
  libraryFilter,
  onSetLibraryFilter,
  searchQuery,
  onSetSearchQuery,
  searchInputRef,
  sourceFolderCount,
  onOpenLocalSourcesSettings,
  onOpenMediaPicker,
  onSelectLibraryItem,
  onJumpToNote,
  onAddToPlaylist,
  onUpdateSnapshot,
  onBumpLibraryVersion,
  openLibraryMenuId,
  onSetOpenLibraryMenuId,
  libraryMenuPlacement,
  onSetLibraryMenuPlacement,
  missingMediaTarget,
  missingTargetNotes,
  relinkPathDraft,
  onSetRelinkPathDraft,
  relinkError,
  onAttemptRelink,
  onBrowseRelinkFile,
  onClearMissingTarget,
  timestampPrecision,
}: MediaLibraryViewProps): React.ReactElement {
  const mediaById = useMemo(() => {
    const map = new Map<string, MediaItem>();
    for (const m of allMedia) {
      map.set(m.id, m);
    }
    return map;
  }, [allMedia]);

  return (
    <div className="content-page library-page">
      {missingMediaTarget ? (
        <div
          style={{
            maxWidth: 880,
            margin: "24px auto",
            padding: 32,
            borderRadius: 14,
            background: "var(--bg-surface-1)",
            border: "1px solid var(--border-control)",
            display: "grid",
            gridTemplateColumns: "1.2fr 1fr",
            gap: 28,
          }}
        >
          <div>
            <span
              style={{
                display: "inline-block",
                padding: "4px 10px",
                borderRadius: 6,
                background: "var(--status-error-bg)",
                color: "var(--status-error-text)",
                fontSize: 12,
                fontWeight: 600,
                marginBottom: 12,
              }}
            >
              ! File unavailable (AV-101)
            </span>
            <h1 style={{ margin: "0 0 8px 0", fontSize: 22 }}>We can’t find this file</h1>
            <p
              style={{
                margin: "0 0 18px 0",
                fontSize: 13,
                color: "var(--text-secondary)",
              }}
            >
              It may have been moved, renamed, or stored on a disconnected drive.
            </p>
            <div
              style={{
                padding: 12,
                borderRadius: 8,
                background: "var(--bg-inset)",
                border: "1px solid var(--border-control)",
                fontSize: 12,
                marginBottom: 16,
              }}
            >
              <div style={{ color: "var(--text-tertiary)", marginBottom: 4 }}>
                Last known location
              </div>
              <code>{missingMediaTarget.path}</code>
            </div>

            <label
              htmlFor="relink-path-input"
              style={{ display: "block", fontSize: 12, marginBottom: 6 }}
            >
              Relocated file path
            </label>
            <input
              id="relink-path-input"
              type="text"
              value={relinkPathDraft}
              onChange={(e) => onSetRelinkPathDraft(e.target.value)}
              style={{
                width: "100%",
                padding: "8px 12px",
                borderRadius: 8,
                border: "1px solid var(--border-control)",
                background: "var(--bg-inset)",
                color: "var(--text-primary)",
                marginBottom: 12,
              }}
            />

            {relinkError && (
              <div
                role="alert"
                style={{
                  padding: "8px 12px",
                  borderRadius: 8,
                  background: "var(--status-error-bg)",
                  color: "var(--status-error-text)",
                  fontSize: 12,
                  marginBottom: 12,
                }}
              >
                <strong>{relinkError.code}:</strong> {relinkError.message}
              </div>
            )}

            <div style={{ display: "flex", flexWrap: "wrap", gap: 10 }}>
              <button
                type="button"
                className="btn-primary"
                onClick={() => onBrowseRelinkFile(missingMediaTarget.id)}
              >
                <IconFolderOpen size={14} />
                <span>Locate file…</span>
              </button>
              <button
                type="button"
                className="btn-secondary"
                onClick={() =>
                  onAttemptRelink(missingMediaTarget.id, relinkPathDraft, false)
                }
              >
                Relink path
              </button>
              <button
                type="button"
                className="btn-secondary"
                onClick={() => {
                  ipcClient.removeMedia(missingMediaTarget.id);
                  onClearMissingTarget();
                  onUpdateSnapshot(ipcClient.getPlaybackSnapshot());
                  onBumpLibraryVersion();
                }}
              >
                Remove from library
              </button>
              <button
                type="button"
                className="btn-secondary"
                onClick={onClearMissingTarget}
              >
                Back
              </button>
            </div>
          </div>

          <div
            style={{
              padding: 20,
              borderRadius: 10,
              background: "var(--bg-surface-2)",
              border: "1px solid var(--border-control)",
              display: "flex",
              flexDirection: "column",
              gap: 10,
            }}
          >
            <h2 style={{ margin: 0, fontSize: 14 }}>Your notes are preserved</h2>
            <p
              style={{
                margin: 0,
                fontSize: 12,
                color: "var(--text-secondary)",
              }}
            >
              {missingTargetNotes.length} timestamped notes for{" "}
              <strong>{missingMediaTarget.display_name}</strong> are safely stored in Audora.
              Relinking restores playback without losing a single note.
            </p>
            <div
              style={{
                display: "flex",
                flexDirection: "column",
                gap: 8,
                marginTop: 6,
                maxHeight: 210,
                overflowY: "auto",
              }}
            >
              {missingTargetNotes.map((n) => (
                <div
                  key={n.id}
                  style={{
                    padding: "8px 10px",
                    borderRadius: 8,
                    background: "var(--bg-surface-1)",
                    border: "1px solid var(--border-subtle)",
                    fontSize: 12,
                  }}
                >
                  <span
                    className="tabular-nums"
                    style={{
                      color: "var(--accent-text)",
                      fontWeight: 600,
                      marginRight: 8,
                    }}
                  >
                    {formatDurationMs(n.position_ms, timestampPrecision)}
                  </span>
                  <span style={{ color: "var(--text-secondary)" }}>
                    {n.body || "Quick marker"}
                  </span>
                </div>
              ))}
            </div>
          </div>
        </div>
      ) : (
        <>
          <div
            style={{
              display: "flex",
              justifyContent: "space-between",
              alignItems: "center",
            }}
          >
            <div>
              <h1 style={{ margin: "0 0 4px 0", fontSize: 24 }}>Media Library</h1>
              <p
                style={{
                  margin: 0,
                  fontSize: 13,
                  color: "var(--text-tertiary)",
                }}
              >
                Recent files, folders, and saved notes — everything stays on your device
              </p>
            </div>
            <div style={{ display: "flex", gap: 10 }}>
              <button
                type="button"
                className="btn-secondary"
                onClick={onOpenLocalSourcesSettings}
              >
                Local sources ({sourceFolderCount})
              </button>
              <button type="button" className="btn-primary" onClick={onOpenMediaPicker}>
                <IconPlus size={14} />
                <span>Add media</span>
              </button>
            </div>
          </div>

          <div
            style={{
              display: "flex",
              justifyContent: "space-between",
              alignItems: "center",
              marginTop: 24,
              gap: 16,
            }}
          >
            <input
              ref={searchInputRef}
              type="search"
              aria-label="Search library and notes"
              placeholder="Search titles or note text… (⌘K)"
              value={searchQuery}
              onChange={(e) => onSetSearchQuery(e.target.value)}
              style={{
                flex: "1 1 240px",
                maxWidth: 420,
                minWidth: 180,
                height: 38,
                padding: "0 14px",
                borderRadius: 8,
                border: "1px solid var(--border-control)",
                background: "var(--bg-surface-1)",
                color: "var(--text-primary)",
              }}
            />

            <div style={{ display: "flex", gap: 8 }}>
              {(["all", "video", "audio"] as const).map((f) => (
                <button
                  key={f}
                  type="button"
                  className="btn-secondary"
                  aria-pressed={libraryFilter === f}
                  onClick={() => onSetLibraryFilter(f)}
                >
                  {f === "all"
                    ? `All (${allMedia.length})`
                    : f === "video"
                      ? "Video"
                      : "Audio"}
                </button>
              ))}
            </div>
          </div>

          {libraryResult.media_items.length === 0 ? (
            <div
              style={{
                marginTop: 32,
                padding: 64,
                borderRadius: 14,
                background: "var(--bg-surface-1)",
                border: "1px solid var(--border-control)",
                textAlign: "center",
              }}
            >
              <h2 style={{ margin: "0 0 8px 0", fontSize: 20 }}>Your library is empty</h2>
              <p
                style={{
                  margin: "0 0 20px 0",
                  fontSize: 13,
                  color: "var(--text-secondary)",
                }}
              >
                Open an audio or video file from your computer to start listening, watching,
                and capturing timestamped notes.
              </p>
              <div style={{ display: "flex", justifyContent: "center", gap: 10 }}>
                <button type="button" className="btn-primary" onClick={onOpenMediaPicker}>
                  <IconFolderOpen size={14} />
                  <span>Open your first file</span>
                </button>
              </div>
            </div>
          ) : (
            <>
              <div className="media-table-surface">
                <table className="media-table">
                  <thead>
                    <tr>
                      <th>Title</th>
                      <th>Type</th>
                      <th>Duration</th>
                      <th>Notes</th>
                      <th>Progress</th>
                      <th>Last played</th>
                      <th aria-label="Actions" />
                    </tr>
                  </thead>
                  <tbody>
                    {libraryResult.media_items.map((item) => {
                      const pct = computeProgressPercent(item);
                      const isMissing = item.availability === "missing";
                      const isMenuOpen = openLibraryMenuId === item.id;
                      return (
                        <tr
                          key={item.id}
                          onClick={() => onSelectLibraryItem(item)}
                          style={{
                            cursor: "pointer",
                            position: isMenuOpen ? "relative" : undefined,
                            zIndex: isMenuOpen ? 60 : undefined,
                            background: isMissing
                              ? "var(--status-error-bg)"
                              : "transparent",
                          }}
                        >
                          <td style={{ fontWeight: 600 }}>
                            <button
                              type="button"
                              className="library-title-btn"
                              onClick={(e) => {
                                e.stopPropagation();
                                onSelectLibraryItem(item);
                              }}
                            >
                              {item.display_name}
                            </button>
                          </td>
                          <td>
                            {isMissing
                              ? "! Missing file"
                              : item.media_kind === "video"
                                ? "Video"
                                : "Audio"}
                          </td>
                          <td className="tabular-nums">
                            {formatDurationMs(item.duration_ms, timestampPrecision)}
                          </td>
                          <td>
                            {item.note_count > 0 ? (
                              <span className="timestamp-pill">
                                {item.note_count} {item.note_count === 1 ? "note" : "notes"}
                              </span>
                            ) : (
                              "—"
                            )}
                          </td>
                          <td className="tabular-nums">
                            {isMissing ? "File moved" : `${pct}%`}
                          </td>
                          <td style={{ color: "var(--text-tertiary)" }}>
                            {isMissing ? (
                              <span
                                style={{
                                  display: "inline-flex",
                                  alignItems: "center",
                                  gap: 4,
                                  color: "var(--accent-text)",
                                  fontWeight: 600,
                                }}
                              >
                                <span>Locate file</span>
                                <IconArrowRight size={12} />
                              </span>
                            ) : item.last_opened_at ? (
                              formatHumanDate(item.last_opened_at)
                            ) : (
                              "Never"
                            )}
                          </td>
                          <td
                            data-row-action-menu
                            onClick={(e) => e.stopPropagation()}
                            style={{
                              position: "relative",
                              textAlign: "right",
                              zIndex: isMenuOpen ? 60 : undefined,
                            }}
                          >
                            <button
                              type="button"
                              className="jump-link-btn"
                              aria-label={`Actions for ${item.display_name}`}
                              aria-expanded={isMenuOpen}
                              onClick={(e) => {
                                const rect = e.currentTarget.getBoundingClientRect();
                                const shouldFlipUp =
                                  typeof window !== "undefined" &&
                                  window.innerHeight - rect.bottom < 235 &&
                                  rect.top > 235;
                                onSetLibraryMenuPlacement(shouldFlipUp ? "up" : "down");
                                onSetOpenLibraryMenuId((prev) =>
                                  prev === item.id ? null : item.id,
                                );
                              }}
                            >
                              <IconMoreHorizontal size={15} />
                            </button>
                            {isMenuOpen && (
                              <div
                                className={`media-row-menu ${
                                  libraryMenuPlacement === "up" ? "open-up" : "open-down"
                                }`}
                              >
                                <button
                                  type="button"
                                  className="btn-secondary"
                                  onClick={() => onSelectLibraryItem(item, true)}
                                >
                                  Play now
                                </button>
                                <button
                                  type="button"
                                  className="btn-secondary"
                                  onClick={() => {
                                    onUpdateSnapshot(ipcClient.addToQueue(item.id));
                                    onSetOpenLibraryMenuId(null);
                                  }}
                                >
                                  Add to queue
                                </button>
                                <button
                                  type="button"
                                  className="btn-secondary"
                                  onClick={() => {
                                    onAddToPlaylist(item);
                                    onSetOpenLibraryMenuId(null);
                                  }}
                                >
                                  Add to playlist…
                                </button>
                                {!isMissing && (
                                  <button
                                    type="button"
                                    className="btn-secondary"
                                    onClick={() => {
                                      ipcClient.markMediaMissing(item.id);
                                      onSetOpenLibraryMenuId(null);
                                      onBumpLibraryVersion();
                                    }}
                                  >
                                    Mark missing
                                  </button>
                                )}
                                <button
                                  type="button"
                                  className="btn-secondary"
                                  onClick={() => {
                                    ipcClient.removeMedia(item.id);
                                    onSetOpenLibraryMenuId(null);
                                    onUpdateSnapshot(ipcClient.getPlaybackSnapshot());
                                    onBumpLibraryVersion();
                                  }}
                                >
                                  Remove from library
                                </button>
                              </div>
                            )}
                          </td>
                        </tr>
                      );
                    })}
                  </tbody>
                </table>
              </div>

              {searchQuery.trim() && libraryResult.matching_notes.length > 0 && (
                <div style={{ marginTop: 28 }} data-testid="search-matching-notes">
                  <h2 style={{ fontSize: 15, margin: "0 0 12px 0" }}>
                    Matching notes ({libraryResult.matching_notes.length})
                  </h2>
                  <div
                    style={{
                      display: "grid",
                      gridTemplateColumns: "repeat(auto-fill, minmax(300px, 1fr))",
                      gap: 12,
                    }}
                  >
                    {libraryResult.matching_notes.map((note) => {
                      const parent = mediaById.get(note.media_id);
                      return (
                        <div
                          key={note.id}
                          className="note-card clickable"
                          onClick={(e) => {
                            const target = e.target as HTMLElement | null;
                            if (target?.closest("button")) return;
                            onJumpToNote(note.id);
                          }}
                        >
                          <div
                            style={{
                              display: "flex",
                              justifyContent: "space-between",
                              alignItems: "center",
                            }}
                          >
                            <span
                              style={{
                                fontSize: 12,
                                fontWeight: 600,
                                color: "var(--text-secondary)",
                              }}
                            >
                              {parent?.display_name ?? "Media"}
                            </span>
                            <button
                              type="button"
                              className="timestamp-pill tabular-nums"
                              onClick={() => onJumpToNote(note.id)}
                            >
                              <IconPlay size={10} />
                              <span>
                                {formatDurationMs(note.position_ms, timestampPrecision)} — Jump to moment
                              </span>
                            </button>
                          </div>
                          <p style={{ margin: 0, fontSize: 12.5 }}>
                            {note.body || "Quick marker"}
                          </p>
                        </div>
                      );
                    })}
                  </div>
                </div>
              )}
            </>
          )}
        </>
      )}
    </div>
  );
});
