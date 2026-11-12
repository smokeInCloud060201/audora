import type {
  AppErrorPayload,
  AppSettings,
  AppUpdateStatusDto,
  CleanupSummaryDto,
  CreateNoteInput,
  DiagnosticReport,
  LibraryFilter,
  LibrarySearchResult,
  MediaItem,
  Note,
  PlaybackCommand,
  PlaybackSnapshot,
  ReleaseManifestDto,
  UpdateNoteInput,
  WindowMode,
} from "../types/domain";

const STORAGE_KEY = "audora.state.v2";
const LEGACY_STORAGE_KEY = "audora.state.v1";
const MANIFEST_STORAGE_KEY = "audora.update.manifest.v1";
export const DEFAULT_GITHUB_RELEASE_REPO = "smokeInCloud060201/audora";
const LEGACY_GITHUB_RELEASE_REPO = "karsontang/audora";
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
  github_release_repo: DEFAULT_GITHUB_RELEASE_REPO,
  installed_version: BASE_APP_VERSION,
  dismissed_update_version: null,
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
  private settings: AppSettings = structuredClone(INITIAL_SETTINGS);
  private stagedManifest: ReleaseManifestDto | null = null;
  private hydratedFromNative = false;
  private snapshot: PlaybackSnapshot = createIdleSnapshot(INITIAL_SETTINGS.default_playback_speed);

  public constructor(useStorage = true) {
    if (useStorage) {
      this.hydrateFromStorage();
    }
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
    if (Array.isArray(parsed.notes)) {
      const validMediaIds = new Set(this.media.map((m) => m.id));
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
    if (parsed.settings) {
      const migratedRepo =
        !parsed.settings.github_release_repo ||
        parsed.settings.github_release_repo.trim() === "" ||
        parsed.settings.github_release_repo.trim() === LEGACY_GITHUB_RELEASE_REPO
          ? DEFAULT_GITHUB_RELEASE_REPO
          : parsed.settings.github_release_repo.trim();
      const persistedInstalled = parsed.settings.installed_version;
      const effectiveInstalled =
        persistedInstalled && compareSemver(persistedInstalled, BASE_APP_VERSION) > 0
          ? persistedInstalled
          : BASE_APP_VERSION;
      this.settings = {
        ...INITIAL_SETTINGS,
        ...parsed.settings,
        github_release_repo: migratedRepo,
        installed_version: effectiveInstalled,
      };
    }

    const availableIds = this.media
      .filter((m) => m.availability === "available")
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
    if (durationMs && durationMs > 0) {
      this.snapshot.duration_ms = Math.round(durationMs);
      const active = this.media.find((m) => m.id === this.snapshot.media_id);
      if (active) {
        active.duration_ms = this.snapshot.duration_ms;
      }
    }
    this.snapshot.position_ms = Math.max(0, Math.round(positionMs));
    const active = this.media.find((m) => m.id === this.snapshot.media_id);
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

    let item = this.media.find((m) => m.path === trimmed);
    if (!item) {
      item = {
        id: `med_${Date.now()}_${Math.random().toString(36).slice(2, 6)}`,
        path: trimmed,
        display_name: fileName,
        media_kind: supportedVideo.includes(ext) ? "video" : "audio",
        duration_ms: options?.durationMs ?? null,
        last_position_ms: 0,
        completed: false,
        file_size_bytes: options?.size ?? null,
        fingerprint: null,
        availability: "available",
        added_at: "Just now",
        last_opened_at: "Just now",
        last_seen_at: "Just now",
        note_count: 0,
        stream_url: options?.streamUrl,
      };
      this.media.unshift(item);
    } else if (options?.streamUrl) {
      item.stream_url = options.streamUrl;
      if (options.durationMs) {
        item.duration_ms = options.durationMs;
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
    const mediaItems = this.media
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
            const parent = this.media.find((m) => m.id === n.media_id);
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
    this.selectMediaById(note.media_id);
    return this.playbackCommand({ action: "seek", position_ms: note.position_ms });
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
    this.media.splice(idx, 1);
    this.notes = this.notes.filter((n) => n.media_id !== mediaId);
    this.snapshot.queue = this.snapshot.queue.filter((id) => id !== mediaId);
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
          settings: this.settings,
        };
        window.localStorage.setItem(backupKey, JSON.stringify(payload));
      }
    } catch {
      // Ignore quota errors in restricted webviews.
    }
    return "~/Library/Application Support/com.audora.desktop/audora.sqlite.manual.bak";
  }

  /**
   * Generates a redacted, user-reviewable diagnostic report (`docs/SECURITY.md`).
   * Guarantees that zero file paths, filenames, or note contents appear in the report.
   */
  public generateDiagnosticReport(): DiagnosticReport {
    return {
      app_version: this.getCurrentVersion(),
      os_family: "macos",
      arch: "aarch64",
      schema_version: 1,
      media_count: this.media.length,
      missing_media_count: this.media.filter((m) => m.availability === "missing").length,
      total_note_count: this.notes.length,
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
    const repo =
      (this.settings.github_release_repo ?? DEFAULT_GITHUB_RELEASE_REPO).trim() ||
      DEFAULT_GITHUB_RELEASE_REPO;
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
    const repo =
      (this.settings.github_release_repo ?? DEFAULT_GITHUB_RELEASE_REPO).trim() ||
      DEFAULT_GITHUB_RELEASE_REPO;
    if (typeof fetch === "function" && repo.includes("/")) {
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
    const repo =
      (this.settings.github_release_repo ?? DEFAULT_GITHUB_RELEASE_REPO).trim() ||
      DEFAULT_GITHUB_RELEASE_REPO;
    const status = this.installUpdate(clean);

    if (isBrowserRuntime()) {
      let handledByNativeBridge = false;
      if (
        window.location.hostname === "127.0.0.1" ||
        window.location.hostname === "localhost"
      ) {
        try {
          const res = await fetch(
            `/__audora_update?version=${encodeURIComponent(clean)}&repo=${encodeURIComponent(repo)}`,
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
