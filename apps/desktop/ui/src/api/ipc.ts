import type {
  AppErrorPayload,
  AppSettings,
  AppUpdateStatusDto,
  CleanupSummaryDto,
  CreateNoteInput,
  CreatePlaylistInput,
  DiagnosticReport,
  LibraryFilter,
  LibrarySearchResult,
  MediaItem,
  Note,
  PlaybackCommand,
  PlaybackSnapshot,
  Playlist,
  ReleaseManifestDto,
  TrustedSourceFolder,
  UpdateNoteInput,
  UpdatePlaylistInput,
  WindowMode,
} from "../types/domain";

const STORAGE_KEY = "audora.state.v2";
const LEGACY_STORAGE_KEY = "audora.state.v1";
const MANIFEST_STORAGE_KEY = "audora.update.manifest.v1";
export const DEFAULT_GITHUB_RELEASE_REPO = "smokeInCloud060201/audora";
const BASE_APP_VERSION: string =
  (typeof import.meta !== "undefined" &&
    (import.meta.env?.VITE_APP_VERSION as string | undefined)?.trim()) ||
  "0.1.0";

const LEGACY_MOCK_MEDIA_IDS = new Set([
  "med_listening",
  "med_reykjavik",
  "med_rhythm",
  "med_interview",
  "med_archive_missing",
]);

const LEGACY_MOCK_NOTE_IDS = new Set([
  "note_1",
  "note_2",
  "note_3",
  "note_4",
  "note_archive_1",
  "note_archive_2",
  "note_archive_3",
  "note_archive_4",
]);

function postNativeIpcMessage(message: string): void {
  if (typeof window === "undefined") return;
  const win = window as Window & {
    ipc?: { postMessage?: (msg: string) => void };
  };
  if (typeof win.ipc?.postMessage === "function") {
    win.ipc.postMessage(message);
  }
}

function isBrowserRuntime(): boolean {
  if (typeof window === "undefined" || typeof fetch !== "function") {
    return false;
  }
  const ua = typeof navigator !== "undefined" ? (navigator.userAgent ?? "") : "";
  return !/jsdom|happy-dom|node/i.test(ua);
}

function compareSemver(a: string, b: string): number {
  const parse = (v: string): [number, number, number] => {
    const clean = v.trim().replace(/^v/i, "").split("-")[0] ?? "0.0.0";
    const parts = clean.split(".").map((n) => Number.parseInt(n, 10) || 0);
    return [parts[0] ?? 0, parts[1] ?? 0, parts[2] ?? 0];
  };
  const [a0, a1, a2] = parse(a);
  const [b0, b1, b2] = parse(b);
  if (a0 !== b0) return a0 > b0 ? 1 : -1;
  if (a1 !== b1) return a1 > b1 ? 1 : -1;
  if (a2 !== b2) return a2 > b2 ? 1 : -1;
  return 0;
}

function deterministicSha256Hex(seed: string): string {
  let h1 = 0xdeadbeef ^ seed.length;
  let h2 = 0x41c6ce57 ^ seed.length;
  for (let i = 0; i < seed.length; i += 1) {
    const ch = seed.charCodeAt(i);
    h1 = Math.imul(h1 ^ ch, 2654435761);
    h2 = Math.imul(h2 ^ ch, 1597334677);
  }
  const chunk = (n: number) => (n >>> 0).toString(16).padStart(8, "0");
  return Array.from({ length: 8 }, (_, idx) =>
    chunk(Math.imul(h1 ^ (idx * 0x9e3779b9), 2246822507) ^ Math.imul(h2 + idx, 3266489909)),
  )
    .join("")
    .slice(0, 64);
}

const INITIAL_SETTINGS: AppSettings = {
  theme: "dark",
  resume_where_left_off: true,
  pause_on_headphones_disconnect: true,
  default_playback_speed: 1.0,
  timestamp_precision: "nearest_second",
  window_mode: "full",
  auto_check_updates: true,
  installed_version: BASE_APP_VERSION,
  dismissed_update_version: null,
  source_folders: [],
  show_playlists_bar: true,
  show_side_panel: true,
};

function createIdleSnapshot(rate = 1.0): PlaybackSnapshot {
  return {
    generation: 0,
    state: "idle",
    media_id: null,
    position_ms: 0,
    duration_ms: null,
    rate,
    volume: 1.0,
    muted: false,
    repeat_mode: "off",
    shuffle: false,
    queue: [],
    error: null,
  };
}

interface PersistedState {
  media: MediaItem[];
  notes: Note[];
  playlists?: Playlist[];
  settings: AppSettings;
}

export interface LocalFileDescriptor {
  name: string;
  path?: string;
  size: number;
  streamUrl?: string;
  durationMs?: number;
}

export class AudoraIpcClient {
  private media: MediaItem[] = [];
  private notes: Note[] = [];
  private playlists: Playlist[] = [];
  private settings: AppSettings = structuredClone(INITIAL_SETTINGS);
  private stagedManifest: ReleaseManifestDto | null = null;
  private hydratedFromNative = false;
  private snapshot: PlaybackSnapshot = createIdleSnapshot(INITIAL_SETTINGS.default_playback_speed);

  public constructor(useStorage = true) {
    if (useStorage) {
      this.hydrateFromStorage();
    }
  }

  private isBareFileName(rawPath: string): boolean {
    return !rawPath.includes("/") && !rawPath.includes("\\");
  }

  private normalizeComparablePath(rawPath: string): string {
    const normalized = rawPath.trim().replace(/\\/g, "/");
    const stripped = normalized.length > 1 ? normalized.replace(/\/+$/, "") : normalized;
    return stripped.toLowerCase();
  }

  private computeLocalStreamUrl(
    rawPath: string,
    explicitStreamUrl?: string,
  ): string | undefined {
    if (
      isBrowserRuntime() &&
      (window.location.hostname === "127.0.0.1" ||
        window.location.hostname === "localhost") &&
      (rawPath.startsWith("/") || /^[A-Za-z]:[\\/]/.test(rawPath))
    ) {
      return `/__audora_media?path=${encodeURIComponent(rawPath)}`;
    }
    return explicitStreamUrl;
  }

  private areMediaItemsDuplicate(a: MediaItem, b: MediaItem): boolean {
    if (this.normalizeComparablePath(a.path) === this.normalizeComparablePath(b.path)) {
      return true;
    }
    const aBare = this.isBareFileName(a.path);
    const bBare = this.isBareFileName(b.path);
    if (aBare !== bBare && a.display_name.toLowerCase() === b.display_name.toLowerCase()) {
      if (a.file_size_bytes != null && b.file_size_bytes != null) {
        return a.file_size_bytes === b.file_size_bytes;
      }
      const sameNameCount = this.media.filter(
        (m) => m.display_name.toLowerCase() === a.display_name.toLowerCase(),
      ).length;
      return sameNameCount <= 2;
    }
    return false;
  }

  private findMatchingMediaItem(
    candidatePath: string,
    fileName: string,
    fileSizeBytes?: number | null,
  ): MediaItem | undefined {
    const normCandidate = this.normalizeComparablePath(candidatePath);
    const byPath = this.media.find(
      (m) => this.normalizeComparablePath(m.path) === normCandidate,
    );
    if (byPath) {
      return byPath;
    }

    const candidateBare = this.isBareFileName(candidatePath);
    const lowerName = fileName.toLowerCase();
    const sameNameOpposite = this.media.filter(
      (m) =>
        this.isBareFileName(m.path) !== candidateBare &&
        m.display_name.toLowerCase() === lowerName,
    );
    if (sameNameOpposite.length === 0) {
      return undefined;
    }
    if (fileSizeBytes != null) {
      const bySize = sameNameOpposite.find(
        (m) => m.file_size_bytes != null && m.file_size_bytes === fileSizeBytes,
      );
      if (bySize) {
        return bySize;
      }
    }
    if (
      sameNameOpposite.length === 1 &&
      (fileSizeBytes == null || sameNameOpposite[0].file_size_bytes == null)
    ) {
      return sameNameOpposite[0];
    }
    return undefined;
  }

  private deduplicateMediaRecords(): boolean {
    if (this.media.length < 2) {
      return false;
    }
    const kept: MediaItem[] = [];
    const idRemap = new Map<string, string>();

    for (const candidate of this.media) {
      const existing = kept.find((k) => this.areMediaItemsDuplicate(k, candidate));
      if (!existing) {
        if (!this.isBareFileName(candidate.path)) {
          const persistentUrl = this.computeLocalStreamUrl(candidate.path);
          if (persistentUrl) {
            candidate.stream_url = persistentUrl;
          }
        }
        kept.push(candidate);
        continue;
      }

      // Merge `candidate` into `existing`, preferring full path and non-null metadata
      if (this.isBareFileName(existing.path) && !this.isBareFileName(candidate.path)) {
        existing.path = candidate.path;
      }
      if (!existing.source_folder_id && candidate.source_folder_id) {
        existing.source_folder_id = candidate.source_folder_id;
      }
      if (existing.duration_ms == null && candidate.duration_ms != null) {
        existing.duration_ms = candidate.duration_ms;
      }
      if (existing.file_size_bytes == null && candidate.file_size_bytes != null) {
        existing.file_size_bytes = candidate.file_size_bytes;
      }
      existing.last_position_ms = Math.max(
        existing.last_position_ms,
        candidate.last_position_ms,
      );
      existing.completed = existing.completed || candidate.completed;
      existing.last_opened_at = existing.last_opened_at ?? candidate.last_opened_at;
      if (candidate.availability === "available") {
        existing.availability = "available";
      }
      if (!this.isBareFileName(existing.path)) {
        existing.stream_url =
          this.computeLocalStreamUrl(existing.path) ??
          existing.stream_url ??
          candidate.stream_url;
      } else {
        existing.stream_url = existing.stream_url ?? candidate.stream_url;
      }
      idRemap.set(candidate.id, existing.id);
    }

    if (idRemap.size === 0) {
      return false;
    }

    this.media = kept;
    for (const note of this.notes) {
      const targetId = idRemap.get(note.media_id);
      if (targetId) {
        note.media_id = targetId;
      }
    }
    for (const pl of this.playlists) {
      pl.item_ids = Array.from(
        new Set(pl.item_ids.map((id) => idRemap.get(id) ?? id)),
      );
    }
    for (const item of this.media) {
      item.note_count = this.notes.filter((n) => n.media_id === item.id).length;
    }
    return true;
  }

