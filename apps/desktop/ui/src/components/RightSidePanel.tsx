import React, { useMemo, useState } from "react";
import { ipcClient } from "../api/ipc";
import type {
  MediaItem,
  Note,
  PlaybackSnapshot,
  SidePanelTab,
  TimestampPrecision,
} from "../types/domain";
import { formatDurationMs, formatHumanDate } from "../utils/format";
import {
  IconArrowRight,
  IconClose,
  IconCollapseRight,
  IconMoreHorizontal,
  IconPlay,
  IconPlus,
  IconRepeat,
  IconShuffle,
} from "./Icons";

interface RightSidePanelProps {
  sideTab: SidePanelTab;
  onSelectSideTab: (tab: SidePanelTab) => void;
  snapshot: PlaybackSnapshot;
  onUpdateSnapshot: (next: PlaybackSnapshot) => void;
  activeMedia: MediaItem | null;
  allMedia: MediaItem[];
  activeNotes: Note[];
  openNoteMenuId: string | null;
  onSetOpenNoteMenuId: React.Dispatch<React.SetStateAction<string | null>>;
  onOpenAddNoteModal: () => void;
  onOpenEditNoteModal: (note: Note) => void;
  onDeleteNote: (noteId: string) => void;
  onJumpToNote: (noteId: string) => void;
  onSelectLibraryItem: (item: MediaItem) => void;
  onOpenSaveQueueModal: () => void;
  onCycleRepeatMode: () => void;
  onCollapse: () => void;
  timestampPrecision: TimestampPrecision;
}

