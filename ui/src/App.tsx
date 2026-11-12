import React, { useEffect, useState } from "react";
import { ipcClient } from "./api/ipc";
import type {
  AppErrorPayload,
  AppSettings,
  LibraryFilter,
  MediaItem,
  Note,
  PlaybackSnapshot,
  PrimarySection,
  RepeatMode,
  SidePanelTab,
} from "./types/domain";
import {
  computeProgressPercent,
  formatTimestamp,
  shouldSuppressGlobalShortcut,
} from "./utils/format";

const SPEED_OPTIONS = [0.5, 0.75, 1.0, 1.25, 1.5, 2.0];

export function App(): React.JSX.Element {
  const [settings, setSettings] = useState<AppSettings>(() => ipcClient.getSettings());
  const [snapshot, setSnapshot] = useState<PlaybackSnapshot>(() =>
    ipcClient.getPlaybackSnapshot(),
  );
  const [section, setSection] = useState<PrimarySection>("now_playing");
  const [sideTab, setSideTab] = useState<SidePanelTab>("notes");
  const [searchQuery, setSearchQuery] = useState<string>("");
  const [libraryFilter, setLibraryFilter] = useState<LibraryFilter>("all");
  const [libraryResult, setLibraryResult] = useState(() =>
    ipcClient.searchLibrary("", "all"),
  );
  const [activeNotes, setActiveNotes] = useState<Note[]>([]);
  const [selectedMissingMedia, setSelectedMissingMedia] = useState<MediaItem | null>(null);
  const [activeError, setActiveError] = useState<AppErrorPayload | null>(null);

  // Add/Edit Note modal state (Frame 4:88)
  const [isNoteModalOpen, setIsNoteModalOpen] = useState<boolean>(false);
  const [capturedPositionMs, setCapturedPositionMs] = useState<number>(0);
  const [editingNoteId, setEditingNoteId] = useState<string | null>(null);
  const [noteBodyDraft, setNoteBodyDraft] = useState<string>("");
  const [isQuickMarkerDraft, setIsQuickMarkerDraft] = useState<boolean>(false);
  const [modalValidationMsg, setModalValidationMsg] = useState<string | null>(null);

  // Toast & Undo state (Frame 4:578)
  const [toastMessage, setToastMessage] = useState<string | null>(null);
  const [lastDeletedNote, setLastDeletedNote] = useState<Note | null>(null);

  const activeMedia =
    libraryResult.media_items.find((m) => m.id === snapshot.media_id) ??
    ipcClient.searchLibrary("", "all").media_items.find((m) => m.id === snapshot.media_id) ??
    null;

  const refreshLibraryAndNotes = (mediaId: string | null = snapshot.media_id) => {
    setLibraryResult(ipcClient.searchLibrary(searchQuery, libraryFilter));
    if (mediaId) {
      setActiveNotes(ipcClient.listNotes(mediaId));
    } else {
      setActiveNotes([]);
    }
  };

  useEffect(() => {
    refreshLibraryAndNotes(snapshot.media_id);
  }, [snapshot.media_id, searchQuery, libraryFilter]);

  useEffect(() => {
    const resolvedTheme =
      settings.theme === "system" ? "dark" : settings.theme;
    document.documentElement.setAttribute("data-theme", resolvedTheme);
  }, [settings.theme]);

  const showToast = (msg: string, deletedNote: Note | null = null) => {
    setToastMessage(msg);
    setLastDeletedNote(deletedNote);
  };

  const openAddNoteModal = () => {
    if (!snapshot.media_id) return;
    // Capture playhead ONCE when triggered (Product requirement & Flow 3)
    setCapturedPositionMs(snapshot.position_ms);
    setEditingNoteId(null);
    setNoteBodyDraft("");
    setIsQuickMarkerDraft(false);
    setModalValidationMsg(null);
    setIsNoteModalOpen(true);
  };

  const handleSaveNote = () => {
    if (!snapshot.media_id) return;
    try {
      if (editingNoteId) {
        ipcClient.updateNote({ id: editingNoteId, body: noteBodyDraft });
        showToast("Note updated");
      } else {
        ipcClient.createNote({
          media_id: snapshot.media_id,
          position_ms: capturedPositionMs,
          is_quick_marker: isQuickMarkerDraft,
          body: noteBodyDraft,
        });
        showToast("Note added");
      }
      setIsNoteModalOpen(false);
      refreshLibraryAndNotes(snapshot.media_id);
    } catch (err) {
      const payload = err as AppErrorPayload;
      setModalValidationMsg(payload.message ?? "Could not save note.");
    }
  };

  // Global keyboard shortcuts (UI_DESIGN.md)
  useEffect(() => {
    const onKeyDown = (e: KeyboardEvent) => {
      const target = e.target as HTMLElement | null;
      if ((e.metaKey || e.ctrlKey) && e.key.toLowerCase() === "k") {
        e.preventDefault();
        setSection("library");
        return;
      }
      if (e.key === "Escape") {
        if (isNoteModalOpen) {
          setIsNoteModalOpen(false);
          return;
        }
        if (settings.window_mode === "minimum") {
          setSettings(ipcClient.setWindowMode("full"));
          return;
        }
      }
      if (
        shouldSuppressGlobalShortcut(
          target?.tagName,
          Boolean(target?.isContentEditable),
          isNoteModalOpen,
        )
      ) {
        return;
      }
      if (e.code === "Space") {
        e.preventDefault();
        setSnapshot(ipcClient.playbackCommand({ action: "toggle_play_pause" }));
      } else if (e.key.toLowerCase() === "n" && e.shiftKey) {
        e.preventDefault();
        if (snapshot.media_id) {
          ipcClient.createNote({
            media_id: snapshot.media_id,
            position_ms: snapshot.position_ms,
            is_quick_marker: true,
            body: "",
          });
          refreshLibraryAndNotes(snapshot.media_id);
          showToast("Quick marker added");
        }
      } else if (e.key.toLowerCase() === "n") {
        e.preventDefault();
        openAddNoteModal();
      } else if (e.key.toLowerCase() === "m") {
        e.preventDefault();
        setSnapshot(
          ipcClient.playbackCommand({ action: "set_muted", muted: !snapshot.muted }),
        );
      } else if (e.key === "ArrowLeft") {
        e.preventDefault();
        setSnapshot(
          ipcClient.playbackCommand({
            action: "skip_relative",
            delta_ms: e.shiftKey ? -15_000 : -5_000,
          }),
        );
      } else if (e.key === "ArrowRight") {
        e.preventDefault();
        setSnapshot(
          ipcClient.playbackCommand({
            action: "skip_relative",
            delta_ms: e.shiftKey ? 15_000 : 5_000,
          }),
        );
      }
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  });

  // Minimum mode view (Frame 2:103)
  if (settings.window_mode === "minimum") {
    return (
      <div className="minimum-mode-shell" data-node-id="2:103">
        <div
          style={{
            display: "flex",
            alignItems: "center",
            gap: "24px",
            width: "100%",
            maxWidth: "620px",
          }}
        >
          <button
            type="button"
            className="play-circle-btn"
            style={{
              width: "56px",
              height: "56px",
              backgroundColor: "var(--accent-primary)",
              color: "var(--accent-on-primary)",
            }}
            aria-label={snapshot.state === "playing" ? "Pause" : "Play"}
            onClick={() =>
              setSnapshot(ipcClient.playbackCommand({ action: "toggle_play_pause" }))
            }
          >
            {snapshot.state === "playing" ? "❚❚" : "▶"}
          </button>
          <div style={{ flex: 1 }}>
            <div style={{ display: "flex", alignItems: "baseline", gap: "12px" }}>
              <span
                className="tabular-nums"
                style={{ fontSize: "48px", fontWeight: 600 }}
              >
                {formatTimestamp(snapshot.position_ms, settings.timestamp_precision)}
              </span>
              <span className="tabular-nums" style={{ color: "var(--text-tertiary)" }}>
                / {formatTimestamp(snapshot.duration_ms, "nearest_second")}
              </span>
            </div>
            <input
              type="range"
              className="scrubber-slider"
              style={{ width: "100%", marginTop: "12px" }}
              aria-label="Seek timeline"
              min={0}
              max={snapshot.duration_ms ?? 100}
              value={snapshot.position_ms}
              onChange={(e) =>
                setSnapshot(
                  ipcClient.playbackCommand({
                    action: "seek",
                    position_ms: Number(e.target.value),
                  }),
                )
              }
            />
          </div>
        </div>
        <div style={{ position: "absolute", bottom: "24px", right: "24px" }}>
          <button
            type="button"
            className="btn-secondary"
            onClick={() => setSettings(ipcClient.setWindowMode("full"))}
          >
            ▦ Open full mode
          </button>
        </div>
      </div>
    );
  }

  const nextRepeatMode = (current: RepeatMode): RepeatMode => {
    if (current === "off") return "all";
    if (current === "all") return "one";
    return "off";
  };

  return (
    <div className="audora-shell" data-node-id="2:3">
      {/* Hybrid 56px Header Bar */}
      <header className="audora-titlebar" data-tauri-drag-region>
        <div className="titlebar-brand">
          <div className="audora-mark" aria-hidden="true">
            <span style={{ height: "8px" }} />
            <span style={{ height: "14px" }} />
            <span style={{ height: "18px" }} />
            <span style={{ height: "10px" }} />
          </div>
          <span className="brand-title">Audora</span>
        </div>

        <div className="titlebar-center">
          <span className="status-dot" aria-hidden="true" />
          <span style={{ fontWeight: 500 }}>
            {activeMedia?.display_name ?? "No media selected"}
          </span>
          <span style={{ color: "var(--text-muted)" }}>— Local</span>
        </div>

        <div className="titlebar-actions">
          <button
            type="button"
            className="btn-secondary"
            aria-label="Switch to Minimum mode"
            onClick={() => setSettings(ipcClient.setWindowMode("minimum"))}
          >
            — Minimum
          </button>
        </div>
      </header>

      <div className="audora-body">
        {/* 76px Navigation Rail */}
        <nav className="audora-nav-rail" aria-label="Primary">
          <div className="nav-rail-group">
            <button
              type="button"
              className="nav-rail-btn"
              aria-current={section === "library" ? "page" : undefined}
              onClick={() => {
                setSelectedMissingMedia(null);
                setActiveError(null);
                setSection("library");
              }}
            >
              <span className="nav-rail-icon" aria-hidden="true">
                ▦
              </span>
              <span>Library</span>
            </button>

            <button
              type="button"
              className="nav-rail-btn"
              aria-current={section === "now_playing" ? "page" : undefined}
              onClick={() => {
                setSelectedMissingMedia(null);
                setSection("now_playing");
              }}
            >
              <span className="nav-rail-icon" aria-hidden="true">
                ▶
              </span>
              <span>Now playing</span>
            </button>
          </div>

          <div className="nav-rail-group">
            <button
              type="button"
              className="nav-rail-btn"
              aria-current={section === "settings" ? "page" : undefined}
              onClick={() => {
                setSelectedMissingMedia(null);
                setSection("settings");
              }}
            >
              <span className="nav-rail-icon" aria-hidden="true">
                ⚙
              </span>
              <span>Settings</span>
            </button>
          </div>
        </nav>

        {/* Main Workspace Stage */}
        <main className="audora-stage">
          {selectedMissingMedia ? (
            /* Missing Media Screen (Frame 4:339) */
            <div className="content-page" data-node-id="4:339">
              <button
                type="button"
                className="btn-secondary"
                onClick={() => setSelectedMissingMedia(null)}
              >
                ‹ Library
              </button>
              <div
                style={{
                  display: "grid",
                  gridTemplateColumns: "1fr 1fr",
                  gap: "48px",
                  marginTop: "32px",
                }}
              >
                <div>
                  <p
                    style={{
                      color: "var(--status-error-fg)",
                      fontSize: "11px",
                      fontWeight: 600,
                    }}
                  >
                    MEDIA OFFLINE
                  </p>
                  <h1 style={{ fontSize: "30px", margin: "8px 0 16px" }}>
                    We can’t find this file
                  </h1>
                  <p style={{ color: "var(--text-tertiary)", lineHeight: 1.5 }}>
                    It may have been moved, renamed, or stored on a disconnected drive.
                    Your notes are safe and will remain here.
                  </p>
                  <div
                    style={{
                      padding: "14px",
                      borderRadius: "8px",
                      backgroundColor: "var(--bg-inset)",
                      border: "1px solid var(--border-control)",
                      margin: "24px 0",
                    }}
                  >
                    <div
                      style={{
                        fontSize: "10px",
                        fontWeight: 600,
                        color: "var(--text-muted)",
                      }}
                    >
                      LAST KNOWN LOCATION
                    </div>
                    <div style={{ fontSize: "12px", marginTop: "6px" }}>
                      {selectedMissingMedia.path}
                    </div>
                  </div>
                  <div style={{ display: "flex", gap: "12px" }}>
                    <button
                      type="button"
                      className="btn-primary"
                      onClick={() => {
                        const relinked = ipcClient.relinkMedia(
                          selectedMissingMedia.id,
                          "/Users/local/Media/Sound Design Reel (Relinked).mp4",
                        );
                        setSelectedMissingMedia(null);
                        ipcClient.selectMediaById(relinked.id);
                        setSnapshot(ipcClient.getPlaybackSnapshot());
                        refreshLibraryAndNotes(relinked.id);
                        setSection("now_playing");
                        showToast("Media file relinked");
                      }}
                    >
                      ▣ Find file…
                    </button>
                    <button
                      type="button"
                      className="btn-secondary"
                      onClick={() => {
                        ipcClient.removeMedia(selectedMissingMedia.id);
                        setSelectedMissingMedia(null);
                        refreshLibraryAndNotes();
                        showToast("Removed from library");
                      }}
                    >
                      Remove from library
                    </button>
                  </div>
                </div>

                <div
                  style={{
                    padding: "20px",
                    borderRadius: "10px",
                    backgroundColor: "var(--bg-surface-1)",
                    border: "1px solid var(--border-control)",
                  }}
                >
                  <h2 style={{ fontSize: "17px", margin: "0 0 4px" }}>
                    Your notes are preserved
                  </h2>
                  <p
                    style={{
                      fontSize: "12px",
                      color: "var(--text-tertiary)",
                      margin: "0 0 16px",
                    }}
                  >
                    {selectedMissingMedia.note_count} notes ·{" "}
                    {formatTimestamp(selectedMissingMedia.duration_ms)} media duration
                  </p>
                </div>
              </div>
            </div>
          ) : activeError ? (
            /* Unsupported Media / Playback Error Screen (Frame 4:399) */
            <div
              className="content-page"
              data-node-id="4:399"
              role="alert"
              style={{ textAlign: "center", maxWidth: "560px", margin: "0 auto" }}
            >
              <p
                style={{
                  color: "var(--status-error-fg)",
                  fontSize: "11px",
                  fontWeight: 600,
                  marginTop: "64px",
                }}
              >
                PLAYBACK ERROR
              </p>
              <h1 style={{ fontSize: "30px", margin: "12px 0" }}>
                {activeError.title}
              </h1>
              <p style={{ color: "var(--text-tertiary)" }}>{activeError.message}</p>
              <div
                style={{
                  textAlign: "left",
                  padding: "16px",
                  borderRadius: "8px",
                  backgroundColor: "var(--bg-inset)",
                  border: "1px solid var(--border-control)",
                  margin: "24px 0",
                }}
              >
                <div style={{ fontWeight: 600, fontSize: "12px", marginBottom: "8px" }}>
                  What you can do
                </div>
                {activeError.recovery_steps.map((step) => (
                  <div
                    key={step}
                    style={{
                      fontSize: "12px",
                      color: "var(--text-tertiary)",
                      marginTop: "4px",
                    }}
                  >
                    {step}
                  </div>
                ))}
              </div>
              <div style={{ display: "flex", justifyContent: "center", gap: "12px" }}>
                <button
                  type="button"
                  className="btn-primary"
                  onClick={() => {
                    setActiveError(null);
                    setSection("library");
                  }}
                >
                  Choose another file
                </button>
              </div>
              <p
                style={{
                  fontSize: "11px",
                  color: "var(--text-muted)",
                  marginTop: "24px",
                }}
              >
                Error {activeError.code} · Decoder unavailable
              </p>
            </div>
          ) : section === "now_playing" ? (
            /* Now Playing: Video (2:3) or Audio (4:3) + Right Panel (2:63 / 4:609) */
            <>
              <div className="player-column">
                <div className="media-canvas">
                  <p className="canvas-eyebrow">
                    {activeMedia?.media_kind === "audio"
                      ? "AUDIO PLAYBACK · LOCAL"
                      : "VIDEO PLAYBACK · LOCAL"}
                  </p>
                  <h1 className="canvas-headline">
                    {activeMedia?.display_name ?? "Select a file from your library"}
                  </h1>
                  <p className="canvas-subtitle">
                    {snapshot.state.toUpperCase()} · Speed {snapshot.rate}×
                  </p>
                </div>

                {/* 128px Transport Bar */}
                <div className="transport-bar" data-node-id="2:47">
                  <div className="scrubber-row">
                    <span className="tabular-nums">
                      {formatTimestamp(snapshot.position_ms, settings.timestamp_precision)}
                    </span>
                    <input
                      type="range"
                      className="scrubber-slider"
                      aria-label="Seek timeline"
                      min={0}
                      max={snapshot.duration_ms ?? 100}
                      value={snapshot.position_ms}
                      onChange={(e) =>
                        setSnapshot(
                          ipcClient.playbackCommand({
                            action: "seek",
                            position_ms: Number(e.target.value),
                          }),
                        )
                      }
                    />
                    <span className="tabular-nums">
                      {formatTimestamp(snapshot.duration_ms, "nearest_second")}
                    </span>
                  </div>

                  <div className="transport-controls-row">
                    <div style={{ display: "flex", gap: "8px" }}>
                      <button
                        type="button"
                        className="btn-secondary"
                        onClick={openAddNoteModal}
                      >
                        + Add note (N)
                      </button>
                    </div>

                    <div className="transport-center-buttons">
                      <button
                        type="button"
                        className="btn-secondary"
                        aria-label="Rewind 10 seconds"
                        onClick={() =>
                          setSnapshot(
                            ipcClient.playbackCommand({
                              action: "skip_relative",
                              delta_ms: -10_000,
                            }),
                          )
                        }
                      >
                        ↶
                      </button>
                      <button
                        type="button"
                        className="btn-secondary"
                        aria-label="Previous item"
                        onClick={() =>
                          setSnapshot(ipcClient.playbackCommand({ action: "previous" }))
                        }
                      >
                        ‹
                      </button>
                      <button
                        type="button"
                        className="play-circle-btn"
                        aria-label={snapshot.state === "playing" ? "Pause" : "Play"}
                        onClick={() =>
                          setSnapshot(
                            ipcClient.playbackCommand({ action: "toggle_play_pause" }),
                          )
                        }
                      >
                        {snapshot.state === "playing" ? "❚❚" : "▶"}
                      </button>
                      <button
                        type="button"
                        className="btn-secondary"
                        aria-label="Next item"
                        onClick={() =>
                          setSnapshot(ipcClient.playbackCommand({ action: "next" }))
                        }
                      >
                        ›
                      </button>
                      <button
                        type="button"
                        className="btn-secondary"
                        aria-label="Forward 10 seconds"
                        onClick={() =>
                          setSnapshot(
                            ipcClient.playbackCommand({
                              action: "skip_relative",
                              delta_ms: 10_000,
                            }),
                          )
                        }
                      >
                        ↷
                      </button>
                    </div>

                    <div style={{ display: "flex", alignItems: "center", gap: "10px" }}>
                      <label htmlFor="playback-rate-select" className="sr-only">
                        Playback speed
                      </label>
                      <select
                        id="playback-rate-select"
                        className="btn-secondary"
                        aria-label="Playback speed"
                        value={snapshot.rate}
                        onChange={(e) =>
                          setSnapshot(
                            ipcClient.playbackCommand({
                              action: "set_rate",
                              rate: Number(e.target.value),
                            }),
                          )
                        }
                      >
                        {SPEED_OPTIONS.map((spd) => (
                          <option key={spd} value={spd}>
                            {spd}×
                          </option>
                        ))}
                      </select>

                      <button
                        type="button"
                        className="btn-secondary"
                        aria-label={snapshot.muted ? "Unmute" : "Mute"}
                        aria-pressed={snapshot.muted}
                        onClick={() =>
                          setSnapshot(
                            ipcClient.playbackCommand({
                              action: "set_muted",
                              muted: !snapshot.muted,
                            }),
                          )
                        }
                      >
                        {snapshot.muted ? "Muted" : "◖))"}
                      </button>
                    </div>
                  </div>
                </div>
              </div>

              {/* Persistent 365px Queue | Notes Side Panel */}
              <aside className="side-panel" aria-label="Queue and Notes">
                <div className="panel-tabs" role="tablist">
                  <button
                    type="button"
                    role="tab"
                    className="panel-tab-btn"
                    aria-selected={sideTab === "queue"}
                    onClick={() => setSideTab("queue")}
                  >
                    ☷ Queue {snapshot.queue.length}
                  </button>
                  <button
                    type="button"
                    role="tab"
                    className="panel-tab-btn"
                    aria-selected={sideTab === "notes"}
                    onClick={() => setSideTab("notes")}
                  >
                    ▤ Notes {activeNotes.length}
                  </button>
                </div>

                {sideTab === "notes" ? (
                  <>
                    <div className="panel-header">
                      <div>
                        <div style={{ fontSize: "18px", fontWeight: 600 }}>Notes</div>
                        <div style={{ fontSize: "11px", color: "var(--text-tertiary)" }}>
                          For this media
                        </div>
                      </div>
                      <button
                        type="button"
                        className="btn-primary"
                        onClick={openAddNoteModal}
                      >
                        + Add note
                      </button>
                    </div>

                    <div className="panel-scroll-list">
                      {activeNotes.length === 0 ? (
                        <div
                          style={{
                            padding: "32px 16px",
                            textAlign: "center",
                            color: "var(--text-tertiary)",
                            fontSize: "12px",
                          }}
                        >
                          No notes yet. Press <strong>N</strong> or click{" "}
                          <strong>+ Add note</strong> to mark a moment.
                        </div>
                      ) : (
                        activeNotes.map((note) => (
                          <div key={note.id} className="note-card">
                            <div>
                              <button
                                type="button"
                                className="timestamp-pill tabular-nums"
                                onClick={() =>
                                  setSnapshot(ipcClient.activateNote(note.id))
                                }
                              >
                                ▶{" "}
                                {formatTimestamp(
                                  note.position_ms,
                                  settings.timestamp_precision,
                                )}
                              </button>
                            </div>
                            <div
                              style={{
                                fontSize: "13px",
                                lineHeight: 1.4,
                                fontStyle: note.is_quick_marker ? "italic" : "normal",
                                color: note.is_quick_marker
                                  ? "var(--text-tertiary)"
                                  : "var(--text-primary)",
                              }}
                            >
                              {note.is_quick_marker
                                ? "Quick marker — click Edit to add details"
                                : note.body}
                            </div>
                            <div className="note-card-footer">
                              <button
                                type="button"
                                className="jump-link-btn"
                                onClick={() =>
                                  setSnapshot(ipcClient.activateNote(note.id))
                                }
                              >
                                Jump to moment
                              </button>
                              <div style={{ display: "flex", gap: "8px" }}>
                                <button
                                  type="button"
                                  className="jump-link-btn"
                                  aria-label="Edit note"
                                  onClick={() => {
                                    setCapturedPositionMs(note.position_ms);
                                    setEditingNoteId(note.id);
                                    setNoteBodyDraft(note.body);
                                    setIsQuickMarkerDraft(note.is_quick_marker);
                                    setModalValidationMsg(null);
                                    setIsNoteModalOpen(true);
                                  }}
                                >
                                  Edit
                                </button>
                                <button
                                  type="button"
                                  className="jump-link-btn"
                                  style={{ color: "var(--status-error-fg)" }}
                                  aria-label="Delete note"
                                  onClick={() => {
                                    const deleted = ipcClient.deleteNote(note.id);
                                    refreshLibraryAndNotes(snapshot.media_id);
                                    if (deleted) {
                                      showToast("Note deleted", deleted);
                                    }
                                  }}
                                >
                                  Delete
                                </button>
                              </div>
                            </div>
                          </div>
                        ))
                      )}
                    </div>
                  </>
                ) : (
                  /* Queue Panel with Repeat & Shuffle controls (UI_DESIGN.md Resolution #3) */
                  <>
                    <div className="panel-header">
                      <div>
                        <div style={{ fontSize: "18px", fontWeight: 600 }}>Queue</div>
                        <div style={{ fontSize: "11px", color: "var(--text-tertiary)" }}>
                          {snapshot.queue.length} local items
                        </div>
                      </div>
                      <div style={{ display: "flex", gap: "6px" }}>
                        <button
                          type="button"
                          className="btn-secondary"
                          aria-pressed={snapshot.shuffle}
                          onClick={() =>
                            setSnapshot(
                              ipcClient.playbackCommand({
                                action: "set_shuffle",
                                shuffle: !snapshot.shuffle,
                              }),
                            )
                          }
                        >
                          Shuffle
                        </button>
                        <button
                          type="button"
                          className="btn-secondary"
                          aria-pressed={snapshot.repeat_mode !== "off"}
                          onClick={() =>
                            setSnapshot(
                              ipcClient.playbackCommand({
                                action: "set_repeat",
                                mode: nextRepeatMode(snapshot.repeat_mode),
                              }),
                            )
                          }
                        >
                          Repeat: {snapshot.repeat_mode}
                        </button>
                      </div>
                    </div>

                    <div className="panel-scroll-list">
                      {snapshot.queue.map((queuedId) => {
                        const item = ipcClient
                          .searchLibrary("", "all")
                          .media_items.find((m) => m.id === queuedId);
                        if (!item) return null;
                        const isPlayingNow = item.id === snapshot.media_id;
                        return (
                          <div
                            key={item.id}
                            className="note-card"
                            style={{ cursor: "pointer" }}
                            onClick={() => {
                              ipcClient.selectMediaById(item.id);
                              setSnapshot(ipcClient.getPlaybackSnapshot());
                            }}
                          >
                            <div style={{ fontWeight: 600, fontSize: "13px" }}>
                              {item.display_name}
                            </div>
                            <div
                              style={{
                                fontSize: "11px",
                                color: isPlayingNow
                                  ? "var(--accent-primary)"
                                  : "var(--text-tertiary)",
                              }}
                            >
                              {isPlayingNow
                                ? "Playing now"
                                : formatTimestamp(item.duration_ms)}
                            </div>
                          </div>
                        );
                      })}
                    </div>
                  </>
                )}
              </aside>
            </>
          ) : section === "library" ? (
            /* Media Library View (Frame 4:206 & 4:300) */
            <div className="content-page" data-node-id="4:206">
              <div
                style={{
                  display: "flex",
                  alignItems: "center",
                  justifyContent: "space-between",
                }}
              >
                <div>
                  <div
                    style={{
                      fontSize: "10px",
                      fontWeight: 600,
                      color: "var(--text-muted)",
                    }}
                  >
                    YOUR COLLECTION
                  </div>
                  <h1 style={{ fontSize: "30px", margin: "6px 0" }}>Media library</h1>
                  <p
                    style={{
                      fontSize: "12px",
                      color: "var(--text-tertiary)",
                      margin: 0,
                    }}
                  >
                    Everything stays on this device.
                  </p>
                </div>

                <div style={{ display: "flex", gap: "8px" }}>
                  <button
                    type="button"
                    className="btn-secondary"
                    onClick={() => {
                      try {
                        ipcClient.openMediaPath("Camera_Original.prores");
                      } catch (err) {
                        setActiveError(err as AppErrorPayload);
                      }
                    }}
                  >
                    Test Unsupported Format
                  </button>
                  <button
                    type="button"
                    className="btn-primary"
                    onClick={() => {
                      const opened = ipcClient.openMediaPath(
                        `/Users/local/Media/Studio Session ${libraryResult.media_items.length + 1}.mp4`,
                      );
                      setSnapshot(ipcClient.getPlaybackSnapshot());
                      refreshLibraryAndNotes(opened.id);
                      setSection("now_playing");
                    }}
                  >
                    ▣ Open media
                  </button>
                </div>
              </div>

              <div
                style={{
                  display: "flex",
                  alignItems: "center",
                  gap: "12px",
                  marginTop: "24px",
                }}
              >
                <input
                  type="search"
                  className="btn-secondary"
                  style={{ width: "420px", height: "40px", padding: "0 14px" }}
                  placeholder="Search titles and notes… (⌘K)"
                  aria-label="Search titles and notes"
                  value={searchQuery}
                  onChange={(e) => setSearchQuery(e.target.value)}
                />

                {(["all", "video", "audio"] as const).map((flt) => (
                  <button
                    key={flt}
                    type="button"
                    className={libraryFilter === flt ? "btn-primary" : "btn-secondary"}
                    onClick={() => setLibraryFilter(flt)}
                  >
                    {flt.charAt(0).toUpperCase() + flt.slice(1)}
                  </button>
                ))}
              </div>

              {libraryResult.media_items.length === 0 ? (
                <div
                  style={{
                    marginTop: "32px",
                    padding: "64px",
                    textAlign: "center",
                    border: "1px dashed var(--border-dashed)",
                    borderRadius: "var(--radius-empty)",
                  }}
                >
                  <h2 style={{ fontSize: "18px" }}>Your library is ready</h2>
                  <p style={{ fontSize: "12px", color: "var(--text-tertiary)" }}>
                    Open a local audio or video file to start listening and taking notes.
                  </p>
                </div>
              ) : (
                <table className="media-table">
                  <thead>
                    <tr>
                      <th>Title</th>
                      <th>Progress</th>
                      <th>Last Opened</th>
                    </tr>
                  </thead>
                  <tbody>
                    {libraryResult.media_items.map((item) => {
                      const pct = computeProgressPercent(item);
                      const isMissing = item.availability === "missing";
                      return (
                        <tr
                          key={item.id}
                          style={{ cursor: "pointer" }}
                          onClick={() => {
                            if (isMissing) {
                              setSelectedMissingMedia(item);
                              return;
                            }
                            ipcClient.selectMediaById(item.id);
                            setSnapshot(ipcClient.getPlaybackSnapshot());
                            setSection("now_playing");
                          }}
                        >
                          <td>
                            <div style={{ fontWeight: 600 }}>{item.display_name}</div>
                            <div
                              style={{
                                fontSize: "12px",
                                color: isMissing
                                  ? "var(--status-error-fg)"
                                  : "var(--text-tertiary)",
                              }}
                            >
                              {isMissing
                                ? "Missing file · Notes preserved"
                                : `${item.media_kind === "video" ? "Video" : "Audio"} · ${formatTimestamp(item.duration_ms)}`}
                            </div>
                          </td>
                          <td className="tabular-nums">
                            {isMissing
                              ? "Missing"
                              : pct === 0
                                ? "Not started"
                                : `${pct}%`}
                          </td>
                          <td>
                            {isMissing
                              ? `${item.note_count} notes safe`
                              : item.last_opened_at ?? item.added_at}
                          </td>
                        </tr>
                      );
                    })}
                  </tbody>
                </table>
              )}
            </div>
          ) : (
            /* Settings View (Frame 4:441) */
            <div className="content-page" data-node-id="4:441">
              <div
                style={{
                  fontSize: "10px",
                  fontWeight: 600,
                  color: "var(--text-muted)",
                }}
              >
                PREFERENCES
              </div>
              <h1 style={{ fontSize: "30px", margin: "6px 0" }}>Settings</h1>
              <p style={{ fontSize: "12px", color: "var(--text-tertiary)" }}>
                Changes are saved automatically on this device.
              </p>

              <section style={{ marginTop: "32px", maxWidth: "650px" }}>
                <h2 style={{ fontSize: "18px" }}>Appearance</h2>
                <div style={{ display: "flex", gap: "12px", marginTop: "12px" }}>
                  {(["dark", "light", "system"] as const).map((thm) => (
                    <button
                      key={thm}
                      type="button"
                      className={settings.theme === thm ? "btn-primary" : "btn-secondary"}
                      onClick={() =>
                        setSettings(ipcClient.updateSettings({ ...settings, theme: thm }))
                      }
                    >
                      {thm.charAt(0).toUpperCase() + thm.slice(1)}
                    </button>
                  ))}
                </div>

                <h2 style={{ fontSize: "18px", marginTop: "32px" }}>Playback & Notes</h2>
                <div
                  style={{
                    display: "flex",
                    flexDirection: "column",
                    gap: "16px",
                    marginTop: "12px",
                  }}
                >
                  <label
                    style={{
                      display: "flex",
                      justifyContent: "space-between",
                      alignItems: "center",
                    }}
                  >
                    <span>
                      <strong>Resume where I left off</strong>
                      <div style={{ fontSize: "12px", color: "var(--text-tertiary)" }}>
                        Remember playback position for each file.
                      </div>
                    </span>
                    <input
                      type="checkbox"
                      checked={settings.resume_where_left_off}
                      onChange={(e) =>
                        setSettings(
                          ipcClient.updateSettings({
                            ...settings,
                            resume_where_left_off: e.target.checked,
                          }),
                        )
                      }
                    />
                  </label>

                  <label
                    style={{
                      display: "flex",
                      justifyContent: "space-between",
                      alignItems: "center",
                    }}
                  >
                    <span>
                      <strong>Pause when headphones disconnect</strong>
                      <div style={{ fontSize: "12px", color: "var(--text-tertiary)" }}>
                        Prevent audio from playing through speakers.
                      </div>
                    </span>
                    <input
                      type="checkbox"
                      checked={settings.pause_on_headphones_disconnect}
                      onChange={(e) =>
                        setSettings(
                          ipcClient.updateSettings({
                            ...settings,
                            pause_on_headphones_disconnect: e.target.checked,
                          }),
                        )
                      }
                    />
                  </label>

                  <label
                    style={{
                      display: "flex",
                      justifyContent: "space-between",
                      alignItems: "center",
                    }}
                  >
                    <span>
                      <strong>Note timestamp precision</strong>
                      <div style={{ fontSize: "12px", color: "var(--text-tertiary)" }}>
                        Controls captured timestamp rounding and display format.
                      </div>
                    </span>
                    <select
                      className="btn-secondary"
                      value={settings.timestamp_precision}
                      onChange={(e) =>
                        setSettings(
                          ipcClient.updateSettings({
                            ...settings,
                            timestamp_precision: e.target
                              .value as AppSettings["timestamp_precision"],
                          }),
                        )
                      }
                    >
                      <option value="nearest_second">Nearest second</option>
                      <option value="milliseconds">Milliseconds</option>
                    </select>
                  </label>
                </div>
              </section>
            </div>
          )}
        </main>
      </div>

      {/* Add / Edit Note Modal (Frame 4:88) */}
      {isNoteModalOpen && (
        <div className="modal-backdrop" role="dialog" aria-modal="true">
          <div className="modal-dialog" data-node-id="4:189">
            <div
              style={{
                display: "flex",
                justifyContent: "space-between",
                alignItems: "center",
              }}
            >
              <div>
                <div
                  className="tabular-nums"
                  style={{
                    fontSize: "11px",
                    fontWeight: 600,
                    color: "var(--accent-primary)",
                  }}
                >
                  CAPTURED AT{" "}
                  {formatTimestamp(capturedPositionMs, settings.timestamp_precision)}
                </div>
                <h2 style={{ fontSize: "22px", margin: "4px 0 0" }}>
                  {editingNoteId ? "Edit note" : "Add a note"}
                </h2>
              </div>
              <button
                type="button"
                className="btn-secondary"
                aria-label="Close dialog"
                onClick={() => setIsNoteModalOpen(false)}
              >
                ×
              </button>
            </div>

            <textarea
              className="note-textarea"
              placeholder="What do you want to remember about this moment?"
              value={noteBodyDraft}
              autoFocus
              onChange={(e) => setNoteBodyDraft(e.target.value)}
              onKeyDown={(e) => {
                if ((e.metaKey || e.ctrlKey) && e.key === "Enter") {
                  e.preventDefault();
                  handleSaveNote();
                }
              }}
            />

            {!editingNoteId && (
              <label
                style={{
                  display: "flex",
                  alignItems: "center",
                  gap: "10px",
                  padding: "12px",
                  borderRadius: "8px",
                  backgroundColor: "var(--bg-surface-2)",
                  border: "1px solid var(--border-control)",
                  fontSize: "12px",
                }}
              >
                <input
                  type="checkbox"
                  checked={isQuickMarkerDraft}
                  onChange={(e) => setIsQuickMarkerDraft(e.target.checked)}
                />
                <span>
                  <strong>Save as quick marker</strong> — Add the timestamp now and write
                  the note later.
                </span>
              </label>
            )}

            {modalValidationMsg && (
              <div style={{ color: "var(--status-error-fg)", fontSize: "12px" }}>
                {modalValidationMsg}
              </div>
            )}

            <div
              style={{
                display: "flex",
                justifyContent: "space-between",
                alignItems: "center",
              }}
            >
              <span style={{ fontSize: "11px", color: "var(--text-muted)" }}>
                ⌘ Enter to save
              </span>
              <div style={{ display: "flex", gap: "8px" }}>
                <button
                  type="button"
                  className="btn-secondary"
                  onClick={() => setIsNoteModalOpen(false)}
                >
                  Cancel
                </button>
                <button
                  type="button"
                  className="btn-primary"
                  onClick={handleSaveNote}
                >
                  Save note
                </button>
              </div>
            </div>
          </div>
        </div>
      )}

      {/* Toast Notification (Frame 4:578) */}
      {toastMessage && (
        <div className="toast-banner" role="status" aria-live="polite">
          <span>✓ {toastMessage}</span>
          {lastDeletedNote && (
            <button
              type="button"
              className="btn-secondary"
              onClick={() => {
                ipcClient.restoreNote(lastDeletedNote);
                refreshLibraryAndNotes(snapshot.media_id);
                setLastDeletedNote(null);
                setToastMessage("Note restored");
              }}
            >
              Undo
            </button>
          )}
          <button
            type="button"
            className="jump-link-btn"
            onClick={() => {
              setToastMessage(null);
              setLastDeletedNote(null);
            }}
          >
            Dismiss
          </button>
        </div>
      )}
    </div>
  );
}