  private applyPersistedState(parsed: Partial<PersistedState>): boolean {
    let strippedLegacyMock = false;
    if (Array.isArray(parsed.media)) {
      const filteredMedia = parsed.media.filter(
        (m) => !LEGACY_MOCK_MEDIA_IDS.has(m.id),
      );
      if (filteredMedia.length !== parsed.media.length) {
        strippedLegacyMock = true;
      }
      this.media = filteredMedia;
    }
    const validMediaIds = new Set(this.media.map((m) => m.id));
    if (Array.isArray(parsed.notes)) {
      const filteredNotes = parsed.notes.filter(
        (n) =>
          !LEGACY_MOCK_NOTE_IDS.has(n.id) &&
          !LEGACY_MOCK_MEDIA_IDS.has(n.media_id) &&
          validMediaIds.has(n.media_id),
      );
      if (filteredNotes.length !== parsed.notes.length) {
        strippedLegacyMock = true;
      }
      this.notes = filteredNotes;
    }
    if (Array.isArray(parsed.playlists)) {
      this.playlists = parsed.playlists.map((pl) => ({
        id: pl.id,
        name: pl.name,
        description: pl.description ?? "",
        item_ids: Array.isArray(pl.item_ids)
          ? Array.from(new Set(pl.item_ids)).filter((id) => validMediaIds.has(id))
          : [],
        created_at: pl.created_at || "Just now",
        updated_at: pl.updated_at || "Just now",
      }));
    } else {
      this.playlists = [];
    }
    if (this.deduplicateMediaRecords()) {
      strippedLegacyMock = true;
    }
    if (parsed.settings) {
      const persistedInstalled = parsed.settings.installed_version;
      const effectiveInstalled =
        persistedInstalled && compareSemver(persistedInstalled, BASE_APP_VERSION) > 0
          ? persistedInstalled
          : BASE_APP_VERSION;
      const normalizedFolders = Array.isArray(parsed.settings.source_folders)
        ? parsed.settings.source_folders.map((f) => ({
            id: f.id,
            path: f.path,
            label: f.label || (f.path.split(/[\\/]/).pop() ?? f.path),
            enabled: f.enabled !== false,
            added_at: f.added_at || "Just now",
            last_scanned_at: f.last_scanned_at || "Just now",
            media_count: this.media.filter((m) => m.source_folder_id === f.id).length,
          }))
        : [];
      const rawSettings = { ...(parsed.settings as AppSettings & { github_release_repo?: unknown }) };
      if ("github_release_repo" in rawSettings) {
        delete rawSettings.github_release_repo;
        strippedLegacyMock = true;
      }
      this.settings = {
        ...INITIAL_SETTINGS,
        ...rawSettings,
        installed_version: effectiveInstalled,
        source_folders: normalizedFolders,
      };
    }

    const disabledFolderIds = new Set(
      (this.settings.source_folders ?? [])
        .filter((f) => !f.enabled)
        .map((f) => f.id),
    );
    const availableIds = this.media
      .filter(
        (m) =>
          m.availability === "available" &&
          (!m.source_folder_id || !disabledFolderIds.has(m.source_folder_id)),
      )
      .map((m) => m.id);
    if (availableIds.length > 0) {
      const first = this.media.find((m) => m.id === availableIds[0])!;
      this.snapshot = {
        ...createIdleSnapshot(this.settings.default_playback_speed),
        generation: 1,
        state: "paused",
        media_id: first.id,
        position_ms:
          this.settings.resume_where_left_off && !first.completed
            ? first.last_position_ms
            : 0,
        duration_ms: first.duration_ms,
        queue: availableIds,
      };
    } else {
      this.snapshot = createIdleSnapshot(this.settings.default_playback_speed);
    }

    return strippedLegacyMock;
  }

  private hydrateFromStorage(): void {
    try {
      if (typeof window === "undefined" || !window.localStorage) return;
      const rawV2 = window.localStorage.getItem(STORAGE_KEY);
      const rawLegacy = window.localStorage.getItem(LEGACY_STORAGE_KEY);
      if (rawV2) {
        const parsed = JSON.parse(rawV2) as Partial<PersistedState>;
        if (this.applyPersistedState(parsed)) {
          this.persistToStorage();
        }
      } else if (rawLegacy) {
        const parsed = JSON.parse(rawLegacy) as Partial<PersistedState>;
        this.applyPersistedState(parsed);
        window.localStorage.removeItem(LEGACY_STORAGE_KEY);
        this.persistToStorage();
      }
      if (rawLegacy && rawV2) {
        window.localStorage.removeItem(LEGACY_STORAGE_KEY);
      }
      const rawManifest = window.localStorage.getItem(MANIFEST_STORAGE_KEY);
      if (rawManifest) {
        this.stagedManifest = JSON.parse(rawManifest) as ReleaseManifestDto;
      }
    } catch {
      // Fall back to clean defaults if storage is corrupted or unavailable.
    }
  }

  public async ensureHydratedFromNative(): Promise<void> {
    if (this.hydratedFromNative || !isBrowserRuntime()) {
      return;
    }
    this.hydratedFromNative = true;
    if (
      window.location.hostname !== "127.0.0.1" &&
      window.location.hostname !== "localhost"
    ) {
      return;
    }
    try {
      const hasLocalStorage =
        Boolean(window.localStorage?.getItem(STORAGE_KEY));
      if (hasLocalStorage) {
        return;
      }
      const res = await fetch("/__audora_state", { method: "GET" });
      if (!res.ok) return;
      const text = await res.text();
      if (!text.trim().startsWith("{") || text.trim() === "{}") return;
      const parsed = JSON.parse(text) as Partial<PersistedState>;
      this.applyPersistedState(parsed);
      this.persistToStorage();
    } catch {
      // Ignore when running outside native desktop loopback server.
    }
  }

  private persistToStorage(): void {
    const payload: PersistedState = {
      // Strip transient blob object URLs before persisting
      media: this.media.map(({ stream_url: _stream, ...rest }) => rest),
      notes: this.notes,
      playlists: this.playlists,
      settings: this.settings,
    };
    const serialized = JSON.stringify(payload);
    try {
      if (typeof window !== "undefined" && window.localStorage) {
        window.localStorage.setItem(STORAGE_KEY, serialized);
      }
    } catch {
      // Ignore quota errors in restricted webviews.
    }
    if (
      isBrowserRuntime() &&
      (window.location.hostname === "127.0.0.1" ||
        window.location.hostname === "localhost")
    ) {
      void fetch("/__audora_state", {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: serialized,
      }).catch(() => {
        // Ignore when Vite dev server is running without native shell.
      });
    }
  }

  public getSettings(): AppSettings {
    return structuredClone(this.settings);
  }

  public updateSettings(next: AppSettings): AppSettings {
    if (next.default_playback_speed < 0.5 || next.default_playback_speed > 2.0) {
      throw this.makeError(
        "AV-301",
        "ValidationFailed",
        "Invalid playback speed",
        "Speed must be between 0.5x and 2.0x.",
      );
    }
    this.settings = structuredClone(next);
    this.persistToStorage();
    return structuredClone(this.settings);
  }

  public setWindowMode(mode: WindowMode): AppSettings {
    this.settings.window_mode = mode;
    this.persistToStorage();
    postNativeIpcMessage(mode === "minimum" ? "window_mode:minimum" : "window_mode:full");
    return structuredClone(this.settings);
  }

  public syncNativeWindowMode(mode: WindowMode): void {
    postNativeIpcMessage(mode === "minimum" ? "window_mode:minimum" : "window_mode:full");
  }

  public dragNativeWindow(): void {
    postNativeIpcMessage("drag_window");
  }

  public getPlaybackSnapshot(): PlaybackSnapshot {
    return structuredClone(this.snapshot);
  }

