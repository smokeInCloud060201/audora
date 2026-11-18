import React, { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { ipcClient, type NativeDroppedEntry } from "./api/ipc";
import { AddToPlaylistModal } from "./components/AddToPlaylistModal";
import { AudoraLogo } from "./components/AudoraLogo";
import {
  IconAlertCircle,
  IconArrowUp,
  IconClose,
  IconFlag,
  IconFolderOpen,
  IconLibrary,
  IconMinimizeMode,
  IconPanelRight,
  IconPause,
  IconPlay,
  IconPlaylists,
  IconPlus,
  IconSettings,
  IconSkipNext,
  IconSkipPrev,
  IconVolume,
  IconVolumeMuted,
} from "./components/Icons";
import { MediaLibraryView } from "./components/MediaLibraryView";
import { MinimumPlayerWindow } from "./components/MinimumPlayerWindow";
import { PlaylistsView } from "./components/PlaylistsView";
import { RightSidePanel } from "./components/RightSidePanel";
import { SettingsView, type SettingsTab } from "./components/SettingsView";
import { UpdateDialog } from "./components/UpdateDialog";
import type {
  AppErrorPayload,
  AppSettings,
  AppUpdateStatusDto,
  DiagnosticReport,
  LibraryFilter,
  MediaItem,
  Note,
  PlaybackSnapshot,
  Playlist,
  PrimarySection,
  RepeatMode,
  SidePanelTab,
  TrustedSourceFolder,
} from "./types/domain";
import {
  formatAriaProgress,
  formatDurationMs,
  shouldSuppressGlobalShortcut,
} from "./utils/format";
import { useModalFocusTrap } from "./utils/useModalFocusTrap";

const PLAYBACK_SPEEDS = [0.5, 0.75, 1.0, 1.25, 1.5, 2.0];
const WAVEFORM_BAR_HEIGHTS = [24, 42, 68, 52, 88, 60, 34, 76, 94, 64, 46, 82, 58, 38, 72, 50, 32, 22];

export function App(): React.ReactElement {
  const [settings, setSettings] = useState<AppSettings>(() => ipcClient.getSettings());
  const [snapshot, setSnapshot] = useState<PlaybackSnapshot>(() =>
    ipcClient.getPlaybackSnapshot(),
  );
  const [section, setSection] = useState<PrimarySection>(() =>
    ipcClient.getPlaybackSnapshot().media_id ? "now_playing" : "library",
  );
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

  // Drag-and-drop state
  const [isDraggingFiles, setIsDraggingFiles] = useState<boolean>(false);

  // Library row action menu
  const [openLibraryMenuId, setOpenLibraryMenuId] = useState<string | null>(null);
  const [libraryMenuPlacement, setLibraryMenuPlacement] = useState<"down" | "up">("down");

  // Missing Media / Relink screen & AV-202 Mismatch Confirmation state (Frame 4:339)
  const [missingMediaTarget, setMissingMediaTarget] = useState<MediaItem | null>(null);
  const [relinkPathDraft, setRelinkPathDraft] = useState<string>("");
  const [relinkError, setRelinkError] = useState<AppErrorPayload | null>(null);
  const [pendingMismatchRelink, setPendingMismatchRelink] = useState<{
    mediaId: string;
    newPath: string;
    probedDurationMs?: number;
    fileSizeBytes?: number;
    payload: AppErrorPayload;
  } | null>(null);

  // Unsupported format screen state (Frame 4:399)
  const [unsupportedError, setUnsupportedError] = useState<{
    fileName: string;
    payload: AppErrorPayload;
  } | null>(null);

  // Minimum mode `? Help` popover (Frame 2:115)
  const [isMinHelpOpen, setIsMinHelpOpen] = useState<boolean>(false);

  // Undo toast for deleted note/playlist (Frame 4:578) & software update state
  const [deletedNoteToast, setDeletedNoteToast] = useState<Note | null>(null);
  const [deletedPlaylistToast, setDeletedPlaylistToast] = useState<Playlist | null>(null);
  const [backupStatusMessage, setBackupStatusMessage] = useState<string | null>(null);
  const [diagnosticReport, setDiagnosticReport] = useState<DiagnosticReport | null>(null);
  const [updateStatus, setUpdateStatus] = useState<AppUpdateStatusDto | null>(null);
  const [isUpdateDialogOpen, setIsUpdateDialogOpen] = useState<boolean>(false);
  const [updateToastMessage, setUpdateToastMessage] = useState<string | null>(null);

  // Playlists state
  const [selectedPlaylistId, setSelectedPlaylistId] = useState<string | null>(null);
  const [addToPlaylistTarget, setAddToPlaylistTarget] = useState<MediaItem | null>(null);
  const [isSaveQueueModalOpen, setIsSaveQueueModalOpen] = useState<boolean>(false);
  const [saveQueueNameDraft, setSaveQueueNameDraft] = useState<string>("");
  const [saveQueueError, setSaveQueueError] = useState<string | null>(null);

  // Trusted Local Source Folders state (Spotify Local Files workflow in Settings)
  const [isPickingSourceFolder, setIsPickingSourceFolder] = useState<boolean>(false);
  const [sourceFolderError, setSourceFolderError] = useState<string | null>(null);
  const [rescanningFolderId, setRescanningFolderId] = useState<string | null>(null);
  const [isPlaylistsModalOpen, setIsPlaylistsModalOpen] = useState<boolean>(false);
  const [isLibraryModalOpen, setIsLibraryModalOpen] = useState<boolean>(false);
  const [isSettingsModalOpen, setIsSettingsModalOpen] = useState<boolean>(false);

  const searchInputRef = useRef<HTMLInputElement | null>(null);
  const noteTextareaRef = useRef<HTMLTextAreaElement | null>(null);
  const noteDialogRef = useRef<HTMLDivElement | null>(null);
  const mismatchDialogRef = useRef<HTMLDivElement | null>(null);
  const saveQueueDialogRef = useRef<HTMLFormElement | null>(null);
  const saveQueueInputRef = useRef<HTMLInputElement | null>(null);
  const fileInputRef = useRef<HTMLInputElement | null>(null);
  const mediaElementRef = useRef<HTMLVideoElement | HTMLAudioElement | null>(null);

  const updateToastTimerRef = useRef<number | null>(null);
  const deletedNoteToastTimerRef = useRef<number | null>(null);
  const deletedPlaylistToastTimerRef = useRef<number | null>(null);

  const userNavigatedRef = useRef<boolean>(false);
  const seekGenerationRef = useRef<number>(0);
  const appliedSeekGenRef = useRef<number>(0);
  const isPointerScrubbingRef = useRef<boolean>(false);
  const scrubPreviewMsRef = useRef<number | null>(null);
  const lastLiveSeekMsRef = useRef<number>(0);
  const [isPointerScrubbing, setIsPointerScrubbing] = useState<boolean>(false);
  const [scrubPreviewMs, setScrubPreviewMs] = useState<number | null>(null);

  useModalFocusTrap(isNoteModalOpen, noteDialogRef, noteTextareaRef);
  useModalFocusTrap(Boolean(pendingMismatchRelink), mismatchDialogRef);
  useModalFocusTrap(isSaveQueueModalOpen, saveQueueDialogRef, saveQueueInputRef);

  const showTransientUpdateToast = useCallback((message: string) => {
    if (updateToastTimerRef.current !== null) {
      window.clearTimeout(updateToastTimerRef.current);
    }
    setUpdateToastMessage(message);
    updateToastTimerRef.current = window.setTimeout(() => {
      updateToastTimerRef.current = null;
      setUpdateToastMessage(null);
    }, 5_000);
  }, []);

  useEffect(() => {
    ipcClient.onPersistenceError = (err) => {
      showTransientUpdateToast(`${err.title} (${err.code}): ${err.message}`);
    };
    return () => {
      ipcClient.onPersistenceError = undefined;
      if (updateToastTimerRef.current !== null) {
        window.clearTimeout(updateToastTimerRef.current);
      }
      if (deletedNoteToastTimerRef.current !== null) {
        window.clearTimeout(deletedNoteToastTimerRef.current);
      }
      if (deletedPlaylistToastTimerRef.current !== null) {
        window.clearTimeout(deletedPlaylistToastTimerRef.current);
      }
    };
  }, [showTransientUpdateToast]);

  const navigateSection = useCallback((target: PrimarySection) => {
    userNavigatedRef.current = true;
    setSection(target);
  }, []);

  const bumpExplicitSeek = useCallback(() => {
    seekGenerationRef.current += 1;
  }, []);

  const handleScrubPointerDown = useCallback(() => {
    isPointerScrubbingRef.current = true;
    setIsPointerScrubbing(true);
  }, []);

  const handleScrubChange = useCallback(
    (nextPosMs: number) => {
      const clamped = Math.max(
        0,
        snapshot.duration_ms != null ? Math.min(nextPosMs, snapshot.duration_ms) : nextPosMs,
      );
      if (isPointerScrubbingRef.current) {
        scrubPreviewMsRef.current = clamped;
        setScrubPreviewMs(clamped);
        const now = Date.now();
        if (now - lastLiveSeekMsRef.current >= 120) {
          lastLiveSeekMsRef.current = now;
          const el = mediaElementRef.current;
          if (el) {
            try {
              el.currentTime = clamped / 1000;
            } catch {
              // Ignore seek before ready state
            }
          }
        }
        return;
      }
      scrubPreviewMsRef.current = null;
      setScrubPreviewMs(null);
      bumpExplicitSeek();
      setSnapshot(ipcClient.playbackCommand({ action: "seek", position_ms: clamped }));
    },
    [snapshot.duration_ms, bumpExplicitSeek],
  );

  const handleScrubCommit = useCallback(
    (finalPosMs: number) => {
      if (!isPointerScrubbingRef.current && scrubPreviewMsRef.current === null && scrubPreviewMs === null) {
        return;
      }
      isPointerScrubbingRef.current = false;
      setIsPointerScrubbing(false);
      const targetPos = scrubPreviewMsRef.current ?? scrubPreviewMs ?? finalPosMs;
      scrubPreviewMsRef.current = null;
      setScrubPreviewMs(null);
      bumpExplicitSeek();
      setSnapshot(ipcClient.playbackCommand({ action: "seek", position_ms: targetPos }));
    },
    [scrubPreviewMs, bumpExplicitSeek],
  );

  useEffect(() => {
    if (!isPointerScrubbing || typeof window === "undefined") {
      return undefined;
    }
    const releaseGlobalScrub = () => {
      if (!isPointerScrubbingRef.current) return;
      isPointerScrubbingRef.current = false;
      setIsPointerScrubbing(false);
      const pendingPos = scrubPreviewMsRef.current;
      scrubPreviewMsRef.current = null;
      setScrubPreviewMs(null);
      if (pendingPos !== null && ipcClient.getPlaybackSnapshot().media_id) {
        bumpExplicitSeek();
        setSnapshot(ipcClient.playbackCommand({ action: "seek", position_ms: pendingPos }));
      }
    };
    window.addEventListener("pointerup", releaseGlobalScrub);
    window.addEventListener("pointercancel", releaseGlobalScrub);
    window.addEventListener("blur", releaseGlobalScrub);
    return () => {
      window.removeEventListener("pointerup", releaseGlobalScrub);
      window.removeEventListener("pointercancel", releaseGlobalScrub);
      window.removeEventListener("blur", releaseGlobalScrub);
    };
  }, [isPointerScrubbing, bumpExplicitSeek]);

  useEffect(() => {
    if (!snapshot.media_id && (isPointerScrubbingRef.current || scrubPreviewMsRef.current !== null)) {
      isPointerScrubbingRef.current = false;
      scrubPreviewMsRef.current = null;
      setIsPointerScrubbing(false);
      setScrubPreviewMs(null);
    }
  }, [snapshot.media_id]);

  // Hydrate persisted native state IMMEDIATELY (`C-6`), then sync enabled folders & probe container durations in background
  useEffect(() => {
    let cancelled = false;
    const init = async () => {
      await ipcClient.ensureHydratedFromNative();
      if (cancelled) return;

      const hydratedSettings = ipcClient.getSettings();
      const hydratedSnapshot = ipcClient.getPlaybackSnapshot();
      setSettings(hydratedSettings);
      ipcClient.syncNativeWindowMode(hydratedSettings.window_mode);
      setSnapshot(hydratedSnapshot);
      if (hydratedSnapshot.media_id && !userNavigatedRef.current) {
        setSection("now_playing");
      }
      setLibraryVersion((v) => v + 1);

      const importedCount = await ipcClient.syncEnabledSourceFolders();
      const probedAny = await ipcClient.refreshMediaContainerDurations();
      if (cancelled) return;
      if (importedCount > 0 || probedAny) {
        const refreshedSettings = ipcClient.getSettings();
        const refreshedSnapshot = ipcClient.getPlaybackSnapshot();
        setSettings(refreshedSettings);
        setSnapshot(refreshedSnapshot);
        if (refreshedSnapshot.media_id && !hydratedSnapshot.media_id && !userNavigatedRef.current) {
          setSection("now_playing");
        }
        setLibraryVersion((v) => v + 1);
      }

      if (ipcClient.getSettings().auto_check_updates !== false) {
        const status = await ipcClient.checkForUpdatesOnline(false);
        if (!cancelled) {
          setUpdateStatus(status);
        }
      }
    };
    void init();
    return () => {
      cancelled = true;
    };
  }, []);

  // Subscribe to native `wry` OS drag-and-drop hooks (`H-9`, `C-1`)
  useEffect(() => {
    if (typeof window === "undefined") return undefined;
    const win = window as Window & {
      __AUDORA_ON_DRAG_STATE__?: (dragging: boolean) => void;
      __AUDORA_ON_NATIVE_DROP__?: (entries: Array<string | NativeDroppedEntry>) => void;
    };
    const extractFirstDropFileName = (
      first: string | NativeDroppedEntry | undefined,
    ): string => {
      if (typeof first === "string") {
        return first.split(/[\\/]/).pop() || "Unknown file";
      }
      if (first && typeof first === "object") {
        if (typeof first.name === "string" && first.name.trim()) {
          return first.name.trim();
        }
        if (typeof first.path === "string" && first.path.trim()) {
          return first.path.split(/[\\/]/).pop() || "Unknown file";
        }
      }
      return "Unknown file";
    };
    win.__AUDORA_ON_DRAG_STATE__ = (dragging: boolean) => {
      setIsDraggingFiles(Boolean(dragging));
    };
    win.__AUDORA_ON_NATIVE_DROP__ = (entries: Array<string | NativeDroppedEntry>) => {
      setIsDraggingFiles(false);
      if (!Array.isArray(entries) || entries.length === 0) return;
      void (async () => {
        try {
          const res = await ipcClient.ingestNativeDroppedPaths(entries);
          if (res.openedItems.length > 0 || res.importedFolderCount > 0) {
            setUnsupportedError(null);
            setMissingMediaTarget(null);
            setSettings(ipcClient.getSettings());
            bumpExplicitSeek();
            setSnapshot(ipcClient.getPlaybackSnapshot());
            setLibraryVersion((v) => v + 1);
            setSection("now_playing");
          }
        } catch (err) {
          if (err && typeof err === "object" && "code" in err) {
            const payload = err as AppErrorPayload & { firstFileName?: string };
            if (payload.code === "AV-104" || payload.code === "AV-103") {
              const fileName = payload.firstFileName ?? extractFirstDropFileName(entries[0]);
              setUnsupportedError({ fileName, payload });
              setSection("now_playing");
            } else {
              showTransientUpdateToast(payload.message || "Unable to open dropped media.");
            }
          } else {
            console.error("Unexpected error handling native OS file drop:", err);
          }
        }
      })();
    };
    return () => {
      delete win.__AUDORA_ON_DRAG_STATE__;
      delete win.__AUDORA_ON_NATIVE_DROP__;
    };
  }, [bumpExplicitSeek, showTransientUpdateToast]);

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

  // Listen for audio output device disconnects (`M-6`: avoid false pauses when plugging devices IN)
  useEffect(() => {
    if (
      typeof navigator === "undefined" ||
      !navigator.mediaDevices ||
      !navigator.mediaDevices.addEventListener
    ) {
      return undefined;
    }
    let knownOutputs: Set<string> | null = null;
    const refreshKnownOutputs = async (): Promise<Set<string> | null> => {
      if (typeof navigator.mediaDevices.enumerateDevices !== "function") {
        return null;
      }
      try {
        const devices = await navigator.mediaDevices.enumerateDevices();
        const outputs = devices
          .filter((d) => d.kind === "audiooutput")
          .map((d, idx) => d.deviceId || `out_${idx}`);
        return outputs.length > 0 ? new Set(outputs) : null;
      } catch {
        return null;
      }
    };
    void refreshKnownOutputs().then((initial) => {
      knownOutputs = initial;
    });

    const onDeviceChange = () => {
      void (async () => {
        const nextOutputs = await refreshKnownOutputs();
        if (knownOutputs && nextOutputs) {
          let removedAny = false;
          for (const prevId of knownOutputs) {
            if (!nextOutputs.has(prevId)) {
              removedAny = true;
              break;
            }
          }
          knownOutputs = nextOutputs;
          if (!removedAny) {
            return;
          }
        } else if (nextOutputs) {
          knownOutputs = nextOutputs;
        }
        setSnapshot(ipcClient.handleAudioRouteDisconnected());
      })();
    };
    navigator.mediaDevices.addEventListener("devicechange", onDeviceChange);
    return () =>
      navigator.mediaDevices.removeEventListener("devicechange", onDeviceChange);
  }, []);

  // Flush playback position & state immediately on window close / page hide (`C-8`)
  useEffect(() => {
    const flush = () => {
      ipcClient.flushPendingPersistence();
    };
    window.addEventListener("beforeunload", flush);
    window.addEventListener("pagehide", flush);
    return () => {
      window.removeEventListener("beforeunload", flush);
      window.removeEventListener("pagehide", flush);
    };
  }, []);

  // Real-time playback clock tick ONLY in non-persisted headless/blob fallback when no native stream exists
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
    if (ipcClient.isPersistedRuntime() && currentMedia) {
      const marked = ipcClient.markMediaMissing(currentMedia.id);
      setSnapshot(ipcClient.getPlaybackSnapshot());
      setMissingMediaTarget(marked);
      setRelinkPathDraft(marked.path);
      setRelinkError(null);
      setSection("library");
      setLibraryVersion((v) => v + 1);
      return undefined;
    }
    const interval = window.setInterval(() => {
      setSnapshot(ipcClient.tickPlayback(250));
    }, 250);
    return () => window.clearInterval(interval);
  }, [snapshot.state, snapshot.media_id, snapshot.rate]);

  // Sync HTML5 `<video>` / `<audio>` element when a real media stream is active (`H-8`)
  useEffect(() => {
    const el = mediaElementRef.current;
    if (!el) return;
    if ("defaultPlaybackRate" in el) {
      el.defaultPlaybackRate = snapshot.rate;
    }
    el.playbackRate = snapshot.rate;
    if ("preservesPitch" in el) {
      el.preservesPitch = true;
    }
    el.volume = snapshot.volume;
    el.muted = snapshot.muted;
    const hasExplicitSeek = appliedSeekGenRef.current !== seekGenerationRef.current;
    if (!isPointerScrubbingRef.current) {
      const targetSec = snapshot.position_ms / 1000;
      const drift = Number.isFinite(el.currentTime) ? Math.abs(el.currentTime - targetSec) : 0;
      if (hasExplicitSeek || el.ended || drift > 1.25) {
        appliedSeekGenRef.current = seekGenerationRef.current;
        try {
          el.currentTime = targetSec;
        } catch {
          // Element metadata not ready yet; `onLoadedMetadata` will sync position.
        }
      }
    }
    if (snapshot.state === "playing" && el.paused) {
      void el.play().catch(() => {
        // Ignore autoplay restrictions in headless browsers
      });
    } else if (snapshot.state !== "playing" && !el.paused) {
      el.pause();
    }
  }, [
    snapshot.generation,
    snapshot.state,
    snapshot.rate,
    snapshot.volume,
    snapshot.muted,
    snapshot.media_id,
    snapshot.position_ms,
    settings.window_mode,
  ]);

  // Keep native desktop window dimensions & chrome synchronized with Full vs Minimum (360×168) mode
  useEffect(() => {
    ipcClient.syncNativeWindowMode(settings.window_mode);
  }, [settings.window_mode]);

  const allMedia = useMemo(
    () => ipcClient.searchLibrary("", "all").media_items,
    [libraryVersion, settings.source_folders],
  );

  const libraryResult = useMemo(() => {
    if (!searchQuery.trim() && libraryFilter === "all") {
      return { media_items: allMedia, matching_notes: [] };
    }
    return ipcClient.searchLibrary(searchQuery, libraryFilter);
  }, [allMedia, searchQuery, libraryFilter, libraryVersion]);

  const activeMedia = useMemo(
    () => allMedia.find((m) => m.id === snapshot.media_id) ?? null,
    [allMedia, snapshot.media_id],
  );

  const activeNotes = useMemo(
    () => (activeMedia ? ipcClient.listNotes(activeMedia.id) : []),
    [activeMedia, libraryVersion],
  );

  const missingTargetNotes = useMemo(
    () => (missingMediaTarget ? ipcClient.listNotes(missingMediaTarget.id) : []),
    [missingMediaTarget, libraryVersion],
  );

  const playlists = useMemo(
    () => ipcClient.listPlaylists(),
    [settings.source_folders, libraryVersion],
  );

  const showPlaylistsBar = settings.show_playlists_bar !== false;
  const showSidePanel = settings.show_side_panel !== false;
  const timestampPrecision = settings.timestamp_precision;

  const togglePlaylistsBar = () => {
    setSettings((prev) =>
      ipcClient.updateSettings({
        ...prev,
        show_playlists_bar: prev.show_playlists_bar === false,
      }),
    );
  };

  const toggleSidePanel = () => {
    setSettings((prev) =>
      ipcClient.updateSettings({
        ...prev,
        show_side_panel: prev.show_side_panel === false,
      }),
    );
  };

  const isAnyModalOpen = Boolean(
    isNoteModalOpen ||
      pendingMismatchRelink ||
      addToPlaylistTarget ||
      isSaveQueueModalOpen ||
      isUpdateDialogOpen ||
      isPlaylistsModalOpen ||
      isLibraryModalOpen ||
      isSettingsModalOpen ||
      (typeof document !== "undefined" && document.querySelector('[role="dialog"]')),
  );

  const shortcutHandlerRef = useRef<(e: KeyboardEvent) => void>(() => {});
  shortcutHandlerRef.current = (e: KeyboardEvent) => {
    if (e.defaultPrevented) return;
    const target = e.target as HTMLElement | null;
    const inputType = target instanceof HTMLInputElement ? target.type : null;

    if ((e.metaKey || e.ctrlKey) && !e.altKey && e.key.toLowerCase() === "k") {
      if (isAnyModalOpen) return;
      e.preventDefault();
      if (settings.window_mode === "minimum") {
        setSettings(ipcClient.setWindowMode("full"));
      }
      navigateSection("library");
      setTimeout(() => searchInputRef.current?.focus(), 20);
      return;
    }

    if ((e.metaKey || e.ctrlKey) && !e.altKey && e.key.toLowerCase() === "o") {
      if (isAnyModalOpen) return;
      e.preventDefault();
      triggerOpenMediaPicker();
      return;
    }

    if (e.key === "Escape") {
      if (addToPlaylistTarget) {
        setAddToPlaylistTarget(null);
        return;
      }
      if (isSaveQueueModalOpen) {
        setIsSaveQueueModalOpen(false);
        return;
      }
      if (isUpdateDialogOpen) {
        setIsUpdateDialogOpen(false);
        return;
      }
      if (pendingMismatchRelink) {
        setPendingMismatchRelink(null);
        return;
      }
      if (isNoteModalOpen) {
        setIsNoteModalOpen(false);
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
      if (isMinHelpOpen) {
        setIsMinHelpOpen(false);
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
        isAnyModalOpen,
        inputType,
      )
    ) {
      return;
    }

    if (e.metaKey || e.ctrlKey || e.altKey) {
      return;
    }

    if (e.code === "Space") {
      if (
        target?.closest(
          'button, a[href], summary, [role="button"], [role="switch"], [role="checkbox"], [role="tab"], [role="menuitem"], input[type="checkbox"], input[type="radio"], input[type="button"]',
        )
      ) {
        return;
      }
      e.preventDefault();
      setSnapshot(ipcClient.playbackCommand({ action: "toggle_play_pause" }));
    } else if (e.key === "ArrowLeft" || e.key === "ArrowRight") {
      if (
        target instanceof HTMLInputElement &&
        target.type === "range" &&
        target.dataset.seekScrubber !== "true"
      ) {
        return;
      }
      if (target?.closest('[role="tablist"], [role="menu"]')) {
        return;
      }
      e.preventDefault();
      const stepMs = e.shiftKey ? 15_000 : 5_000;
      const deltaMs = e.key === "ArrowLeft" ? -stepMs : stepMs;
      bumpExplicitSeek();
      setSnapshot(ipcClient.playbackCommand({ action: "skip_relative", delta_ms: deltaMs }));
    } else if (e.key === "ArrowUp" || e.key === "ArrowDown") {
      if (
        (target instanceof HTMLInputElement && target.type === "range") ||
        target?.closest(
          '[role="menu"], [role="menuitem"], [role="listbox"], select, .panel-scroll-list, .content-page, .playlist-picker-list',
        )
      ) {
        return;
      }
      e.preventDefault();
      const deltaVol = e.key === "ArrowUp" ? 0.05 : -0.05;
      const baseVol = snapshot.muted && deltaVol > 0 ? 0 : snapshot.volume;
      const nextVol = Math.round(Math.max(0, Math.min(1, baseVol + deltaVol)) * 100) / 100;
      if (snapshot.muted && nextVol > 0) {
        ipcClient.playbackCommand({ action: "set_muted", muted: false });
      }
      setSnapshot(ipcClient.playbackCommand({ action: "set_volume", volume: nextVol }));
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
    } else if (e.key === "[") {
      e.preventDefault();
      togglePlaylistsBar();
    } else if (e.key === "]") {
      e.preventDefault();
      toggleSidePanel();
    }
  };

  // Bind global keydown listener ONCE instead of tearing down on every timeupdate (`H-6`)
  useEffect(() => {
    const listener = (e: KeyboardEvent) => shortcutHandlerRef.current(e);
    window.addEventListener("keydown", listener);
    return () => window.removeEventListener("keydown", listener);
  }, []);

  // Dismiss open row/note action menus when clicking outside
  useEffect(() => {
    if (!openLibraryMenuId && !openNoteMenuId) return;
    const onPointerDown = (e: MouseEvent) => {
      const target = e.target as HTMLElement | null;
      if (target?.closest("[data-row-action-menu]")) return;
      setOpenLibraryMenuId(null);
      setOpenNoteMenuId(null);
    };
    window.addEventListener("mousedown", onPointerDown);
    return () => window.removeEventListener("mousedown", onPointerDown);
  }, [openLibraryMenuId, openNoteMenuId]);

  const getLivePlayheadMs = (): number => {
    if (scrubPreviewMs !== null) {
      return scrubPreviewMs;
    }
    const el = mediaElementRef.current;
    if (el && el.readyState >= 1 && Number.isFinite(el.currentTime) && el.currentTime >= 0) {
      const liveMs = Math.round(el.currentTime * 1000);
      return snapshot.duration_ms != null ? Math.min(liveMs, snapshot.duration_ms) : liveMs;
    }
    return snapshot.position_ms;
  };

  const openAddNoteModal = () => {
    if (!activeMedia) return;
    const capturedMs = getLivePlayheadMs();
    if (settings.window_mode === "minimum") {
      setIsMinHelpOpen(false);
      setSettings(ipcClient.setWindowMode("full"));
    }
    setEditingNote(null);
    setCapturedTimestampMs(capturedMs);
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
    const capturedMs = getLivePlayheadMs();
    void (async () => {
      try {
        await ipcClient.saveNoteVerified({
          mode: "create",
          media_id: activeMedia.id,
          position_ms: capturedMs,
          is_quick_marker: true,
          body: "",
        });
        setSideTab("notes");
        if (!showSidePanel) {
          setSettings(ipcClient.updateSettings({ ...settings, show_side_panel: true }));
        }
        setSnapshot(ipcClient.getPlaybackSnapshot());
        setLibraryVersion((v) => v + 1);
        showTransientUpdateToast("Quick marker added");
      } catch (err) {
        const payload = err as AppErrorPayload;
        showTransientUpdateToast(payload.message ?? "Unable to save quick marker.");
      }
    })();
  };

  const handleSaveNote = () => {
    if (!activeMedia) return;
    void (async () => {
      try {
        const saved = editingNote
          ? await ipcClient.saveNoteVerified({
              mode: "update",
              id: editingNote.id,
              body: noteBodyDraft,
            })
          : await ipcClient.saveNoteVerified({
              mode: "create",
              media_id: activeMedia.id,
              position_ms: capturedTimestampMs,
              is_quick_marker: isQuickMarkerDraft,
              body: noteBodyDraft,
            });
        setNoteModalError(null);
        setIsNoteModalOpen(false);
        setSideTab("notes");
        if (!showSidePanel) {
          setSettings(ipcClient.updateSettings({ ...settings, show_side_panel: true }));
        }
        setSnapshot(ipcClient.getPlaybackSnapshot());
        setLibraryVersion((v) => v + 1);
        showTransientUpdateToast(
          saved.is_quick_marker ? "Quick marker added" : "Note added",
        );
      } catch (err) {
        const payload = err as AppErrorPayload;
        setNoteModalError(payload.message ?? "Unable to save note.");
      }
    })();
  };

  const handleDeleteNote = (noteId: string) => {
    const deleted = ipcClient.deleteNote(noteId);
    setOpenNoteMenuId(null);
    if (deleted) {
      if (deletedNoteToastTimerRef.current !== null) {
        window.clearTimeout(deletedNoteToastTimerRef.current);
      }
      setDeletedNoteToast(deleted);
      setLibraryVersion((v) => v + 1);
      deletedNoteToastTimerRef.current = window.setTimeout(() => {
        deletedNoteToastTimerRef.current = null;
        setDeletedNoteToast((current) => (current?.id === deleted.id ? null : current));
      }, 6000);
    }
  };

  const handleUndoDelete = () => {
    if (!deletedNoteToast) return;
    if (deletedNoteToastTimerRef.current !== null) {
      window.clearTimeout(deletedNoteToastTimerRef.current);
      deletedNoteToastTimerRef.current = null;
    }
    ipcClient.restoreNote(deletedNoteToast);
    setDeletedNoteToast(null);
    setLibraryVersion((v) => v + 1);
  };

  const handlePlaylistDeleted = (deleted: Playlist) => {
    if (deletedPlaylistToastTimerRef.current !== null) {
      window.clearTimeout(deletedPlaylistToastTimerRef.current);
    }
    setDeletedPlaylistToast(deleted);
    setLibraryVersion((v) => v + 1);
    deletedPlaylistToastTimerRef.current = window.setTimeout(() => {
      deletedPlaylistToastTimerRef.current = null;
      setDeletedPlaylistToast((curr) => (curr?.id === deleted.id ? null : curr));
    }, 6_000);
  };

  const handleUndoDeletePlaylist = () => {
    if (!deletedPlaylistToast) return;
    if (deletedPlaylistToastTimerRef.current !== null) {
      window.clearTimeout(deletedPlaylistToastTimerRef.current);
      deletedPlaylistToastTimerRef.current = null;
    }
    const restored = ipcClient.restorePlaylist(deletedPlaylistToast);
    setDeletedPlaylistToast(null);
    setSelectedPlaylistId(restored.id);
    setLibraryVersion((v) => v + 1);
  };

  const handlePlayPlaylist = (
    playlistId: string,
    options?: { startMediaId?: string; shuffle?: boolean },
  ) => {
    try {
      const prevMediaId = snapshot.media_id;
      const nextSnap = ipcClient.playPlaylist(playlistId, options);
      setUnsupportedError(null);
      setMissingMediaTarget(null);
      setSnapshot(nextSnap);
      setSection("now_playing");
      const el = mediaElementRef.current;
      if (el && prevMediaId === nextSnap.media_id) {
        try {
          el.currentTime = nextSnap.position_ms / 1000;
        } catch {
          // `onLoadedMetadata` will sync position
        }
        if (el.paused) {
          void el.play().catch(() => {});
        }
      }
    } catch (err) {
      const payload = err as AppErrorPayload;
      showTransientUpdateToast(payload.message ?? "Unable to play playlist.");
    }
  };

  const handleSaveQueueAsPlaylist = (e: React.FormEvent) => {
    e.preventDefault();
    setSaveQueueError(null);
    try {
      const created = ipcClient.createPlaylistFromQueue(saveQueueNameDraft);
      setIsSaveQueueModalOpen(false);
      setSaveQueueNameDraft("");
      setSelectedPlaylistId(created.id);
      if (!showPlaylistsBar) {
        setSettings(ipcClient.updateSettings({ ...settings, show_playlists_bar: true }));
      }
      setLibraryVersion((v) => v + 1);
      showTransientUpdateToast(
        `Saved queue as playlist "${created.name}" (${created.item_ids.length} items).`,
      );
    } catch (err) {
      const payload = err as AppErrorPayload;
      setSaveQueueError(payload.message ?? "Unable to save queue as playlist.");
    }
  };

  const handleSelectLibraryItem = (item: MediaItem, autoPlay = false) => {
    setUnsupportedError(null);
    setOpenLibraryMenuId(null);
    if (item.availability === "missing") {
      setMissingMediaTarget(item);
      setRelinkPathDraft(item.path);
      setRelinkError(null);
      setSection("library");
      return;
    }
    try {
      ipcClient.selectMediaById(item.id, { autoPlay });
      setMissingMediaTarget(null);
      setSnapshot(ipcClient.getPlaybackSnapshot());
      setLibraryVersion((v) => v + 1);
      setSection("now_playing");
    } catch (err) {
      const payload = err as AppErrorPayload;
      if (payload.code === "AV-101") {
        setMissingMediaTarget(item);
      }
    }
  };

  const triggerOpenMediaPicker = () => {
    void (async () => {
      try {
        const res = await ipcClient.pickAndOpenNativeMediaFiles();
        if (!res.supported) {
          fileInputRef.current?.click();
          return;
        }
        if (res.cancelled || res.items.length === 0) {
          return;
        }
        setUnsupportedError(null);
        setMissingMediaTarget(null);
        setSnapshot(ipcClient.getPlaybackSnapshot());
        setLibraryVersion((v) => v + 1);
        setSection("now_playing");
      } catch (err) {
        const payload = err as AppErrorPayload & { firstFileName?: string };
        if (payload.code === "AV-104") {
          setUnsupportedError({
            fileName: payload.firstFileName ?? "Unknown file",
            payload,
          });
          setSection("now_playing");
        }
      }
    })();
  };

  const probeBlobIsoBmffDurationMs = async (file: File): Promise<number | undefined> => {
    const ext = file.name.split(".").pop()?.toLowerCase() ?? "";
    if (!["mp4", "m4a", "m4v", "mov"].includes(ext) || typeof file.slice !== "function") {
      return undefined;
    }
    const readAscii4 = (view: DataView, off: number) =>
      String.fromCharCode(
        view.getUint8(off),
        view.getUint8(off + 1),
        view.getUint8(off + 2),
        view.getUint8(off + 3),
      );
    const parseMvhdInMoov = (
      view: DataView,
      moovStart: number,
      moovHdrLen: number,
      moovEnd: number,
    ): number | undefined => {
      let cpos = moovStart + moovHdrLen;
      let guard = 0;
      while (cpos + 8 <= moovEnd && guard < 512) {
        guard += 1;
        const csz32 = view.getUint32(cpos, false);
        const ctype = readAscii4(view, cpos + 4);
        let csize = csz32;
        let chdr = 8;
        if (csz32 === 1) {
          if (cpos + 16 > moovEnd) break;
          csize = Number(view.getBigUint64(cpos + 8, false));
          chdr = 16;
        }
        if (csize < chdr || cpos + csize > moovEnd) break;
        if (ctype === "mvhd") {
          const pOff = cpos + chdr;
          const pLen = csize - chdr;
          if (pLen >= 20) {
            const ver = view.getUint8(pOff);
            let ts = 0n;
            let dur = 0n;
            if (ver === 1 && pLen >= 32) {
              ts = BigInt(view.getUint32(pOff + 20, false));
              const raw = view.getBigUint64(pOff + 24, false);
              dur = raw === 0xffffffffffffffffn ? 0n : raw;
            } else if (ver === 0) {
              ts = BigInt(view.getUint32(pOff + 12, false));
              const raw = view.getUint32(pOff + 16, false);
              dur = raw === 0xffffffff ? 0n : BigInt(raw);
            }
            if (ts > 0n && dur > 0n) {
              const ms = Number((dur * 1000n) / ts);
              if (Number.isFinite(ms) && ms > 0) return ms;
            }
          }
        }
        cpos += csize;
      }
      return undefined;
    };
    try {
      const headLen = Math.min(file.size, 131_072);
      const headBuf = await file.slice(0, headLen).arrayBuffer();
      const headView = new DataView(headBuf);
      let pos = 0;
      while (pos + 8 <= headView.byteLength) {
        const sz32 = headView.getUint32(pos, false);
        const btype = readAscii4(headView, pos + 4);
        let bsize = sz32;
        let hdrLen = 8;
        if (sz32 === 1) {
          if (pos + 16 > headView.byteLength) break;
          bsize = Number(headView.getBigUint64(pos + 8, false));
          hdrLen = 16;
        } else if (sz32 === 0) {
          bsize = headView.byteLength - pos;
        }
        if (bsize < hdrLen) break;
        if (btype === "moov" && pos + bsize <= headView.byteLength) {
          const found = parseMvhdInMoov(headView, pos, hdrLen, pos + bsize);
          if (found !== undefined) return found;
          break;
        }
        pos += bsize;
      }

      // Inspect tail 128 KiB slice for non-faststart MP4/MOV files where `moov` is written after `mdat`.
      if (file.size > 16) {
        const tailStart = Math.max(0, file.size - 131_072);
        const tailBuf = await file.slice(tailStart, file.size).arrayBuffer();
        const tailView = new DataView(tailBuf);
        for (let i = 0; i + 16 <= tailView.byteLength; i += 1) {
          if (tailView.getUint32(i + 4, false) !== 0x6d6f6f76) {
            continue;
          }
          const sz32 = tailView.getUint32(i, false);
          let bsize = sz32;
          let hdrLen = 8;
          if (sz32 === 1) {
            if (i + 16 > tailView.byteLength) continue;
            bsize = Number(tailView.getBigUint64(i + 8, false));
            hdrLen = 16;
          } else if (sz32 === 0) {
            bsize = tailView.byteLength - i;
          }
          if (bsize < hdrLen + 8 || i + bsize > tailView.byteLength) {
            continue;
          }
          const found = parseMvhdInMoov(tailView, i, hdrLen, i + bsize);
          if (found !== undefined) return found;
        }
      }
    } catch {
      // Fall back to media element metadata
    }
    return undefined;
  };

  const handleNativeFilesSelected = (fileList: FileList | null) => {
    if (!fileList || fileList.length === 0) return;
    const files = Array.from(fileList);
    void (async () => {
      const descriptors = await Promise.all(
        files.map(async (f) => ({
          name: f.name,
          size: f.size,
          durationMs: await probeBlobIsoBmffDurationMs(f),
          streamUrl:
            typeof URL !== "undefined" && typeof URL.createObjectURL === "function"
              ? URL.createObjectURL(f)
              : undefined,
        })),
      );
      try {
        ipcClient.openLocalFiles(descriptors);
        setUnsupportedError(null);
        setMissingMediaTarget(null);
        bumpExplicitSeek();
        setSnapshot(ipcClient.getPlaybackSnapshot());
        setLibraryVersion((v) => v + 1);
        setSection("now_playing");
      } catch (err) {
        for (const d of descriptors) {
          if (
            d.streamUrl?.startsWith("blob:") &&
            typeof URL !== "undefined" &&
            typeof URL.revokeObjectURL === "function"
          ) {
            try {
              URL.revokeObjectURL(d.streamUrl);
            } catch {
              // Ignore revoke errors
            }
          }
        }
        const payload = err as AppErrorPayload & { firstFileName?: string };
        if (payload.code === "AV-104" || payload.code === "AV-103") {
          setUnsupportedError({
            fileName: payload.firstFileName ?? descriptors[0]?.name ?? "Unknown file",
            payload,
          });
          setSection("now_playing");
        }
      }
    })();
  };

  const handleAttemptRelink = (
    mediaId: string,
    newPath: string,
    confirmMismatch = false,
    explicitProbedDurationMs?: number,
    explicitFileSizeBytes?: number,
  ) => {
    void (async () => {
      let probedDurationMs = explicitProbedDurationMs;
      if (probedDurationMs === undefined) {
        const probeRes = await ipcClient.probeNativeMediaDuration(newPath);
        if (probeRes && !probeRes.exists) {
          setRelinkError({
            code: "AV-101",
            category: "MediaUnavailable",
            title: "Replacement file not found",
            message:
              "Selected replacement file does not exist or is not registered yet. Use 'Locate file…' to select it from disk.",
            recovery_steps: ["Click 'Locate file…' to browse and authorize the relocated file."],
            recoverable: true,
          });
          return;
        }
        if (probeRes?.durationMs) {
          probedDurationMs = probeRes.durationMs;
        }
      }
      try {
        const relinked = ipcClient.relinkMedia(mediaId, newPath, {
          confirmMismatch,
          probedDurationMs,
          fileSizeBytes: explicitFileSizeBytes,
        });
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
          setPendingMismatchRelink({
            mediaId,
            newPath,
            probedDurationMs,
            fileSizeBytes: explicitFileSizeBytes,
            payload,
          });
        } else {
          setRelinkError(payload);
        }
      }
    })();
  };

  const handleBrowseRelinkFile = (mediaId: string) => {
    void (async () => {
      const picked = await ipcClient.pickSingleMediaFileForRelink();
      if (!picked.supported) {
        handleAttemptRelink(mediaId, relinkPathDraft, false);
        return;
      }
      if (picked.cancelled || !picked.file) {
        return;
      }
      setRelinkPathDraft(picked.file.path);
      handleAttemptRelink(
        mediaId,
        picked.file.path,
        false,
        picked.file.durationMs,
        picked.file.size,
      );
    })();
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

  const handleCheckForUpdates = async () => {
    try {
      const status = await ipcClient.checkForUpdatesOnline(true);
      setUpdateStatus(status);
      setIsUpdateDialogOpen(true);
    } catch {
      showTransientUpdateToast("Unable to check for updates right now.");
    }
  };

  const handleInstallUpdate = async (version: string) => {
    try {
      const { status, message } = await ipcClient.installNativeUpdate(version);
      setUpdateStatus(status);
      setSettings(ipcClient.getSettings());
      setIsUpdateDialogOpen(false);
      showTransientUpdateToast(message);
    } catch (err) {
      const payload = err as AppErrorPayload;
      setIsUpdateDialogOpen(false);
      showTransientUpdateToast(payload.message ?? "Failed to install update.");
    }
  };

  const handleDismissUpdate = (version: string) => {
    const nextStatus = ipcClient.dismissUpdate(version);
    setUpdateStatus(nextStatus);
    setSettings(ipcClient.getSettings());
    setIsUpdateDialogOpen(false);
    showTransientUpdateToast(`Deferred Audora v${version} update.`);
  };

  const handleCleanupAppData = () => {
    void (async () => {
      const summary = await ipcClient.cleanupApplicationDataNative();
      setSettings(ipcClient.getSettings());
      setSnapshot(ipcClient.getPlaybackSnapshot());
      setUpdateStatus(ipcClient.checkForUpdates(false));
      setDiagnosticReport(null);
      setMissingMediaTarget(null);
      setUnsupportedError(null);
      setLibraryVersion((v) => v + 1);
      setBackupStatusMessage(
        `Cleaned up local database & update cache (${summary.removed_paths.length} paths) — media files on disk untouched`,
      );
    })();
  };

  const sourceFolders = useMemo(
    () => ipcClient.getSourceFolders(),
    [settings, libraryVersion],
  );

  const currentVersion = updateStatus?.current_version || ipcClient.getCurrentVersion();

  const handleAddSourceButtonClick = async () => {
    if (isPickingSourceFolder) return;
    setSourceFolderError(null);
    setIsPickingSourceFolder(true);
    try {
      const res = await ipcClient.pickAndImportSourceFolder();
      if (!res.cancelled && res.folder) {
        setSettings(ipcClient.getSettings());
        setSnapshot(ipcClient.getPlaybackSnapshot());
        setLibraryVersion((v) => v + 1);
        showTransientUpdateToast(
          `Added source "${res.folder.label}" (${res.importedCount} media file${res.importedCount === 1 ? "" : "s"} imported).`,
        );
      }
    } catch (err) {
      const payload = err as AppErrorPayload;
      setSourceFolderError(payload.message ?? "Unable to select local source folder.");
    } finally {
      setIsPickingSourceFolder(false);
    }
  };

  const handleRescanSourceFolder = async (folder: TrustedSourceFolder) => {
    setSourceFolderError(null);
    setRescanningFolderId(folder.id);
    try {
      const res = await ipcClient.rescanSourceFolder(folder.id);
      setSettings(ipcClient.getSettings());
      setSnapshot(ipcClient.getPlaybackSnapshot());
      setLibraryVersion((v) => v + 1);
      showTransientUpdateToast(
        `Rescanned "${res.folder.label}" (${res.importedCount} new media file${res.importedCount === 1 ? "" : "s"}).`,
      );
    } catch (err) {
      const payload = err as AppErrorPayload;
      setSourceFolderError(payload.message ?? "Unable to rescan source folder.");
    } finally {
      setRescanningFolderId(null);
    }
  };

  const handleToggleSourceFolder = (folder: TrustedSourceFolder) => {
    ipcClient.toggleSourceFolder(folder.id, !folder.enabled);
    setSettings(ipcClient.getSettings());
    setSnapshot(ipcClient.getPlaybackSnapshot());
    setLibraryVersion((v) => v + 1);
  };

  const handleDeleteSourceFolder = (folder: TrustedSourceFolder) => {
    const res = ipcClient.removeSourceFolder(folder.id);
    setSettings(ipcClient.getSettings());
    setSnapshot(ipcClient.getPlaybackSnapshot());
    setLibraryVersion((v) => v + 1);
    if (res.removed) {
      const notedNote =
        res.preservedNotedCount > 0
          ? ` (${res.preservedNotedCount} item${res.preservedNotedCount === 1 ? "" : "s"} with saved notes preserved)`
          : "";
      showTransientUpdateToast(
        `Removed source "${res.folderLabel}"${notedNote} — files on disk untouched.`,
      );
    }
  };

  const handleJumpToNote = (noteId: string) => {
    try {
      const prevMediaId = snapshot.media_id;
      const nextSnap = ipcClient.activateNote(noteId);
      bumpExplicitSeek();
      setSnapshot(nextSnap);
      setSection("now_playing");
      const el = mediaElementRef.current;
      if (el && prevMediaId === nextSnap.media_id) {
        try {
          el.currentTime = nextSnap.position_ms / 1000;
        } catch {
          // Element metadata not ready yet; `onLoadedMetadata` will sync position.
        }
        if (el.paused) {
          void el.play().catch(() => {
            // Ignore autoplay restrictions in headless browsers
          });
        }
      }
    } catch (err) {
      const payload = err as AppErrorPayload;
      if (payload.code === "AV-101") {
        const noteMedia = allMedia.find((m) =>
          ipcClient.listNotes(m.id).some((n) => n.id === noteId),
        );
        if (noteMedia) {
          setMissingMediaTarget(noteMedia);
          setRelinkPathDraft(noteMedia.path);
          setRelinkError(null);
          setSection("library");
        }
      }
      showTransientUpdateToast(payload.message ?? "Unable to jump to this moment.");
    }
  };

  const handleMediaStreamError = (mediaId: string, mediaGen: number) => {
    const initialSnap = ipcClient.getPlaybackSnapshot();
    if (initialSnap.media_id !== mediaId || initialSnap.generation !== mediaGen) {
      return;
    }
    const failedMedia = ipcClient.getMediaById(mediaId);
    if (!failedMedia) return;
    void (async () => {
      const probe = await ipcClient.probeNativeMediaDuration(failedMedia.path);
      const currentSnap = ipcClient.getPlaybackSnapshot();
      if (currentSnap.media_id !== mediaId || currentSnap.generation !== mediaGen) {
        return;
      }
      if (probe && probe.exists) {
        // File exists on disk (`C-7`): surface `AV-104` decode failure instead of marking missing!
        const nextSnap = ipcClient.markPlaybackDecodeFailed(failedMedia.id);
        setSnapshot(nextSnap);
        if (settings.window_mode === "minimum") {
          setSettings(ipcClient.setWindowMode("full"));
        }
        if (nextSnap.error) {
          setUnsupportedError({
            fileName: failedMedia.display_name,
            payload: nextSnap.error,
          });
        }
        setSection("now_playing");
        return;
      }
      const marked = ipcClient.markMediaMissing(failedMedia.id);
      setSnapshot(ipcClient.getPlaybackSnapshot());
      setLibraryVersion((v) => v + 1);
      if (settings.window_mode === "minimum") {
        setSettings(ipcClient.setWindowMode("full"));
      }
      setMissingMediaTarget(marked);
      setRelinkPathDraft(marked.path);
      setRelinkError(null);
      setSection("library");
    })();
  };

  const handleMediaEndedSync = (mediaId: string, mediaGen: number) => {
    const beforeSnap = ipcClient.getPlaybackSnapshot();
    if (beforeSnap.media_id !== mediaId || beforeSnap.generation !== mediaGen) {
      return;
    }
    bumpExplicitSeek();
    setSnapshot(ipcClient.handleMediaEnded(mediaId, mediaGen));
    setLibraryVersion((v) => v + 1);
  };

  const handleMediaLoadedMetadata = (
    el: HTMLVideoElement | HTMLAudioElement,
    mediaId: string,
    mediaGen: number,
  ) => {
    const latestSnap = ipcClient.getPlaybackSnapshot();
    if (latestSnap.media_id !== mediaId || latestSnap.generation !== mediaGen) {
      return;
    }
    if ("defaultPlaybackRate" in el) {
      el.defaultPlaybackRate = latestSnap.rate;
    }
    el.playbackRate = latestSnap.rate;
    if ("preservesPitch" in el) {
      el.preservesPitch = true;
    }
    el.volume = latestSnap.volume;
    el.muted = latestSnap.muted;

    const probedDurationMs =
      Number.isFinite(el.duration) && el.duration > 0 ? el.duration * 1000 : undefined;
    if (latestSnap.position_ms > 0) {
      try {
        el.currentTime = latestSnap.position_ms / 1000;
      } catch {
        // Ignore seek before ready state
      }
    }
    const syncedSnap = ipcClient.syncMediaElementPosition(
      latestSnap.position_ms,
      probedDurationMs,
      mediaId,
      mediaGen,
    );
    setSnapshot(syncedSnap);

    // Upgrade audio-only `.mp4`/`.mov`/`.webm` containers (`videoWidth === 0 && videoHeight === 0`)
    // to `"audio"` so they render the Audio-Only Canvas instead of a blank video frame (`C-2`).
    if (
      el instanceof HTMLVideoElement &&
      el.videoWidth === 0 &&
      el.videoHeight === 0
    ) {
      const upgraded = ipcClient.upgradeMediaKindToAudio(mediaId);
      if (upgraded) {
        if (!el.paused) {
          el.pause();
        }
        setLibraryVersion((v) => v + 1);
        return;
      }
    }

    if (latestSnap.state === "playing" && el.paused) {
      void el.play().catch(() => {
        // Ignore autoplay restrictions in headless browsers
      });
    }
  };

  const handleMediaCanPlay = (
    el: HTMLVideoElement | HTMLAudioElement,
    mediaId: string,
    mediaGen: number,
  ) => {
    const latestSnap = ipcClient.getPlaybackSnapshot();
    if (latestSnap.media_id !== mediaId || latestSnap.generation !== mediaGen) {
      return;
    }
    if (el.playbackRate !== latestSnap.rate) {
      el.playbackRate = latestSnap.rate;
    }
    if (latestSnap.state === "playing" && el.paused) {
      void el.play().catch(() => {
        // Ignore autoplay restrictions in headless browsers
      });
    }
  };

  const handleMediaTimeUpdate = (
    el: HTMLVideoElement | HTMLAudioElement,
    mediaId: string,
    mediaGen: number,
  ) => {
    const nextSnap = ipcClient.syncMediaElementPosition(
      el.currentTime * 1000,
      Number.isFinite(el.duration) ? el.duration * 1000 : undefined,
      mediaId,
      mediaGen,
    );
    if (nextSnap.generation !== mediaGen) {
      if (nextSnap.media_id !== mediaId && !el.paused) {
        el.pause();
      }
      bumpExplicitSeek();
      setLibraryVersion((v) => v + 1);
    } else if (nextSnap.state === "ended" && !el.paused) {
      el.pause();
      setLibraryVersion((v) => v + 1);
    }
    setSnapshot(nextSnap);
  };

  const isMinimumMode = settings.window_mode === "minimum";
  const displayPositionMs = scrubPreviewMs ?? snapshot.position_ms;

  return (
    <>
      {activeMedia?.media_kind === "audio" && activeMedia.stream_url && (
        <audio
          key={activeMedia.id}
          ref={mediaElementRef as React.RefObject<HTMLAudioElement>}
          src={activeMedia.stream_url}
          autoPlay={snapshot.state === "playing"}
          onLoadedMetadata={(e) =>
            handleMediaLoadedMetadata(e.currentTarget, activeMedia.id, snapshot.generation)
          }
          onCanPlay={(e) =>
            handleMediaCanPlay(e.currentTarget, activeMedia.id, snapshot.generation)
          }
          onTimeUpdate={(e) =>
            handleMediaTimeUpdate(e.currentTarget, activeMedia.id, snapshot.generation)
          }
          onEnded={() => handleMediaEndedSync(activeMedia.id, snapshot.generation)}
          onError={() => handleMediaStreamError(activeMedia.id, snapshot.generation)}
        />
      )}

      {/* Hidden native fallback file picker input */}
      <input
        ref={fileInputRef}
        type="file"
        multiple
        accept="audio/*,video/*,.mp4,.mov,.webm,.mkv,.mp3,.wav,.flac,.ogg,.opus,.m4a,.aac"
        style={{ display: "none" }}
        onChange={(e) => {
          const selected = e.currentTarget.files;
          handleNativeFilesSelected(selected);
          e.currentTarget.value = "";
        }}
      />

      {/* Minimum mode · 360×168 view (Figma Frame 2:103 / 20:2) */}
      {isMinimumMode && (
        <MinimumPlayerWindow
          activeMedia={activeMedia}
          snapshot={snapshot}
          displayPositionMs={displayPositionMs}
          timestampPrecision={timestampPrecision}
          isHelpOpen={isMinHelpOpen}
          onToggleHelp={() => setIsMinHelpOpen((prev) => !prev)}
          onCloseHelp={() => setIsMinHelpOpen(false)}
          onUpdateSettings={setSettings}
          onUpdateSnapshot={setSnapshot}
          onScrubPointerDown={handleScrubPointerDown}
          onScrubChange={handleScrubChange}
          onScrubCommit={handleScrubCommit}
          onOpenMediaPicker={triggerOpenMediaPicker}
          onDropFiles={handleNativeFilesSelected}
        />
      )}

      {/* Keep `.audora-shell` & `<video>` mounted when in Minimum Mode to prevent dual streams or 00:00 resets (`C-7`) */}
      <div
        className="audora-shell"
        inert={isAnyModalOpen || undefined}
        aria-hidden={isMinimumMode || undefined}
        style={isMinimumMode ? { display: "none" } : undefined}
        onDragOver={(e) => {
          if (!Array.from(e.dataTransfer?.types ?? []).includes("Files")) return;
          e.preventDefault();
          setIsDraggingFiles(true);
        }}
        onDragLeave={(e) => {
          if (!e.currentTarget.contains(e.relatedTarget as Node | null)) {
            setIsDraggingFiles(false);
          }
        }}
        onDrop={(e) => {
          if (!Array.from(e.dataTransfer?.types ?? []).includes("Files")) return;
          e.preventDefault();
          setIsDraggingFiles(false);
          handleNativeFilesSelected(e.dataTransfer?.files ?? null);
        }}
      >

      {isDraggingFiles && (
        <div className="drop-zone-overlay" data-testid="drop-zone-overlay">
          <div style={{ color: "var(--accent-primary)" }}>
            <IconFolderOpen size={28} />
          </div>
          <div style={{ fontSize: 16, fontWeight: 600 }}>Drop audio or video files to open</div>
          <div style={{ fontSize: 12, color: "var(--text-secondary)" }}>
            Files stay on your device and are never modified
          </div>
        </div>
      )}

      {/* Hybrid 56px Header Bar (Frame 2:4 & Brand System 12:68) */}
      <header
        className="audora-titlebar"
        data-tauri-drag-region
        onMouseDown={(e) => {
          if (e.button !== 0 || e.detail !== 1) return;
          const target = e.target as HTMLElement | null;
          if (target?.closest("button, input, a, select, textarea, [role='button']")) {
            return;
          }
          ipcClient.dragNativeWindow();
        }}
      >
        <div className="titlebar-brand">
          <AudoraLogo variant="mark" size={28} isPlaying={snapshot.state === "playing"} />
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
            <IconMinimizeMode size={14} />
            <span>Minimum mode</span>
          </button>
          <button
            type="button"
            className="btn-primary"
            onClick={triggerOpenMediaPicker}
          >
            <IconFolderOpen size={14} />
            <span>Open media</span>
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
                navigateSection("library");
              }}
            >
              <span className="nav-rail-icon" aria-hidden="true">
                <IconLibrary size={18} />
              </span>
              <span>Library</span>
            </button>
            <button
              type="button"
              className="nav-rail-btn"
              aria-current={section === "now_playing" ? "page" : undefined}
              onClick={() => navigateSection("now_playing")}
            >
              <span className="nav-rail-icon" aria-hidden="true">
                <IconPlay size={18} />
              </span>
              <span>Playing</span>
            </button>
          </div>

          <div className="nav-rail-group">
            {updateStatus?.update_available && (
              <button
                type="button"
                className="nav-rail-btn nav-rail-update-btn"
                data-testid="nav-rail-update-btn"
                aria-label={`Update available: v${updateStatus.latest_version}`}
                title={`New version v${updateStatus.latest_version} available`}
                onClick={() => setIsUpdateDialogOpen(true)}
              >
                <span className="nav-rail-update-dot" aria-hidden="true" />
                <span className="nav-rail-icon" aria-hidden="true">
                  <IconArrowUp size={17} />
                </span>
                <span>Update</span>
              </button>
            )}
            <button
              type="button"
              className="nav-rail-btn"
              aria-current={section === "settings" ? "page" : undefined}
              onClick={() => navigateSection("settings")}
            >
              <span className="nav-rail-icon" aria-hidden="true">
                <IconSettings size={18} />
              </span>
              <span>Settings</span>
            </button>
            <span
              className="nav-rail-version tabular-nums"
              data-testid="nav-rail-version"
              title={`Current version: v${currentVersion}`}
            >
              v{currentVersion}
            </span>
          </div>
        </nav>

        {/* Main Workspace Stage */}
        <main className="audora-stage">
          <div
            data-testid="now-playing-workspace"
            aria-hidden={section !== "now_playing"}
            style={{
              display: section === "now_playing" ? "flex" : "none",
              flex: 1,
              minWidth: 0,
              minHeight: 0,
            }}
          >
            {showPlaylistsBar && (
              <PlaylistsView
                playlists={playlists}
                allMedia={allMedia}
                activeMediaId={snapshot.media_id}
                selectedPlaylistId={selectedPlaylistId}
                onSelectPlaylist={setSelectedPlaylistId}
                onPlayPlaylist={handlePlayPlaylist}
                onRelinkMissingMedia={(item) => {
                  setMissingMediaTarget(item);
                  setRelinkPathDraft(item.path);
                  setRelinkError(null);
                  setSection("library");
                }}
                onOpenLocalFiles={triggerOpenMediaPicker}
                onPlaylistsMutated={(msg) => {
                  setLibraryVersion((v) => v + 1);
                  if (msg) {
                    showTransientUpdateToast(msg);
                  }
                }}
                onPlaylistDeleted={handlePlaylistDeleted}
                onCollapse={togglePlaylistsBar}
                timestampPrecision={timestampPrecision}
                onModalOpenChange={setIsPlaylistsModalOpen}
              />
            )}

              <div className="player-column" style={{ position: "relative" }}>
                {activeMedia?.media_kind === "audio" ? (
                  /* Dedicated Audio-Only Canvas (Frame 4:3) */
                  <div
                    className={`audio-canvas ${snapshot.state === "playing" ? "is-playing" : ""}`}
                    data-testid="audio-only-canvas"
                  >
                    <div className="audio-ring-outer" aria-hidden="true" />
                    <div className="audio-ring-inner" aria-hidden="true" />
                    <div className="audio-artwork-card">
                      <div
                        className={`waveform-bars ${snapshot.state === "playing" ? "is-playing" : ""}`}
                        aria-hidden="true"
                      >
                        {WAVEFORM_BAR_HEIGHTS.map((h, idx) => (
                          <span
                            key={idx}
                            className="waveform-bar"
                            style={
                              {
                                height: `${h}px`,
                                opacity: snapshot.state === "playing" ? 0.95 : 0.55,
                                "--wave-dur": `${700 + ((idx * 137) % 460)}ms`,
                                "--wave-delay": `-${(idx * 95) % 820}ms`,
                              } as React.CSSProperties
                            }
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
                        LOCAL AUDIO
                      </span>
                    </div>
                    <h1 className="canvas-headline" style={{ zIndex: 1 }}>
                      {activeMedia.display_name}
                    </h1>
                    <p className="canvas-subtitle" style={{ zIndex: 1 }}>
                      Local audio · {formatDurationMs(snapshot.duration_ms, timestampPrecision)}
                    </p>
                  </div>
                ) : (
                  /* Video Playback Canvas & Empty Player State (Frame 2:31) — kept mounted beneath unsupportedError overlay (`M-6`) */
                  <div
                    className={`media-canvas ${activeMedia?.stream_url ? "has-video-stream" : ""}`}
                    data-testid="video-canvas"
                  >
                    {activeMedia?.stream_url && (
                      <video
                        key={activeMedia.id}
                        ref={mediaElementRef as React.RefObject<HTMLVideoElement>}
                        src={activeMedia.stream_url}
                        autoPlay={snapshot.state === "playing"}
                        style={{
                          position: "absolute",
                          inset: 0,
                          width: "100%",
                          height: "100%",
                          objectFit: "contain",
                          zIndex: 0,
                        }}
                        onLoadedMetadata={(e) =>
                          handleMediaLoadedMetadata(
                            e.currentTarget,
                            activeMedia.id,
                            snapshot.generation,
                          )
                        }
                        onCanPlay={(e) =>
                          handleMediaCanPlay(e.currentTarget, activeMedia.id, snapshot.generation)
                        }
                        onTimeUpdate={(e) =>
                          handleMediaTimeUpdate(
                            e.currentTarget,
                            activeMedia.id,
                            snapshot.generation,
                          )
                        }
                        onEnded={() => handleMediaEndedSync(activeMedia.id, snapshot.generation)}
                        onError={() => handleMediaStreamError(activeMedia.id, snapshot.generation)}
                      />
                    )}
                    {(!activeMedia || !activeMedia.stream_url) && !unsupportedError && (
                      <div style={{ position: "relative", zIndex: 1 }}>
                        <p className="canvas-eyebrow">
                          {activeMedia ? "LOCAL VIDEO" : "NO MEDIA LOADED"}
                        </p>
                        <h1 className="canvas-headline">
                          {activeMedia ? activeMedia.display_name : "Open a local media file"}
                        </h1>
                        <p className="canvas-subtitle">
                          {activeMedia
                            ? `${activeMedia.path} · ${formatDurationMs(snapshot.duration_ms, timestampPrecision)}`
                            : "Choose an audio or video file from your device to begin."}
                        </p>
                        {!activeMedia && (
                          <div style={{ display: "flex", gap: 10, marginTop: 18 }}>
                            <button
                              type="button"
                              className="btn-primary"
                              onClick={triggerOpenMediaPicker}
                            >
                              <IconFolderOpen size={14} />
                              <span>Open media…</span>
                            </button>
                          </div>
                        )}
                      </div>
                    )}
                  </div>
                )}

                {unsupportedError && (
                  /* Unsupported Format Error Card Overlay (Frame 4:399) */
                  <div
                    style={{
                      position: "absolute",
                      inset: "24px 36px 152px 36px",
                      display: "flex",
                      alignItems: "center",
                      justifyContent: "center",
                      background: "var(--bg-media-canvas)",
                      borderRadius: "var(--radius-card)",
                      border: "1px solid var(--border-subtle)",
                      zIndex: 10,
                      padding: 24,
                    }}
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
                          display: "inline-flex",
                          alignItems: "center",
                          gap: 6,
                          padding: "4px 10px",
                          borderRadius: 6,
                          background: "var(--status-error-bg)",
                          color: "var(--status-error-text)",
                          fontSize: 12,
                          fontWeight: 600,
                          marginBottom: 12,
                        }}
                      >
                        <IconAlertCircle size={14} />
                        <span>
                          {unsupportedError.payload.code === "AV-103"
                            ? `Playback error (${unsupportedError.payload.code})`
                            : `Unsupported format (${unsupportedError.payload.code})`}
                        </span>
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
                          onClick={triggerOpenMediaPicker}
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
                )}

                {/* 128px Transport Bar with Scrubber Note Markers & Volume Slider (Frame 2:47) */}
                <section className="transport-bar" aria-label="Playback controls">
                  <div className="scrubber-row">
                    <span className="tabular-nums">
                      {formatDurationMs(displayPositionMs, timestampPrecision)}
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
                              title={`${formatDurationMs(note.position_ms, timestampPrecision)} — ${
                                note.body || "Quick marker"
                              }`}
                              aria-label={`Jump to note at ${formatDurationMs(note.position_ms, timestampPrecision)}`}
                              onClick={() => handleJumpToNote(note.id)}
                            />
                          );
                        })}
                      <input
                        type="range"
                        data-seek-scrubber="true"
                        aria-label="Seek timeline"
                        aria-valuetext={formatAriaProgress(displayPositionMs, snapshot.duration_ms)}
                        className="scrubber-slider"
                        disabled={!activeMedia}
                        min={0}
                        max={snapshot.duration_ms ?? 100}
                        value={displayPositionMs}
                        style={
                          {
                            "--scrub-pct": `${
                              snapshot.duration_ms && snapshot.duration_ms > 0
                                ? Math.min(100, Math.max(0, (displayPositionMs / snapshot.duration_ms) * 100))
                                : 0
                            }%`,
                          } as React.CSSProperties
                        }
                        onPointerDown={handleScrubPointerDown}
                        onChange={(e) => handleScrubChange(Number(e.target.value))}
                        onPointerUp={(e) => handleScrubCommit(Number(e.currentTarget.value))}
                        onPointerCancel={(e) => handleScrubCommit(Number(e.currentTarget.value))}
                      />
                    </div>
                    <span className="tabular-nums">
                      {formatDurationMs(snapshot.duration_ms, timestampPrecision)}
                    </span>
                  </div>

                  <div className="transport-controls-row">
                    <div className="transport-side-left">
                      <button
                        type="button"
                        className="btn-secondary transport-collapsible-btn"
                        data-testid="toggle-playlists-bar-btn"
                        aria-pressed={showPlaylistsBar}
                        aria-label={showPlaylistsBar ? "Hide playlists bar" : "Show playlists bar"}
                        title={showPlaylistsBar ? "Hide playlists bar ([)" : "Show playlists bar ([)"}
                        onClick={togglePlaylistsBar}
                      >
                        <IconPlaylists size={14} />
                        <span className="transport-btn-label">Playlists</span>
                      </button>
                      <button
                        type="button"
                        className="btn-secondary transport-icon-btn"
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
                        {snapshot.muted || snapshot.volume === 0 ? (
                          <IconVolumeMuted size={15} />
                        ) : (
                          <IconVolume size={15} />
                        )}
                      </button>
                      <input
                        type="range"
                        aria-label="Volume"
                        aria-valuetext={
                          snapshot.muted
                            ? "Muted"
                            : `${Math.round(snapshot.volume * 100)}%`
                        }
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
                        className="btn-secondary transport-speed-btn tabular-nums"
                        aria-label="Playback speed"
                        onClick={cyclePlaybackSpeed}
                      >
                        {snapshot.rate}×
                      </button>
                    </div>

                    <div className="transport-center-buttons">
                      <button
                        type="button"
                        className="btn-secondary transport-icon-btn"
                        disabled={!activeMedia}
                        aria-label="Previous item or restart"
                        onClick={() => {
                          bumpExplicitSeek();
                          setSnapshot(ipcClient.playbackCommand({ action: "previous" }));
                        }}
                      >
                        <IconSkipPrev size={15} />
                      </button>
                      <button
                        type="button"
                        className="play-circle-btn"
                        disabled={!activeMedia}
                        aria-label={snapshot.state === "playing" ? "Pause" : "Play"}
                        onClick={() =>
                          setSnapshot(
                            ipcClient.playbackCommand({ action: "toggle_play_pause" }),
                          )
                        }
                      >
                        {snapshot.state === "playing" ? (
                          <IconPause size={18} />
                        ) : (
                          <IconPlay size={18} />
                        )}
                      </button>
                      <button
                        type="button"
                        className="btn-secondary transport-icon-btn"
                        disabled={!activeMedia}
                        aria-label="Next item"
                        onClick={() => {
                          bumpExplicitSeek();
                          setSnapshot(ipcClient.playbackCommand({ action: "next" }));
                        }}
                      >
                        <IconSkipNext size={15} />
                      </button>
                    </div>

                    <div className="transport-side-right">
                      <button
                        type="button"
                        className="btn-secondary transport-collapsible-btn"
                        disabled={!activeMedia}
                        aria-label="Add current media to a playlist"
                        title="Add current media to a playlist"
                        onClick={() => {
                          if (activeMedia) {
                            setAddToPlaylistTarget(activeMedia);
                          }
                        }}
                      >
                        <IconPlus size={13} />
                        <span className="transport-btn-label">Playlist</span>
                      </button>
                      <button
                        type="button"
                        className="btn-secondary transport-collapsible-btn"
                        disabled={!activeMedia}
                        aria-label="Add quick marker"
                        title="Add quick marker at current timestamp (Shift+N)"
                        onClick={handleCreateQuickMarker}
                      >
                        <IconFlag size={13} />
                        <span className="transport-btn-label">Marker</span>
                      </button>
                      <button
                        type="button"
                        className="btn-secondary transport-collapsible-btn"
                        disabled={!activeMedia}
                        aria-label="Add timestamped note"
                        title={`Add note at ${formatDurationMs(displayPositionMs, timestampPrecision)} (N)`}
                        onClick={openAddNoteModal}
                      >
                        <IconPlus size={13} />
                        <span className="transport-btn-label">Note</span>
                      </button>
                      <button
                        type="button"
                        className="btn-secondary transport-collapsible-btn"
                        data-testid="toggle-side-panel-btn"
                        aria-pressed={showSidePanel}
                        aria-label={showSidePanel ? "Hide notes and queue bar" : "Show notes and queue bar"}
                        title={showSidePanel ? "Hide Notes & Queue bar (])" : "Show Notes & Queue bar (])"}
                        onClick={toggleSidePanel}
                      >
                        <IconPanelRight size={14} />
                        <span className="transport-btn-label">Notes &amp; Queue</span>
                      </button>
                    </div>
                  </div>
                </section>
              </div>

              {/* 365px Right Side Panel: Queue | Notes (Frames 2:63 & 4:609) */}
              {showSidePanel && (
                <RightSidePanel
                  sideTab={sideTab}
                  onSelectSideTab={setSideTab}
                  snapshot={snapshot}
                  onUpdateSnapshot={setSnapshot}
                  activeMedia={activeMedia}
                  allMedia={allMedia}
                  activeNotes={activeNotes}
                  openNoteMenuId={openNoteMenuId}
                  onSetOpenNoteMenuId={setOpenNoteMenuId}
                  onOpenAddNoteModal={openAddNoteModal}
                  onOpenEditNoteModal={openEditNoteModal}
                  onDeleteNote={handleDeleteNote}
                  onJumpToNote={handleJumpToNote}
                  onSelectLibraryItem={handleSelectLibraryItem}
                  onOpenSaveQueueModal={() => {
                    setSaveQueueError(null);
                    setSaveQueueNameDraft("");
                    setIsSaveQueueModalOpen(true);
                  }}
                  onCycleRepeatMode={cycleRepeatMode}
                  onCollapse={toggleSidePanel}
                  timestampPrecision={timestampPrecision}
                />
              )}
          </div>

          {section === "library" && (
            <MediaLibraryView
              allMedia={allMedia}
              libraryResult={libraryResult}
              libraryFilter={libraryFilter}
              onSetLibraryFilter={setLibraryFilter}
              searchQuery={searchQuery}
              onSetSearchQuery={setSearchQuery}
              searchInputRef={searchInputRef}
              sourceFolderCount={sourceFolders.length}
              onOpenLocalSourcesSettings={() => {
                setSection("settings");
                setSettingsTab("sources");
              }}
              onOpenMediaPicker={triggerOpenMediaPicker}
              onSelectLibraryItem={handleSelectLibraryItem}
              onJumpToNote={handleJumpToNote}
              onAddToPlaylist={setAddToPlaylistTarget}
              onUpdateSnapshot={setSnapshot}
              onBumpLibraryVersion={() => setLibraryVersion((v) => v + 1)}
              openLibraryMenuId={openLibraryMenuId}
              onSetOpenLibraryMenuId={setOpenLibraryMenuId}
              libraryMenuPlacement={libraryMenuPlacement}
              onSetLibraryMenuPlacement={setLibraryMenuPlacement}
              missingMediaTarget={missingMediaTarget}
              missingTargetNotes={missingTargetNotes}
              relinkPathDraft={relinkPathDraft}
              onSetRelinkPathDraft={setRelinkPathDraft}
              relinkError={relinkError}
              onAttemptRelink={handleAttemptRelink}
              onBrowseRelinkFile={handleBrowseRelinkFile}
              onClearMissingTarget={() => setMissingMediaTarget(null)}
              timestampPrecision={timestampPrecision}
              onModalOpenChange={setIsLibraryModalOpen}
            />
          )}

          {section === "settings" && (
            <SettingsView
              settings={settings}
              onUpdateSettings={(next) => {
                setSettings(next);
                setSnapshot(ipcClient.getPlaybackSnapshot());
              }}
              settingsTab={settingsTab}
              onSelectSettingsTab={setSettingsTab}
              playbackSpeeds={PLAYBACK_SPEEDS}
              sourceFolders={sourceFolders}
              isPickingSourceFolder={isPickingSourceFolder}
              sourceFolderError={sourceFolderError}
              rescanningFolderId={rescanningFolderId}
              onAddSource={() => void handleAddSourceButtonClick()}
              onToggleSource={handleToggleSourceFolder}
              onRescanSource={(folder) => void handleRescanSourceFolder(folder)}
              onDeleteSource={handleDeleteSourceFolder}
              backupStatusMessage={backupStatusMessage}
              onSetBackupStatusMessage={setBackupStatusMessage}
              diagnosticReport={diagnosticReport}
              onSetDiagnosticReport={setDiagnosticReport}
              onCleanupAppData={handleCleanupAppData}
              currentVersion={currentVersion}
              onSetUpdateStatus={setUpdateStatus}
              onCheckForUpdates={() => void handleCheckForUpdates()}
              onModalOpenChange={setIsSettingsModalOpen}
            />
          )}
        </main>
      </div>
      </div>

      {/* Add / Edit Note Modal (Frame 4:88) */}
      {isNoteModalOpen && (
        <div className="modal-backdrop" role="dialog" aria-modal="true" aria-label="Note composer">
          <div ref={noteDialogRef} className="modal-dialog">
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
                aria-label="Close note composer"
                onClick={() => setIsNoteModalOpen(false)}
              >
                <IconClose size={14} />
              </button>
            </div>

            <div style={{ display: "flex", alignItems: "center", gap: 10 }}>
              <span className="timestamp-pill tabular-nums">
                <IconPlay size={10} />
                <span>{formatDurationMs(capturedTimestampMs, timestampPrecision)}</span>
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
          <div ref={mismatchDialogRef} className="modal-dialog">
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
                    pendingMismatchRelink.probedDurationMs,
                    pendingMismatchRelink.fileSizeBytes,
                  )
                }
              >
                Relink anyway
              </button>
            </div>
          </div>
        </div>
      )}

      {/* Add Media Item to Playlist Modal */}
      {addToPlaylistTarget && (
        <AddToPlaylistModal
          mediaItem={addToPlaylistTarget}
          playlists={playlists}
          onChanged={(msg) => {
            setLibraryVersion((v) => v + 1);
            if (msg) {
              showTransientUpdateToast(msg);
            }
          }}
          onClose={() => setAddToPlaylistTarget(null)}
        />
      )}

      {/* Save Current Queue as Playlist Modal */}
      {isSaveQueueModalOpen && (
        <div
          className="modal-backdrop"
          role="dialog"
          aria-modal="true"
          aria-label="Save queue as playlist"
          data-testid="save-queue-playlist-modal"
        >
          <form ref={saveQueueDialogRef} className="modal-dialog" onSubmit={handleSaveQueueAsPlaylist}>
            <div style={{ display: "flex", justifyContent: "space-between", alignItems: "center" }}>
              <h2 style={{ margin: 0, fontSize: 17 }}>Save queue as playlist</h2>
              <button
                type="button"
                className="btn-secondary"
                style={{ width: 30, height: 30, padding: 0 }}
                aria-label="Close modal"
                onClick={() => setIsSaveQueueModalOpen(false)}
              >
                <IconClose size={14} />
              </button>
            </div>
            <p style={{ margin: 0, fontSize: 12.5, color: "var(--text-tertiary)" }}>
              Creates a new playlist containing all {snapshot.queue.length}{" "}
              {snapshot.queue.length === 1 ? "item" : "items"} currently in your queue.
            </p>
            <input
              ref={saveQueueInputRef}
              type="text"
              aria-label="Playlist name"
              placeholder="Playlist name…"
              value={saveQueueNameDraft}
              onChange={(e) => setSaveQueueNameDraft(e.target.value)}
              style={{
                width: "100%",
                padding: "10px 12px",
                borderRadius: 8,
                border: "1px solid var(--border-control)",
                background: "var(--bg-inset)",
                color: "var(--text-primary)",
              }}
            />
            {saveQueueError && (
              <div role="alert" style={{ fontSize: 12, color: "var(--status-error-text)" }}>
                {saveQueueError}
              </div>
            )}
            <div style={{ display: "flex", justifyContent: "flex-end", gap: 10 }}>
              <button
                type="button"
                className="btn-secondary"
                onClick={() => setIsSaveQueueModalOpen(false)}
              >
                Cancel
              </button>
              <button type="submit" className="btn-primary">
                Save playlist
              </button>
            </div>
          </form>
        </div>
      )}

      {/* Software Update Dialog */}
      {isUpdateDialogOpen && updateStatus && (
        <UpdateDialog
          updateStatus={updateStatus}
          onConfirmInstall={(version) => void handleInstallUpdate(version)}
          onDismiss={(version) => handleDismissUpdate(version)}
          onCheckAgain={() => void handleCheckForUpdates()}
          onClose={() => setIsUpdateDialogOpen(false)}
        />
      )}

      {/* Stacked 6-second Undo Toast Banners (Frame 4:578) & Status Toast (`L-4`) */}
      {(deletedNoteToast || deletedPlaylistToast || updateToastMessage) && (
        <div className="toast-stack">
          {deletedNoteToast && (
            <div className="toast-banner" role="status">
              <span>Note deleted</span>
              <button type="button" className="jump-link-btn" onClick={handleUndoDelete}>
                Undo
              </button>
            </div>
          )}

          {deletedPlaylistToast && (
            <div className="toast-banner" role="status" data-testid="deleted-playlist-toast">
              <span>Playlist “{deletedPlaylistToast.name}” deleted</span>
              <button type="button" className="jump-link-btn" onClick={handleUndoDeletePlaylist}>
                Undo
              </button>
            </div>
          )}

          {updateToastMessage && (
            <div className="toast-banner" role="status" data-testid="update-toast">
              <span>{updateToastMessage}</span>
              <button
                type="button"
                className="jump-link-btn"
                onClick={() => setUpdateToastMessage(null)}
              >
                Dismiss
              </button>
            </div>
          )}
        </div>
      )}

      {/* Assistive Technology Live Announcement Region (announces media & state transitions without spamming every second) */}
      <div className="sr-only" aria-live="polite" aria-atomic="true">
        {activeMedia ? `${activeMedia.display_name}, ${snapshot.state}` : "No media loaded"}
      </div>
    </>
  );
}
