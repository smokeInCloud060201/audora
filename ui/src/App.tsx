import React, { useEffect, useMemo, useRef, useState } from "react";
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
  ThemeMode,
  TimestampPrecision,
} from "./types/domain";
import { formatDurationMs, progressPercent } from "./utils/format";

const PLAYBACK_SPEEDS = [0.5, 0.75, 1.0, 1.25, 1.5, 2.0];
const WAVEFORM_BAR_HEIGHTS = [24, 42, 68, 52, 88, 60, 34, 76, 94, 64, 46, 82, 58, 38, 72, 50, 32, 22];

type SettingsTab = "appearance" | "playback" | "notes" | "keyboard" | "privacy" | "about";

export function App(): React.ReactElement {
  const [settings, setSettings] = useState<AppSettings>(() => ipcClient.getSettings());
  const [snapshot, setSnapshot] = useState<PlaybackSnapshot>(() =>
    ipcClient.getPlaybackSnapshot(),
  );
  const [section, setSection] = useState<PrimarySection>("now_playing");
  const [sideTab, setSideTab] = useState<SidePanelTab>("notes");
  const [settingsTab, setSettingsTab] = useState<SettingsTab>("appearance");
  const [libraryFilter, setLibraryFilter] = useState<LibraryFilter>("all");
  const [searchQuery, setSearchQuery] = useState<string>("");
  const [libraryVersion, setLibraryVersion] = useState<number>(0);

  // Add/Edit Note modal state (Frame 4:88)
  const [isNoteModalOpen, setIsNoteModalOpen] = useState<boolean>(false);
  const [editingNote, setEditingNote] = useState<Note | null>(null);
  const [capturedTimestampMs, setCapturedTimestampMs] = useState<number>(0);
  const [noteBodyDraft, setNoteBodyDraft] = useState<string>("");
  const [isQuickMarkerDraft, setIsQuickMarkerDraft] = useState<boolean>(false);
  const [noteModalError, setNoteModalError] = useState<string | null>(null);
  const [openNoteMenuId, setOpenNoteMenuId] = useState<string | null>(null);

  // Open Media dialog & drag-and-drop state
  const [isOpenMediaModalOpen, setIsOpenMediaModalOpen] = useState<boolean>(false);
  const [openPathDraft, setOpenPathDraft] = useState<string>("");
  const [isDraggingFiles, setIsDraggingFiles] = useState<boolean>(false);

  // Library row action menu
  const [openLibraryMenuId, setOpenLibraryMenuId] = useState<string | null>(null);

  // Missing Media / Relink screen & AV-202 Mismatch Confirmation state (Frame 4:339)
  const [missingMediaTarget, setMissingMediaTarget] = useState<MediaItem | null>(null);
  const [relinkPathDraft, setRelinkPathDraft] = useState<string>(
    "/Users/local/Media/Sound Design Reel.mp4",
  );
  const [relinkError, setRelinkError] = useState<AppErrorPayload | null>(null);
  const [pendingMismatchRelink, setPendingMismatchRelink] = useState<{
    mediaId: string;
    newPath: string;
    payload: AppErrorPayload;
  } | null>(null);

  // Unsupported format screen state (Frame 4:399)
  const [unsupportedError, setUnsupportedError] = useState<{
    fileName: string;
    payload: AppErrorPayload;
  } | null>(null);

  // Minimum mode `? Help` popover (Frame 2:115)
  const [isMinHelpOpen, setIsMinHelpOpen] = useState<boolean>(false);

  // Undo toast for deleted note (Frame 4:578)
  const [deletedNoteToast, setDeletedNoteToast] = useState<Note | null>(null);

  const searchInputRef = useRef<HTMLInputElement | null>(null);
  const noteTextareaRef = useRef<HTMLTextAreaElement | null>(null);
  const fileInputRef = useRef<HTMLInputElement | null>(null);
  const mediaElementRef = useRef<HTMLVideoElement | HTMLAudioElement | null>(null);

  // Apply Dark / Light / System theme tokens to `<html data-theme="...">`
  useEffect(() => {
    const applyTheme = () => {
      const resolvedTheme =
        settings.theme === "system"
          ? typeof window !== "undefined" &&
            window.matchMedia?.("(prefers-color-scheme: light)").matches
            ? "light"
            : "dark"
          : settings.theme;
      document.documentElement.setAttribute("data-theme", resolvedTheme);
    };
    applyTheme();

    if (
      settings.theme === "system" &&
      typeof window !== "undefined" &&
      window.matchMedia
    ) {
      const mql = window.matchMedia("(prefers-color-scheme: light)");
      const listener = () => applyTheme();
      mql.addEventListener?.("change", listener);
      return () => mql.removeEventListener?.("change", listener);
    }
    return undefined;
  }, [settings.theme]);

  // Listen for audio output device changes (`Pause when headphones disconnect`)
  useEffect(() => {
    if (
      typeof navigator === "undefined" ||
      !navigator.mediaDevices ||
      !navigator.mediaDevices.addEventListener
    ) {
      return undefined;
    }
    const onDeviceChange = () => {
      setSnapshot(ipcClient.handleAudioRouteDisconnected());
    };
    navigator.mediaDevices.addEventListener("devicechange", onDeviceChange);
    return () =>
      navigator.mediaDevices.removeEventListener("devicechange", onDeviceChange);
  }, []);

  // Real-time playback clock tick when playing a library item without an external HTML5 media stream
  useEffect(() => {
    if (snapshot.state !== "playing") {
      return undefined;
    }
    const currentMedia = snapshot.media_id
      ? ipcClient.getMediaById(snapshot.media_id)
      : null;
    if (currentMedia?.stream_url) {
      return undefined;
    }
    const interval = window.setInterval(() => {
      setSnapshot(ipcClient.tickPlayback(250));
    }, 250);
    return () => window.clearInterval(interval);
  }, [snapshot.state, snapshot.media_id, snapshot.rate]);

  // Sync HTML5 `<video>` / `<audio>` element when a real blob stream is active
  useEffect(() => {
    const el = mediaElementRef.current;
    if (!el) return;
    el.playbackRate = snapshot.rate;
    if ("preservesPitch" in el) {
      el.preservesPitch = true;
    }
    el.volume = snapshot.volume;
    el.muted = snapshot.muted;
    if (snapshot.state === "playing" && el.paused) {
      void el.play().catch(() => {
        // Ignore autoplay restrictions in headless browsers
      });
    } else if (snapshot.state !== "playing" && !el.paused) {
      el.pause();
    }
  }, [snapshot.state, snapshot.rate, snapshot.volume, snapshot.muted, snapshot.media_id]);

  const libraryResult = useMemo(
    () => ipcClient.searchLibrary(searchQuery, libraryFilter),
    [searchQuery, libraryFilter, snapshot, isNoteModalOpen, deletedNoteToast, libraryVersion],
  );

  const allMedia = useMemo(
    () => ipcClient.searchLibrary("", "all").media_items,
    [snapshot, isNoteModalOpen, deletedNoteToast, missingMediaTarget, libraryVersion],
  );

  const activeMedia = useMemo(
    () => allMedia.find((m) => m.id === snapshot.media_id) ?? null,
    [allMedia, snapshot.media_id],
  );

  const activeNotes = useMemo(
    () => (activeMedia ? ipcClient.listNotes(activeMedia.id) : []),
    [activeMedia, isNoteModalOpen, deletedNoteToast, libraryVersion],
  );

  const missingTargetNotes = useMemo(
    () => (missingMediaTarget ? ipcClient.listNotes(missingMediaTarget.id) : []),
    [missingMediaTarget, libraryVersion],
  );

  // Global keyboard shortcuts (Section 5 of `docs/UI_DESIGN.md`)
  useEffect(() => {
    const onKeyDown = (e: KeyboardEvent) => {
      const target = e.target as HTMLElement | null;
      const isEditable =
        target instanceof HTMLInputElement ||
        target instanceof HTMLTextAreaElement ||
        target?.isContentEditable;

      if ((e.metaKey || e.ctrlKey) && e.key.toLowerCase() === "k") {
        e.preventDefault();
        setSection("library");
        setTimeout(() => searchInputRef.current?.focus(), 20);
        return;
      }

      if ((e.metaKey || e.ctrlKey) && e.key.toLowerCase() === "o") {
        e.preventDefault();
        setIsOpenMediaModalOpen(true);
        return;
      }

      if (e.key === "Escape") {
        if (pendingMismatchRelink) {
          setPendingMismatchRelink(null);
          return;
        }
        if (isNoteModalOpen) {
          setIsNoteModalOpen(false);
          return;
        }
        if (isOpenMediaModalOpen) {
          setIsOpenMediaModalOpen(false);
          return;
        }
        if (openNoteMenuId) {
          setOpenNoteMenuId(null);
          return;
        }
        if (openLibraryMenuId) {
          setOpenLibraryMenuId(null);
          return;
        }
        if (settings.window_mode === "minimum") {
          setSettings(ipcClient.setWindowMode("full"));
          return;
        }
      }

      if (isEditable) {
        return;
      }

      if (e.code === "Space") {
        e.preventDefault();
        setSnapshot(ipcClient.playbackCommand({ action: "toggle_play_pause" }));
      } else if (e.key === "ArrowLeft") {
        e.preventDefault();
        setSnapshot(ipcClient.playbackCommand({ action: "skip_relative", delta_ms: -5000 }));
      } else if (e.key === "ArrowRight") {
        e.preventDefault();
        setSnapshot(ipcClient.playbackCommand({ action: "skip_relative", delta_ms: 5000 }));
      } else if (e.key.toLowerCase() === "n" && e.shiftKey) {
        e.preventDefault();
        handleCreateQuickMarker();
      } else if (e.key.toLowerCase() === "n") {
        e.preventDefault();
        openAddNoteModal();
      } else if (e.key.toLowerCase() === "m") {
        e.preventDefault();
        setSettings((prev) =>
          ipcClient.setWindowMode(prev.window_mode === "full" ? "minimum" : "full"),
        );
      }
    };

    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  });

  const openAddNoteModal = () => {
    if (!activeMedia) return;
    setEditingNote(null);
    setCapturedTimestampMs(snapshot.position_ms);
    setNoteBodyDraft("");
    setIsQuickMarkerDraft(false);
    setNoteModalError(null);
    setIsNoteModalOpen(true);
    setTimeout(() => noteTextareaRef.current?.focus(), 20);
  };

  const openEditNoteModal = (note: Note) => {
    setEditingNote(note);
    setCapturedTimestampMs(note.position_ms);
    setNoteBodyDraft(note.body);
    setIsQuickMarkerDraft(note.is_quick_marker);
    setNoteModalError(null);
    setOpenNoteMenuId(null);
    setIsNoteModalOpen(true);
    setTimeout(() => noteTextareaRef.current?.focus(), 20);
  };

  const handleCreateQuickMarker = () => {
    if (!activeMedia) return;
    ipcClient.createNote({
      media_id: activeMedia.id,
      position_ms: snapshot.position_ms,
      is_quick_marker: true,
      body: "",
    });
    setSideTab("notes");
    setSnapshot(ipcClient.getPlaybackSnapshot());
    setLibraryVersion((v) => v + 1);
  };

  const handleSaveNote = () => {
    if (!activeMedia) return;
    try {
      if (editingNote) {
        ipcClient.updateNote({
          id: editingNote.id,
          body: noteBodyDraft,
        });
      } else {
        ipcClient.createNote({
          media_id: activeMedia.id,
          position_ms: capturedTimestampMs,
          is_quick_marker: isQuickMarkerDraft,
          body: noteBodyDraft,
        });
      }
      setIsNoteModalOpen(false);
      setSideTab("notes");
      setSnapshot(ipcClient.getPlaybackSnapshot());
      setLibraryVersion((v) => v + 1);
    } catch (err) {
      const payload = err as AppErrorPayload;
      setNoteModalError(payload.message ?? "Unable to save note.");
    }
  };

  const handleDeleteNote = (noteId: string) => {
    const deleted = ipcClient.deleteNote(noteId);
    setOpenNoteMenuId(null);
    if (deleted) {
      setDeletedNoteToast(deleted);
      setLibraryVersion((v) => v + 1);
      setTimeout(() => {
        setDeletedNoteToast((current) => (current?.id === deleted.id ? null : current));
      }, 6000);
    }
  };

  const handleUndoDelete = () => {
    if (!deletedNoteToast) return;
    ipcClient.restoreNote(deletedNoteToast);
    setDeletedNoteToast(null);
    setLibraryVersion((v) => v + 1);
  };

  const handleSelectLibraryItem = (item: MediaItem) => {
    setUnsupportedError(null);
    setOpenLibraryMenuId(null);
    if (item.availability === "missing") {
      setMissingMediaTarget(item);
      setRelinkError(null);
      setSection("library");
      return;
    }
    try {
      ipcClient.selectMediaById(item.id);
      setMissingMediaTarget(null);
      setSnapshot(ipcClient.getPlaybackSnapshot());
      setSection("now_playing");
    } catch (err) {
      const payload = err as AppErrorPayload;
      if (payload.code === "AV-101") {
        setMissingMediaTarget(item);
      }
    }
  };

  const handleOpenMediaSubmit = (e: React.FormEvent) => {
    e.preventDefault();
    try {
      ipcClient.openMediaPath(openPathDraft);
      setUnsupportedError(null);
      setMissingMediaTarget(null);
      setIsOpenMediaModalOpen(false);
      setOpenPathDraft("");
      setSnapshot(ipcClient.getPlaybackSnapshot());
      setLibraryVersion((v) => v + 1);
      setSection("now_playing");
    } catch (err) {
      const payload = err as AppErrorPayload;
      if (payload.code === "AV-104") {
        const fileName = openPathDraft.trim().split(/[\\/]/).pop() ?? openPathDraft.trim();
        setUnsupportedError({ fileName, payload });
        setIsOpenMediaModalOpen(false);
        setOpenPathDraft("");
        setSection("now_playing");
      }
    }
  };

  const handleNativeFilesSelected = (fileList: FileList | null) => {
    if (!fileList || fileList.length === 0) return;
    const descriptors = Array.from(fileList).map((f) => ({
      name: f.name,
      size: f.size,
      streamUrl:
        typeof URL !== "undefined" && typeof URL.createObjectURL === "function"
          ? URL.createObjectURL(f)
          : undefined,
    }));
    try {
      ipcClient.openLocalFiles(descriptors);
      setUnsupportedError(null);
      setMissingMediaTarget(null);
      setIsOpenMediaModalOpen(false);
      setSnapshot(ipcClient.getPlaybackSnapshot());
      setLibraryVersion((v) => v + 1);
      setSection("now_playing");
    } catch (err) {
      const payload = err as AppErrorPayload;
      if (payload.code === "AV-104") {
        setUnsupportedError({
          fileName: descriptors[0]?.name ?? "Unknown file",
          payload,
        });
        setIsOpenMediaModalOpen(false);
        setSection("now_playing");
      }
    }
  };

  const handleAttemptRelink = (mediaId: string, newPath: string, confirmMismatch = false) => {
    try {
      const relinked = ipcClient.relinkMedia(mediaId, newPath, { confirmMismatch });
      setPendingMismatchRelink(null);
      setRelinkError(null);
      setMissingMediaTarget(null);
      ipcClient.selectMediaById(relinked.id);
      setSnapshot(ipcClient.getPlaybackSnapshot());
      setLibraryVersion((v) => v + 1);
      setSection("now_playing");
    } catch (err) {
      const payload = err as AppErrorPayload;
      if (payload.code === "AV-202") {
        setPendingMismatchRelink({ mediaId, newPath, payload });
      } else {
        setRelinkError(payload);
      }
    }
  };

  const cyclePlaybackSpeed = () => {
    const idx = PLAYBACK_SPEEDS.indexOf(snapshot.rate);
    const nextRate = PLAYBACK_SPEEDS[(idx + 1) % PLAYBACK_SPEEDS.length];
    setSnapshot(ipcClient.playbackCommand({ action: "set_rate", rate: nextRate }));
  };

  const cycleRepeatMode = () => {
    const order: RepeatMode[] = ["off", "all", "one"];
    const nextMode = order[(order.indexOf(snapshot.repeat_mode) + 1) % order.length];
    setSnapshot(ipcClient.playbackCommand({ action: "set_repeat", mode: nextMode }));
  };

  // Minimum mode view (Frame 2:103 & Help Popover Frame 2:115)
  if (settings.window_mode === "minimum") {
    const pct = progressPercent(snapshot.position_ms, snapshot.duration_ms);
    return (
      <div className="minimum-mode-shell" data-testid="minimum-mode-shell">
        <div
          style={{
            position: "absolute",
            top: 16,
            left: 16,
            right: 16,
            display: "flex",
            justifyContent: "space-between",
          }}
        >
          <span
            style={{
              padding: "4px 10px",
              borderRadius: 6,
              background: "var(--bg-surface-1)",
              border: "1px solid var(--border-control)",
              fontSize: 12,
              color: "var(--text-secondary)",
            }}
          >
            {activeMedia?.display_name ?? "No media"} ·{" "}
            <span className="tabular-nums">{formatDurationMs(snapshot.position_ms)}</span>
          </span>
          <div style={{ display: "flex", gap: 8 }}>
            <button
              type="button"
              className="btn-secondary"
              aria-expanded={isMinHelpOpen}
              onClick={() => setIsMinHelpOpen((prev) => !prev)}
            >
              ? Help
            </button>
            <button
              type="button"
              className="btn-secondary"
              onClick={() => setSettings(ipcClient.setWindowMode("full"))}
            >
              ⤢ Full mode
            </button>
          </div>
        </div>

        <button
          type="button"
          className="play-circle-btn"
          aria-label={snapshot.state === "playing" ? "Pause" : "Play"}
          onClick={() =>
            setSnapshot(ipcClient.playbackCommand({ action: "toggle_play_pause" }))
          }
          style={{ width: 64, height: 64, fontSize: 22 }}
        >
          {snapshot.state === "playing" ? "❚❚" : "▶"}
        </button>
        <p style={{ marginTop: 12, fontSize: 13, color: "var(--text-tertiary)" }}>
          Minimal playback canvas ({pct}%)
        </p>

        {isMinHelpOpen && (
          <div
            className="minimum-help-popover"
            data-testid="minimum-help-popover"
            style={{ position: "absolute", bottom: 48, right: 20 }}
          >
            <div style={{ fontSize: 12, fontWeight: 600, color: "var(--text-primary)" }}>
              Space to play · Drag to seek
            </div>
            <div style={{ fontSize: 11, color: "var(--text-tertiary)" }}>
              Press N to add a timestamped note or Esc to return to the full workspace.
            </div>
            <button
              type="button"
              className="btn-primary"
              onClick={() => {
                setIsMinHelpOpen(false);
                setSettings(ipcClient.setWindowMode("full"));
              }}
            >
              Open full mode — Notes, library, and queue
            </button>
          </div>
        )}

        <div
          style={{
            position: "absolute",
            bottom: 0,
            left: 0,
            right: 0,
            height: 28,
            background: "var(--bg-surface-1)",
            display: "flex",
            alignItems: "center",
            padding: "0 16px",
          }}
        >
          <input
            type="range"
            aria-label="Seek timeline"
            className="scrubber-slider"
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
    );
  }

  return (
    <div
      className="audora-shell"
      onDragOver={(e) => {
        e.preventDefault();
        setIsDraggingFiles(true);
      }}
      onDragLeave={() => setIsDraggingFiles(false)}
      onDrop={(e) => {
        e.preventDefault();
        setIsDraggingFiles(false);
        handleNativeFilesSelected(e.dataTransfer?.files ?? null);
      }}
    >
      {/* Hidden native file picker input */}
      <input
        ref={fileInputRef}
        type="file"
        multiple
        accept="audio/*,video/*,.mp4,.mov,.webm,.mkv,.mp3,.wav,.flac,.ogg,.opus,.m4a"
        style={{ display: "none" }}
        onChange={(e) => handleNativeFilesSelected(e.target.files)}
      />

      {isDraggingFiles && (
        <div className="drop-zone-overlay" data-testid="drop-zone-overlay">
          <div style={{ fontSize: 24 }}>▣</div>
          <div style={{ fontSize: 16, fontWeight: 600 }}>Drop audio or video files to open</div>
          <div style={{ fontSize: 12, color: "var(--text-secondary)" }}>
            Files stay on your device and are never modified
          </div>
        </div>
      )}

      {/* Hybrid 56px Header Bar (Frame 2:4) */}
      <header className="audora-titlebar" data-tauri-drag-region>
        <div className="titlebar-brand">
          <div className="audora-mark" aria-hidden="true">
            <span style={{ height: 10 }} />
            <span style={{ height: 16 }} />
            <span style={{ height: 12 }} />
          </div>
          <span className="brand-title">Audora</span>
        </div>

        <div className="titlebar-center">
          <span className="status-dot" aria-hidden="true" />
          <span style={{ color: "var(--text-tertiary)" }}>Now playing</span>
          <span style={{ color: "var(--text-muted)" }}>—</span>
          <span style={{ fontWeight: 600 }}>
            {activeMedia ? activeMedia.display_name : "No media loaded"}
          </span>
          <span className="status-pill" data-state={snapshot.state} data-testid="playback-status-pill">
            {snapshot.state.charAt(0).toUpperCase() + snapshot.state.slice(1)}
          </span>
        </div>

        <div className="titlebar-actions">
          <button
            type="button"
            className="btn-secondary"
            aria-pressed={false}
            onClick={() => setSettings(ipcClient.setWindowMode("minimum"))}
          >
            ⤡ Minimum mode
          </button>
          <button
            type="button"
            className="btn-primary"
            onClick={() => setIsOpenMediaModalOpen(true)}
          >
            ▣ Open media
          </button>
        </div>
      </header>

      <div className="audora-body">
        {/* 76px Left Navigation Rail (Frame 2:20) */}
        <nav className="audora-nav-rail" aria-label="Primary navigation">
          <div className="nav-rail-group">
            <button
              type="button"
              className="nav-rail-btn"
              aria-current={section === "library" ? "page" : undefined}
              onClick={() => {
                setMissingMediaTarget(null);
                setSection("library");
              }}
            >
              <span className="nav-rail-icon" aria-hidden="true">
                ▤
              </span>
              <span>Library</span>
            </button>
            <button
              type="button"
              className="nav-rail-btn"
              aria-current={section === "now_playing" ? "page" : undefined}
              onClick={() => setSection("now_playing")}
            >
              <span className="nav-rail-icon" aria-hidden="true">
                ▶
              </span>
              <span>Playing</span>
            </button>
          </div>

          <div className="nav-rail-group">
            <button
              type="button"
              className="nav-rail-btn"
              aria-current={section === "settings" ? "page" : undefined}
              onClick={() => setSection("settings")}
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
          {section === "now_playing" && (
            <>
              <div className="player-column">
                {unsupportedError ? (
                  /* Unsupported Format Error Card (Frame 4:399) */
                  <div
                    className="media-canvas"
                    style={{ alignItems: "center", justifyContent: "center" }}
                  >
                    <div
                      role="alert"
                      style={{
                        maxWidth: 520,
                        padding: 32,
                        borderRadius: 14,
                        background: "var(--bg-surface-1)",
                        border: "1px solid var(--border-control)",
                      }}
                    >
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
                        ✕ Unsupported format ({unsupportedError.payload.code})
                      </span>
                      <h2 style={{ margin: "0 0 8px 0", fontSize: 22 }}>
                        {unsupportedError.payload.title}
                      </h2>
                      <p
                        style={{
                          margin: "0 0 16px 0",
                          color: "var(--text-secondary)",
                          fontSize: 13,
                        }}
                      >
                        {unsupportedError.payload.message}
                      </p>
                      <div
                        style={{
                          padding: 14,
                          borderRadius: 10,
                          background: "var(--bg-surface-2)",
                          fontSize: 12,
                          color: "var(--text-secondary)",
                          marginBottom: 20,
                        }}
                      >
                        <strong
                          style={{
                            display: "block",
                            marginBottom: 6,
                            color: "var(--text-primary)",
                          }}
                        >
                          What you can try
                        </strong>
                        {unsupportedError.payload.recovery_steps.map((step) => (
                          <div key={step}>{step}</div>
                        ))}
                      </div>
                      <div style={{ display: "flex", gap: 12 }}>
                        <button
                          type="button"
                          className="btn-primary"
                          onClick={() => setIsOpenMediaModalOpen(true)}
                        >
                          Choose another file
                        </button>
                        <button
                          type="button"
                          className="btn-secondary"
                          onClick={() => setUnsupportedError(null)}
                        >
                          Dismiss
                        </button>
                      </div>
                    </div>
                  </div>
                ) : activeMedia?.media_kind === "audio" ? (
                  /* Dedicated Audio-Only Canvas (Frame 4:3) */
                  <div className="audio-canvas" data-testid="audio-only-canvas">
                    {activeMedia.stream_url && (
                      <audio
                        ref={mediaElementRef as React.RefObject<HTMLAudioElement>}
                        src={activeMedia.stream_url}
                        onTimeUpdate={(e) =>
                          setSnapshot(
                            ipcClient.syncMediaElementPosition(
                              e.currentTarget.currentTime * 1000,
                              Number.isFinite(e.currentTarget.duration)
                                ? e.currentTarget.duration * 1000
                                : undefined,
                            ),
                          )
                        }
                        onEnded={() => setSnapshot(ipcClient.handleMediaEnded())}
                      />
                    )}
                    <div className="audio-ring-outer" aria-hidden="true" />
                    <div className="audio-ring-inner" aria-hidden="true" />
                    <div className="audio-artwork-card">
                      <div className="waveform-bars" aria-hidden="true">
                        {WAVEFORM_BAR_HEIGHTS.map((h, idx) => (
                          <span
                            key={idx}
                            className="waveform-bar"
                            style={{
                              height: `${h}px`,
                              opacity: snapshot.state === "playing" ? 0.95 : 0.55,
                            }}
                          />
                        ))}
                      </div>
                      <span
                        style={{
                          marginTop: 18,
                          fontSize: 10,
                          fontWeight: 600,
                          letterSpacing: "0.1em",
                          color: "var(--text-tertiary)",
                        }}
                      >
                        FIELD NOTES — AUDIO
                      </span>
                    </div>
                    <h1 className="canvas-headline" style={{ zIndex: 1 }}>
                      {activeMedia.display_name}
                    </h1>
                    <p className="canvas-subtitle" style={{ zIndex: 1 }}>
                      Ambient recording · Lossless audio ·{" "}
                      {formatDurationMs(snapshot.duration_ms)}
                    </p>
                  </div>
                ) : (
                  /* Video Playback Canvas (Frame 2:31) */
                  <div className="media-canvas" data-testid="video-canvas">
                    {activeMedia?.stream_url && (
                      <video
                        ref={mediaElementRef as React.RefObject<HTMLVideoElement>}
                        src={activeMedia.stream_url}
                        style={{
                          position: "absolute",
                          inset: 0,
                          width: "100%",
                          height: "100%",
                          objectFit: "contain",
                          zIndex: 0,
                        }}
                        onTimeUpdate={(e) =>
                          setSnapshot(
                            ipcClient.syncMediaElementPosition(
                              e.currentTarget.currentTime * 1000,
                              Number.isFinite(e.currentTarget.duration)
                                ? e.currentTarget.duration * 1000
                                : undefined,
                            ),
                          )
                        }
                        onEnded={() => setSnapshot(ipcClient.handleMediaEnded())}
                      />
                    )}
                    <div style={{ position: "relative", zIndex: 1 }}>
                      <p className="canvas-eyebrow">VIDEO · CHAPTER 03</p>
                      <h1 className="canvas-headline">
                        {activeMedia ? activeMedia.display_name : "Open a local media file"}
                      </h1>
                      <p className="canvas-subtitle">
                        {activeMedia
                          ? `${activeMedia.path} · ${formatDurationMs(snapshot.duration_ms)}`
                          : "Choose an audio or video file from your computer to begin."}
                      </p>
                    </div>
                  </div>
                )}

                {/* 128px Transport Bar with Scrubber Note Markers & Volume Slider (Frame 2:47) */}
                <section className="transport-bar" aria-label="Playback controls">
                  <div className="scrubber-row">
                    <span className="tabular-nums">
                      {formatDurationMs(snapshot.position_ms)}
                    </span>
                    <div className="scrubber-track-wrapper">
                      {snapshot.duration_ms &&
                        snapshot.duration_ms > 0 &&
                        activeNotes.map((note) => {
                          if (note.position_ms > (snapshot.duration_ms ?? 0)) {
                            return null;
                          }
                          const leftPct = Math.min(
                            100,
                            Math.max(0, (note.position_ms / snapshot.duration_ms!) * 100),
                          );
                          return (
                            <button
                              key={note.id}
                              type="button"
                              className="scrubber-note-marker"
                              style={{ left: `${leftPct}%` }}
                              title={`${formatDurationMs(note.position_ms)} — ${
                                note.body || "Quick marker"
                              }`}
                              aria-label={`Jump to note at ${formatDurationMs(note.position_ms)}`}
                              onClick={() =>
                                setSnapshot(ipcClient.activateNote(note.id))
                              }
                            />
                          );
                        })}
                      <input
                        type="range"
                        aria-label="Seek timeline"
                        className="scrubber-slider"
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
                    <span className="tabular-nums">
                      {formatDurationMs(snapshot.duration_ms)}
                    </span>
                  </div>

                  <div className="transport-controls-row">
                    <div style={{ display: "flex", alignItems: "center", gap: 8 }}>
                      <button
                        type="button"
                        className="btn-secondary"
                        aria-label={snapshot.muted ? "Unmute" : "Mute"}
                        onClick={() =>
                          setSnapshot(
                            ipcClient.playbackCommand({
                              action: "set_muted",
                              muted: !snapshot.muted,
                            }),
                          )
                        }
                      >
                        {snapshot.muted || snapshot.volume === 0 ? "🔇" : "◖))"}
                      </button>
                      <input
                        type="range"
                        aria-label="Volume"
                        min={0}
                        max={100}
                        value={snapshot.muted ? 0 : Math.round(snapshot.volume * 100)}
                        style={{ width: 72, accentColor: "var(--accent-primary)" }}
                        onChange={(e) => {
                          const v = Number(e.target.value) / 100;
                          if (snapshot.muted && v > 0) {
                            ipcClient.playbackCommand({ action: "set_muted", muted: false });
                          }
                          setSnapshot(
                            ipcClient.playbackCommand({ action: "set_volume", volume: v }),
                          );
                        }}
                      />
                      <button
                        type="button"
                        className="btn-secondary tabular-nums"
                        aria-label="Playback speed"
                        onClick={cyclePlaybackSpeed}
                      >
                        {snapshot.rate}×
                      </button>
                    </div>

                    <div className="transport-center-buttons">
                      <button
                        type="button"
                        className="btn-secondary"
                        aria-label="Previous item or restart"
                        onClick={() =>
                          setSnapshot(ipcClient.playbackCommand({ action: "previous" }))
                        }
                      >
                        ⏮
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
                        ⏭
                      </button>
                    </div>

                    <div style={{ display: "flex", gap: 8 }}>
                      <button
                        type="button"
                        className="btn-secondary"
                        title="Add quick marker at current timestamp (Shift+N)"
                        onClick={handleCreateQuickMarker}
                      >
                        ⚑ Marker
                      </button>
                      <button
                        type="button"
                        className="btn-secondary"
                        onClick={openAddNoteModal}
                      >
                        ＋ Add note at {formatDurationMs(snapshot.position_ms)}
                      </button>
                    </div>
                  </div>
                </section>
              </div>

              {/* 365px Right Side Panel: Queue | Notes (Frames 2:63 & 4:609) */}
              <aside className="side-panel" aria-label="Queue and notes">
                <div className="panel-tabs" role="tablist">
                  <button
                    type="button"
                    role="tab"
                    aria-selected={sideTab === "queue"}
                    className="panel-tab-btn"
                    onClick={() => setSideTab("queue")}
                  >
                    Queue ({snapshot.queue.length})
                  </button>
                  <button
                    type="button"
                    role="tab"
                    aria-selected={sideTab === "notes"}
                    className="panel-tab-btn"
                    onClick={() => setSideTab("notes")}
                  >
                    Notes ({activeNotes.length})
                  </button>
                </div>

                {sideTab === "notes" ? (
                  <>
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
                        onClick={openAddNoteModal}
                      >
                        ＋ New
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
                            <article key={note.id} className="note-card">
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
                                    onClick={() =>
                                      setSnapshot(ipcClient.activateNote(note.id))
                                    }
                                  >
                                    ▶ {formatDurationMs(note.position_ms)}
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
                                <div style={{ position: "relative", display: "flex", gap: 6 }}>
                                  <button
                                    type="button"
                                    className="jump-link-btn"
                                    aria-label={`More actions for note at ${formatDurationMs(note.position_ms)}`}
                                    onClick={() =>
                                      setOpenNoteMenuId((prev) =>
                                        prev === note.id ? null : note.id,
                                      )
                                    }
                                  >
                                    ⋯
                                  </button>
                                  {openNoteMenuId === note.id && (
                                    <div
                                      style={{
                                        position: "absolute",
                                        right: 0,
                                        top: 20,
                                        background: "var(--bg-dialog)",
                                        border: "1px solid var(--border-control)",
                                        borderRadius: 8,
                                        padding: 6,
                                        display: "flex",
                                        flexDirection: "column",
                                        gap: 4,
                                        zIndex: 10,
                                        minWidth: 110,
                                      }}
                                    >
                                      <button
                                        type="button"
                                        className="btn-secondary"
                                        onClick={() => openEditNoteModal(note)}
                                      >
                                        Edit note
                                      </button>
                                      <button
                                        type="button"
                                        className="btn-secondary"
                                        onClick={() => handleDeleteNote(note.id)}
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
                                <span
                                  style={{ fontSize: 11, color: "var(--text-muted)" }}
                                >
                                  Added {note.created_at}
                                </span>
                                <button
                                  type="button"
                                  className="jump-link-btn"
                                  onClick={() =>
                                    setSnapshot(ipcClient.activateNote(note.id))
                                  }
                                >
                                  Jump to moment →
                                </button>
                              </div>
                            </article>
                          );
                        })
                      )}
                    </div>
                  </>
                ) : (
                  /* Queue Panel (Frame 4:609) */
                  <>
                    <div className="panel-header">
                      <div>
                        <div style={{ fontSize: 14, fontWeight: 600 }}>Up next</div>
                        <div style={{ fontSize: 11, color: "var(--text-tertiary)" }}>
                          {snapshot.queue.length} items in queue
                        </div>
                      </div>
                      <div style={{ display: "flex", gap: 6 }}>
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
                          ⇄ Shuffle
                        </button>
                        <button
                          type="button"
                          className="btn-secondary"
                          aria-pressed={snapshot.repeat_mode !== "off"}
                          onClick={cycleRepeatMode}
                        >
                          ↻ Repeat: {snapshot.repeat_mode}
                        </button>
                      </div>
                    </div>

                    <div className="panel-scroll-list">
                      {snapshot.queue.map((id, idx) => {
                        const qItem = allMedia.find((m) => m.id === id);
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
                                onClick={() => handleSelectLibraryItem(qItem)}
                                style={{
                                  border: "none",
                                  background: "transparent",
                                  color: "inherit",
                                  textAlign: "left",
                                  padding: 0,
                                  cursor: "pointer",
                                  flex: 1,
                                }}
                              >
                                <div style={{ fontSize: 13, fontWeight: 600 }}>
                                  {isCurrent ? "▶ " : `${idx + 1}. `}
                                  {qItem.display_name}
                                </div>
                                <div
                                  style={{ fontSize: 11, color: "var(--text-tertiary)" }}
                                >
                                  {qItem.media_kind === "video" ? "Video" : "Audio"} ·{" "}
                                  <span className="tabular-nums">
                                    {formatDurationMs(qItem.duration_ms)}
                                  </span>
                                </div>
                              </button>
                              <button
                                type="button"
                                className="jump-link-btn"
                                aria-label={`Remove ${qItem.display_name} from queue`}
                                title="Remove from queue"
                                onClick={() =>
                                  setSnapshot(ipcClient.removeFromQueue(qItem.id))
                                }
                              >
                                ×
                              </button>
                            </div>
                          </div>
                        );
                      })}
                    </div>
                  </>
                )}
              </aside>
            </>
          )}

          {section === "library" && (
            <div className="content-page">
              {missingMediaTarget ? (
                /* Missing Media Recovery View with Preserved Notes & AV-202 Dialog (Frame 4:339) */
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
                    <h1 style={{ margin: "0 0 8px 0", fontSize: 22 }}>
                      We can’t find this file
                    </h1>
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
                      onChange={(e) => setRelinkPathDraft(e.target.value)}
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

                    <div style={{ display: "flex", gap: 10 }}>
                      <button
                        type="button"
                        className="btn-primary"
                        onClick={() =>
                          handleAttemptRelink(missingMediaTarget.id, relinkPathDraft, false)
                        }
                      >
                        Locate file…
                      </button>
                      <button
                        type="button"
                        className="btn-secondary"
                        onClick={() => {
                          ipcClient.removeMedia(missingMediaTarget.id);
                          setMissingMediaTarget(null);
                          setSnapshot(ipcClient.getPlaybackSnapshot());
                          setLibraryVersion((v) => v + 1);
                        }}
                      >
                        Remove from library
                      </button>
                      <button
                        type="button"
                        className="btn-secondary"
                        onClick={() => setMissingMediaTarget(null)}
                      >
                        Back
                      </button>
                    </div>
                  </div>

                  {/* Preserved Notes Card (Frame 4:378) */}
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
                      <strong>{missingMediaTarget.display_name}</strong> are safely stored in
                      Audora. Relinking restores playback without losing a single note.
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
                            {formatDurationMs(n.position_ms)}
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
                /* Library List & Empty State (Frames 4:206 & 4:300) */
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
                        Recent files, folders, and saved notes — everything stays on your
                        device
                      </p>
                    </div>
                    <div style={{ display: "flex", gap: 10 }}>
                      <button
                        type="button"
                        className="btn-secondary"
                        onClick={() => fileInputRef.current?.click()}
                      >
                        Choose local files…
                      </button>
                      <button
                        type="button"
                        className="btn-primary"
                        onClick={() => setIsOpenMediaModalOpen(true)}
                      >
                        ＋ Add media
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
                      onChange={(e) => setSearchQuery(e.target.value)}
                      style={{
                        width: 420,
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
                          onClick={() => setLibraryFilter(f)}
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
                    /* Empty State (Frame 4:300) */
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
                      <h2 style={{ margin: "0 0 8px 0", fontSize: 20 }}>
                        Your library is empty
                      </h2>
                      <p
                        style={{
                          margin: "0 0 20px 0",
                          fontSize: 13,
                          color: "var(--text-secondary)",
                        }}
                      >
                        Open an audio or video file from your computer to start listening,
                        watching, and capturing timestamped notes.
                      </p>
                      <button
                        type="button"
                        className="btn-primary"
                        onClick={() => setIsOpenMediaModalOpen(true)}
                      >
                        ▣ Open your first file
                      </button>
                    </div>
                  ) : (
                    <>
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
                            const pct = progressPercent(
                              item.last_position_ms,
                              item.duration_ms,
                            );
                            const isMissing = item.availability === "missing";
                            return (
                              <tr
                                key={item.id}
                                onClick={() => handleSelectLibraryItem(item)}
                                style={{
                                  cursor: "pointer",
                                  background: isMissing
                                    ? "var(--status-error-bg)"
                                    : "transparent",
                                }}
                              >
                                <td style={{ fontWeight: 600 }}>{item.display_name}</td>
                                <td>
                                  {isMissing
                                    ? "! Missing file"
                                    : item.media_kind === "video"
                                      ? "Video"
                                      : "Audio"}
                                </td>
                                <td className="tabular-nums">
                                  {formatDurationMs(item.duration_ms)}
                                </td>
                                <td>
                                  {item.note_count > 0 ? (
                                    <span className="timestamp-pill">
                                      {item.note_count} notes
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
                                        color: "var(--accent-text)",
                                        fontWeight: 600,
                                      }}
                                    >
                                      Locate file →
                                    </span>
                                  ) : (
                                    (item.last_opened_at ?? "Never")
                                  )}
                                </td>
                                <td
                                  onClick={(e) => e.stopPropagation()}
                                  style={{ position: "relative", textAlign: "right" }}
                                >
                                  <button
                                    type="button"
                                    className="jump-link-btn"
                                    aria-label={`Actions for ${item.display_name}`}
                                    onClick={() =>
                                      setOpenLibraryMenuId((prev) =>
                                        prev === item.id ? null : item.id,
                                      )
                                    }
                                  >
                                    ⋯
                                  </button>
                                  {openLibraryMenuId === item.id && (
                                    <div
                                      style={{
                                        position: "absolute",
                                        right: 16,
                                        top: 36,
                                        background: "var(--bg-dialog)",
                                        border: "1px solid var(--border-control)",
                                        borderRadius: 8,
                                        padding: 6,
                                        display: "flex",
                                        flexDirection: "column",
                                        gap: 4,
                                        zIndex: 20,
                                        minWidth: 150,
                                      }}
                                    >
                                      <button
                                        type="button"
                                        className="btn-secondary"
                                        onClick={() => handleSelectLibraryItem(item)}
                                      >
                                        Play now
                                      </button>
                                      <button
                                        type="button"
                                        className="btn-secondary"
                                        onClick={() => {
                                          setSnapshot(ipcClient.addToQueue(item.id));
                                          setOpenLibraryMenuId(null);
                                        }}
                                      >
                                        Add to queue
                                      </button>
                                      {!isMissing && (
                                        <button
                                          type="button"
                                          className="btn-secondary"
                                          onClick={() => {
                                            ipcClient.markMediaMissing(item.id);
                                            setOpenLibraryMenuId(null);
                                            setLibraryVersion((v) => v + 1);
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
                                          setOpenLibraryMenuId(null);
                                          setSnapshot(ipcClient.getPlaybackSnapshot());
                                          setLibraryVersion((v) => v + 1);
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

                      {/* Matching Notes Search Results Section (Sprint 4.3) */}
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
                              const parent = allMedia.find((m) => m.id === note.media_id);
                              return (
                                <div key={note.id} className="note-card">
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
                                      onClick={() => {
                                        setSnapshot(ipcClient.activateNote(note.id));
                                        setSection("now_playing");
                                      }}
                                    >
                                      ▶ {formatDurationMs(note.position_ms)} — Jump to moment
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
          )}

          {section === "settings" && (
            /* Settings View with All 6 Sub-Navigation Tabs (Frame 4:441) */
            <div className="content-page">
              <div style={{ marginBottom: 24 }}>
                <h1 style={{ margin: "0 0 4px 0", fontSize: 24 }}>Settings</h1>
                <p style={{ margin: 0, fontSize: 13, color: "var(--text-tertiary)" }}>
                  Customize appearance, playback behavior, notes, and local storage
                </p>
              </div>

              <div className="settings-layout">
                <nav className="settings-subnav" aria-label="Settings categories">
                  {(
                    [
                      ["appearance", "Appearance"],
                      ["playback", "Playback"],
                      ["notes", "Notes"],
                      ["keyboard", "Keyboard"],
                      ["privacy", "Privacy"],
                      ["about", "About"],
                    ] as const
                  ).map(([id, label]) => (
                    <button
                      key={id}
                      type="button"
                      className="settings-subnav-btn"
                      aria-current={settingsTab === id ? "true" : undefined}
                      onClick={() => setSettingsTab(id)}
                    >
                      <span>{label}</span>
                    </button>
                  ))}
                </nav>

                <div style={{ flex: 1, maxWidth: 680 }}>
                  {settingsTab === "appearance" && (
                    <section
                      style={{
                        padding: 24,
                        borderRadius: 10,
                        background: "var(--bg-surface-1)",
                        border: "1px solid var(--border-control)",
                      }}
                    >
                      <h2 style={{ margin: "0 0 6px 0", fontSize: 16 }}>Theme</h2>
                      <p
                        style={{
                          margin: "0 0 16px 0",
                          fontSize: 12,
                          color: "var(--text-tertiary)",
                        }}
                      >
                        Choose a dark studio appearance, a daylight surface, or follow your OS
                        preference
                      </p>
                      <div className="theme-preview-grid">
                        {(["dark", "light", "system"] as ThemeMode[]).map((t) => (
                          <button
                            key={t}
                            type="button"
                            className="theme-preview-card"
                            aria-pressed={settings.theme === t}
                            onClick={() =>
                              setSettings(
                                ipcClient.updateSettings({ ...settings, theme: t }),
                              )
                            }
                          >
                            <div
                              className="theme-mini-window"
                              style={{
                                background:
                                  t === "light"
                                    ? "#f6f8fa"
                                    : t === "dark"
                                      ? "#090b0e"
                                      : "linear-gradient(90deg, #090b0e 50%, #f6f8fa 50%)",
                              }}
                            >
                              <span
                                style={{
                                  width: 24,
                                  height: 4,
                                  borderRadius: 2,
                                  background: "var(--accent-primary)",
                                }}
                              />
                              <span
                                style={{
                                  width: "70%",
                                  height: 6,
                                  borderRadius: 3,
                                  background: "var(--border-control)",
                                }}
                              />
                            </div>
                            <span style={{ fontSize: 13, fontWeight: 600 }}>
                              {t.charAt(0).toUpperCase() + t.slice(1)}
                            </span>
                          </button>
                        ))}
                      </div>
                    </section>
                  )}

                  {settingsTab === "playback" && (
                    <section
                      style={{
                        padding: 24,
                        borderRadius: 10,
                        background: "var(--bg-surface-1)",
                        border: "1px solid var(--border-control)",
                        display: "flex",
                        flexDirection: "column",
                        gap: 18,
                      }}
                    >
                      <h2 style={{ margin: 0, fontSize: 16 }}>Playback behavior</h2>

                      <label
                        style={{
                          display: "flex",
                          justifyContent: "space-between",
                          alignItems: "center",
                        }}
                      >
                        <div>
                          <div style={{ fontSize: 13, fontWeight: 600 }}>
                            Resume where I left off
                          </div>
                          <div style={{ fontSize: 12, color: "var(--text-tertiary)" }}>
                            Remember playback position for each file in your library
                          </div>
                        </div>
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
                        <div>
                          <div style={{ fontSize: 13, fontWeight: 600 }}>
                            Pause when headphones disconnect
                          </div>
                          <div style={{ fontSize: 12, color: "var(--text-tertiary)" }}>
                            Automatically pause playback if audio output changes
                          </div>
                        </div>
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

                      <div
                        style={{
                          display: "flex",
                          justifyContent: "space-between",
                          alignItems: "center",
                        }}
                      >
                        <div>
                          <div style={{ fontSize: 13, fontWeight: 600 }}>
                            Default playback speed
                          </div>
                          <div style={{ fontSize: 12, color: "var(--text-tertiary)" }}>
                            Applied when opening a new media file
                          </div>
                        </div>
                        <select
                          aria-label="Default playback speed"
                          value={settings.default_playback_speed}
                          onChange={(e) =>
                            setSettings(
                              ipcClient.updateSettings({
                                ...settings,
                                default_playback_speed: Number(e.target.value),
                              }),
                            )
                          }
                          style={{
                            padding: "6px 10px",
                            borderRadius: 8,
                            border: "1px solid var(--border-control)",
                            background: "var(--bg-surface-2)",
                            color: "var(--text-primary)",
                          }}
                        >
                          {PLAYBACK_SPEEDS.map((s) => (
                            <option key={s} value={s}>
                              {s}×
                            </option>
                          ))}
                        </select>
                      </div>
                    </section>
                  )}

                  {settingsTab === "notes" && (
                    <section
                      style={{
                        padding: 24,
                        borderRadius: 10,
                        background: "var(--bg-surface-1)",
                        border: "1px solid var(--border-control)",
                      }}
                    >
                      <h2 style={{ margin: "0 0 14px 0", fontSize: 16 }}>
                        Timestamped notes
                      </h2>
                      <div
                        style={{
                          display: "flex",
                          justifyContent: "space-between",
                          alignItems: "center",
                        }}
                      >
                        <div>
                          <div style={{ fontSize: 13, fontWeight: 600 }}>
                            Note timestamp precision
                          </div>
                          <div style={{ fontSize: 12, color: "var(--text-tertiary)" }}>
                            Format used when pressing N to add a note
                          </div>
                        </div>
                        <div style={{ display: "flex", gap: 8 }}>
                          {(
                            [
                              ["nearest_second", "Nearest second"],
                              ["milliseconds", "Milliseconds"],
                            ] as [TimestampPrecision, string][]
                          ).map(([val, label]) => (
                            <button
                              key={val}
                              type="button"
                              className="btn-secondary"
                              aria-pressed={settings.timestamp_precision === val}
                              onClick={() =>
                                setSettings(
                                  ipcClient.updateSettings({
                                    ...settings,
                                    timestamp_precision: val,
                                  }),
                                )
                              }
                            >
                              {label}
                            </button>
                          ))}
                        </div>
                      </div>
                    </section>
                  )}

                  {settingsTab === "keyboard" && (
                    <section
                      style={{
                        padding: 24,
                        borderRadius: 10,
                        background: "var(--bg-surface-1)",
                        border: "1px solid var(--border-control)",
                      }}
                    >
                      <h2 style={{ margin: "0 0 14px 0", fontSize: 16 }}>
                        Keyboard shortcuts
                      </h2>
                      <div style={{ display: "flex", flexDirection: "column", gap: 10 }}>
                        {[
                          ["Play / Pause", "Space"],
                          ["Seek backward / forward 5s", "← / →"],
                          ["Add note at current timestamp", "N"],
                          ["Drop quick marker", "Shift + N"],
                          ["Save note in composer", "⌘/Ctrl + Enter"],
                          ["Search library & notes", "⌘/Ctrl + K"],
                          ["Open media file", "⌘/Ctrl + O"],
                          ["Toggle Minimum / Full mode", "M"],
                        ].map(([actionLabel, shortcut]) => (
                          <div
                            key={actionLabel}
                            style={{
                              display: "flex",
                              justifyContent: "space-between",
                              padding: "8px 0",
                              borderBottom: "1px solid var(--border-subtle)",
                              fontSize: 13,
                            }}
                          >
                            <span>{actionLabel}</span>
                            <code
                              style={{
                                padding: "2px 8px",
                                borderRadius: 6,
                                background: "var(--bg-surface-2)",
                                color: "var(--accent-text)",
                              }}
                            >
                              {shortcut}
                            </code>
                          </div>
                        ))}
                      </div>
                    </section>
                  )}

                  {settingsTab === "privacy" && (
                    <section
                      style={{
                        padding: 24,
                        borderRadius: 10,
                        background: "var(--bg-surface-1)",
                        border: "1px solid var(--border-control)",
                        display: "flex",
                        flexDirection: "column",
                        gap: 12,
                      }}
                    >
                      <h2 style={{ margin: 0, fontSize: 16 }}>
                        Local-first privacy & storage
                      </h2>
                      <p
                        style={{
                          margin: 0,
                          fontSize: 13,
                          color: "var(--text-secondary)",
                        }}
                      >
                        Audora stores your library and notes locally in a SQLite WAL database
                        with automatic pre-migration backups (`.pre-vN.bak`). Audora makes
                        zero background network requests and never modifies your media files.
                      </p>
                      <div
                        style={{
                          padding: 12,
                          borderRadius: 8,
                          background: "var(--bg-inset)",
                          border: "1px solid var(--border-control)",
                          fontSize: 12,
                        }}
                      >
                        <div style={{ color: "var(--text-tertiary)", marginBottom: 4 }}>
                          Local database path
                        </div>
                        <code>~/Library/Application Support/com.audora.app/audora.sqlite</code>
                      </div>
                    </section>
                  )}

                  {settingsTab === "about" && (
                    <section
                      style={{
                        padding: 24,
                        borderRadius: 10,
                        background: "var(--bg-surface-1)",
                        border: "1px solid var(--border-control)",
                        display: "flex",
                        flexDirection: "column",
                        gap: 10,
                      }}
                    >
                      <h2 style={{ margin: 0, fontSize: 16 }}>Audora v0.1.0</h2>
                      <p
                        style={{
                          margin: 0,
                          fontSize: 13,
                          color: "var(--text-secondary)",
                        }}
                      >
                        Cross-platform local-first audio and video player with timestamped
                        notes.
                      </p>
                      <div style={{ fontSize: 12, color: "var(--text-tertiary)" }}>
                        Licensed under Apache License 2.0 · Dynamically linked GStreamer 1.28.1
                        (LGPL-2.1+)
                      </div>
                    </section>
                  )}
                </div>
              </div>
            </div>
          )}
        </main>
      </div>

      {/* Add / Edit Note Modal (Frame 4:88) */}
      {isNoteModalOpen && (
        <div className="modal-backdrop" role="dialog" aria-modal="true" aria-label="Note composer">
          <div className="modal-dialog">
            <div style={{ display: "flex", justifyContent: "space-between", alignItems: "center" }}>
              <div>
                <h2 style={{ margin: 0, fontSize: 17 }}>
                  {editingNote ? "Edit note" : "Add note at current moment"}
                </h2>
                <p style={{ margin: "2px 0 0 0", fontSize: 12, color: "var(--text-tertiary)" }}>
                  {activeMedia?.display_name} · Playback continues while you type
                </p>
              </div>
              <button
                type="button"
                className="btn-secondary"
                onClick={() => setIsNoteModalOpen(false)}
              >
                ✕
              </button>
            </div>

            <div style={{ display: "flex", alignItems: "center", gap: 10 }}>
              <span className="timestamp-pill tabular-nums">
                ▶ {formatDurationMs(capturedTimestampMs)}
              </span>
              <span style={{ fontSize: 12, color: "var(--text-tertiary)" }}>
                Captured timestamp
              </span>
            </div>

            <textarea
              ref={noteTextareaRef}
              className="note-textarea"
              placeholder="Write what stood out at this moment…"
              value={noteBodyDraft}
              onChange={(e) => setNoteBodyDraft(e.target.value)}
              onKeyDown={(e) => {
                if ((e.metaKey || e.ctrlKey) && e.key === "Enter") {
                  e.preventDefault();
                  handleSaveNote();
                }
              }}
            />

            {!editingNote && (
              <label style={{ display: "flex", alignItems: "center", gap: 8, fontSize: 12 }}>
                <input
                  type="checkbox"
                  checked={isQuickMarkerDraft}
                  onChange={(e) => setIsQuickMarkerDraft(e.target.checked)}
                />
                <span>Save as quick marker (allows empty text)</span>
              </label>
            )}

            {noteModalError && (
              <div role="alert" style={{ fontSize: 12, color: "var(--status-error-text)" }}>
                {noteModalError}
              </div>
            )}

            <div style={{ display: "flex", justifyContent: "space-between", alignItems: "center" }}>
              <span style={{ fontSize: 11, color: "var(--text-muted)" }}>
                ⌘ + Enter to save · Esc to cancel
              </span>
              <div style={{ display: "flex", gap: 10 }}>
                <button
                  type="button"
                  className="btn-secondary"
                  onClick={() => setIsNoteModalOpen(false)}
                >
                  Cancel
                </button>
                <button type="button" className="btn-primary" onClick={handleSaveNote}>
                  Save note
                </button>
              </div>
            </div>
          </div>
        </div>
      )}

      {/* Relink Mismatch Confirmation Modal (`AV-202` in Sprint 4.3) */}
      {pendingMismatchRelink && (
        <div
          className="modal-backdrop"
          role="dialog"
          aria-modal="true"
          aria-label="Confirm relink mismatch"
        >
          <div className="modal-dialog">
            <h2 style={{ margin: 0, fontSize: 17 }}>
              {pendingMismatchRelink.payload.title} ({pendingMismatchRelink.payload.code})
            </h2>
            <p style={{ margin: 0, fontSize: 13, color: "var(--text-secondary)" }}>
              {pendingMismatchRelink.payload.message}
            </p>
            <div style={{ display: "flex", justifyContent: "flex-end", gap: 10 }}>
              <button
                type="button"
                className="btn-secondary"
                onClick={() => setPendingMismatchRelink(null)}
              >
                Cancel
              </button>
              <button
                type="button"
                className="btn-primary"
                onClick={() =>
                  handleAttemptRelink(
                    pendingMismatchRelink.mediaId,
                    pendingMismatchRelink.newPath,
                    true,
                  )
                }
              >
                Relink anyway
              </button>
            </div>
          </div>
        </div>
      )}

      {/* Open Media Dialog */}
      {isOpenMediaModalOpen && (
        <div className="modal-backdrop" role="dialog" aria-modal="true" aria-label="Open media file">
          <form className="modal-dialog" onSubmit={handleOpenMediaSubmit}>
            <div style={{ display: "flex", justifyContent: "space-between", alignItems: "center" }}>
              <h2 style={{ margin: 0, fontSize: 17 }}>Open local media file</h2>
              <button
                type="button"
                className="btn-secondary"
                onClick={() => fileInputRef.current?.click()}
              >
                Browse disk…
              </button>
            </div>
            <p style={{ margin: 0, fontSize: 12, color: "var(--text-tertiary)" }}>
              Supported formats: MP4 (H.264), WebM (VP8/VP9), WAV, FLAC, Ogg/Opus, MP3, M4A.
            </p>
            <input
              type="text"
              aria-label="Media file path"
              placeholder="/Users/local/Media/Session.mp4 or Camera_Original.prores"
              value={openPathDraft}
              onChange={(e) => setOpenPathDraft(e.target.value)}
              style={{
                width: "100%",
                padding: "10px 12px",
                borderRadius: 8,
                border: "1px solid var(--border-control)",
                background: "var(--bg-inset)",
                color: "var(--text-primary)",
              }}
            />
            <div style={{ display: "flex", justifyContent: "flex-end", gap: 10 }}>
              <button
                type="button"
                className="btn-secondary"
                onClick={() => setIsOpenMediaModalOpen(false)}
              >
                Cancel
              </button>
              <button type="submit" className="btn-primary">
                Open file
              </button>
            </div>
          </form>
        </div>
      )}

      {/* 6-second Undo Toast Banner (Frame 4:578) */}
      {deletedNoteToast && (
        <div className="toast-banner" role="status">
          <span>Note deleted</span>
          <button type="button" className="jump-link-btn" onClick={handleUndoDelete}>
            Undo
          </button>
        </div>
      )}
    </div>
  );
}