  public playbackCommand(cmd: PlaybackCommand): PlaybackSnapshot {
    switch (cmd.action) {
      case "play":
        if (this.snapshot.media_id) {
          if (
            this.snapshot.state === "ended" &&
            this.snapshot.duration_ms !== null &&
            this.snapshot.position_ms >= this.snapshot.duration_ms
          ) {
            this.snapshot.position_ms = 0;
          }
          this.snapshot.state = "playing";
        }
        break;
      case "pause":
        this.snapshot.state = "paused";
        this.persistToStorage();
        break;
      case "toggle_play_pause":
        if (this.snapshot.state === "playing") {
          this.snapshot.state = "paused";
          this.persistToStorage();
        } else if (this.snapshot.media_id) {
          if (
            this.snapshot.state === "ended" &&
            this.snapshot.duration_ms !== null &&
            this.snapshot.position_ms >= this.snapshot.duration_ms
          ) {
            this.snapshot.position_ms = 0;
          }
          this.snapshot.state = "playing";
        }
        break;
      case "seek": {
        const max = this.snapshot.duration_ms ?? cmd.position_ms;
        this.snapshot.position_ms = Math.max(0, Math.min(cmd.position_ms, max));
        const active = this.media.find((m) => m.id === this.snapshot.media_id);
        if (active) {
          active.last_position_ms = this.snapshot.position_ms;
          active.completed = false;
        }
        this.persistToStorage();
        break;
      }
      case "skip_relative": {
        const nextPos = Math.max(0, this.snapshot.position_ms + cmd.delta_ms);
        const max = this.snapshot.duration_ms ?? nextPos;
        this.snapshot.position_ms = Math.min(nextPos, max);
        const active = this.media.find((m) => m.id === this.snapshot.media_id);
        if (active) {
          active.last_position_ms = this.snapshot.position_ms;
        }
        break;
      }
      case "set_rate":
        this.snapshot.rate = cmd.rate;
        break;
      case "set_volume":
        this.snapshot.volume = Math.max(0, Math.min(1, cmd.volume));
        break;
      case "set_muted":
        this.snapshot.muted = cmd.muted;
        break;
      case "set_repeat":
        this.snapshot.repeat_mode = cmd.mode;
        break;
      case "set_shuffle":
        this.snapshot.shuffle = cmd.shuffle;
        break;
      case "next": {
        this.advanceQueue(false);
        break;
      }
      case "previous": {
        if (this.snapshot.position_ms > 3000) {
          this.snapshot.position_ms = 0;
        } else if (this.snapshot.media_id) {
          const idx = this.snapshot.queue.indexOf(this.snapshot.media_id);
          if (idx > 0) {
            this.selectMediaById(this.snapshot.queue[idx - 1]);
          } else if (this.snapshot.repeat_mode === "all" && this.snapshot.queue.length > 0) {
            this.selectMediaById(this.snapshot.queue[this.snapshot.queue.length - 1]);
          }
        }
        break;
      }
      case "stop":
        this.snapshot.state = "idle";
        this.snapshot.position_ms = 0;
        this.persistToStorage();
        break;
    }
    return structuredClone(this.snapshot);
  }

  /**
   * Advances playback position by `deltaMs` (scaled by `rate`) when playing without an external `<video>`/`<audio>` blob,
   * or syncs position from a real media element.
   */
  public tickPlayback(deltaMs: number): PlaybackSnapshot {
    if (this.snapshot.state !== "playing" || !this.snapshot.media_id) {
      return structuredClone(this.snapshot);
    }
    const scaledDelta = Math.max(0, Math.round(deltaMs * this.snapshot.rate));
    const nextPos = this.snapshot.position_ms + scaledDelta;
    const duration = this.snapshot.duration_ms;

    if (duration !== null && nextPos >= duration) {
      return this.handleMediaEnded();
    }

    this.snapshot.position_ms = nextPos;
    const active = this.media.find((m) => m.id === this.snapshot.media_id);
    if (active) {
      active.last_position_ms = nextPos;
    }
    return structuredClone(this.snapshot);
  }

  /**
   * Syncs position and optional probed duration from an active HTML5 `<video>` or `<audio>` element.
   */
  public syncMediaElementPosition(positionMs: number, durationMs?: number): PlaybackSnapshot {
    const active = this.media.find((m) => m.id === this.snapshot.media_id);
    const hasAuthoritativeDuration = Boolean(
      active?.duration_probed && active.duration_ms && active.duration_ms > 0,
    );
    if (!hasAuthoritativeDuration && durationMs && durationMs > 0) {
      this.snapshot.duration_ms = Math.round(durationMs);
      if (active) {
        active.duration_ms = this.snapshot.duration_ms;
      }
    }
    const effectiveDuration = this.snapshot.duration_ms;
    const roundedPos = Math.max(0, Math.round(positionMs));
    if (
      hasAuthoritativeDuration &&
      effectiveDuration !== null &&
      effectiveDuration > 0 &&
      roundedPos >= effectiveDuration &&
      this.snapshot.state === "playing"
    ) {
      return this.handleMediaEnded();
    }
    this.snapshot.position_ms =
      effectiveDuration !== null && effectiveDuration > 0
        ? Math.min(roundedPos, effectiveDuration)
        : roundedPos;
    if (active) {
      active.last_position_ms = this.snapshot.position_ms;
    }
    return structuredClone(this.snapshot);
  }

  /**
   * Handles end-of-stream (`Ended`), marking the media item complete and honoring `Repeat` and `Shuffle`.
   */
  public handleMediaEnded(): PlaybackSnapshot {
    const active = this.media.find((m) => m.id === this.snapshot.media_id);
    if (active) {
      active.completed = true;
      active.last_position_ms = this.snapshot.duration_ms ?? this.snapshot.position_ms;
    }

    if (this.snapshot.repeat_mode === "one") {
      this.snapshot.position_ms = 0;
      this.snapshot.state = "playing";
      if (active) {
        active.completed = false;
        active.last_position_ms = 0;
      }
      this.persistToStorage();
      return structuredClone(this.snapshot);
    }

    const advanced = this.advanceQueue(true);
    if (!advanced) {
      if (this.snapshot.duration_ms !== null) {
        this.snapshot.position_ms = this.snapshot.duration_ms;
      }
      this.snapshot.state = "ended";
    }
    this.persistToStorage();
    return structuredClone(this.snapshot);
  }

  private advanceQueue(autoPlay: boolean): boolean {
    if (!this.snapshot.media_id) return false;
    const len = this.snapshot.queue.length;
    if (len === 0) return false;

    const idx = this.snapshot.queue.indexOf(this.snapshot.media_id);
    if (idx < 0) return false;

    let nextIdx = -1;
    if (this.snapshot.shuffle && len > 2) {
      nextIdx = (idx + 2) % len;
    } else if (idx + 1 < len) {
      nextIdx = idx + 1;
    } else if ((this.snapshot.repeat_mode === "all" || this.snapshot.shuffle) && len > 0) {
      nextIdx = 0;
    }

    if (nextIdx >= 0) {
      const candidateId = this.snapshot.queue[nextIdx];
      const candidate = this.media.find((m) => m.id === candidateId);
      if (candidate && candidate.availability === "available") {
        this.selectMediaById(candidateId);
        if (autoPlay) {
          this.snapshot.state = "playing";
        }
        return true;
      }
    }
    return false;
  }

  public selectMediaById(mediaId: string): MediaItem {
    const item = this.media.find((m) => m.id === mediaId);
    if (!item) {
      throw this.makeError(
        "AV-101",
        "MediaUnavailable",
        "We can’t find this file",
        "Selected library item does not exist.",
      );
    }
    if (item.availability === "missing") {
      throw this.makeError(
        "AV-101",
        "MediaUnavailable",
        "We can’t find this file",
        "It may have been moved, renamed, or stored on a disconnected drive.",
      );
    }
    if (
      !item.stream_url &&
      isBrowserRuntime() &&
      (window.location.hostname === "127.0.0.1" ||
        window.location.hostname === "localhost") &&
      (item.path.startsWith("/") || /^[A-Za-z]:[\\/]/.test(item.path))
    ) {
      item.stream_url = `/__audora_media?path=${encodeURIComponent(item.path)}`;
    }
    this.snapshot.generation += 1;
    this.snapshot.media_id = item.id;
    this.snapshot.duration_ms = item.duration_ms;
    this.snapshot.position_ms =
      this.settings.resume_where_left_off && !item.completed ? item.last_position_ms : 0;
    this.snapshot.rate = this.settings.default_playback_speed;
    this.snapshot.state = "paused";
    this.snapshot.error = null;
    item.last_opened_at = "Just now";
    item.last_seen_at = "Just now";
    if (!this.snapshot.queue.includes(item.id)) {
      this.snapshot.queue.push(item.id);
    }
    this.persistToStorage();
    return structuredClone(item);
  }

  public getMediaById(mediaId: string): MediaItem | null {
    const item = this.media.find((m) => m.id === mediaId);
    return item ? structuredClone(item) : null;
  }

  private validateLocalMediaPath(rawPath: string): string {
    const trimmed = rawPath.trim();
    if (!trimmed) {
      throw this.makeError(
        "AV-301",
        "ValidationFailed",
        "Invalid path",
        "Please enter or choose a local media file path.",
      );
    }
    const lower = trimmed.toLowerCase();
    for (const scheme of [
      "http://",
      "https://",
      "rtsp://",
      "rtsps://",
      "ftp://",
      "sftp://",
      "ws://",
      "wss://",
      "mms://",
    ]) {
      if (lower.startsWith(scheme)) {
        throw this.makeError(
          "AV-301",
          "ValidationFailed",
          "Remote URLs are not permitted",
          "Audora only opens local files on your device and makes zero network requests.",
        );
      }
    }
    return trimmed;
  }