export const RightSidePanel = React.memo(function RightSidePanel({
  sideTab,
  onSelectSideTab,
  snapshot,
  onUpdateSnapshot,
  activeMedia,
  allMedia,
  activeNotes,
  openNoteMenuId,
  onSetOpenNoteMenuId,
  onOpenAddNoteModal,
  onOpenEditNoteModal,
  onDeleteNote,
  onJumpToNote,
  onSelectLibraryItem,
  onOpenSaveQueueModal,
  onCycleRepeatMode,
  onCollapse,
  timestampPrecision,
}: RightSidePanelProps): React.ReactElement {
  const [noteMenuPlacement, setNoteMenuPlacement] = useState<"down" | "up">("down");
  const mediaById = useMemo(() => {
    const map = new Map<string, MediaItem>();
    for (const m of allMedia) {
      map.set(m.id, m);
    }
    return map;
  }, [allMedia]);

  return (
    <aside className="side-panel" aria-label="Queue and notes" data-testid="side-panel">
      <div className="panel-tabs" role="tablist">
        <button
          id="side-tab-queue"
          type="button"
          role="tab"
          aria-selected={sideTab === "queue"}
          aria-controls="side-tabpanel-queue"
          tabIndex={sideTab === "queue" ? 0 : -1}
          className="panel-tab-btn"
          onClick={() => onSelectSideTab("queue")}
          onKeyDown={(e) => {
            if (e.key === "ArrowRight" || e.key === "ArrowLeft") {
              e.preventDefault();
              onSelectSideTab(sideTab === "queue" ? "notes" : "queue");
            }
          }}
        >
          Queue ({snapshot.queue.length})
        </button>
        <button
          id="side-tab-notes"
          type="button"
          role="tab"
          aria-selected={sideTab === "notes"}
          aria-controls="side-tabpanel-notes"
          tabIndex={sideTab === "notes" ? 0 : -1}
          className="panel-tab-btn"
          onClick={() => onSelectSideTab("notes")}
          onKeyDown={(e) => {
            if (e.key === "ArrowRight" || e.key === "ArrowLeft") {
              e.preventDefault();
              onSelectSideTab(sideTab === "notes" ? "queue" : "notes");
            }
          }}
        >
          Notes ({activeNotes.length})
        </button>
        <button
          type="button"
          className="panel-icon-btn"
          style={{ alignSelf: "center", margin: "0 8px" }}
          aria-label="Hide notes and queue bar"
          title="Hide Notes & Queue bar (])"
          onClick={onCollapse}
        >
          <IconCollapseRight size={14} />
        </button>
      </div>

      {sideTab === "notes" ? (
        <div
          id="side-tabpanel-notes"
          role="tabpanel"
          aria-labelledby="side-tab-notes"
          style={{ display: "flex", flexDirection: "column", flex: 1, minHeight: 0 }}
        >
          <div className="panel-header">
            <div>
              <div style={{ fontSize: 14, fontWeight: 600 }}>Timestamped notes</div>
              <div style={{ fontSize: 11, color: "var(--text-tertiary)" }}>
                Click any timestamp to jump to that moment
              </div>
            </div>
            <button
              type="button"
              className="btn-primary"
              disabled={!activeMedia}
              onClick={onOpenAddNoteModal}
            >
              <IconPlus size={13} />
              <span>New</span>
            </button>
          </div>

          <div className="panel-scroll-list">
            {activeNotes.length === 0 ? (
              <div
                style={{
                  padding: 24,
                  textAlign: "center",
                  color: "var(--text-tertiary)",
                  fontSize: 13,
                }}
              >
                No notes yet. Press <strong>N</strong> to capture a note or{" "}
                <strong>Shift+N</strong> for a quick marker.
              </div>
            ) : (
              activeNotes.map((note) => {
                const isOutOfRange =
                  activeMedia?.duration_ms !== null &&
                  activeMedia?.duration_ms !== undefined &&
                  note.position_ms > activeMedia.duration_ms;
                return (
                  <article
                    key={note.id}
                    className="note-card clickable"
                    style={
                      openNoteMenuId === note.id
                        ? { position: "relative", zIndex: 30 }
                        : undefined
                    }
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
                      <div style={{ display: "flex", alignItems: "center", gap: 6 }}>
                        <button
                          type="button"
                          className="timestamp-pill tabular-nums"
                          onClick={() => onJumpToNote(note.id)}
                        >
                          <IconPlay size={10} />
                          <span>
                            {formatDurationMs(note.position_ms, timestampPrecision)}
                          </span>
                        </button>
                        {isOutOfRange && (
                          <span
                            style={{
                              padding: "2px 6px",
                              borderRadius: 4,
                              background: "var(--status-error-bg)",
                              color: "var(--status-error-text)",
                              fontSize: 10,
                              fontWeight: 600,
                            }}
                          >
                            Out of range
                          </span>
                        )}
                      </div>
                      <div
                        data-row-action-menu
                        style={{ position: "relative", display: "flex", gap: 6 }}
                      >
                        <button
                          type="button"
                          className="jump-link-btn"
                          aria-label={`More actions for note at ${formatDurationMs(note.position_ms, timestampPrecision)}`}
                          aria-expanded={openNoteMenuId === note.id}
                          onClick={(e) => {
                            const rect = e.currentTarget.getBoundingClientRect();
                            const shouldFlipUp =
                              typeof window !== "undefined" &&
                              window.innerHeight - rect.bottom < 140 &&
                              rect.top > 140;
                            setNoteMenuPlacement(shouldFlipUp ? "up" : "down");
                            onSetOpenNoteMenuId((prev) =>
                              prev === note.id ? null : note.id,
                            );
                          }}
                        >
                          <IconMoreHorizontal size={15} />
                        </button>
                        {openNoteMenuId === note.id && (
                          <div
                            className={`note-row-menu ${
                              noteMenuPlacement === "up" ? "open-up" : "open-down"
                            }`}
                          >
                            <button
                              type="button"
                              className="btn-secondary"
                              onClick={() => onOpenEditNoteModal(note)}
                            >
                              Edit note
                            </button>
                            <button
                              type="button"
                              className="btn-secondary"
                              onClick={() => onDeleteNote(note.id)}
                            >
                              Delete note
                            </button>
                          </div>
                        )}
                      </div>
                    </div>
                    <p
                      style={{
                        margin: 0,
                        fontSize: 12.5,
                        color: note.is_quick_marker
                          ? "var(--text-tertiary)"
                          : "var(--text-primary)",
                        fontStyle: note.is_quick_marker ? "italic" : "normal",
                      }}
                    >
                      {note.is_quick_marker
                        ? "Quick marker (click Edit to add note)"
                        : note.body}
                    </p>
                    <div className="note-card-footer">
                      <span style={{ fontSize: 11, color: "var(--text-muted)" }}>
                        Added {formatHumanDate(note.created_at)}
                      </span>
                      <button
                        type="button"
                        className="jump-link-btn"
                        onClick={() => onJumpToNote(note.id)}
                      >
                        <span>Jump to moment</span>
                        <IconArrowRight size={12} />
                      </button>
                    </div>
                  </article>
                );
              })
            )}
          </div>
        </div>
      ) : (
        <div
          id="side-tabpanel-queue"
          role="tabpanel"
          aria-labelledby="side-tab-queue"
          style={{ display: "flex", flexDirection: "column", flex: 1, minHeight: 0 }}
        >
          <div className="panel-header">
            <div>
              <div style={{ fontSize: 14, fontWeight: 600 }}>Up next</div>
              <div style={{ fontSize: 11, color: "var(--text-tertiary)" }}>
                {snapshot.queue.length} items in queue
              </div>
            </div>
            <div style={{ display: "flex", flexWrap: "wrap", gap: 6 }}>
              {snapshot.queue.length > 0 && (
                <button
                  type="button"
                  className="btn-secondary"
                  title="Save current queue as a playlist"
                  onClick={onOpenSaveQueueModal}
                >
                  <IconPlus size={13} />
                  <span>Save</span>
                </button>
              )}
              <button
                type="button"
                className="btn-secondary"
                aria-pressed={snapshot.shuffle}
                onClick={() =>
                  onUpdateSnapshot(
                    ipcClient.playbackCommand({
                      action: "set_shuffle",
                      shuffle: !snapshot.shuffle,
                    }),
                  )
                }
              >
                <IconShuffle size={13} />
                <span>Shuffle</span>
              </button>
              <button
                type="button"
                className="btn-secondary"
                aria-pressed={snapshot.repeat_mode !== "off"}
                onClick={onCycleRepeatMode}
              >
                <IconRepeat size={13} />
                <span>Repeat: {snapshot.repeat_mode}</span>
              </button>
            </div>
          </div>

          <div className="panel-scroll-list">
            {snapshot.queue.map((id, idx) => {
              const qItem = mediaById.get(id);
              if (!qItem) return null;
              const isCurrent = qItem.id === snapshot.media_id;
              return (
                <div
                  key={qItem.id}
                  className="note-card"
                  style={{
                    borderColor: isCurrent
                      ? "var(--accent-primary)"
                      : "var(--border-control)",
                    background: isCurrent
                      ? "var(--accent-surface)"
                      : "var(--bg-surface-2)",
                  }}
                >
                  <div
                    style={{
                      display: "flex",
                      justifyContent: "space-between",
                      alignItems: "center",
                      gap: 8,
                    }}
                  >
                    <button
                      type="button"
                      onClick={() => onSelectLibraryItem(qItem)}
                      style={{
                        border: "none",
                        background: "transparent",
                        color: "inherit",
                        textAlign: "left",
                        padding: 0,
                        cursor: "pointer",
                        flex: 1,
                        minWidth: 0,
                      }}
                    >
                      <div
                        style={{
                          display: "flex",
                          alignItems: "center",
                          gap: 5,
                          fontSize: 13,
                          fontWeight: 600,
                        }}
                      >
                        {isCurrent ? <IconPlay size={12} /> : <span>{idx + 1}.</span>}
                        <span
                          style={{
                            overflow: "hidden",
                            textOverflow: "ellipsis",
                            whiteSpace: "nowrap",
                          }}
                        >
                          {qItem.display_name}
                        </span>
                      </div>
                      <div style={{ fontSize: 11, color: "var(--text-tertiary)" }}>
                        {qItem.media_kind === "video" ? "Video" : "Audio"} ·{" "}
                        <span className="tabular-nums">
                          {formatDurationMs(qItem.duration_ms, timestampPrecision)}
                        </span>
                      </div>
                    </button>
                    <button
                      type="button"
                      className="jump-link-btn"
                      aria-label={`Remove ${qItem.display_name} from queue`}
                      title="Remove from queue"
                      onClick={() => onUpdateSnapshot(ipcClient.removeFromQueue(qItem.id))}
                    >
                      <IconClose size={13} />
                    </button>
                  </div>
                </div>
              );
            })}
          </div>
        </div>
      )}
    </aside>
  );
});