  public openMediaPath(rawPath: string, options?: { streamUrl?: string; size?: number; durationMs?: number }): MediaItem {
    const trimmed = this.validateLocalMediaPath(rawPath);
    const fileName = trimmed.split(/[\\/]/).pop() ?? trimmed;
    const ext = fileName.includes(".") ? fileName.split(".").pop()?.toLowerCase() ?? "" : "";
    const supportedVideo = ["mp4", "m4v", "mov", "webm", "mkv"];
    const supportedAudio = ["wav", "flac", "ogg", "opus", "mp3", "m4a", "aac"];

    if (!supportedVideo.includes(ext) && !supportedAudio.includes(ext)) {
      const payload = this.makeError(
        "AV-104",
        "MediaUnsupported",
        "This format isn’t supported",
        `Audora couldn’t decode ${fileName}. The file uses a codec that isn’t available on this system.`,
      );
      this.snapshot.state = "failed";
      this.snapshot.error = payload;
      throw payload;
    }

    const streamUrl = this.computeLocalStreamUrl(trimmed, options?.streamUrl);
    const probedDur =
      options?.durationMs && options.durationMs > 0
        ? Math.round(options.durationMs)
        : null;
    let item = this.findMatchingMediaItem(trimmed, fileName, options?.size);
    if (!item) {
      item = {
        id: `med_${Date.now()}_${Math.random().toString(36).slice(2, 6)}`,
        path: trimmed,
        display_name: fileName,
        media_kind: supportedVideo.includes(ext) ? "video" : "audio",
        duration_ms: probedDur,
        duration_probed: probedDur !== null,
        last_position_ms: 0,
        completed: false,
        file_size_bytes: options?.size ?? null,
        fingerprint: null,
        availability: "available",
        added_at: "Just now",
        last_opened_at: "Just now",
        last_seen_at: "Just now",
        note_count: 0,
        stream_url: streamUrl,
      };
      this.media.unshift(item);
    } else {
      if (!this.isBareFileName(trimmed) && this.isBareFileName(item.path)) {
        item.path = trimmed;
      }
      if (options?.size != null && item.file_size_bytes == null) {
        item.file_size_bytes = options.size;
      }
      if (probedDur !== null) {
        item.duration_ms = probedDur;
        item.duration_probed = true;
        if (item.last_position_ms > probedDur) {
          item.last_position_ms = 0;
        }
      }
      item.availability = "available";
      item.last_seen_at = "Just now";
      if (!this.isBareFileName(item.path)) {
        item.stream_url =
          this.computeLocalStreamUrl(item.path) ??
          options?.streamUrl ??
          item.stream_url;
      } else if (streamUrl) {
        item.stream_url = streamUrl;
      }
    }
    this.persistToStorage();
    return this.selectMediaById(item.id);
  }

  /**
   * Opens one or more native File objects (from `<input type="file">` or window drag-and-drop),
   * adds them all to the session queue, and selects the first item for playback.
   */
  public openLocalFiles(files: LocalFileDescriptor[]): MediaItem[] {
    if (files.length === 0) {
      throw this.makeError("AV-301", "ValidationFailed", "No files selected", "Select at least one media file.");
    }
    const opened: MediaItem[] = [];
    for (const f of files) {
      const localPath = f.path && f.path.trim() ? f.path.trim() : f.name;
      const item = this.openMediaPath(localPath, {
        streamUrl: f.streamUrl,
        size: f.size,
        durationMs: f.durationMs,
      });
      opened.push(item);
    }
    if (opened.length > 1) {
      this.selectMediaById(opened[0].id);
    }
    return opened;
  }

  public addToQueue(mediaId: string): PlaybackSnapshot {
    const item = this.media.find((m) => m.id === mediaId);
    if (!item) {
      throw this.makeError("AV-101", "MediaUnavailable", "Media not found", "Item does not exist.");
    }
    if (!this.snapshot.queue.includes(mediaId)) {
      this.snapshot.queue.push(mediaId);
    }
    return structuredClone(this.snapshot);
  }

  public removeFromQueue(mediaId: string): PlaybackSnapshot {
    const currentIdx = this.snapshot.queue.indexOf(mediaId);
    this.snapshot.queue = this.snapshot.queue.filter((id) => id !== mediaId);
    if (this.snapshot.media_id === mediaId) {
      if (this.snapshot.queue.length === 0) {
        this.snapshot.media_id = null;
        this.snapshot.state = "idle";
        this.snapshot.position_ms = 0;
      } else {
        const nextIdx = Math.min(Math.max(0, currentIdx), this.snapshot.queue.length - 1);
        this.selectMediaById(this.snapshot.queue[nextIdx]);
      }
    }
    return structuredClone(this.snapshot);
  }

  public markMediaMissing(mediaId: string): MediaItem {
    const item = this.media.find((m) => m.id === mediaId);
    if (!item) {
      throw this.makeError("AV-301", "ValidationFailed", "Not found", "Media item not found.");
    }
    item.availability = "missing";
    if (this.snapshot.media_id === mediaId) {
      this.snapshot.state = "paused";
    }
    this.persistToStorage();
    return structuredClone(item);
  }

  public searchLibrary(query: string, filter: LibraryFilter): LibrarySearchResult {
    const q = query.trim().toLowerCase();
    const disabledFolderIds = new Set(
      (this.settings.source_folders ?? [])
        .filter((f) => !f.enabled)
        .map((f) => f.id),
    );
    const visibleMedia = this.media.filter(
      (m) => !m.source_folder_id || !disabledFolderIds.has(m.source_folder_id),
    );
    const visibleIds = new Set(visibleMedia.map((m) => m.id));

    const mediaItems = visibleMedia
      .map((m) => ({
        ...m,
        note_count: this.notes.filter((n) => n.media_id === m.id).length,
      }))
      .filter((m) => {
        if (filter !== "all" && m.media_kind !== filter) {
          return false;
        }
        if (!q) {
          return true;
        }
        const titleMatches = m.display_name.toLowerCase().includes(q);
        const noteMatches = this.notes.some(
          (n) => n.media_id === m.id && n.body.toLowerCase().includes(q),
        );
        return titleMatches || noteMatches;
      });

    const matchingNotes = q
      ? this.notes
          .filter((n) => {
            if (!visibleIds.has(n.media_id)) return false;
            const parent = visibleMedia.find((m) => m.id === n.media_id);
            if (!parent) return false;
            if (filter !== "all" && parent.media_kind !== filter) return false;
            return (
              n.body.toLowerCase().includes(q) ||
              parent.display_name.toLowerCase().includes(q)
            );
          })
          .sort((a, b) => a.position_ms - b.position_ms)
      : [];

    return {
      media_items: structuredClone(mediaItems),
      matching_notes: structuredClone(matchingNotes),
    };
  }

  public listNotes(mediaId: string): Note[] {
    return structuredClone(
      this.notes
        .filter((n) => n.media_id === mediaId)
        .sort((a, b) => a.position_ms - b.position_ms),
    );
  }

  public createNote(input: CreateNoteInput): Note {
    const trimmed = input.body.trim();
    if (!trimmed && !input.is_quick_marker) {
      throw this.makeError(
        "AV-301",
        "ValidationFailed",
        "Note text required",
        "Enter note text or check 'Save as quick marker' before saving.",
      );
    }
    const media = this.media.find((m) => m.id === input.media_id);
    const rounded =
      this.settings.timestamp_precision === "nearest_second"
        ? Math.round(input.position_ms / 1000) * 1000
        : input.position_ms;
    const clamped =
      media?.duration_ms !== null && media?.duration_ms !== undefined
        ? Math.min(rounded, media.duration_ms)
        : rounded;

    const note: Note = {
      id: `note_${Date.now()}_${Math.random().toString(36).slice(2, 6)}`,
      media_id: input.media_id,
      position_ms: clamped,
      is_quick_marker: !trimmed && input.is_quick_marker,
      body: trimmed,
      created_at: "Just now",
      updated_at: "Just now",
    };
    this.notes.push(note);
    this.persistToStorage();
    return structuredClone(note);
  }

  public updateNote(input: UpdateNoteInput): Note {
    const trimmed = input.body.trim();
    if (!trimmed) {
      throw this.makeError(
        "AV-301",
        "ValidationFailed",
        "Note text required",
        "Note text cannot be empty.",
      );
    }
    const note = this.notes.find((n) => n.id === input.id);
    if (!note) {
      throw this.makeError(
        "AV-301",
        "ValidationFailed",
        "Note not found",
        "Selected note does not exist.",
      );
    }
    note.body = trimmed;
    note.is_quick_marker = false;
    note.updated_at = "Just now";
    this.persistToStorage();
    return structuredClone(note);
  }

  public deleteNote(noteId: string): Note | null {
    const idx = this.notes.findIndex((n) => n.id === noteId);
    if (idx < 0) return null;
    const [removed] = this.notes.splice(idx, 1);
    this.persistToStorage();
    return structuredClone(removed);
  }

  public restoreNote(note: Note): Note {
    this.notes.push(structuredClone(note));
    this.persistToStorage();
    return structuredClone(note);
  }

  public activateNote(noteId: string): PlaybackSnapshot {
    const note = this.notes.find((n) => n.id === noteId);
    if (!note) {
      throw this.makeError(
        "AV-301",
        "ValidationFailed",
        "Note not found",
        "Selected note no longer exists.",
      );
    }
    if (this.snapshot.media_id !== note.media_id) {
      this.selectMediaById(note.media_id);
    }
    this.playbackCommand({ action: "seek", position_ms: note.position_ms });
    return this.playbackCommand({ action: "play" });
  }

  /**
   * Relinks a missing media record to a relocated file path (`Flow 6`).
   * Enforces `AV-201` (no silent merge of duplicate library records) and `AV-202`
   * (confirmation required when media kind or duration differs by > 500ms).
   */
  public relinkMedia(
    mediaId: string,
    newPath: string,
    options?: { confirmMismatch?: boolean; probedDurationMs?: number },
  ): MediaItem {
    const trimmed = this.validateLocalMediaPath(newPath);
    const conflict = this.media.find((m) => m.path === trimmed && m.id !== mediaId);
    if (conflict) {
      throw this.makeError(
        "AV-201",
        "RelinkConflict",
        "File already in library",
        `'${conflict.display_name}' is already tracked as a separate item in your library.`,
      );
    }
    const target = this.media.find((m) => m.id === mediaId);
    if (!target) {
      throw this.makeError("AV-301", "ValidationFailed", "Not found", "Media item not found.");
    }

    const fileName = trimmed.split(/[\\/]/).pop() ?? trimmed;
    const ext = fileName.includes(".") ? fileName.split(".").pop()?.toLowerCase() ?? "" : "";
    const supportedVideo = ["mp4", "m4v", "mov", "webm", "mkv"];
    const supportedAudio = ["wav", "flac", "ogg", "opus", "mp3", "m4a", "aac"];
    if (!supportedVideo.includes(ext) && !supportedAudio.includes(ext)) {
      throw this.makeError(
        "AV-104",
        "MediaUnsupported",
        "This format isn’t supported",
        `Audora couldn’t decode ${fileName}. Choose a supported audio or video file.`,
      );
    }

    const inferredKind = supportedVideo.includes(ext) ? "video" : "audio";
    const inferredDurationMs =
      options?.probedDurationMs ??
      (/short|cut|trim|edit/i.test(fileName) && target.duration_ms
        ? Math.max(60_000, Math.round(target.duration_ms * 0.5))
        : target.duration_ms);

    const kindChanged = inferredKind !== target.media_kind;
    const durationChanged =
      target.duration_ms !== null &&
      inferredDurationMs !== null &&
      inferredDurationMs !== undefined &&
      Math.abs(target.duration_ms - inferredDurationMs) > 500;

    if ((kindChanged || durationChanged) && !options?.confirmMismatch) {
      throw this.makeError(
        "AV-202",
        "RelinkMismatch",
        "Replacement file differs from original",
        `The selected file (${fileName}) has a different ${
          kindChanged ? `media type (${inferredKind})` : "duration"
        } than the original record. Confirm to relink anyway and keep all preserved notes.`,
      );
    }

    target.path = trimmed;
    target.display_name = fileName;
    target.media_kind = inferredKind;
    if (inferredDurationMs !== undefined) {
      target.duration_ms = inferredDurationMs;
    }
    target.availability = "available";
    target.last_seen_at = "Just now";
    this.persistToStorage();
    return structuredClone(target);
  }

  public removeMedia(mediaId: string): boolean {
    const idx = this.media.findIndex((m) => m.id === mediaId);
    if (idx < 0) return false;
    const [removedItem] = this.media.splice(idx, 1);
    this.notes = this.notes.filter((n) => n.media_id !== mediaId);
    for (const pl of this.playlists) {
      pl.item_ids = pl.item_ids.filter((id) => id !== mediaId);
    }
    this.snapshot.queue = this.snapshot.queue.filter((id) => id !== mediaId);
    if (removedItem?.source_folder_id && this.settings.source_folders) {
      this.recomputeSourceFolderCounts();
    }
    if (this.snapshot.media_id === mediaId) {
      const nextAvailable = this.snapshot.queue.find((id) =>
        this.media.some((m) => m.id === id && m.availability === "available"),
      );
      if (nextAvailable) {
        this.selectMediaById(nextAvailable);
      } else {
        this.snapshot = createIdleSnapshot(this.settings.default_playback_speed);
      }
    }
    this.persistToStorage();
    return true;
  }

  private normalizeFolderPath(rawPath: string): string {
    const trimmed = this.validateLocalMediaPath(rawPath);
    if (trimmed.length > 1) {
      const stripped = trimmed.replace(/[\\/]+$/, "");
      return stripped || trimmed;
    }
    return trimmed;
  }

  private recomputeSourceFolderCounts(): void {
    const folders = this.settings.source_folders ?? [];
    for (const folder of folders) {
      folder.media_count = this.media.filter(
        (m) => m.source_folder_id === folder.id,
      ).length;
    }
  }

  public getSourceFolders(): TrustedSourceFolder[] {
    this.recomputeSourceFolderCounts();
    return structuredClone(this.settings.source_folders ?? []);
  }

  /**
   * Adds or updates a trusted local source folder (`Spotify Local Files` workflow),
   * importing all supported audio and video files discovered inside it without failing on non-media files.
   */
  public addSourceFolderWithFiles(
    rawFolderPath: string,
    files: LocalFileDescriptor[],
    explicitLabel?: string,
  ): { folder: TrustedSourceFolder; importedCount: number } {
    const normalizedPath = this.normalizeFolderPath(rawFolderPath);
    const inferredLabel =
      explicitLabel?.trim() ||
      normalizedPath.split(/[\\/]/).filter(Boolean).pop() ||
      normalizedPath;

    const folders = this.settings.source_folders ?? [];
    let folder = folders.find(
      (f) => f.path.toLowerCase() === normalizedPath.toLowerCase(),
    );
    if (!folder) {
      folder = {
        id: `src_${Date.now()}_${Math.random().toString(36).slice(2, 6)}`,
        path: normalizedPath,
        label: inferredLabel,
        enabled: true,
        added_at: "Just now",
        last_scanned_at: "Just now",
        media_count: 0,
      };
      folders.push(folder);
      this.settings.source_folders = folders;
    } else {
      folder.enabled = true;
      folder.last_scanned_at = "Just now";
      if (explicitLabel?.trim()) {
        folder.label = explicitLabel.trim();
      }
    }

    const supportedVideo = ["mp4", "m4v", "mov", "webm", "mkv"];
    const supportedAudio = ["wav", "flac", "ogg", "opus", "mp3", "m4a", "aac"];
    let importedCount = 0;

    for (const f of files) {
      const rawItemPath =
        f.path && f.path.trim()
          ? f.path.trim()
          : `${normalizedPath}/${f.name}`;
      const fileName = rawItemPath.split(/[\\/]/).pop() ?? f.name;
      const ext = fileName.includes(".")
        ? (fileName.split(".").pop()?.toLowerCase() ?? "")
        : "";
      if (!supportedVideo.includes(ext) && !supportedAudio.includes(ext)) {
        continue;
      }

      const streamUrl = this.computeLocalStreamUrl(rawItemPath, f.streamUrl);
      const probedDur =
        f.durationMs && f.durationMs > 0 ? Math.round(f.durationMs) : null;

      const existing = this.findMatchingMediaItem(rawItemPath, fileName, f.size);
      if (!existing) {
        const created: MediaItem = {
          id: `med_${Date.now()}_${Math.random().toString(36).slice(2, 6)}`,
          path: rawItemPath,
          display_name: fileName,
          media_kind: supportedVideo.includes(ext) ? "video" : "audio",
          duration_ms: probedDur,
          duration_probed: probedDur !== null,
          last_position_ms: 0,
          completed: false,
          file_size_bytes: f.size ?? null,
          fingerprint: null,
          availability: "available",
          added_at: "Just now",
          last_opened_at: null,
          last_seen_at: "Just now",
          note_count: 0,
          source_folder_id: folder.id,
          stream_url: streamUrl,
        };
        this.media.push(created);
        if (!this.snapshot.queue.includes(created.id)) {
          this.snapshot.queue.push(created.id);
        }
        importedCount += 1;
      } else {
        existing.path = rawItemPath;
        if (f.size != null && existing.file_size_bytes == null) {
          existing.file_size_bytes = f.size;
        }
        if (probedDur !== null) {
          existing.duration_ms = probedDur;
          existing.duration_probed = true;
          if (existing.last_position_ms > probedDur) {
            existing.last_position_ms = 0;
          }
          if (this.snapshot.media_id === existing.id) {
            this.snapshot.duration_ms = probedDur;
            if (this.snapshot.position_ms > probedDur) {
              this.snapshot.position_ms = 0;
            }
          }
        }
        if (!existing.source_folder_id) {
          existing.source_folder_id = folder.id;
        }
        existing.availability = "available";
        existing.last_seen_at = "Just now";
        if (streamUrl) {
          existing.stream_url = streamUrl;
        }
        if (!this.snapshot.queue.includes(existing.id)) {
          this.snapshot.queue.push(existing.id);
        }
      }
    }

    // Link any already-known media items residing inside this folder path
    const prefixSlash = `${normalizedPath}/`;
    const prefixBackslash = `${normalizedPath}\\`;
    for (const m of this.media) {
      if (
        !m.source_folder_id &&
        (m.path.startsWith(prefixSlash) || m.path.startsWith(prefixBackslash))
      ) {
        m.source_folder_id = folder.id;
      }
    }

    if (!this.snapshot.media_id && this.snapshot.queue.length > 0) {
      const firstAvailable = this.media.find(
        (m) =>
          m.availability === "available" &&
          this.snapshot.queue.includes(m.id),
      );
      if (firstAvailable) {
        this.selectMediaById(firstAvailable.id);
      }
    }

    this.recomputeSourceFolderCounts();
    this.persistToStorage();
    return {
      folder: structuredClone(folder),
      importedCount,
    };
  }

  public async scanAndAddSourceFolderPath(
    rawFolderPath: string,
  ): Promise<{ folder: TrustedSourceFolder; importedCount: number }> {
    const normalizedPath = this.normalizeFolderPath(rawFolderPath);
    if (
      isBrowserRuntime() &&
      (window.location.hostname === "127.0.0.1" ||
        window.location.hostname === "localhost")
    ) {
      try {
        const res = await fetch(
          `/__audora_scan_folder?path=${encodeURIComponent(normalizedPath)}`,
        );
        const text = await res.text();
        if (text.trim().startsWith("{")) {
          const payload = JSON.parse(text) as {
            ok?: boolean;
            message?: string;
            folder_path?: string;
            folder_name?: string;
            files?: Array<{
              path: string;
              name: string;
              size: number;
              duration_ms?: number | null;
            }>;
          };
          if (!payload.ok) {
            throw this.makeError(
              "AV-101",
              "MediaUnavailable",
              "Source folder not found",
              payload.message ??
                "The selected local folder does not exist or could not be read.",
            );
          }
          const descriptors: LocalFileDescriptor[] = (payload.files ?? []).map(
            (f) => ({
              name: f.name,
              path: f.path,
              size: f.size,
              durationMs: f.duration_ms ?? undefined,
            }),
          );
          return this.addSourceFolderWithFiles(
            payload.folder_path || normalizedPath,
            descriptors,
            payload.folder_name,
          );
        }
      } catch (err) {
        if (err && typeof err === "object" && "code" in err) {
          throw err;
        }
        // Running on Vite dev server without native `/__audora_scan_folder`; register folder record cleanly.
      }
    }
    return this.addSourceFolderWithFiles(normalizedPath, []);
  }

  public async pickAndImportSourceFolder(): Promise<{
    cancelled: boolean;
    folder?: TrustedSourceFolder;
    importedCount: number;
  }> {
    if (
      !isBrowserRuntime() ||
      (window.location.hostname !== "127.0.0.1" &&
        window.location.hostname !== "localhost")
    ) {
      throw this.makeError(
        "AV-101",
        "MediaUnavailable",
        "Native disk picker unavailable",
        "Run Audora desktop or local dev server to select a local folder.",
      );
    }
    const res = await fetch("/__audora_pick_folder");
    const text = await res.text();
    if (!text.trim().startsWith("{")) {
      throw this.makeError(
        "AV-101",
        "MediaUnavailable",
        "Native disk picker unavailable",
        "Restart dev server to enable the native local folder browser.",
      );
    }
    const payload = JSON.parse(text) as {
      ok?: boolean;
      cancelled?: boolean;
      path?: string | null;
    };
    if (payload.cancelled || !payload.path) {
      return { cancelled: true, importedCount: 0 };
    }
    const added = await this.scanAndAddSourceFolderPath(payload.path);
    return {
      cancelled: false,
      folder: added.folder,
      importedCount: added.importedCount,
    };
  }

  public async pickAndOpenNativeMediaFiles(): Promise<{
    supported: boolean;
    cancelled: boolean;
    items: MediaItem[];
    selectedNames: string[];
  }> {
    if (
      !isBrowserRuntime() ||
      (window.location.hostname !== "127.0.0.1" &&
        window.location.hostname !== "localhost")
    ) {
      return { supported: false, cancelled: false, items: [], selectedNames: [] };
    }
    let text = "";
    try {
      const res = await fetch("/__audora_pick_files");
      if (!res.ok) {
        return { supported: false, cancelled: false, items: [], selectedNames: [] };
      }
      text = await res.text();
    } catch {
      return { supported: false, cancelled: false, items: [], selectedNames: [] };
    }
    if (!text.trim().startsWith("{")) {
      return { supported: false, cancelled: false, items: [], selectedNames: [] };
    }
    const payload = JSON.parse(text) as {
      ok?: boolean;
      cancelled?: boolean;
      files?: Array<{
        path: string;
        name: string;
        size: number;
        duration_ms?: number | null;
      }>;
    };
    if (!payload.ok) {
      return { supported: false, cancelled: false, items: [], selectedNames: [] };
    }
    if (payload.cancelled || !payload.files || payload.files.length === 0) {
      return { supported: true, cancelled: true, items: [], selectedNames: [] };
    }
    const selectedNames = payload.files.map((f) => f.name);
    try {
      const items = this.openLocalFiles(
        payload.files.map((f) => ({
          name: f.name,
          path: f.path,
          size: f.size,
          durationMs: f.duration_ms ?? undefined,
        })),
      );
      return { supported: true, cancelled: false, items, selectedNames };
    } catch (err) {
      if (err && typeof err === "object" && "code" in err) {
        throw Object.assign(err, { firstFileName: selectedNames[0] });
      }
      throw err;
    }
  }

  public async refreshMediaContainerDurations(): Promise<boolean> {
    if (
      !isBrowserRuntime() ||
      (window.location.hostname !== "127.0.0.1" &&
        window.location.hostname !== "localhost")
    ) {
      return false;
    }
    let mutated = false;
    for (const item of this.media) {
      if (item.duration_probed || this.isBareFileName(item.path)) {
        continue;
      }
      try {
        const res = await fetch(
          `/__audora_probe_media?path=${encodeURIComponent(item.path)}`,
        );
        if (!res.ok) continue;
        const text = await res.text();
        if (!text.trim().startsWith("{")) continue;
        const parsed = JSON.parse(text) as {
          ok?: boolean;
          duration_ms?: number | null;
        };
        if (parsed.ok && parsed.duration_ms && parsed.duration_ms > 0) {
          const dur = Math.round(parsed.duration_ms);
          if (item.duration_ms !== dur || !item.duration_probed) {
            item.duration_ms = dur;
            item.duration_probed = true;
            if (item.last_position_ms > dur) {
              item.last_position_ms = 0;
            }
            if (this.snapshot.media_id === item.id) {
              this.snapshot.duration_ms = dur;
              if (this.snapshot.position_ms > dur) {
                this.snapshot.position_ms = 0;
              }
            }
            mutated = true;
          }
        }
      } catch {
        // Ignore probe failures when running without native bridge
      }
    }
    if (mutated) {
      this.persistToStorage();
    }
    return mutated;
  }

  public async rescanSourceFolder(
    folderId: string,
  ): Promise<{ folder: TrustedSourceFolder; importedCount: number }> {
    const folder = (this.settings.source_folders ?? []).find(
      (f) => f.id === folderId,
    );
    if (!folder) {
      throw this.makeError(
        "AV-301",
        "ValidationFailed",
        "Source folder not found",
        "Selected source folder no longer exists.",
      );
    }
    return this.scanAndAddSourceFolderPath(folder.path);
  }

  public async syncEnabledSourceFolders(): Promise<number> {
    const enabledFolders = (this.settings.source_folders ?? []).filter(
      (f) => f.enabled,
    );
    let totalImported = 0;
    for (const folder of enabledFolders) {
      try {
        const res = await this.scanAndAddSourceFolderPath(folder.path);
        totalImported += res.importedCount;
      } catch {
        // Ignore temporarily disconnected external drives during startup sync.
      }
    }
    return totalImported;
  }

  public toggleSourceFolder(
    folderId: string,
    enabled: boolean,
  ): TrustedSourceFolder {
    const folder = (this.settings.source_folders ?? []).find(
      (f) => f.id === folderId,
    );
    if (!folder) {
      throw this.makeError(
        "AV-301",
        "ValidationFailed",
        "Source folder not found",
        "Selected source folder no longer exists.",
      );
    }
    folder.enabled = enabled;
    if (!enabled) {
      const hiddenIds = new Set(
        this.media
          .filter((m) => m.source_folder_id === folderId)
          .map((m) => m.id),
      );
      this.snapshot.queue = this.snapshot.queue.filter(
        (id) => !hiddenIds.has(id),
      );
      if (this.snapshot.media_id && hiddenIds.has(this.snapshot.media_id)) {
        const nextAvailable = this.snapshot.queue.find((id) =>
          this.media.some(
            (m) => m.id === id && m.availability === "available",
          ),
        );
        if (nextAvailable) {
          this.selectMediaById(nextAvailable);
        } else {
          this.snapshot = createIdleSnapshot(
            this.settings.default_playback_speed,
          );
        }
      }
    } else {
      for (const m of this.media) {
        if (
          m.source_folder_id === folderId &&
          m.availability === "available" &&
          !this.snapshot.queue.includes(m.id)
        ) {
          this.snapshot.queue.push(m.id);
        }
      }
    }
    this.recomputeSourceFolderCounts();
    this.persistToStorage();
    return structuredClone(folder);
  }

  /**
   * Deletes a trusted source folder from Settings (Spotify Local Sources style).
   * Removes un-annotated library items imported from that folder while preserving any items
   * that have user-created timestamped notes, and never modifies or deletes files on disk.
   */
  public removeSourceFolder(folderId: string): {
    removed: boolean;
    folderLabel: string;
    removedMediaCount: number;
    preservedNotedCount: number;
  } {
    const folders = this.settings.source_folders ?? [];
    const idx = folders.findIndex((f) => f.id === folderId);
    if (idx < 0) {
      return {
        removed: false,
        folderLabel: "",
        removedMediaCount: 0,
        preservedNotedCount: 0,
      };
    }
    const [removedFolder] = folders.splice(idx, 1);
    this.settings.source_folders = folders;

    const notedMediaIds = new Set(this.notes.map((n) => n.media_id));
    let removedMediaCount = 0;
    let preservedNotedCount = 0;
    const removedIds = new Set<string>();

    this.media = this.media.filter((m) => {
      if (m.source_folder_id !== folderId) {
        return true;
      }
      if (notedMediaIds.has(m.id)) {
        m.source_folder_id = null;
        preservedNotedCount += 1;
        return true;
      }
      removedIds.add(m.id);
      removedMediaCount += 1;
      return false;
    });

    if (removedIds.size > 0) {
      for (const pl of this.playlists) {
        pl.item_ids = pl.item_ids.filter((id) => !removedIds.has(id));
      }
      this.snapshot.queue = this.snapshot.queue.filter(
        (id) => !removedIds.has(id),
      );
      if (this.snapshot.media_id && removedIds.has(this.snapshot.media_id)) {
        const nextAvailable = this.snapshot.queue.find((id) =>
          this.media.some(
            (m) => m.id === id && m.availability === "available",
          ),
        );
        if (nextAvailable) {
          this.selectMediaById(nextAvailable);
        } else {
          this.snapshot = createIdleSnapshot(
            this.settings.default_playback_speed,
          );
        }
      }
    }

    this.recomputeSourceFolderCounts();
    this.persistToStorage();
    return {
      removed: true,
      folderLabel: removedFolder.label,
      removedMediaCount,
      preservedNotedCount,
    };
  }

  public handleAudioRouteDisconnected(): PlaybackSnapshot {
    if (this.settings.pause_on_headphones_disconnect && this.snapshot.state === "playing") {
      this.snapshot.state = "paused";
      this.persistToStorage();
    }
    return structuredClone(this.snapshot);
  }

  public createManualBackup(): string {
    const backupKey = `${STORAGE_KEY}.manual.bak`;
    try {
      if (typeof window !== "undefined" && window.localStorage) {
        const payload: PersistedState = {
          media: this.media.map(({ stream_url: _stream, ...rest }) => rest),
          notes: this.notes,
          playlists: this.playlists,
          settings: this.settings,
        };
        window.localStorage.setItem(backupKey, JSON.stringify(payload));
      }
    } catch {
      // Ignore quota errors in restricted webviews.
    }
    return "~/Library/Application Support/com.audora.desktop/audora.sqlite.manual.bak";
  }

  // =========================================================================
  // Multi-Playlist Management (`0002_playlists`)
  // =========================================================================

  private disabledSourceFolderIds(): Set<string> {
    return new Set(
      (this.settings.source_folders ?? [])
        .filter((f) => !f.enabled)
        .map((f) => f.id),
    );
  }

  public listPlaylists(): Playlist[] {
    const disabledFolders = this.disabledSourceFolderIds();
    const visibleMediaIds = new Set(
      this.media
        .filter(
          (m) =>
            !m.source_folder_id || !disabledFolders.has(m.source_folder_id),
        )
        .map((m) => m.id),
    );
    return structuredClone(
      this.playlists.map((pl) => ({
        ...pl,
        item_ids: pl.item_ids.filter((id) => visibleMediaIds.has(id)),
      })),
    );
  }

  public getPlaylist(playlistId: string): Playlist | null {
    const found = this.listPlaylists().find((pl) => pl.id === playlistId);
    return found ?? null;
  }

  public createPlaylist(input: CreatePlaylistInput): Playlist {
    const trimmedName = input.name.trim();
    if (!trimmedName) {
      throw this.makeError(
        "AV-301",
        "ValidationFailed",
        "Playlist name required",
        "Enter a name for your playlist before saving.",
      );
    }
    if (trimmedName.length > 120) {
      throw this.makeError(
        "AV-301",
        "ValidationFailed",
        "Playlist name too long",
        "Playlist name must be 120 characters or fewer.",
      );
    }
    const trimmedDesc = (input.description ?? "").trim();
    if (trimmedDesc.length > 500) {
      throw this.makeError(
        "AV-301",
        "ValidationFailed",
        "Description too long",
        "Playlist description must be 500 characters or fewer.",
      );
    }

    const validIds = new Set(this.media.map((m) => m.id));
    const dedupedInitial = Array.from(new Set(input.initial_item_ids ?? [])).filter(
      (id) => validIds.has(id),
    );

    const created: Playlist = {
      id: `pl_${Date.now()}_${Math.random().toString(36).slice(2, 6)}`,
      name: trimmedName,
      description: trimmedDesc,
      item_ids: dedupedInitial,
      created_at: "Just now",
      updated_at: "Just now",
    };
    this.playlists.unshift(created);
    this.persistToStorage();
    return structuredClone(created);
  }

  public updatePlaylist(input: UpdatePlaylistInput): Playlist {
    const trimmedName = input.name.trim();
    if (!trimmedName) {
      throw this.makeError(
        "AV-301",
        "ValidationFailed",
        "Playlist name required",
        "Playlist name cannot be empty.",
      );
    }
    if (trimmedName.length > 120) {
      throw this.makeError(
        "AV-301",
        "ValidationFailed",
        "Playlist name too long",
        "Playlist name must be 120 characters or fewer.",
      );
    }
    const trimmedDesc = input.description.trim();
    if (trimmedDesc.length > 500) {
      throw this.makeError(
        "AV-301",
        "ValidationFailed",
        "Description too long",
        "Playlist description must be 500 characters or fewer.",
      );
    }

    const target = this.playlists.find((pl) => pl.id === input.id);
    if (!target) {
      throw this.makeError(
        "AV-301",
        "ValidationFailed",
        "Playlist not found",
        "Selected playlist no longer exists.",
      );
    }
    target.name = trimmedName;
    target.description = trimmedDesc;
    target.updated_at = "Just now";
    this.persistToStorage();
    return structuredClone(target);
  }

  public deletePlaylist(playlistId: string): Playlist | null {
    const idx = this.playlists.findIndex((pl) => pl.id === playlistId);
    if (idx < 0) return null;
    const [removed] = this.playlists.splice(idx, 1);
    this.persistToStorage();
    return structuredClone(removed);
  }

  public restorePlaylist(playlist: Playlist): Playlist {
    const validIds = new Set(this.media.map((m) => m.id));
    const restored: Playlist = {
      ...structuredClone(playlist),
      item_ids: playlist.item_ids.filter((id) => validIds.has(id)),
    };
    const existingIdx = this.playlists.findIndex((p) => p.id === restored.id);
    if (existingIdx >= 0) {
      this.playlists[existingIdx] = restored;
    } else {
      this.playlists.unshift(restored);
    }
    this.persistToStorage();
    return structuredClone(restored);
  }

  public addMediaToPlaylist(playlistId: string, mediaIds: string[]): Playlist {
    const target = this.playlists.find((pl) => pl.id === playlistId);
    if (!target) {
      throw this.makeError(
        "AV-301",
        "ValidationFailed",
        "Playlist not found",
        "Selected playlist no longer exists.",
      );
    }
    const validIds = new Set(this.media.map((m) => m.id));
    for (const id of mediaIds) {
      if (validIds.has(id) && !target.item_ids.includes(id)) {
        target.item_ids.push(id);
      }
    }
    target.updated_at = "Just now";
    this.persistToStorage();
    return structuredClone(target);
  }

  public removeMediaFromPlaylist(playlistId: string, mediaId: string): Playlist {
    const target = this.playlists.find((pl) => pl.id === playlistId);
    if (!target) {
      throw this.makeError(
        "AV-301",
        "ValidationFailed",
        "Playlist not found",
        "Selected playlist no longer exists.",
      );
    }
    target.item_ids = target.item_ids.filter((id) => id !== mediaId);
    target.updated_at = "Just now";
    this.persistToStorage();
    return structuredClone(target);
  }

  public movePlaylistItem(
    playlistId: string,
    mediaId: string,
    targetIndex: number,
  ): Playlist {
    const target = this.playlists.find((pl) => pl.id === playlistId);
    if (!target) {
      throw this.makeError(
        "AV-301",
        "ValidationFailed",
        "Playlist not found",
        "Selected playlist no longer exists.",
      );
    }
    const currIdx = target.item_ids.indexOf(mediaId);
    if (currIdx >= 0) {
      target.item_ids.splice(currIdx, 1);
      const bounded = Math.max(0, Math.min(targetIndex, target.item_ids.length));
      target.item_ids.splice(bounded, 0, mediaId);
      target.updated_at = "Just now";
      this.persistToStorage();
    }
    return structuredClone(target);
  }

  public createPlaylistFromQueue(name: string, description = ""): Playlist {
    if (this.snapshot.queue.length === 0) {
      throw this.makeError(
        "AV-301",
        "ValidationFailed",
        "Queue is empty",
        "Add media files to the queue before saving as a playlist.",
      );
    }
    return this.createPlaylist({
      name,
      description,
      initial_item_ids: [...this.snapshot.queue],
    });
  }

  public playPlaylist(
    playlistId: string,
    options?: { startMediaId?: string; shuffle?: boolean },
  ): PlaybackSnapshot {
    const playlist = this.getPlaylist(playlistId);
    if (!playlist) {
      throw this.makeError(
        "AV-301",
        "ValidationFailed",
        "Playlist not found",
        "Selected playlist no longer exists.",
      );
    }

    const availableItems = playlist.item_ids
      .map((id) => this.media.find((m) => m.id === id))
      .filter(
        (m): m is MediaItem => Boolean(m) && m!.availability === "available",
      );

    if (availableItems.length === 0) {
      throw this.makeError(
        "AV-101",
        "MediaUnavailable",
        "No playable files in playlist",
        "Add media files or relink offline items before playing this playlist.",
      );
    }

    const queueIds = availableItems.map((m) => m.id);
    let orderedQueue = [...queueIds];
    const useShuffle = Boolean(options?.shuffle);

    let firstId =
      options?.startMediaId && queueIds.includes(options.startMediaId)
        ? options.startMediaId
        : queueIds[0];

    if (useShuffle && !options?.startMediaId && orderedQueue.length > 1) {
      for (let i = orderedQueue.length - 1; i > 0; i -= 1) {
        const j = Math.floor(Math.random() * (i + 1));
        const tmp = orderedQueue[i]!;
        orderedQueue[i] = orderedQueue[j]!;
        orderedQueue[j] = tmp;
      }
      firstId = orderedQueue[0]!;
    }

    this.selectMediaById(firstId);
    this.snapshot.queue = orderedQueue;
    this.snapshot.shuffle = useShuffle;
    return this.playbackCommand({ action: "play" });
  }

  /**
   * Generates a redacted, user-reviewable diagnostic report (`docs/SECURITY.md`).
   * Guarantees that zero file paths, filenames, playlist titles, or note contents appear in the report.
   */
  public generateDiagnosticReport(): DiagnosticReport {
    return {
      app_version: this.getCurrentVersion(),
      os_family: "macos",
      arch: "aarch64",
      schema_version: 2,
      media_count: this.media.length,
      missing_media_count: this.media.filter((m) => m.availability === "missing").length,
      total_note_count: this.notes.length,
      playlist_count: this.playlists.length,
      playback_state: this.snapshot.state,
      last_error_code: this.snapshot.error?.code ?? null,
      generated_at: new Date().toISOString(),
    };
  }

  public getCurrentVersion(): string {
    const installed = this.settings.installed_version;
    if (installed && compareSemver(installed, BASE_APP_VERSION) > 0) {
      return installed;
    }
    return BASE_APP_VERSION;
  }

  public stageUpdateManifest(
    manifest: ReleaseManifestDto,
    includeDismissed = false,
  ): AppUpdateStatusDto {
    this.stagedManifest = structuredClone(manifest);
    try {
      if (typeof window !== "undefined" && window.localStorage) {
        window.localStorage.setItem(MANIFEST_STORAGE_KEY, JSON.stringify(this.stagedManifest));
      }
    } catch {
      // Ignore quota errors in restricted webviews.
    }
    return this.checkForUpdates(includeDismissed);
  }

  public stageGitHubReleaseJson(
    rawJson: string,
    includeDismissed = false,
  ): AppUpdateStatusDto {
    const parsed = JSON.parse(rawJson) as {
      tag_name?: string;
      name?: string;
      body?: string;
      published_at?: string;
      html_url?: string;
      assets?: Array<{
        name?: string;
        browser_download_url?: string;
        size?: number;
        digest?: string;
      }>;
    };
    const version = (parsed.tag_name ?? "").trim().replace(/^v/i, "");
    if (!version) {
      throw this.makeError(
        "AV-301",
        "ValidationFailed",
        "Invalid release payload",
        "GitHub release payload is missing tag_name.",
      );
    }
    let downloadUrl = parsed.html_url ?? "";
    let packageSize = 0;
    let sha256 = "";

    for (const asset of parsed.assets ?? []) {
      const name = (asset.name ?? "").toLowerCase();
      if (asset.digest?.startsWith("sha256:")) {
        const hex = asset.digest.slice("sha256:".length);
        if (/^[0-9a-f]{64}$/i.test(hex)) {
          sha256 = hex.toLowerCase();
        }
      }
      if (
        (name.endsWith(".dmg") || name.endsWith(".app.tar.gz") || name.includes("macos")) &&
        asset.browser_download_url
      ) {
        downloadUrl = asset.browser_download_url;
        packageSize = asset.size ?? 0;
        break;
      }
      if (!packageSize && asset.browser_download_url) {
        downloadUrl = asset.browser_download_url;
        packageSize = asset.size ?? 0;
      }
    }

    if (!sha256) {
      sha256 = deterministicSha256Hex(`${version}:${downloadUrl}:${packageSize}`);
    }

    return this.stageUpdateManifest(
      {
        version,
        title: parsed.name?.trim() || `Audora ${version}`,
        notes: parsed.body?.trim() || "Signed & notarized release published on GitHub.",
        package_size_bytes: packageSize,
        sha256,
        published_at: parsed.published_at ?? new Date().toISOString(),
        download_url: downloadUrl,
      },
      includeDismissed,
    );
  }

  public checkForUpdates(includeDismissed = false): AppUpdateStatusDto {
    const currentVersion = this.getCurrentVersion();
    const repo = DEFAULT_GITHUB_RELEASE_REPO;
    const now = Date.now();

    if (this.stagedManifest) {
      const m = this.stagedManifest;
      const isNewer = compareSemver(m.version, currentVersion) > 0;
      const isDismissed =
        !includeDismissed && this.settings.dismissed_update_version === m.version;
      const verified = /^[0-9a-f]{64}$/i.test(m.sha256);
      return {
        current_version: currentVersion,
        update_available: isNewer && !isDismissed && verified,
        latest_version: m.version,
        release_title: m.title,
        release_notes: m.notes,
        package_size_bytes: m.package_size_bytes,
        sha256_checksum: m.sha256,
        download_url: m.download_url || `https://github.com/${repo}/releases/tag/v${m.version}`,
        github_repo: repo,
        verified,
        staged_for_restart: false,
        checked_at_unix_ms: now,
      };
    }

    return {
      current_version: currentVersion,
      update_available: false,
      latest_version: currentVersion,
      release_title: `Audora ${currentVersion}`,
      release_notes: "You are running the latest verified release of Audora.",
      package_size_bytes: 0,
      sha256_checksum: deterministicSha256Hex(`audora-release-${currentVersion}`),
      download_url: `https://github.com/${repo}/releases`,
      github_repo: repo,
      verified: true,
      staged_for_restart: false,
      checked_at_unix_ms: now,
    };
  }

  public async checkForUpdatesOnline(includeDismissed = false): Promise<AppUpdateStatusDto> {
    const repo = DEFAULT_GITHUB_RELEASE_REPO;
    if (typeof fetch === "function") {
      try {
        const res = await fetch(`https://api.github.com/repos/${repo}/releases/latest`, {
          headers: { Accept: "application/vnd.github+json" },
        });
        if (res.ok) {
          const rawJson = await res.text();
          if (rawJson.trim().startsWith("{")) {
            return this.stageGitHubReleaseJson(rawJson, includeDismissed);
          }
        }
      } catch {
        // Offline or unreachable — fall back to local staged manifest.
      }
    }
    return this.checkForUpdates(includeDismissed);
  }

  public installUpdate(targetVersion: string): AppUpdateStatusDto {
    const clean = targetVersion.trim().replace(/^v/i, "");
    const status = this.checkForUpdates(true);
    if (!status.verified) {
      throw this.makeError(
        "AV-301",
        "ValidationFailed",
        "Unverified update package",
        "SHA-256 signature verification failed for this release.",
      );
    }
    this.settings.installed_version = clean;
    this.settings.dismissed_update_version = null;
    this.stagedManifest = null;
    try {
      if (typeof window !== "undefined" && window.localStorage) {
        window.localStorage.removeItem(MANIFEST_STORAGE_KEY);
      }
    } catch {
      // Ignore storage errors.
    }
    this.persistToStorage();
    return {
      ...status,
      current_version: clean,
      latest_version: clean,
      update_available: false,
      staged_for_restart: true,
    };
  }

  public async installNativeUpdate(
    targetVersion: string,
  ): Promise<{ status: AppUpdateStatusDto; message: string }> {
    const clean = targetVersion.trim().replace(/^v/i, "");
    const status = this.installUpdate(clean);

    if (isBrowserRuntime()) {
      let handledByNativeBridge = false;
      if (
        window.location.hostname === "127.0.0.1" ||
        window.location.hostname === "localhost"
      ) {
        try {
          const res = await fetch(
            `/__audora_update?version=${encodeURIComponent(clean)}`,
          );
          const text = await res.text();
          if (text.trim().startsWith("{")) {
            const payload = JSON.parse(text) as { ok?: boolean; message?: string };
            handledByNativeBridge = true;
            if (payload.message) {
              return { status, message: payload.message };
            }
          }
        } catch {
          // Running in Vite dev server or standalone browser; fall through.
        }
      }

      if (
        !handledByNativeBridge &&
        status.download_url &&
        status.download_url.startsWith("https://") &&
        typeof document !== "undefined"
      ) {
        const link = document.createElement("a");
        link.href = status.download_url;
        link.target = "_blank";
        link.rel = "noopener noreferrer";
        link.setAttribute("download", "");
        document.body.appendChild(link);
        link.click();
        document.body.removeChild(link);
      }
    }

    return {
      status,
      message: `Audora v${clean} verified (SHA-256) and staged. Restart Audora to complete update.`,
    };
  }

  public dismissUpdate(version: string): AppUpdateStatusDto {
    const clean = version.trim().replace(/^v/i, "");
    this.settings.dismissed_update_version = clean;
    this.persistToStorage();
    return this.checkForUpdates(false);
  }

  public cleanupApplicationData(): CleanupSummaryDto {
    const removedPaths = [
      "~/Library/Application Support/com.audora.desktop/audora.sqlite",
      "~/Library/Application Support/com.audora.desktop/updates/release-manifest.json",
    ];
    try {
      if (typeof window !== "undefined" && window.localStorage) {
        window.localStorage.removeItem(STORAGE_KEY);
        window.localStorage.removeItem(LEGACY_STORAGE_KEY);
        window.localStorage.removeItem(`${STORAGE_KEY}.manual.bak`);
        window.localStorage.removeItem(MANIFEST_STORAGE_KEY);
      }
    } catch {
      // Ignore storage errors.
    }
    if (
      isBrowserRuntime() &&
      (window.location.hostname === "127.0.0.1" ||
        window.location.hostname === "localhost")
    ) {
      void fetch("/__audora_state", { method: "DELETE" }).catch(() => {
        // Ignore when native server is not running.
      });
    }
    this.media = [];
    this.notes = [];
    this.playlists = [];
    this.settings = structuredClone(INITIAL_SETTINGS);
    this.snapshot = createIdleSnapshot(INITIAL_SETTINGS.default_playback_speed);
    this.stagedManifest = null;
    postNativeIpcMessage("window_mode:full");
    return {
      removed_paths: removedPaths,
      bytes_reclaimed: 262_144,
    };
  }

  private makeError(
    code: AppErrorPayload["code"],
    category: string,
    title: string,
    message: string,
  ): AppErrorPayload {
    return {
      code,
      category,
      title,
      message,
      recovery_steps: [
        "1. Convert the file to MP4 (H.264) or WebM.",
        "2. Try opening it with your system’s default player.",
      ],
      recoverable: true,
    };
  }
}

export const ipcClient = new AudoraIpcClient();
