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
  RepeatMode,
  ThemeMode,
  TrustedSourceFolder,
  UpdateNoteInput,
  UpdatePlaylistInput,
  WindowMode,
} from "../types/domain";
import { formatTimestamp } from "../utils/format";

const STORAGE_KEY = "audora.state.v2";
const LEGACY_STORAGE_KEY = "audora.state.v1";
const MANIFEST_STORAGE_KEY = "audora.update.manifest.v1";
const MAX_NOTE_BODY_CHARS = 10_000;
const ISO_BMFF_EXTENSIONS = new Set(["mp4", "m4a", "m4v", "mov"]);
const SUPPORTED_VIDEO_EXTS = ["mp4", "m4v", "mov", "webm", "mkv"];
const SUPPORTED_AUDIO_EXTS = ["wav", "flac", "ogg", "opus", "mp3", "m4a", "aac"];
const SUPPORTED_MEDIA_EXT_SET = new Set([...SUPPORTED_VIDEO_EXTS, ...SUPPORTED_AUDIO_EXTS]);
export const DEFAULT_GITHUB_RELEASE_REPO = "smokeInCloud060201/audora";
const BASE_APP_VERSION: string =
  (typeof import.meta !== "undefined" &&
    (import.meta.env?.VITE_APP_VERSION as string | undefined)?.trim()) ||
  "1.1.2";

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

interface NativePlatformBootstrap {
  os?: string;
  arch?: string;
  app_version?: string;
  data_dir?: string;
  initial_theme?: ThemeMode;
  initial_window_mode?: WindowMode;
  cli_files?: Array<{
    path: string;
    name: string;
    size: number;
    duration_ms?: number | null;
  }>;
}

function getNativeSessionToken(): string | null {
  if (typeof window === "undefined") return null;
  const win = window as Window & { __AUDORA_SESSION_TOKEN__?: unknown };
  return typeof win.__AUDORA_SESSION_TOKEN__ === "string" && win.__AUDORA_SESSION_TOKEN__.length > 0
    ? win.__AUDORA_SESSION_TOKEN__
    : null;
}

function getNativePlatformInfo(): {
  os: string;
  arch: string;
  appVersion: string | null;
  dataDir: string;
  initialTheme: ThemeMode | null;
  initialWindowMode: WindowMode | null;
  cliFiles: NonNullable<NativePlatformBootstrap["cli_files"]>;
} {
  if (typeof window !== "undefined") {
    const win = window as Window & { __AUDORA_PLATFORM__?: NativePlatformBootstrap };
    const boot = win.__AUDORA_PLATFORM__;
    if (boot?.os && boot?.arch && boot?.data_dir) {
      return {
        os: boot.os,
        arch: boot.arch,
        appVersion: typeof boot.app_version === "string" && boot.app_version.trim() ? boot.app_version.trim() : null,
        dataDir: boot.data_dir,
        initialTheme:
          boot.initial_theme === "light" || boot.initial_theme === "dark" || boot.initial_theme === "system"
            ? boot.initial_theme
            : null,
        initialWindowMode:
          boot.initial_window_mode === "minimum" || boot.initial_window_mode === "full"
            ? boot.initial_window_mode
            : null,
        cliFiles: Array.isArray(boot.cli_files) ? boot.cli_files : [],
      };
    }
  }
  const ua = typeof navigator !== "undefined" ? (navigator.userAgent ?? "").toLowerCase() : "";
  if (ua.includes("win")) {
    return {
      os: "windows",
      arch: ua.includes("arm") ? "aarch64" : "x86_64",
      appVersion: null,
      dataDir: "%APPDATA%\\com.audora.desktop\\data",
      initialTheme: null,
      initialWindowMode: null,
      cliFiles: [],
    };
  }
  if (ua.includes("linux")) {
    return {
      os: "linux",
      arch: ua.includes("aarch64") || ua.includes("arm64") ? "aarch64" : "x86_64",
      appVersion: null,
      dataDir: "~/.local/share/com.audora.desktop",
      initialTheme: null,
      initialWindowMode: null,
      cliFiles: [],
    };
  }
  return {
    os: "macos",
    arch: ua.includes("x86_64") || ua.includes("intel") ? "x86_64" : "aarch64",
    appVersion: null,
    dataDir: "~/Library/Application Support/com.audora.desktop",
    initialTheme: null,
    initialWindowMode: null,
    cliFiles: [],
  };
}

function isJsonResponse(res: Response): boolean {
  const ct = (res.headers.get("content-type") ?? "").toLowerCase();
  return ct.includes("application/json");
}

function nativeFetch(url: string, init?: RequestInit): Promise<Response> {
  const token = getNativeSessionToken();
  const headers = new Headers(init?.headers ?? {});
  if (token) {
    headers.set("X-Audora-Token", token);
  }
  return fetch(url, {
    ...init,
    headers,
  });
}

function revokeIfBlobUrl(url: string | undefined): void {
  if (
    url &&
    url.startsWith("blob:") &&
    typeof URL !== "undefined" &&
    typeof URL.revokeObjectURL === "function"
  ) {
    try {
      URL.revokeObjectURL(url);
    } catch {
      // Ignore revoke errors in test environments
    }
  }
}

function shufflePermutation(ids: string[], avoidFirstId?: string | null): string[] {
  const arr = [...ids];
  if (arr.length <= 1) return arr;
  for (let i = arr.length - 1; i > 0; i -= 1) {
    const j = Math.floor(Math.random() * (i + 1));
    const tmp = arr[i]!;
    arr[i] = arr[j]!;
    arr[j] = tmp;
  }
  if (arr.length >= 2 && arr.every((id, idx) => id === ids[idx])) {
    const first = arr[0]!;
    arr[0] = arr[arr.length - 1]!;
    arr[arr.length - 1] = first;
  }
  if (avoidFirstId && arr.length >= 2 && arr[0] === avoidFirstId) {
    const swapIdx = arr.findIndex((id) => id !== avoidFirstId);
    if (swapIdx > 0) {
      const tmp = arr[0]!;
      arr[0] = arr[swapIdx]!;
      arr[swapIdx] = tmp;
    }
  }
  return arr;
}

export function compareSemver(a: string, b: string): number {
  const parse = (v: string): { core: [number, number, number]; pre: string | null } => {
    const clean = v.trim().replace(/^v/i, "").split("+")[0] ?? "0.0.0";
    const dashIdx = clean.indexOf("-");
    const coreStr = dashIdx >= 0 ? clean.slice(0, dashIdx) : clean;
    const pre = dashIdx >= 0 && clean.slice(dashIdx + 1).length > 0 ? clean.slice(dashIdx + 1) : null;
    const parts = coreStr.split(".").map((n) => Number.parseInt(n, 10) || 0);
    return { core: [parts[0] ?? 0, parts[1] ?? 0, parts[2] ?? 0], pre };
  };
  const pa = parse(a);
  const pb = parse(b);
  for (let i = 0; i < 3; i += 1) {
    if (pa.core[i] !== pb.core[i]) {
      return pa.core[i]! > pb.core[i]! ? 1 : -1;
    }
  }
  if (pa.pre === null && pb.pre === null) return 0;
  if (pa.pre !== null && pb.pre === null) return -1;
  if (pa.pre === null && pb.pre !== null) return 1;
  const segsA = pa.pre!.split(".");
  const segsB = pb.pre!.split(".");
  const maxLen = Math.max(segsA.length, segsB.length);
  for (let i = 0; i < maxLen; i += 1) {
    const sa = segsA[i];
    const sb = segsB[i];
    if (sa === undefined) return -1;
    if (sb === undefined) return 1;
    const na = /^\d+$/.test(sa) ? Number.parseInt(sa, 10) : null;
    const nb = /^\d+$/.test(sb) ? Number.parseInt(sb, 10) : null;
    if (na !== null && nb !== null) {
      if (na !== nb) return na > nb ? 1 : -1;
    } else if (na !== null) {
      return -1;
    } else if (nb !== null) {
      return 1;
    } else if (sa !== sb) {
      return sa > sb ? 1 : -1;
    }
  }
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

interface PersistedPlaybackSession {
  active_media_id: string | null;
  queue: string[];
  volume: number;
  muted: boolean;
  rate: number;
  repeat_mode: RepeatMode;
  shuffle: boolean;
}

interface PersistedState {
  media: MediaItem[];
  notes: Note[];
  playlists?: Playlist[];
  settings: AppSettings;
  playback_session?: PersistedPlaybackSession;
}

export interface LocalFileDescriptor {
  name: string;
  path?: string;
  size: number;
  streamUrl?: string;
  durationMs?: number;
}

export interface NativeDroppedEntry {
  kind?: "file" | "dir";
  path: string;
  name?: string;
  size?: number;
  duration_ms?: number | null;
}

interface QueuedRpcEntry {
  epoch: number;
  payload: Record<string, unknown>;
  bestEffort?: boolean;
  onSettled?: (ok: boolean) => void;
  settled?: boolean;
}

export class AudoraIpcClient {
  public onPersistenceError?: (err: AppErrorPayload) => void;
  private readonly useStorage: boolean;
  private stateEpoch = 0;
  private media: MediaItem[] = [];
  private notes: Note[] = [];
  private playlists: Playlist[] = [];
  private settings: AppSettings = structuredClone(INITIAL_SETTINGS);
  private stagedManifest: ReleaseManifestDto | null = null;
  private stagedRestartVersion: string | null = null;
  private hydrationPromise: Promise<void> | null = null;
  private nativeHydrationState:
    | "idle"
    | "in_progress"
    | "hydrated"
    | "failed"
    | "unavailable" = "idle";
  private preHydrationDirtySettings = new Set<keyof AppSettings>();
  private folderSyncPromise: Promise<number> | null = null;
  private durationProbePromise: Promise<boolean> | null = null;
  private cliBootProcessed = false;
  private hasNativeRelationalRpc = Boolean(getNativeSessionToken());
  private rpcQueue: QueuedRpcEntry[] = [];
  private inFlightRpcEntry: QueuedRpcEntry | null = null;
  private isFlushingRpc = false;
  private rpcFlushPromise: Promise<void> | null = null;
  private pendingNativeSave: string | null = null;
  private isSavingNative = false;
  private seekDebounceTimer: ReturnType<typeof setTimeout> | null = null;
  private lastProgressPersistMs = 0;
  private shuffleOrder: string[] = [];
  private unshuffledQueue: string[] | null = null;
  private snapshot: PlaybackSnapshot = createIdleSnapshot(INITIAL_SETTINGS.default_playback_speed);

  public constructor(useStorage = true) {
    this.useStorage = useStorage;
    if (useStorage) {
      const boot = getNativePlatformInfo();
      if (boot.initialTheme) {
        this.settings.theme = boot.initialTheme;
      }
      if (boot.initialWindowMode) {
        this.settings.window_mode = boot.initialWindowMode;
      }
      if (boot.appVersion) {
        this.settings.installed_version = boot.appVersion;
      }
      this.hydrateFromStorage();
      if (isBrowserRuntime() && typeof window !== "undefined") {
        const win = window as Window & { __AUDORA_COLLECT_QUIT_BATCH__?: () => string };
        win.__AUDORA_COLLECT_QUIT_BATCH__ = () => JSON.stringify(this.collectPendingRpcBatchForExit());
      }
    }
  }

  private isNativeLocalOrigin(): boolean {
    return (
      this.useStorage &&
      isBrowserRuntime() &&
      (window.location.hostname === "127.0.0.1" ||
        window.location.hostname === "localhost")
    );
  }

  private shouldTrackPreHydrationSettings(): boolean {
    return (
      this.isNativeLocalOrigin() &&
      (this.nativeHydrationState === "idle" ||
        this.nativeHydrationState === "in_progress" ||
        this.nativeHydrationState === "failed")
    );
  }

  private settleRpcEntry(entry: QueuedRpcEntry | null | undefined, ok: boolean): void {
    if (!entry || entry.settled) return;
    entry.settled = true;
    entry.onSettled?.(ok);
  }

  private transitionSnapshotToIdle(preservedQueue: string[] = []): void {
    this.snapshot = {
      generation: this.snapshot.generation + 1,
      state: "idle",
      media_id: null,
      position_ms: 0,
      duration_ms: null,
      rate: this.snapshot.rate,
      volume: this.snapshot.volume,
      muted: this.snapshot.muted,
      repeat_mode: this.snapshot.repeat_mode,
      shuffle: this.snapshot.shuffle,
      queue: preservedQueue,
      error: null,
    };
  }

  private collectPendingRpcBatchForExit(): Array<Record<string, unknown>> {
    if (this.seekDebounceTimer !== null) {
      clearTimeout(this.seekDebounceTimer);
      this.seekDebounceTimer = null;
    }
    const drained: QueuedRpcEntry[] = [];
    if (this.inFlightRpcEntry) {
      drained.push(this.inFlightRpcEntry);
      this.inFlightRpcEntry = null;
    }
    drained.push(...this.rpcQueue);
    this.rpcQueue = [];
    const batch = drained
      .filter((entry) => entry.epoch === this.stateEpoch && !entry.bestEffort)
      .map((entry) => entry.payload);
    for (const entry of drained) {
      this.settleRpcEntry(entry, false);
    }
    const active = this.media.find((m) => m.id === this.snapshot.media_id);
    if (active) {
      batch.push({
        op: "update_progress",
        media_id: active.id,
        last_position_ms: active.last_position_ms,
        completed: active.completed,
        session: this.buildSessionDto(),
      });
    } else if (
      this.media.length > 0 ||
      this.snapshot.queue.length > 0 ||
      this.snapshot.media_id !== null
    ) {
      batch.push({
        op: "save_session",
        session: this.buildSessionDto(),
      });
    }
    return batch;
  }

  public isPersistedRuntime(): boolean {
    return this.useStorage;
  }

  public getAppDataDirectoryDisplay(): string {
    return getNativePlatformInfo().dataDir;
  }

  private isBareFileName(rawPath: string): boolean {
    return !rawPath.includes("/") && !rawPath.includes("\\");
  }

  private normalizeComparablePath(rawPath: string): string {
    const normalized = rawPath.trim().replace(/\\/g, "/");
    const stripped = normalized.length > 1 ? normalized.replace(/\/+$/, "") : normalized;
    return getNativePlatformInfo().os === "windows" ? stripped.toLowerCase() : stripped;
  }

  private computeLocalStreamUrl(
    rawPath: string,
    explicitStreamUrl?: string,
  ): string | undefined {
    if (
      isBrowserRuntime() &&
      (window.location.hostname === "127.0.0.1" ||
        window.location.hostname === "localhost") &&
      (rawPath.startsWith("/") || rawPath.startsWith("\\\\") || /^[A-Za-z]:[\\/]/.test(rawPath))
    ) {
      const tok = getNativeSessionToken();
      const tokenParam = tok ? `&token=${encodeURIComponent(tok)}` : "";
      return `/__audora_media?path=${encodeURIComponent(rawPath)}${tokenParam}`;
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
            if (candidate.stream_url && candidate.stream_url !== persistentUrl) {
              revokeIfBlobUrl(candidate.stream_url);
            }
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
      const nextStreamUrl = !this.isBareFileName(existing.path)
        ? (this.computeLocalStreamUrl(existing.path) ??
            existing.stream_url ??
            candidate.stream_url)
        : (existing.stream_url ?? candidate.stream_url);
      if (existing.stream_url && existing.stream_url !== nextStreamUrl) {
        revokeIfBlobUrl(existing.stream_url);
      }
      if (candidate.stream_url && candidate.stream_url !== nextStreamUrl) {
        revokeIfBlobUrl(candidate.stream_url);
      }
      existing.stream_url = nextStreamUrl;
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
      const nextStreamUrls = new Set(
        filteredMedia.map((m) => m.stream_url).filter((u): u is string => Boolean(u)),
      );
      for (const oldItem of this.media) {
        if (oldItem.stream_url && !nextStreamUrls.has(oldItem.stream_url)) {
          revokeIfBlobUrl(oldItem.stream_url);
        }
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
    const fallbackIso = new Date().toISOString();
    if (Array.isArray(parsed.playlists)) {
      this.playlists = parsed.playlists.map((pl) => ({
        id: pl.id,
        name: pl.name,
        description: pl.description ?? "",
        item_ids: Array.isArray(pl.item_ids)
          ? Array.from(new Set(pl.item_ids)).filter((id) => validMediaIds.has(id))
          : [],
        created_at: pl.created_at || fallbackIso,
        updated_at: pl.updated_at || fallbackIso,
      }));
    } else {
      this.playlists = [];
    }
    if (this.deduplicateMediaRecords()) {
      strippedLegacyMock = true;
    }
    if (parsed.settings) {
      const runtimeBase = getNativePlatformInfo().appVersion ?? BASE_APP_VERSION;
      const persistedInstalled = parsed.settings.installed_version;
      const effectiveInstalled =
        !getNativeSessionToken() &&
        persistedInstalled &&
        compareSemver(persistedInstalled, runtimeBase) > 0
          ? persistedInstalled
          : runtimeBase;
      const normalizedFolders = Array.isArray(parsed.settings.source_folders)
        ? parsed.settings.source_folders.map((f) => ({
            id: f.id,
            path: f.path,
            label: f.label || (f.path.split(/[\\/]/).pop() ?? f.path),
            enabled: f.enabled !== false,
            added_at: f.added_at || fallbackIso,
            last_scanned_at: f.last_scanned_at || fallbackIso,
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

    for (const m of this.media) {
      if (!this.isBareFileName(m.path)) {
        const computed = this.computeLocalStreamUrl(m.path);
        if (computed) {
          if (m.stream_url && m.stream_url !== computed) {
            revokeIfBlobUrl(m.stream_url);
          }
          m.stream_url = computed;
        }
      }
    }

    this.unshuffledQueue = null;
    this.shuffleOrder = [];

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
    const savedSession = parsed.playback_session;
    const restoredRate =
      savedSession &&
      typeof savedSession.rate === "number" &&
      savedSession.rate >= 0.5 &&
      savedSession.rate <= 2.0
        ? savedSession.rate
        : this.settings.default_playback_speed;
    const restoredVolume =
      savedSession && typeof savedSession.volume === "number"
        ? Math.max(0, Math.min(1, savedSession.volume))
        : 1.0;
    const restoredMuted = Boolean(savedSession?.muted);
    const restoredRepeatMode = savedSession?.repeat_mode ?? "off";
    const restoredShuffle = Boolean(savedSession?.shuffle);

    if (availableIds.length > 0) {
      const availableSet = new Set(availableIds);
      const restoredQueue =
        savedSession && Array.isArray(savedSession.queue) && savedSession.queue.length > 0
          ? Array.from(new Set(savedSession.queue.filter((id) => availableSet.has(id))))
          : [];
      const finalQueue = restoredQueue.length > 0 ? restoredQueue : availableIds;
      const preferredId =
        savedSession?.active_media_id && availableSet.has(savedSession.active_media_id)
          ? savedSession.active_media_id
          : finalQueue[0]!;
      if (!finalQueue.includes(preferredId)) {
        finalQueue.unshift(preferredId);
      }
      const activeItem =
        this.media.find((m) => m.id === preferredId) ??
        this.media.find((m) => m.id === availableIds[0])!;

      this.snapshot = {
        ...createIdleSnapshot(restoredRate),
        generation: Math.max(1, this.snapshot.generation + 1),
        state: "paused",
        media_id: activeItem.id,
        position_ms:
          this.settings.resume_where_left_off && !activeItem.completed
            ? activeItem.last_position_ms
            : 0,
        duration_ms: activeItem.duration_ms,
        volume: restoredVolume,
        muted: restoredMuted,
        repeat_mode: restoredRepeatMode,
        shuffle: restoredShuffle,
        queue: finalQueue,
      };
      if (this.snapshot.shuffle) {
        if (savedSession && Array.isArray(savedSession.queue) && savedSession.queue.length > 0) {
          this.shuffleOrder = [...finalQueue];
        } else {
          this.rebuildShuffleOrder(this.snapshot.media_id);
          this.snapshot.queue = [...this.shuffleOrder];
        }
      }
    } else {
      this.snapshot = {
        ...createIdleSnapshot(restoredRate),
        generation: Math.max(1, this.snapshot.generation + 1),
        volume: restoredVolume,
        muted: restoredMuted,
        repeat_mode: restoredRepeatMode,
        shuffle: restoredShuffle,
      };
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
          this.persistToStorage({ skipNative: true });
        }
      } else if (rawLegacy) {
        const parsed = JSON.parse(rawLegacy) as Partial<PersistedState>;
        this.applyPersistedState(parsed);
        window.localStorage.removeItem(LEGACY_STORAGE_KEY);
        this.persistToStorage({ skipNative: true });
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
    if (!this.useStorage || !isBrowserRuntime()) {
      return;
    }
    if (this.hydrationPromise) {
      return this.hydrationPromise;
    }
    this.hydrationPromise = this.performNativeHydration();
    return this.hydrationPromise;
  }

  private hasNonDefaultSettings(): boolean {
    return (
      this.settings.theme !== INITIAL_SETTINGS.theme ||
      this.settings.resume_where_left_off !== INITIAL_SETTINGS.resume_where_left_off ||
      this.settings.pause_on_headphones_disconnect !==
        INITIAL_SETTINGS.pause_on_headphones_disconnect ||
      this.settings.default_playback_speed !== INITIAL_SETTINGS.default_playback_speed ||
      this.settings.timestamp_precision !== INITIAL_SETTINGS.timestamp_precision ||
      this.settings.window_mode !== INITIAL_SETTINGS.window_mode ||
      this.settings.auto_check_updates !== INITIAL_SETTINGS.auto_check_updates ||
      Boolean(this.settings.dismissed_update_version) ||
      (this.settings.source_folders?.length ?? 0) > 0 ||
      this.settings.show_playlists_bar !== INITIAL_SETTINGS.show_playlists_bar ||
      this.settings.show_side_panel !== INITIAL_SETTINGS.show_side_panel
    );
  }

  private async performNativeHydration(): Promise<void> {
    if (
      window.location.hostname !== "127.0.0.1" &&
      window.location.hostname !== "localhost"
    ) {
      this.nativeHydrationState = "unavailable";
      return;
    }
    this.nativeHydrationState = "in_progress";
    const hydrationEpoch = this.stateEpoch;
    try {
      const res = await nativeFetch("/__audora_state", { method: "GET" });
      if (hydrationEpoch !== this.stateEpoch) {
        return;
      }
      if (!res.ok) {
        if (res.status === 404) {
          this.nativeHydrationState = "unavailable";
          if (this.preHydrationDirtySettings.size > 0) {
            this.preHydrationDirtySettings.clear();
            this.persistToStorage({ skipNative: true });
          }
        } else {
          // Never overwrite SQLite or treat a server/DB error (e.g. 500) as an empty DB (`R2-C2`).
          this.nativeHydrationState = "failed";
          this.hydrationPromise = null;
          return;
        }
      } else if (!isJsonResponse(res)) {
        this.nativeHydrationState = "unavailable";
        if (this.preHydrationDirtySettings.size > 0) {
          this.preHydrationDirtySettings.clear();
          this.persistToStorage({ skipNative: true });
        }
      } else {
        const text = await res.text();
        if (hydrationEpoch !== this.stateEpoch) {
          return;
        }
        const trimmed = text.trim();
        let parsedObj: Partial<PersistedState> | null = null;
        if (trimmed.startsWith("{")) {
          const candidate: unknown = JSON.parse(trimmed);
          if (candidate && typeof candidate === "object" && !Array.isArray(candidate)) {
            parsedObj = candidate as Partial<PersistedState>;
          }
        }
        if (parsedObj === null) {
          this.nativeHydrationState = "failed";
          this.hydrationPromise = null;
          return;
        }
        this.hasNativeRelationalRpc = true;
        let canRemoveLocalStorage = false;
        if (trimmed !== "{}") {
          const dirtyKeys = new Set(this.preHydrationDirtySettings);
          const dirtyOverrides: Partial<AppSettings> = {};
          const copySettingKey = <K extends keyof AppSettings>(key: K) => {
            dirtyOverrides[key] = structuredClone(this.settings[key]);
          };
          for (const key of dirtyKeys) {
            copySettingKey(key);
          }
          // `audora.sqlite` relational tables are the authoritative source of truth (`C-6`).
          this.applyPersistedState(parsedObj);
          Object.assign(this.settings, dirtyOverrides);
          if (dirtyKeys.has("default_playback_speed")) {
            this.snapshot.rate = this.settings.default_playback_speed;
          }
          this.preHydrationDirtySettings.clear();
          this.nativeHydrationState = "hydrated";
          if (dirtyKeys.size > 0) {
            this.dispatchRelationalRpc({ op: "save_settings", settings: this.settings });
          }
          canRemoveLocalStorage = true;
        } else {
          const hadDirty = this.preHydrationDirtySettings.size > 0;
          this.preHydrationDirtySettings.clear();
          this.nativeHydrationState = "hydrated";
          if (
            this.media.length > 0 ||
            this.notes.length > 0 ||
            this.playlists.length > 0 ||
            this.hasNonDefaultSettings() ||
            hadDirty
          ) {
            // One-time seed of empty SQLite relational tables from existing localStorage state.
            try {
              const seedRes = await nativeFetch("/__audora_state", {
                method: "POST",
                headers: { "Content-Type": "application/json" },
                body: this.serializePersistedState(),
              });
              canRemoveLocalStorage = seedRes.ok && hydrationEpoch === this.stateEpoch;
            } catch {
              canRemoveLocalStorage = false;
            }
          } else {
            canRemoveLocalStorage = true;
          }
        }
        if (canRemoveLocalStorage) {
          try {
            if (typeof window !== "undefined" && window.localStorage) {
              window.localStorage.removeItem(STORAGE_KEY);
            }
          } catch {
            // Ignore storage cleanup errors in restricted webviews.
          }
        }
      }
    } catch {
      if (this.hasNativeRelationalRpc) {
        this.nativeHydrationState = "failed";
        this.hydrationPromise = null;
        return;
      }
      // Ignore when running on Vite dev server without `/__audora_state`.
      this.nativeHydrationState = "unavailable";
      if (this.preHydrationDirtySettings.size > 0) {
        this.preHydrationDirtySettings.clear();
        this.persistToStorage({ skipNative: true });
      }
    }

    // Ingest any media paths passed on the native CLI (`audora /path/to/video.mp4`) (`C-6`).
    if (!this.cliBootProcessed) {
      this.cliBootProcessed = true;
      const { cliFiles } = getNativePlatformInfo();
      if (cliFiles.length > 0) {
        try {
          this.openLocalFiles(
            cliFiles.map((f) => ({
              name: f.name,
              path: f.path,
              size: f.size,
              durationMs: f.duration_ms ?? undefined,
            })),
          );
          this.playbackCommand({ action: "play" });
        } catch {
          // Ignore invalid CLI arguments already logged in Rust
        }
      }
    }
  }

  private serializePersistedState(): string {
    const payload: PersistedState = {
      media: this.media.map(({ stream_url: _stream, ...rest }) => rest),
      notes: this.notes,
      playlists: this.playlists,
      settings: this.settings,
      playback_session: {
        active_media_id: this.snapshot.media_id,
        queue: this.snapshot.queue,
        volume: this.snapshot.volume,
        muted: this.snapshot.muted,
        rate: this.snapshot.rate,
        repeat_mode: this.snapshot.repeat_mode,
        shuffle: this.snapshot.shuffle,
      },
    };
    return JSON.stringify(payload);
  }

  private enqueueNativeStateSave(serialized: string, keepalive = false): void {
    if (
      !this.useStorage ||
      !isBrowserRuntime() ||
      this.nativeHydrationState === "failed" ||
      (window.location.hostname !== "127.0.0.1" &&
        window.location.hostname !== "localhost")
    ) {
      return;
    }
    const capturedEpoch = this.stateEpoch;
    this.pendingNativeSave = serialized;
    if (keepalive) {
      const bodyToSend = this.pendingNativeSave;
      this.pendingNativeSave = null;
      void nativeFetch("/__audora_state", {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: bodyToSend,
        keepalive: true,
      }).catch(() => {});
      return;
    }
    if (this.isSavingNative) {
      return;
    }
    this.isSavingNative = true;
    void (async () => {
      try {
        while (this.pendingNativeSave !== null) {
          const nextBody = this.pendingNativeSave;
          this.pendingNativeSave = null;
          if (capturedEpoch !== this.stateEpoch) {
            break;
          }
          await nativeFetch("/__audora_state", {
            method: "POST",
            headers: { "Content-Type": "application/json" },
            body: nextBody,
          }).catch(() => {});
        }
      } finally {
        this.isSavingNative = false;
      }
    })();
  }

  private persistToStorage(options?: { skipNative?: boolean; debounceNativeMs?: number }): void {
    if (!this.useStorage) {
      return;
    }
    if (this.hasNativeRelationalRpc && options?.skipNative) {
      return;
    }
    const serialized = this.serializePersistedState();
    if (!this.hasNativeRelationalRpc) {
      try {
        if (typeof window !== "undefined" && window.localStorage) {
          window.localStorage.setItem(STORAGE_KEY, serialized);
        }
      } catch {
        // Ignore quota errors in restricted webviews.
      }
    }
    if (options?.skipNative) {
      return;
    }
    if (options?.debounceNativeMs && options.debounceNativeMs > 0) {
      if (this.seekDebounceTimer !== null) {
        clearTimeout(this.seekDebounceTimer);
      }
      this.seekDebounceTimer = setTimeout(() => {
        this.seekDebounceTimer = null;
        this.enqueueNativeStateSave(this.serializePersistedState());
      }, options.debounceNativeMs);
      return;
    }
    this.enqueueNativeStateSave(serialized);
  }

  private buildSessionDto(): PersistedPlaybackSession {
    return {
      active_media_id: this.snapshot.media_id,
      queue: this.snapshot.queue,
      volume: this.snapshot.volume,
      muted: this.snapshot.muted,
      rate: this.snapshot.rate,
      repeat_mode: this.snapshot.repeat_mode,
      shuffle: this.snapshot.shuffle,
    };
  }

  private stripStreamUrl(item: MediaItem): Omit<MediaItem, "stream_url"> {
    const { stream_url: _stream, ...rest } = item;
    return rest;
  }

  private isNativeRelationalRpcActive(): boolean {
    return (
      this.useStorage &&
      this.hasNativeRelationalRpc &&
      isBrowserRuntime() &&
      (window.location.hostname === "127.0.0.1" ||
        window.location.hostname === "localhost")
    );
  }

  private dispatchRelationalRpc(
    opPayload: Record<string, unknown>,
    options?: {
      keepalive?: boolean;
      bestEffort?: boolean;
      onSettled?: (ok: boolean) => void;
    },
  ): boolean {
    if (!this.isNativeRelationalRpcActive()) {
      return false;
    }

    if (options?.keepalive) {
      const keepaliveEpoch = this.stateEpoch;
      void nativeFetch("/__audora_rpc", {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify(opPayload),
        keepalive: true,
      })
        .then((res) => {
          if (keepaliveEpoch === this.stateEpoch) {
            options.onSettled?.(res.ok);
          }
        })
        .catch(() => {
          if (keepaliveEpoch === this.stateEpoch) {
            options.onSettled?.(false);
          }
        });
      return true;
    }

    const currentEpoch = this.stateEpoch;
    // Coalesce consecutive `update_progress` ONLY for the same `media_id` (preserving `session`),
    // or consecutive `save_session` RPCs (`C-8`).
    const opName = opPayload.op;
    const lastIdx = this.rpcQueue.length - 1;
    const prev = lastIdx >= 0 ? this.rpcQueue[lastIdx] : undefined;
    if (
      !options?.onSettled &&
      prev &&
      !prev.onSettled &&
      prev.epoch === currentEpoch &&
      opName === "update_progress" &&
      prev.payload.op === "update_progress" &&
      prev.payload.media_id === opPayload.media_id
    ) {
      this.rpcQueue[lastIdx] = {
        epoch: currentEpoch,
        payload: {
          ...opPayload,
          session: opPayload.session ?? prev.payload.session,
        },
      };
    } else if (
      !options?.onSettled &&
      prev &&
      !prev.onSettled &&
      prev.epoch === currentEpoch &&
      opName === "save_session" &&
      prev.payload.op === "save_session"
    ) {
      this.rpcQueue[lastIdx] = {
        epoch: currentEpoch,
        payload: opPayload,
      };
    } else {
      this.rpcQueue.push({
        epoch: currentEpoch,
        payload: opPayload,
        bestEffort: options?.bestEffort,
        onSettled: options?.onSettled,
      });
    }

    if (!this.isFlushingRpc) {
      this.isFlushingRpc = true;
      this.rpcFlushPromise = (async () => {
        try {
          while (this.rpcQueue.length > 0) {
            const next = this.rpcQueue.shift()!;
            if (next.epoch !== this.stateEpoch) {
              continue;
            }
            this.inFlightRpcEntry = next;
            try {
              const sendOnce = async (): Promise<{ ok: boolean; retryable: boolean }> => {
                if (next.epoch !== this.stateEpoch) {
                  return { ok: false, retryable: false };
                }
                try {
                  const res = await nativeFetch("/__audora_rpc", {
                    method: "POST",
                    headers: { "Content-Type": "application/json" },
                    body: JSON.stringify(next.payload),
                  });
                  if (res.ok) {
                    return { ok: true, retryable: false };
                  }
                  const isClientError = res.status >= 400 && res.status < 500;
                  return { ok: false, retryable: !isClientError };
                } catch {
                  return { ok: false, retryable: true };
                }
              };
              let result = await sendOnce();
              if (
                !result.ok &&
                result.retryable &&
                !next.bestEffort &&
                next.epoch === this.stateEpoch
              ) {
                result = await sendOnce();
              }
              if (this.inFlightRpcEntry === next) {
                this.inFlightRpcEntry = null;
              }
              if (next.epoch === this.stateEpoch) {
                this.settleRpcEntry(next, result.ok);
                if (!result.ok && !next.bestEffort) {
                  this.onPersistenceError?.(
                    this.makeError(
                      "AV-401",
                      "PersistenceFailure",
                      "Database recovery needed",
                      "Audora couldn’t persist your latest change to SQLite. Check available disk space and try again.",
                    ),
                  );
                }
              }
            } finally {
              if (this.inFlightRpcEntry === next) {
                this.inFlightRpcEntry = null;
              }
            }
          }
        } finally {
          this.isFlushingRpc = false;
          this.rpcFlushPromise = null;
        }
      })();
    }
    return true;
  }

  private persistProgressAndSession(options?: { immediate?: boolean; keepalive?: boolean }): void {
    if (!this.useStorage) return;
    const active = this.media.find((m) => m.id === this.snapshot.media_id);
    if (active) {
      if (
        this.dispatchRelationalRpc(
          {
            op: "update_progress",
            media_id: active.id,
            last_position_ms: active.last_position_ms,
            completed: active.completed,
            session: this.buildSessionDto(),
          },
          { keepalive: options?.keepalive },
        )
      ) {
        return;
      }
    } else if (
      this.dispatchRelationalRpc(
        {
          op: "save_session",
          session: this.buildSessionDto(),
        },
        { keepalive: options?.keepalive },
      )
    ) {
      return;
    }
    this.persistToStorage(options?.immediate ? undefined : { debounceNativeMs: 200 });
  }

  private throttledPersistProgress(): void {
    if (!this.useStorage) return;
    const now = Date.now();
    if (now - this.lastProgressPersistMs >= 2_000) {
      this.lastProgressPersistMs = now;
      this.persistProgressAndSession();
    }
  }

  public flushPendingPersistence(): void {
    if (!this.useStorage) return;
    if (this.seekDebounceTimer !== null) {
      clearTimeout(this.seekDebounceTimer);
      this.seekDebounceTimer = null;
    }
    if (this.hasNativeRelationalRpc) {
      const batch = this.collectPendingRpcBatchForExit();
      if (batch.length > 0) {
        this.dispatchRelationalRpc({ op: "batch", ops: batch }, { keepalive: true });
      }
      return;
    }
    const serialized = this.serializePersistedState();
    try {
      if (typeof window !== "undefined" && window.localStorage) {
        window.localStorage.setItem(STORAGE_KEY, serialized);
      }
    } catch {
      // Ignore storage errors on unload
    }
    this.enqueueNativeStateSave(serialized, true);
  }

  public getSettings(): AppSettings {
    return structuredClone(this.settings);
  }

  public updateSettings(next: AppSettings): AppSettings {
    if (
      !Number.isFinite(next.default_playback_speed) ||
      next.default_playback_speed < 0.5 ||
      next.default_playback_speed > 2.0
    ) {
      throw this.makeError(
        "AV-301",
        "ValidationFailed",
        "Invalid playback speed",
        "Speed must be between 0.5x and 2.0x.",
      );
    }
    const prevDefaultSpeed = this.settings.default_playback_speed;
    const deferSettingsSave = this.shouldTrackPreHydrationSettings();
    if (deferSettingsSave) {
      for (const key of Object.keys(next) as Array<keyof AppSettings>) {
        if (JSON.stringify(next[key]) !== JSON.stringify(this.settings[key])) {
          this.preHydrationDirtySettings.add(key);
        }
      }
    }
    this.settings = structuredClone(next);
    if (this.snapshot.media_id === null || this.snapshot.rate === prevDefaultSpeed) {
      this.snapshot.rate = this.settings.default_playback_speed;
    }
    if (!deferSettingsSave) {
      if (!this.dispatchRelationalRpc({ op: "save_settings", settings: this.settings })) {
        this.persistToStorage();
      }
    } else if (!this.hasNativeRelationalRpc) {
      this.persistToStorage({ skipNative: true });
    }
    return structuredClone(this.settings);
  }

  public setWindowMode(mode: WindowMode): AppSettings {
    const deferSettingsSave = this.shouldTrackPreHydrationSettings();
    if (deferSettingsSave && this.settings.window_mode !== mode) {
      this.preHydrationDirtySettings.add("window_mode");
    }
    this.settings.window_mode = mode;
    if (!deferSettingsSave) {
      if (!this.dispatchRelationalRpc({ op: "save_settings", settings: this.settings })) {
        this.persistToStorage();
      }
    } else if (!this.hasNativeRelationalRpc) {
      this.persistToStorage({ skipNative: true });
    }
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
          const active = this.media.find((m) => m.id === this.snapshot.media_id);
          if (!active || active.availability === "missing") {
            throw this.makeError(
              "AV-101",
              "MediaUnavailable",
              "We can’t find this file",
              "It may have been moved, renamed, or stored on a disconnected drive.",
            );
          }
          if (this.snapshot.state === "ended") {
            this.snapshot.position_ms = 0;
            this.snapshot.generation += 1;
          } else if (this.snapshot.error !== null) {
            this.snapshot.generation += 1;
          }
          this.snapshot.error = null;
          this.snapshot.state = "playing";
        }
        break;
      case "pause":
        this.snapshot.state = "paused";
        this.persistProgressAndSession({ immediate: true });
        break;
      case "toggle_play_pause":
        if (this.snapshot.state === "playing") {
          this.snapshot.state = "paused";
          this.persistProgressAndSession({ immediate: true });
        } else if (this.snapshot.media_id) {
          const active = this.media.find((m) => m.id === this.snapshot.media_id);
          if (!active || active.availability === "missing") {
            throw this.makeError(
              "AV-101",
              "MediaUnavailable",
              "We can’t find this file",
              "It may have been moved, renamed, or stored on a disconnected drive.",
            );
          }
          if (this.snapshot.state === "ended") {
            this.snapshot.position_ms = 0;
            this.snapshot.generation += 1;
          } else if (this.snapshot.error !== null) {
            this.snapshot.generation += 1;
          }
          this.snapshot.error = null;
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
        this.persistProgressAndSession();
        break;
      }
      case "skip_relative": {
        const nextPos = Math.max(0, this.snapshot.position_ms + cmd.delta_ms);
        const max = this.snapshot.duration_ms ?? nextPos;
        this.snapshot.position_ms = Math.min(nextPos, max);
        const active = this.media.find((m) => m.id === this.snapshot.media_id);
        if (active) {
          active.last_position_ms = this.snapshot.position_ms;
          if (
            cmd.delta_ms < 0 ||
            this.snapshot.duration_ms === null ||
            this.snapshot.position_ms < this.snapshot.duration_ms
          ) {
            active.completed = false;
          }
        }
        if (
          this.snapshot.state === "ended" &&
          (this.snapshot.duration_ms === null ||
            this.snapshot.position_ms < this.snapshot.duration_ms)
        ) {
          this.snapshot.state = "paused";
        }
        this.persistProgressAndSession();
        break;
      }
      case "set_rate":
        this.snapshot.rate = Number.isFinite(cmd.rate)
          ? Math.max(0.5, Math.min(2.0, cmd.rate))
          : 1.0;
        this.persistProgressAndSession();
        break;
      case "set_volume":
        this.snapshot.volume = Math.max(0, Math.min(1, cmd.volume));
        this.persistProgressAndSession();
        break;
      case "set_muted":
        this.snapshot.muted = cmd.muted;
        this.persistProgressAndSession();
        break;
      case "set_repeat":
        this.snapshot.repeat_mode = cmd.mode;
        this.persistProgressAndSession();
        break;
      case "set_shuffle":
        this.snapshot.shuffle = cmd.shuffle;
        if (cmd.shuffle) {
          this.unshuffledQueue = [...this.snapshot.queue];
          this.rebuildShuffleOrder(this.snapshot.media_id);
          this.snapshot.queue = [...this.shuffleOrder];
        } else {
          if (this.unshuffledQueue) {
            const currentQueueSet = new Set(this.snapshot.queue);
            const restored = this.unshuffledQueue.filter((id) => currentQueueSet.has(id));
            for (const id of this.snapshot.queue) {
              if (!restored.includes(id)) {
                restored.push(id);
              }
            }
            this.snapshot.queue = restored;
            this.unshuffledQueue = null;
          }
          this.shuffleOrder = [];
        }
        this.persistProgressAndSession();
        break;
      case "next": {
        const wasPlaying = this.snapshot.state === "playing";
        this.advanceQueue(wasPlaying);
        break;
      }
      case "previous": {
        if (this.snapshot.position_ms > 3000) {
          this.snapshot.position_ms = 0;
          const active = this.media.find((m) => m.id === this.snapshot.media_id);
          if (active) {
            active.last_position_ms = 0;
            active.completed = false;
          }
          if (this.snapshot.state === "ended") {
            this.snapshot.state = "paused";
          }
          this.persistProgressAndSession();
        } else if (this.snapshot.media_id) {
          const wasPlaying = this.snapshot.state === "playing";
          const prevCandidate = this.findPreviousPlayableQueueItem();
          if (prevCandidate) {
            this.selectMediaById(prevCandidate.id, {
              autoPlay: wasPlaying,
              startFromBeginning: true,
              preserveShuffleOrder: true,
            });
          } else {
            // Restart Track 1 from `0ms` even under 3s (`M-5` parity with `audora-core`).
            this.snapshot.position_ms = 0;
            const active = this.media.find((m) => m.id === this.snapshot.media_id);
            if (active) {
              active.last_position_ms = 0;
              active.completed = false;
            }
            if (this.snapshot.state === "ended") {
              this.snapshot.state = "paused";
            }
            this.persistProgressAndSession();
          }
        }
        break;
      }
      case "stop":
        this.snapshot.state = "idle";
        this.snapshot.position_ms = 0;
        this.persistProgressAndSession({ immediate: true });
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
      return this.handleMediaEnded(this.snapshot.media_id, this.snapshot.generation);
    }

    this.snapshot.position_ms = nextPos;
    const active = this.media.find((m) => m.id === this.snapshot.media_id);
    if (active) {
      active.last_position_ms = nextPos;
    }
    this.throttledPersistProgress();
    return structuredClone(this.snapshot);
  }

  /**
   * Syncs position and optional probed duration from an active HTML5 `<video>` or `<audio>` element.
   * Ignores stale events emitted by a prior track or load generation.
   */
  public syncMediaElementPosition(
    positionMs: number,
    durationMs?: number,
    expectedMediaId?: string,
    expectedGeneration?: number,
  ): PlaybackSnapshot {
    if (
      (expectedMediaId !== undefined && this.snapshot.media_id !== expectedMediaId) ||
      (expectedGeneration !== undefined && this.snapshot.generation !== expectedGeneration)
    ) {
      return structuredClone(this.snapshot);
    }
    const active = this.media.find((m) => m.id === this.snapshot.media_id);
    const activeExt = active?.path
      ? (active.path.split(".").pop()?.toLowerCase() ?? "")
      : "";
    const isIsoBmff = ISO_BMFF_EXTENSIONS.has(activeExt);
    const hasAuthoritativeDuration = Boolean(
      active?.duration_probed && active.duration_ms && active.duration_ms > 0,
    );
    if (!hasAuthoritativeDuration && durationMs && Number.isFinite(durationMs) && durationMs > 0) {
      const roundedDur = Math.round(durationMs);
      const changed = this.snapshot.duration_ms !== roundedDur;
      this.snapshot.duration_ms = roundedDur;
      if (active && (active.duration_ms !== roundedDur || !active.duration_probed)) {
        active.duration_ms = roundedDur;
        active.duration_probed = true;
        if (
          changed &&
          !this.dispatchRelationalRpc({
            op: "upsert_media_batch",
            items: [this.stripStreamUrl(active)],
          })
        ) {
          this.persistToStorage({ debounceNativeMs: 400 });
        }
      }
    } else if (
      !isIsoBmff &&
      durationMs &&
      Number.isFinite(durationMs) &&
      durationMs > 0 &&
      this.snapshot.duration_ms !== null &&
      Math.round(durationMs) > this.snapshot.duration_ms
    ) {
      // Allow real decoded VBR MP3/AAC duration to expand if CBR/ADTS estimate was slightly lower (`M-6`).
      const expanded = Math.round(durationMs);
      this.snapshot.duration_ms = expanded;
      if (active) {
        active.duration_ms = expanded;
      }
    }
    const effectiveDuration = this.snapshot.duration_ms;
    const roundedPos = Math.max(0, Math.round(positionMs));
    if (
      isIsoBmff &&
      hasAuthoritativeDuration &&
      effectiveDuration !== null &&
      effectiveDuration > 0 &&
      roundedPos >= effectiveDuration &&
      this.snapshot.state === "playing"
    ) {
      return this.handleMediaEnded(this.snapshot.media_id ?? undefined, this.snapshot.generation);
    }
    this.snapshot.position_ms =
      effectiveDuration !== null && effectiveDuration > 0
        ? Math.min(roundedPos, effectiveDuration)
        : roundedPos;
    if (active) {
      active.last_position_ms = this.snapshot.position_ms;
    }
    this.throttledPersistProgress();
    return structuredClone(this.snapshot);
  }

  /**
   * Handles end-of-stream (`Ended`), marking the media item complete and honoring `Repeat` and `Shuffle`.
   * Ignores duplicate or stale end notifications from an older media item or load generation.
   */
  public handleMediaEnded(
    expectedMediaId?: string,
    expectedGeneration?: number,
  ): PlaybackSnapshot {
    if (
      !this.snapshot.media_id ||
      this.snapshot.state === "ended" ||
      (expectedMediaId !== undefined && this.snapshot.media_id !== expectedMediaId) ||
      (expectedGeneration !== undefined && this.snapshot.generation !== expectedGeneration)
    ) {
      return structuredClone(this.snapshot);
    }

    const active = this.media.find((m) => m.id === this.snapshot.media_id);
    if (active) {
      active.completed = true;
      active.last_position_ms = this.snapshot.duration_ms ?? this.snapshot.position_ms;
    }

    if (this.snapshot.repeat_mode === "one") {
      this.snapshot.generation += 1;
      this.snapshot.position_ms = 0;
      this.snapshot.state = "playing";
      if (active) {
        active.completed = false;
        active.last_position_ms = 0;
      }
      this.persistProgressAndSession({ immediate: true });
      return structuredClone(this.snapshot);
    }

    if (active) {
      this.dispatchRelationalRpc({
        op: "update_progress",
        media_id: active.id,
        last_position_ms: active.last_position_ms,
        completed: true,
      });
    }

    const advanced = this.advanceQueue(true);
    if (!advanced) {
      if (this.snapshot.duration_ms !== null) {
        this.snapshot.position_ms = this.snapshot.duration_ms;
      }
      this.snapshot.state = "ended";
      if (!this.dispatchRelationalRpc({ op: "save_session", session: this.buildSessionDto() })) {
        this.persistToStorage();
      }
    } else if (!this.hasNativeRelationalRpc) {
      this.persistToStorage();
    }
    return structuredClone(this.snapshot);
  }

  private isQueueCandidatePlayable(candidateId: string | undefined): MediaItem | null {
    if (!candidateId) return null;
    const candidate = this.media.find((m) => m.id === candidateId);
    if (!candidate || candidate.availability !== "available") {
      return null;
    }
    if (candidate.source_folder_id) {
      const disabledFolders = this.disabledSourceFolderIds();
      if (disabledFolders.has(candidate.source_folder_id)) {
        return null;
      }
    }
    return candidate;
  }

  private rebuildShuffleOrder(anchorMediaId?: string | null): void {
    const queue = this.snapshot.queue;
    if (queue.length === 0) {
      this.shuffleOrder = [];
      return;
    }
    if (anchorMediaId && queue.includes(anchorMediaId)) {
      const rest = queue.filter((id) => id !== anchorMediaId);
      this.shuffleOrder = [anchorMediaId, ...shufflePermutation(rest)];
    } else {
      this.shuffleOrder = shufflePermutation(queue);
    }
  }

  private syncShuffleOrderWithQueue(): void {
    const queue = this.snapshot.queue;
    if (queue.length === 0) {
      this.shuffleOrder = [];
      return;
    }
    if (this.shuffleOrder.length === 0) {
      this.rebuildShuffleOrder(this.snapshot.media_id);
      return;
    }
    const queueSet = new Set(queue);
    const filtered = this.shuffleOrder.filter((id) => queueSet.has(id));
    const filteredSet = new Set(filtered);
    const added = queue.filter((id) => !filteredSet.has(id));
    this.shuffleOrder =
      added.length > 0 ? [...filtered, ...shufflePermutation(added)] : filtered;
  }

  private findPreviousPlayableQueueItem(): MediaItem | null {
    if (!this.snapshot.media_id) return null;
    if (this.snapshot.shuffle) {
      this.syncShuffleOrderWithQueue();
      const sLen = this.shuffleOrder.length;
      if (sLen === 0) return null;
      const sIdx = this.shuffleOrder.indexOf(this.snapshot.media_id);
      if (sIdx < 0) return null;
      for (let i = sIdx - 1; i >= 0; i -= 1) {
        const found = this.isQueueCandidatePlayable(this.shuffleOrder[i]);
        if (found) return found;
      }
      if (this.snapshot.repeat_mode === "all") {
        for (let i = sLen - 1; i >= sIdx; i -= 1) {
          const found = this.isQueueCandidatePlayable(this.shuffleOrder[i]);
          if (found) return found;
        }
      }
      return null;
    }

    const len = this.snapshot.queue.length;
    if (len === 0) return null;
    const idx = this.snapshot.queue.indexOf(this.snapshot.media_id);
    if (idx < 0) return null;

    for (let i = idx - 1; i >= 0; i -= 1) {
      const found = this.isQueueCandidatePlayable(this.snapshot.queue[i]);
      if (found) return found;
    }
    if (this.snapshot.repeat_mode === "all") {
      for (let i = len - 1; i >= idx; i -= 1) {
        const found = this.isQueueCandidatePlayable(this.snapshot.queue[i]);
        if (found) return found;
      }
    }
    return null;
  }

  private advanceQueue(autoPlay: boolean): boolean {
    if (!this.snapshot.media_id) return false;
    const len = this.snapshot.queue.length;
    if (len === 0) return false;

    if (this.snapshot.shuffle) {
      this.syncShuffleOrderWithQueue();
      const sLen = this.shuffleOrder.length;
      const sIdx = this.shuffleOrder.indexOf(this.snapshot.media_id);
      if (sIdx < 0) return false;

      for (let i = sIdx + 1; i < sLen; i += 1) {
        const candidate = this.isQueueCandidatePlayable(this.shuffleOrder[i]);
        if (candidate) {
          this.selectMediaById(candidate.id, {
            autoPlay,
            startFromBeginning: true,
            preserveShuffleOrder: true,
          });
          return true;
        }
      }

      if (this.snapshot.repeat_mode === "all") {
        const currentId = this.snapshot.media_id;
        const nextPass = shufflePermutation(this.snapshot.queue, currentId);
        this.shuffleOrder = nextPass;
        this.snapshot.queue = [...nextPass];
        for (let i = 0; i < nextPass.length; i += 1) {
          const candidate = this.isQueueCandidatePlayable(nextPass[i]);
          if (candidate) {
            this.selectMediaById(candidate.id, {
              autoPlay,
              startFromBeginning: true,
              preserveShuffleOrder: true,
            });
            return true;
          }
        }
      }
      return false;
    }

    const idx = this.snapshot.queue.indexOf(this.snapshot.media_id);
    if (idx < 0) return false;

    if (this.snapshot.repeat_mode === "all") {
      for (let offset = 1; offset <= len; offset += 1) {
        const candidateIdx = (idx + offset) % len;
        const candidate = this.isQueueCandidatePlayable(this.snapshot.queue[candidateIdx]);
        if (candidate) {
          this.selectMediaById(candidate.id, { autoPlay, startFromBeginning: true });
          return true;
        }
      }
      return false;
    }

    for (let candidateIdx = idx + 1; candidateIdx < len; candidateIdx += 1) {
      const candidate = this.isQueueCandidatePlayable(this.snapshot.queue[candidateIdx]);
      if (candidate) {
        this.selectMediaById(candidate.id, { autoPlay, startFromBeginning: true });
        return true;
      }
    }
    return false;
  }

  public selectMediaById(
    mediaId: string,
    options?: {
      autoPlay?: boolean;
      startFromBeginning?: boolean;
      preserveShuffleOrder?: boolean;
    },
  ): MediaItem {
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
    if (!this.isBareFileName(item.path)) {
      const freshUrl = this.computeLocalStreamUrl(item.path);
      if (freshUrl) {
        if (item.stream_url && item.stream_url !== freshUrl) {
          revokeIfBlobUrl(item.stream_url);
        }
        item.stream_url = freshUrl;
      }
    }
    const hadActiveMedia = this.snapshot.media_id !== null;
    this.snapshot.generation += 1;
    this.snapshot.media_id = item.id;
    this.snapshot.duration_ms = item.duration_ms;
    const nearEnd =
      item.duration_ms !== null &&
      item.duration_ms > 500 &&
      item.last_position_ms >= item.duration_ms - 500;
    this.snapshot.position_ms =
      !options?.startFromBeginning &&
      this.settings.resume_where_left_off &&
      !item.completed &&
      !nearEnd
        ? item.last_position_ms
        : 0;
    if (!hadActiveMedia || !Number.isFinite(this.snapshot.rate) || this.snapshot.rate < 0.5 || this.snapshot.rate > 2.0) {
      this.snapshot.rate = this.settings.default_playback_speed;
    }
    this.snapshot.state = options?.autoPlay ? "playing" : "paused";
    this.snapshot.error = null;
    const nowIso = new Date().toISOString();
    item.last_opened_at = nowIso;
    item.last_seen_at = nowIso;
    if (!this.snapshot.queue.includes(item.id)) {
      this.snapshot.queue.push(item.id);
    }
    if (this.snapshot.shuffle && !options?.preserveShuffleOrder) {
      this.rebuildShuffleOrder(item.id);
      this.snapshot.queue = [...this.shuffleOrder];
    }
    if (
      !this.dispatchRelationalRpc({
        op: "upsert_media_batch",
        items: [this.stripStreamUrl(item)],
      })
    ) {
      this.persistToStorage();
    } else {
      this.dispatchRelationalRpc({ op: "save_session", session: this.buildSessionDto() });
    }
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
      "smb://",
      "dav://",
      "davs://",
      "nfs://",
      "afp://",
      "file://",
    ]) {
      if (lower.startsWith(scheme)) {
        throw this.makeError(
          "AV-301",
          "ValidationFailed",
          "Remote URLs are not permitted",
          "Audora only opens local files on your device; remote media URLs and network mounts are not permitted.",
        );
      }
    }
    if (
      (trimmed.startsWith("\\\\") || trimmed.startsWith("//")) &&
      (lower.includes("@") || lower.includes("davwwwroot"))
    ) {
      throw this.makeError(
        "AV-301",
        "ValidationFailed",
        "Remote URLs are not permitted",
        "Audora only opens local files on your device; WebDAV network paths are not permitted.",
      );
    }
    return trimmed;
  }

  public openMediaPath(
    rawPath: string,
    options?: {
      streamUrl?: string;
      size?: number;
      durationMs?: number;
      autoPlay?: boolean;
    },
  ): MediaItem {
    const trimmed = this.validateLocalMediaPath(rawPath);
    const fileName = trimmed.split(/[\\/]/).pop() ?? trimmed;
    const ext = fileName.includes(".") ? fileName.split(".").pop()?.toLowerCase() ?? "" : "";

    if (!SUPPORTED_MEDIA_EXT_SET.has(ext)) {
      const payload = Object.assign(
        this.makeError(
          "AV-104",
          "MediaUnsupported",
          "This format isn’t supported",
          `Audora couldn’t decode ${fileName}. The file uses a codec that isn’t available on this system.`,
        ),
        { firstFileName: fileName },
      );
      if (this.snapshot.media_id === null) {
        this.snapshot.state = "failed";
        this.snapshot.error = payload;
      }
      throw payload;
    }

    this.unshuffledQueue = [];

    const nowIso = new Date().toISOString();
    const streamUrl = this.computeLocalStreamUrl(trimmed, options?.streamUrl);
    const probedDur =
      options?.durationMs && options.durationMs > 0
        ? Math.round(options.durationMs)
        : null;
    let item = this.findMatchingMediaItem(trimmed, fileName, options?.size);
    if (!item) {
      if (options?.streamUrl && options.streamUrl !== streamUrl) {
        revokeIfBlobUrl(options.streamUrl);
      }
      item = {
        id: `med_${Date.now()}_${Math.random().toString(36).slice(2, 6)}`,
        path: trimmed,
        display_name: fileName,
        media_kind: SUPPORTED_VIDEO_EXTS.includes(ext) ? "video" : "audio",
        duration_ms: probedDur,
        duration_probed: probedDur !== null,
        last_position_ms: 0,
        completed: false,
        file_size_bytes: options?.size ?? null,
        fingerprint: null,
        availability: "available",
        added_at: nowIso,
        last_opened_at: nowIso,
        last_seen_at: nowIso,
        note_count: 0,
        stream_url: streamUrl,
      };
      this.media.unshift(item);
    } else {
      const resolvedPath =
        !this.isBareFileName(trimmed) && this.isBareFileName(item.path) ? trimmed : item.path;
      const nextUrl = !this.isBareFileName(resolvedPath)
        ? (this.computeLocalStreamUrl(resolvedPath) ??
            options?.streamUrl ??
            item.stream_url)
        : (streamUrl ?? item.stream_url);
      if (item.stream_url && item.stream_url !== nextUrl) {
        revokeIfBlobUrl(item.stream_url);
      }
      if (options?.streamUrl && options.streamUrl !== nextUrl) {
        revokeIfBlobUrl(options.streamUrl);
      }
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
      item.last_seen_at = nowIso;
      item.stream_url = nextUrl;
    }
    if (
      !this.dispatchRelationalRpc({
        op: "upsert_media_batch",
        items: [this.stripStreamUrl(item)],
      })
    ) {
      this.persistToStorage();
    }
    return this.selectMediaById(item.id, { autoPlay: options?.autoPlay });
  }

  /**
   * Opens one or more native File objects (from `<input type="file">` or window drag-and-drop),
   * adds them all to the session queue, and selects the first item for playback.
   */
  public openLocalFiles(
    files: LocalFileDescriptor[],
    options?: { autoPlay?: boolean },
  ): MediaItem[] {
    if (files.length === 0) {
      throw this.makeError("AV-301", "ValidationFailed", "No files selected", "Select at least one media file.");
    }
    // Pre-validate every path & extension before mutating `this.media` (`M-6`).
    for (const f of files) {
      const rawCandidate = f.path && f.path.trim() ? f.path.trim() : f.name;
      const validated = this.validateLocalMediaPath(rawCandidate);
      const fileName = validated.split(/[\\/]/).pop() ?? validated;
      const ext = fileName.includes(".")
        ? (fileName.split(".").pop()?.toLowerCase() ?? "")
        : "";
      if (!SUPPORTED_MEDIA_EXT_SET.has(ext)) {
        const payload = Object.assign(
          this.makeError(
            "AV-104",
            "MediaUnsupported",
            "This format isn’t supported",
            `Audora couldn’t decode ${fileName}. The file uses a codec that isn’t available on this system.`,
          ),
          { firstFileName: fileName },
        );
        if (this.snapshot.media_id === null) {
          this.snapshot.state = "failed";
          this.snapshot.error = payload;
        }
        throw payload;
      }
    }
    this.unshuffledQueue = [];
    const opened: MediaItem[] = [];
    for (const f of files) {
      const localPath = f.path && f.path.trim() ? f.path.trim() : f.name;
      const item = this.openMediaPath(localPath, {
        streamUrl: f.streamUrl,
        size: f.size,
        durationMs: f.durationMs,
        autoPlay: files.length === 1 ? options?.autoPlay : undefined,
      });
      opened.push(item);
    }
    if (opened.length > 1) {
      this.selectMediaById(opened[0]!.id, { autoPlay: options?.autoPlay });
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
      if (this.snapshot.shuffle && !this.shuffleOrder.includes(mediaId)) {
        this.shuffleOrder.push(mediaId);
      }
      if (this.unshuffledQueue && !this.unshuffledQueue.includes(mediaId)) {
        this.unshuffledQueue.push(mediaId);
      }
      if (!this.dispatchRelationalRpc({ op: "save_session", session: this.buildSessionDto() })) {
        this.persistToStorage({ debounceNativeMs: 200 });
      }
    }
    return structuredClone(this.snapshot);
  }

  public removeFromQueue(mediaId: string): PlaybackSnapshot {
    const currentIdx = this.snapshot.queue.indexOf(mediaId);
    const wasPlaying = this.snapshot.state === "playing";
    this.snapshot.queue = this.snapshot.queue.filter((id) => id !== mediaId);
    this.shuffleOrder = this.shuffleOrder.filter((id) => id !== mediaId);
    if (this.unshuffledQueue) {
      this.unshuffledQueue = this.unshuffledQueue.filter((id) => id !== mediaId);
    }
    let selectedNext = false;
    if (this.snapshot.media_id === mediaId) {
      if (this.snapshot.queue.length === 0) {
        this.transitionSnapshotToIdle([]);
      } else {
        const startIdx = Math.min(Math.max(0, currentIdx), this.snapshot.queue.length - 1);
        let nextPlayable: MediaItem | null = null;
        for (let offset = 0; offset < this.snapshot.queue.length; offset += 1) {
          const idx = (startIdx + offset) % this.snapshot.queue.length;
          const candidate = this.isQueueCandidatePlayable(this.snapshot.queue[idx]);
          if (candidate) {
            nextPlayable = candidate;
            break;
          }
        }
        if (nextPlayable) {
          this.selectMediaById(nextPlayable.id, {
            autoPlay: wasPlaying,
            startFromBeginning: true,
            preserveShuffleOrder: true,
          });
          selectedNext = true;
        } else {
          this.transitionSnapshotToIdle(this.snapshot.queue);
        }
      }
    }
    if (
      !selectedNext &&
      !this.dispatchRelationalRpc({ op: "save_session", session: this.buildSessionDto() })
    ) {
      this.persistToStorage({ debounceNativeMs: 200 });
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
    if (
      !this.dispatchRelationalRpc({
        op: "upsert_media_batch",
        items: [this.stripStreamUrl(item)],
      })
    ) {
      this.persistToStorage();
    }
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
    const visibleById = new Map<string, MediaItem>();
    for (const m of visibleMedia) {
      visibleById.set(m.id, m);
    }

    const notesByMedia = new Map<string, Note[]>();
    for (const n of this.notes) {
      let arr = notesByMedia.get(n.media_id);
      if (!arr) {
        arr = [];
        notesByMedia.set(n.media_id, arr);
      }
      arr.push(n);
    }

    const mediaItems = visibleMedia
      .map((m) => ({
        ...m,
        note_count: notesByMedia.get(m.id)?.length ?? 0,
      }))
      .filter((m) => {
        if (filter !== "all" && m.media_kind !== filter) {
          return false;
        }
        if (!q) {
          return true;
        }
        if (m.display_name.toLowerCase().includes(q)) {
          return true;
        }
        const mediaNotes = notesByMedia.get(m.id);
        return mediaNotes ? mediaNotes.some((n) => n.body.toLowerCase().includes(q)) : false;
      });

    const matchingNotes = q
      ? this.notes
          .filter((n) => {
            const parent = visibleById.get(n.media_id);
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

  private prepareCreatedNote(input: CreateNoteInput): Note {
    if (!Number.isFinite(input.position_ms) || input.position_ms < 0) {
      throw this.makeError(
        "AV-301",
        "ValidationFailed",
        "Invalid timestamp",
        "Note timestamp must be a non-negative millisecond value.",
      );
    }
    const trimmed = input.body.trim();
    if (!trimmed && !input.is_quick_marker) {
      throw this.makeError(
        "AV-301",
        "ValidationFailed",
        "Note text required",
        "Enter note text or check 'Save as quick marker' before saving.",
      );
    }
    if (trimmed.length > MAX_NOTE_BODY_CHARS) {
      throw this.makeError(
        "AV-301",
        "ValidationFailed",
        "Note text too long",
        `Note text must not exceed ${MAX_NOTE_BODY_CHARS.toLocaleString()} characters.`,
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

    const nowIso = new Date().toISOString();
    return {
      id: `note_${Date.now()}_${Math.random().toString(36).slice(2, 6)}`,
      media_id: input.media_id,
      position_ms: clamped,
      is_quick_marker: !trimmed && input.is_quick_marker,
      body: trimmed,
      created_at: nowIso,
      updated_at: nowIso,
    };
  }

  private prepareUpdatedNote(input: UpdateNoteInput): { target: Note; preview: Note } {
    const trimmed = input.body.trim();
    if (!trimmed) {
      throw this.makeError(
        "AV-301",
        "ValidationFailed",
        "Note text required",
        "Note text cannot be empty.",
      );
    }
    if (trimmed.length > MAX_NOTE_BODY_CHARS) {
      throw this.makeError(
        "AV-301",
        "ValidationFailed",
        "Note text too long",
        `Note text must not exceed ${MAX_NOTE_BODY_CHARS.toLocaleString()} characters.`,
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
    return {
      target: note,
      preview: {
        ...note,
        body: trimmed,
        is_quick_marker: false,
        updated_at: new Date().toISOString(),
      },
    };
  }

  public createNote(input: CreateNoteInput): Note {
    const note = this.prepareCreatedNote(input);
    this.notes.push(note);
    if (!this.dispatchRelationalRpc({ op: "upsert_note", note })) {
      this.persistToStorage();
    }
    return structuredClone(note);
  }

  public updateNote(input: UpdateNoteInput): Note {
    const { target, preview } = this.prepareUpdatedNote(input);
    target.body = preview.body;
    target.is_quick_marker = preview.is_quick_marker;
    target.updated_at = preview.updated_at;
    if (!this.dispatchRelationalRpc({ op: "upsert_note", note: target })) {
      this.persistToStorage();
    }
    return structuredClone(target);
  }

  public async saveNoteVerified(
    input: ({ mode: "create" } & CreateNoteInput) | ({ mode: "update" } & UpdateNoteInput),
  ): Promise<Note> {
    const saveEpoch = this.stateEpoch;
    if (input.mode === "create") {
      const candidate = this.prepareCreatedNote(input);
      if (this.isNativeRelationalRpcActive()) {
        const ok = await new Promise<boolean>((resolve) => {
          this.dispatchRelationalRpc(
            { op: "upsert_note", note: candidate },
            { onSettled: resolve },
          );
        });
        if (!ok || saveEpoch !== this.stateEpoch) {
          throw this.makeError(
            "AV-401",
            "PersistenceFailure",
            "Database recovery needed",
            "Audora couldn’t save this note to SQLite. Your draft text is still here — check disk space and try again.",
          );
        }
        this.notes.push(candidate);
        return structuredClone(candidate);
      }
      this.notes.push(candidate);
      this.persistToStorage();
      return structuredClone(candidate);
    }

    const { target, preview } = this.prepareUpdatedNote(input);
    if (this.isNativeRelationalRpcActive()) {
      const ok = await new Promise<boolean>((resolve) => {
        this.dispatchRelationalRpc(
          { op: "upsert_note", note: preview },
          { onSettled: resolve },
        );
      });
      if (!ok || saveEpoch !== this.stateEpoch) {
        throw this.makeError(
          "AV-401",
          "PersistenceFailure",
          "Database recovery needed",
          "Audora couldn’t update this note in SQLite. Your draft text is still here — check disk space and try again.",
        );
      }
      target.body = preview.body;
      target.is_quick_marker = preview.is_quick_marker;
      target.updated_at = preview.updated_at;
      return structuredClone(target);
    }
    target.body = preview.body;
    target.is_quick_marker = preview.is_quick_marker;
    target.updated_at = preview.updated_at;
    this.persistToStorage();
    return structuredClone(target);
  }

  public deleteNote(noteId: string): Note | null {
    const idx = this.notes.findIndex((n) => n.id === noteId);
    if (idx < 0) return null;
    const [removed] = this.notes.splice(idx, 1);
    if (!this.dispatchRelationalRpc({ op: "delete_note", id: noteId })) {
      this.persistToStorage();
    }
    return structuredClone(removed);
  }

  public restoreNote(note: Note): Note {
    if (!this.media.some((m) => m.id === note.media_id)) {
      throw this.makeError(
        "AV-301",
        "ValidationFailed",
        "Media not found",
        "The media item for this note is no longer in your library.",
      );
    }
    const copy = structuredClone(note);
    this.notes.push(copy);
    if (!this.dispatchRelationalRpc({ op: "upsert_note", note: copy })) {
      this.persistToStorage();
    }
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
  public async pickSingleMediaFileForRelink(): Promise<{
    supported: boolean;
    unavailable?: boolean;
    cancelled: boolean;
    file: { path: string; name: string; size: number; durationMs?: number } | null;
  }> {
    const pickEpoch = this.stateEpoch;
    await this.ensureHydratedFromNative();
    if (pickEpoch !== this.stateEpoch) {
      return { supported: true, cancelled: true, file: null };
    }
    if (
      !isBrowserRuntime() ||
      (window.location.hostname !== "127.0.0.1" &&
        window.location.hostname !== "localhost")
    ) {
      return { supported: false, cancelled: false, file: null };
    }
    try {
      const res = await nativeFetch("/__audora_pick_single_file", { method: "POST" });
      if (pickEpoch !== this.stateEpoch) {
        return { supported: true, cancelled: true, file: null };
      }
      if (!res.ok) {
        if (res.status === 409) {
          return { supported: true, cancelled: true, file: null };
        }
        return { supported: false, unavailable: true, cancelled: false, file: null };
      }
      const text = await res.text();
      if (!text.trim().startsWith("{")) {
        return { supported: false, cancelled: false, file: null };
      }
      const payload = JSON.parse(text) as {
        ok?: boolean;
        cancelled?: boolean;
        unavailable?: boolean;
        status?: string;
        file?: {
          path: string;
          name: string;
          size: number;
          duration_ms?: number | null;
        } | null;
      };
      if (payload.unavailable || payload.status === "unavailable") {
        return { supported: false, unavailable: true, cancelled: false, file: null };
      }
      if (!payload.ok) {
        if (payload.status === "busy") {
          return { supported: true, cancelled: true, file: null };
        }
        return { supported: false, cancelled: false, file: null };
      }
      if (payload.cancelled || !payload.file) {
        return { supported: true, cancelled: true, file: null };
      }
      return {
        supported: true,
        cancelled: false,
        file: {
          path: payload.file.path,
          name: payload.file.name,
          size: payload.file.size,
          durationMs: payload.file.duration_ms ?? undefined,
        },
      };
    } catch {
      return { supported: false, cancelled: false, file: null };
    }
  }

  public async probeNativeMediaDuration(
    rawPath: string,
  ): Promise<{ exists: boolean; durationMs?: number } | null> {
    if (
      !isBrowserRuntime() ||
      (window.location.hostname !== "127.0.0.1" &&
        window.location.hostname !== "localhost")
    ) {
      return null;
    }
    try {
      const res = await nativeFetch(
        `/__audora_probe_media?path=${encodeURIComponent(rawPath.trim())}`,
      );
      if (res.status === 404) {
        return { exists: false };
      }
      if (!res.ok) {
        return null;
      }
      const text = await res.text();
      if (!text.trim().startsWith("{")) return null;
      const parsed = JSON.parse(text) as { ok?: boolean; duration_ms?: number | null };
      if (!parsed.ok) return { exists: false };
      return {
        exists: true,
        durationMs:
          parsed.duration_ms && parsed.duration_ms > 0
            ? Math.round(parsed.duration_ms)
            : undefined,
      };
    } catch {
      return null;
    }
  }

  public relinkMedia(
    mediaId: string,
    newPath: string,
    options?: {
      confirmMismatch?: boolean;
      probedDurationMs?: number;
      fileSizeBytes?: number;
    },
  ): MediaItem {
    const trimmed = this.validateLocalMediaPath(newPath);
    const normTrimmed = this.normalizeComparablePath(trimmed);
    const conflict = this.media.find(
      (m) => this.normalizeComparablePath(m.path) === normTrimmed && m.id !== mediaId,
    );
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
    if (!SUPPORTED_MEDIA_EXT_SET.has(ext)) {
      throw this.makeError(
        "AV-104",
        "MediaUnsupported",
        "This format isn’t supported",
        `Audora couldn’t decode ${fileName}. Choose a supported audio or video file.`,
      );
    }
    if (options?.fileSizeBytes === 0) {
      throw this.makeError(
        "AV-103",
        "MediaCorrupt",
        "Empty file",
        `${fileName} is empty (0 bytes) and cannot be played.`,
      );
    }

    const inferredKind = SUPPORTED_VIDEO_EXTS.includes(ext) ? "video" : "audio";
    const inferredDurationMs: number | null =
      options?.probedDurationMs !== undefined
        ? Math.round(options.probedDurationMs)
        : !this.useStorage
          ? /short|cut|trim|edit/i.test(fileName) && target.duration_ms
            ? Math.max(60_000, Math.round(target.duration_ms * 0.5))
            : target.duration_ms
          : null;

    const kindChanged = inferredKind !== target.media_kind;
    const durationChanged =
      target.duration_ms !== null &&
      inferredDurationMs !== null &&
      Math.abs(target.duration_ms - inferredDurationMs) > 500;

    if ((kindChanged || durationChanged) && !options?.confirmMismatch) {
      const diffParts: string[] = [];
      if (kindChanged) {
        diffParts.push(`media type (${target.media_kind} → ${inferredKind})`);
      }
      if (durationChanged) {
        diffParts.push(
          `duration (${formatTimestamp(target.duration_ms)} → ${formatTimestamp(inferredDurationMs)})`,
        );
      }
      throw this.makeError(
        "AV-202",
        "RelinkMismatch",
        "Confirm file relink",
        `The selected file (${fileName}) differs from the original record in ${diffParts.join(" and ")}. Existing notes will be kept. Confirm to relink anyway.`,
      );
    }

    revokeIfBlobUrl(target.stream_url);
    target.path = trimmed;
    target.display_name = fileName;
    target.media_kind = inferredKind;
    if (options?.fileSizeBytes !== undefined) {
      target.file_size_bytes = options.fileSizeBytes;
    } else if (this.useStorage) {
      target.file_size_bytes = null;
    }

    if (options?.probedDurationMs !== undefined) {
      target.duration_ms = Math.round(options.probedDurationMs);
      target.duration_probed = true;
    } else if (this.useStorage) {
      target.duration_ms = null;
      target.duration_probed = false;
    } else {
      target.duration_ms = inferredDurationMs;
    }
    if (target.duration_ms !== null && target.last_position_ms > target.duration_ms) {
      target.last_position_ms = target.duration_ms;
    }

    target.stream_url = this.computeLocalStreamUrl(trimmed);
    target.availability = "available";
    target.last_seen_at = new Date().toISOString();
    if (this.snapshot.media_id === target.id) {
      this.snapshot.generation += 1;
      this.snapshot.duration_ms = target.duration_ms;
      if (target.duration_ms !== null && this.snapshot.position_ms > target.duration_ms) {
        this.snapshot.position_ms = target.duration_ms;
      }
      this.snapshot.error = null;
    }
    if (
      !this.dispatchRelationalRpc({
        op: "upsert_media_batch",
        items: [this.stripStreamUrl(target)],
      })
    ) {
      this.persistToStorage();
    }
    return structuredClone(target);
  }

  public upgradeMediaKindToAudio(mediaId: string): MediaItem | null {
    const item = this.media.find((m) => m.id === mediaId);
    if (!item) return null;
    if (item.media_kind === "audio") {
      return structuredClone(item);
    }
    item.media_kind = "audio";
    if (this.snapshot.media_id === mediaId) {
      this.snapshot.generation += 1;
    }
    if (
      !this.dispatchRelationalRpc({
        op: "upsert_media_batch",
        items: [this.stripStreamUrl(item)],
      })
    ) {
      this.persistToStorage({ debounceNativeMs: 400 });
    }
    return structuredClone(item);
  }

  public removeMedia(mediaId: string): boolean {
    const idx = this.media.findIndex((m) => m.id === mediaId);
    if (idx < 0) return false;
    const [removedItem] = this.media.splice(idx, 1);
    revokeIfBlobUrl(removedItem?.stream_url);
    this.notes = this.notes.filter((n) => n.media_id !== mediaId);
    for (const pl of this.playlists) {
      pl.item_ids = pl.item_ids.filter((id) => id !== mediaId);
    }
    this.snapshot.queue = this.snapshot.queue.filter((id) => id !== mediaId);
    this.shuffleOrder = this.shuffleOrder.filter((id) => id !== mediaId);
    if (this.unshuffledQueue) {
      this.unshuffledQueue = this.unshuffledQueue.filter((id) => id !== mediaId);
    }
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
        this.transitionSnapshotToIdle(this.snapshot.queue);
      }
    }
    if (this.dispatchRelationalRpc({ op: "delete_media_ids", ids: [mediaId] })) {
      if (removedItem?.source_folder_id && this.settings.source_folders) {
        this.dispatchRelationalRpc({ op: "save_settings", settings: this.settings });
      }
      this.dispatchRelationalRpc({ op: "save_session", session: this.buildSessionDto() });
    } else {
      this.persistToStorage();
    }
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

    const nowIso = new Date().toISOString();
    const folders = this.settings.source_folders ?? [];
    const normFolderPath = this.normalizeComparablePath(normalizedPath);
    let folder = folders.find(
      (f) => this.normalizeComparablePath(f.path) === normFolderPath,
    );
    if (!folder) {
      folder = {
        id: `src_${Date.now()}_${Math.random().toString(36).slice(2, 6)}`,
        path: normalizedPath,
        label: inferredLabel,
        enabled: true,
        added_at: nowIso,
        last_scanned_at: nowIso,
        media_count: 0,
      };
      folders.push(folder);
      this.settings.source_folders = folders;
    } else {
      folder.enabled = true;
      folder.last_scanned_at = nowIso;
      if (explicitLabel?.trim()) {
        folder.label = explicitLabel.trim();
      }
    }

    const supportedVideo = ["mp4", "m4v", "mov", "webm", "mkv"];
    const supportedAudio = ["wav", "flac", "ogg", "opus", "mp3", "m4a", "aac"];
    let importedCount = 0;
    const upsertedItems: MediaItem[] = [];

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
        if (f.streamUrl && f.streamUrl !== streamUrl) {
          revokeIfBlobUrl(f.streamUrl);
        }
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
          added_at: nowIso,
          last_opened_at: null,
          last_seen_at: nowIso,
          note_count: 0,
          source_folder_id: folder.id,
          stream_url: streamUrl,
        };
        this.media.push(created);
        upsertedItems.push(created);
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
        existing.last_seen_at = nowIso;
        if (streamUrl) {
          if (existing.stream_url && existing.stream_url !== streamUrl) {
            revokeIfBlobUrl(existing.stream_url);
          }
          existing.stream_url = streamUrl;
        }
        if (f.streamUrl && f.streamUrl !== existing.stream_url) {
          revokeIfBlobUrl(f.streamUrl);
        }
        if (!upsertedItems.includes(existing)) {
          upsertedItems.push(existing);
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
        if (!upsertedItems.includes(m)) {
          upsertedItems.push(m);
        }
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
    if (this.dispatchRelationalRpc({ op: "save_settings", settings: this.settings })) {
      if (upsertedItems.length > 0) {
        this.dispatchRelationalRpc({
          op: "upsert_media_batch",
          items: upsertedItems.map((m) => this.stripStreamUrl(m)),
        });
      }
      this.dispatchRelationalRpc({ op: "save_session", session: this.buildSessionDto() });
    } else {
      this.persistToStorage();
    }
    return {
      folder: structuredClone(folder),
      importedCount,
    };
  }

  public async scanAndAddSourceFolderPath(
    rawFolderPath: string,
  ): Promise<{ folder: TrustedSourceFolder; importedCount: number }> {
    const normalizedPath = this.normalizeFolderPath(rawFolderPath);
    const scanEpoch = this.stateEpoch;
    if (
      isBrowserRuntime() &&
      (window.location.hostname === "127.0.0.1" ||
        window.location.hostname === "localhost")
    ) {
      try {
        const res = await nativeFetch(
          `/__audora_scan_folder?path=${encodeURIComponent(normalizedPath)}`,
        );
        const text = await res.text();
        if (scanEpoch !== this.stateEpoch) {
          const fallbackIso = new Date().toISOString();
          return {
            folder: {
              id: "",
              path: normalizedPath,
              label: normalizedPath.split(/[\\/]/).filter(Boolean).pop() || normalizedPath,
              enabled: false,
              added_at: fallbackIso,
              last_scanned_at: fallbackIso,
              media_count: 0,
            },
            importedCount: 0,
          };
        }
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
        if (scanEpoch !== this.stateEpoch) {
          const fallbackIso = new Date().toISOString();
          return {
            folder: {
              id: "",
              path: normalizedPath,
              label: normalizedPath.split(/[\\/]/).filter(Boolean).pop() || normalizedPath,
              enabled: false,
              added_at: fallbackIso,
              last_scanned_at: fallbackIso,
              media_count: 0,
            },
            importedCount: 0,
          };
        }
        if (err && typeof err === "object" && "code" in err) {
          throw err;
        }
        // Running on Vite dev server without native `/__audora_scan_folder`; register folder record cleanly.
      }
    }
    if (scanEpoch !== this.stateEpoch) {
      const fallbackIso = new Date().toISOString();
      return {
        folder: {
          id: "",
          path: normalizedPath,
          label: normalizedPath.split(/[\\/]/).filter(Boolean).pop() || normalizedPath,
          enabled: false,
          added_at: fallbackIso,
          last_scanned_at: fallbackIso,
          media_count: 0,
        },
        importedCount: 0,
      };
    }
    return this.addSourceFolderWithFiles(normalizedPath, []);
  }

  public async pickAndImportSourceFolder(): Promise<{
    cancelled: boolean;
    folder?: TrustedSourceFolder;
    importedCount: number;
  }> {
    const pickEpoch = this.stateEpoch;
    await this.ensureHydratedFromNative();
    if (pickEpoch !== this.stateEpoch) {
      return { cancelled: true, importedCount: 0 };
    }
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
    const res = await nativeFetch("/__audora_pick_folder", { method: "POST" });
    if (pickEpoch !== this.stateEpoch) {
      return { cancelled: true, importedCount: 0 };
    }
    const text = await res.text();
    if (pickEpoch !== this.stateEpoch) {
      return { cancelled: true, importedCount: 0 };
    }
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
    unavailable?: boolean;
    cancelled: boolean;
    items: MediaItem[];
    selectedNames: string[];
  }> {
    const pickEpoch = this.stateEpoch;
    await this.ensureHydratedFromNative();
    if (pickEpoch !== this.stateEpoch) {
      return { supported: true, cancelled: true, items: [], selectedNames: [] };
    }
    if (
      !isBrowserRuntime() ||
      (window.location.hostname !== "127.0.0.1" &&
        window.location.hostname !== "localhost")
    ) {
      return { supported: false, cancelled: false, items: [], selectedNames: [] };
    }
    let text = "";
    try {
      const res = await nativeFetch("/__audora_pick_files", { method: "POST" });
      if (pickEpoch !== this.stateEpoch) {
        return { supported: true, cancelled: true, items: [], selectedNames: [] };
      }
      if (!res.ok) {
        if (res.status === 409) {
          return { supported: true, cancelled: true, items: [], selectedNames: [] };
        }
        return {
          supported: false,
          unavailable: true,
          cancelled: false,
          items: [],
          selectedNames: [],
        };
      }
      text = await res.text();
      if (pickEpoch !== this.stateEpoch) {
        return { supported: true, cancelled: true, items: [], selectedNames: [] };
      }
    } catch {
      return { supported: false, cancelled: false, items: [], selectedNames: [] };
    }
    if (!text.trim().startsWith("{")) {
      return { supported: false, cancelled: false, items: [], selectedNames: [] };
    }
    const payload = JSON.parse(text) as {
      ok?: boolean;
      cancelled?: boolean;
      unavailable?: boolean;
      status?: string;
      files?: Array<{
        path: string;
        name: string;
        size: number;
        duration_ms?: number | null;
      }>;
    };
    if (payload.unavailable || payload.status === "unavailable") {
      return {
        supported: false,
        unavailable: true,
        cancelled: false,
        items: [],
        selectedNames: [],
      };
    }
    if (!payload.ok) {
      if (payload.status === "busy") {
        return { supported: true, cancelled: true, items: [], selectedNames: [] };
      }
      return { supported: false, cancelled: false, items: [], selectedNames: [] };
    }
    if (payload.cancelled || !payload.files || payload.files.length === 0) {
      return { supported: true, cancelled: true, items: [], selectedNames: [] };
    }
    const emptyFile = payload.files.find((f) => f.size === 0);
    if (emptyFile) {
      const corruptErr = Object.assign(
        this.makeError(
          "AV-103",
          "MediaCorrupt",
          "Empty file",
          `${emptyFile.name} is empty (0 bytes) and cannot be played.`,
        ),
        { firstFileName: emptyFile.name },
      );
      if (this.snapshot.media_id === null) {
        this.snapshot.state = "failed";
        this.snapshot.error = corruptErr;
      }
      throw corruptErr;
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

  public async ingestNativeDroppedPaths(
    entries: Array<string | NativeDroppedEntry>,
    options?: { autoPlay?: boolean },
  ): Promise<{
    openedItems: MediaItem[];
    importedFolderCount: number;
  }> {
    const ingestEpoch = this.stateEpoch;
    await this.ensureHydratedFromNative();
    if (ingestEpoch !== this.stateEpoch) {
      return { openedItems: [], importedFolderCount: 0 };
    }
    const fileDescriptors: LocalFileDescriptor[] = [];
    let importedFolderCount = 0;

    for (const rawEntry of entries) {
      if (typeof rawEntry === "string") {
        const trimmed = rawEntry.trim();
        if (!trimmed) continue;
        const name = trimmed.split(/[\\/]/).pop() ?? trimmed;
        const ext = name.includes(".") ? (name.split(".").pop()?.toLowerCase() ?? "") : "";
        if (SUPPORTED_MEDIA_EXT_SET.has(ext)) {
          this.validateLocalMediaPath(trimmed);
          const probed = await this.probeNativeMediaDuration(trimmed);
          if (ingestEpoch !== this.stateEpoch) {
            return { openedItems: [], importedFolderCount: 0 };
          }
          fileDescriptors.push({
            name,
            path: trimmed,
            size: 0,
            durationMs: probed?.durationMs,
          });
        } else {
          try {
            const folderRes = await this.scanAndAddSourceFolderPath(trimmed);
            if (ingestEpoch !== this.stateEpoch) {
              return { openedItems: [], importedFolderCount: 0 };
            }
            importedFolderCount += folderRes.importedCount;
          } catch {
            if (ingestEpoch !== this.stateEpoch) {
              return { openedItems: [], importedFolderCount: 0 };
            }
            if (entries.length === 1) {
              this.openMediaPath(trimmed, { autoPlay: options?.autoPlay });
            }
          }
        }
        continue;
      }

      if (!rawEntry || typeof rawEntry !== "object") {
        continue;
      }
      const trimmedPath = typeof rawEntry.path === "string" ? rawEntry.path.trim() : "";
      if (!trimmedPath) continue;

      if (rawEntry.kind === "dir") {
        const folderRes = await this.scanAndAddSourceFolderPath(trimmedPath);
        if (ingestEpoch !== this.stateEpoch) {
          return { openedItems: [], importedFolderCount: 0 };
        }
        importedFolderCount += folderRes.importedCount;
        continue;
      }

      const validatedPath = this.validateLocalMediaPath(trimmedPath);
      const displayName =
        typeof rawEntry.name === "string" && rawEntry.name.trim()
          ? rawEntry.name.trim()
          : (validatedPath.split(/[\\/]/).pop() ?? validatedPath);

      if (rawEntry.size === 0) {
        const corruptErr = Object.assign(
          this.makeError(
            "AV-103",
            "MediaCorrupt",
            "Empty file",
            `${displayName} is empty (0 bytes) and cannot be played.`,
          ),
          { firstFileName: displayName },
        );
        if (this.snapshot.media_id === null) {
          this.snapshot.state = "failed";
          this.snapshot.error = corruptErr;
        }
        throw corruptErr;
      }

      const ext = displayName.includes(".")
        ? (displayName.split(".").pop()?.toLowerCase() ?? "")
        : "";
      if (!SUPPORTED_MEDIA_EXT_SET.has(ext)) {
        const unsupportedErr = Object.assign(
          this.makeError(
            "AV-104",
            "MediaUnsupported",
            "This format isn’t supported",
            `Audora couldn’t decode ${displayName}. The file uses a codec that isn’t available on this system.`,
          ),
          { firstFileName: displayName },
        );
        if (this.snapshot.media_id === null) {
          this.snapshot.state = "failed";
          this.snapshot.error = unsupportedErr;
        }
        throw unsupportedErr;
      }

      fileDescriptors.push({
        name: displayName,
        path: validatedPath,
        size: typeof rawEntry.size === "number" ? rawEntry.size : 0,
        durationMs:
          rawEntry.duration_ms && rawEntry.duration_ms > 0
            ? Math.round(rawEntry.duration_ms)
            : undefined,
      });
    }

    const openedItems =
      fileDescriptors.length > 0
        ? this.openLocalFiles(fileDescriptors, { autoPlay: options?.autoPlay })
        : [];
    if (
      options?.autoPlay &&
      openedItems.length === 0 &&
      importedFolderCount > 0 &&
      this.snapshot.media_id
    ) {
      this.playbackCommand({ action: "play" });
    }
    return { openedItems, importedFolderCount };
  }

  public markPlaybackDecodeFailed(mediaId: string): PlaybackSnapshot {
    const item = this.media.find((m) => m.id === mediaId);
    const label = item?.display_name ?? "this media file";
    const err = this.makeError(
      "AV-104",
      "MediaUnsupported",
      "This format isn’t supported",
      `Audora couldn’t decode ${label}. The file exists on disk, so your library entry and notes were preserved.`,
    );
    if (this.snapshot.media_id === mediaId) {
      this.snapshot.state = "paused";
      this.snapshot.error = err;
    }
    return structuredClone(this.snapshot);
  }

  public async refreshMediaContainerDurations(): Promise<boolean> {
    if (this.durationProbePromise) {
      return this.durationProbePromise;
    }
    const probeEpoch = this.stateEpoch;
    this.durationProbePromise = (async () => {
      try {
        if (
          !isBrowserRuntime() ||
          (window.location.hostname !== "127.0.0.1" &&
            window.location.hostname !== "localhost")
        ) {
          return false;
        }
        const mutatedItems: MediaItem[] = [];
        for (const item of [...this.media]) {
          if (probeEpoch !== this.stateEpoch) {
            return false;
          }
          if (item.duration_probed || this.isBareFileName(item.path)) {
            continue;
          }
          try {
            const res = await nativeFetch(
              `/__audora_probe_media?path=${encodeURIComponent(item.path)}`,
            );
            if (probeEpoch !== this.stateEpoch) {
              return false;
            }
            if (!res.ok || !isJsonResponse(res)) continue;
            const text = await res.text();
            if (probeEpoch !== this.stateEpoch) {
              return false;
            }
            if (!text.trim().startsWith("{")) continue;
            const parsed = JSON.parse(text) as {
              ok?: boolean;
              duration_ms?: number | null;
            };
            if (parsed.ok && parsed.duration_ms && parsed.duration_ms > 0) {
              const dur = Math.round(parsed.duration_ms);
              const liveItem = this.media.find((m) => m.id === item.id);
              if (liveItem && (liveItem.duration_ms !== dur || !liveItem.duration_probed)) {
                liveItem.duration_ms = dur;
                liveItem.duration_probed = true;
                if (liveItem.last_position_ms > dur) {
                  liveItem.last_position_ms = 0;
                }
                if (this.snapshot.media_id === liveItem.id) {
                  this.snapshot.duration_ms = dur;
                  if (this.snapshot.position_ms > dur) {
                    this.snapshot.position_ms = 0;
                  }
                }
                mutatedItems.push(liveItem);
              }
            }
          } catch {
            // Ignore probe failures when running without native bridge
          }
        }
        if (probeEpoch !== this.stateEpoch) {
          return false;
        }
        if (mutatedItems.length > 0) {
          if (
            !this.dispatchRelationalRpc({
              op: "upsert_media_batch",
              items: mutatedItems.map((m) => this.stripStreamUrl(m)),
            })
          ) {
            this.persistToStorage();
          }
        }
        return mutatedItems.length > 0;
      } finally {
        this.durationProbePromise = null;
      }
    })();
    return this.durationProbePromise;
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
    if (this.folderSyncPromise) {
      return this.folderSyncPromise;
    }
    const syncEpoch = this.stateEpoch;
    this.folderSyncPromise = (async () => {
      try {
        const enabledFolders = (this.settings.source_folders ?? []).filter(
          (f) => f.enabled,
        );
        let totalImported = 0;
        for (const folder of enabledFolders) {
          if (syncEpoch !== this.stateEpoch) {
            return 0;
          }
          try {
            const res = await this.scanAndAddSourceFolderPath(folder.path);
            if (syncEpoch !== this.stateEpoch) {
              return 0;
            }
            totalImported += res.importedCount;
          } catch {
            // Ignore temporarily disconnected external drives during startup sync.
          }
        }
        return totalImported;
      } finally {
        this.folderSyncPromise = null;
      }
    })();
    return this.folderSyncPromise;
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
      this.shuffleOrder = this.shuffleOrder.filter((id) => !hiddenIds.has(id));
      if (this.unshuffledQueue) {
        this.unshuffledQueue = this.unshuffledQueue.filter((id) => !hiddenIds.has(id));
      }
      if (this.snapshot.media_id && hiddenIds.has(this.snapshot.media_id)) {
        const nextAvailable = this.snapshot.queue.find((id) =>
          this.media.some(
            (m) => m.id === id && m.availability === "available",
          ),
        );
        if (nextAvailable) {
          this.selectMediaById(nextAvailable);
        } else {
          this.transitionSnapshotToIdle(this.snapshot.queue);
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
    if (this.dispatchRelationalRpc({ op: "save_settings", settings: this.settings })) {
      this.dispatchRelationalRpc({ op: "save_session", session: this.buildSessionDto() });
    } else {
      this.persistToStorage();
    }
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
    const preservedItems: MediaItem[] = [];

    this.media = this.media.filter((m) => {
      if (m.source_folder_id !== folderId) {
        return true;
      }
      if (notedMediaIds.has(m.id)) {
        m.source_folder_id = null;
        preservedItems.push(m);
        preservedNotedCount += 1;
        return true;
      }
      revokeIfBlobUrl(m.stream_url);
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
      this.shuffleOrder = this.shuffleOrder.filter((id) => !removedIds.has(id));
      if (this.unshuffledQueue) {
        this.unshuffledQueue = this.unshuffledQueue.filter((id) => !removedIds.has(id));
      }
      if (this.snapshot.media_id && removedIds.has(this.snapshot.media_id)) {
        const nextAvailable = this.snapshot.queue.find((id) =>
          this.media.some(
            (m) => m.id === id && m.availability === "available",
          ),
        );
        if (nextAvailable) {
          this.selectMediaById(nextAvailable);
        } else {
          this.transitionSnapshotToIdle(this.snapshot.queue);
        }
      }
    }

    this.recomputeSourceFolderCounts();
    if (this.isNativeRelationalRpcActive()) {
      if (preservedItems.length > 0) {
        this.dispatchRelationalRpc({
          op: "upsert_media_batch",
          items: preservedItems.map((m) => this.stripStreamUrl(m)),
        });
      }
      if (removedIds.size > 0) {
        this.dispatchRelationalRpc({
          op: "delete_media_ids",
          ids: Array.from(removedIds),
        });
      }
      this.dispatchRelationalRpc({ op: "save_settings", settings: this.settings });
      this.dispatchRelationalRpc(
        { op: "revoke_source_folder", path: removedFolder.path },
        { bestEffort: true },
      );
      this.dispatchRelationalRpc({ op: "save_session", session: this.buildSessionDto() });
    } else {
      this.persistToStorage();
      if (
        this.useStorage &&
        isBrowserRuntime() &&
        (window.location.hostname === "127.0.0.1" ||
          window.location.hostname === "localhost")
      ) {
        void nativeFetch(
          `/__audora_revoke_folder?path=${encodeURIComponent(removedFolder.path)}`,
          { method: "POST" },
        ).catch(() => {});
      }
    }
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
      this.persistProgressAndSession({ immediate: true });
    }
    return structuredClone(this.snapshot);
  }

  public createManualBackup(): string {
    if (!this.hasNativeRelationalRpc) {
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
    }
    const sep = getNativePlatformInfo().os === "windows" ? "\\" : "/";
    return `${getNativePlatformInfo().dataDir}${sep}audora.sqlite.manual.bak`;
  }

  public async createManualBackupNative(): Promise<string> {
    const fallbackPath = this.createManualBackup();
    if (this.hasNativeRelationalRpc) {
      this.persistProgressAndSession({ immediate: true });
      if (this.rpcFlushPromise) {
        await this.rpcFlushPromise;
      }
    } else {
      this.persistToStorage();
    }
    if (
      isBrowserRuntime() &&
      (window.location.hostname === "127.0.0.1" ||
        window.location.hostname === "localhost")
    ) {
      try {
        const res = await nativeFetch("/__audora_backup", { method: "POST" });
        if (res.ok) {
          const text = await res.text();
          if (text.trim().startsWith("{")) {
            const parsed = JSON.parse(text) as {
              ok?: boolean;
              backup_path?: string;
            };
            if (parsed.ok && parsed.backup_path) {
              return parsed.backup_path;
            }
          }
        }
      } catch {
        // Fall back to localStorage snapshot path in browser dev mode
      }
    }
    return fallbackPath;
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

    const nowIso = new Date().toISOString();
    const created: Playlist = {
      id: `pl_${Date.now()}_${Math.random().toString(36).slice(2, 6)}`,
      name: trimmedName,
      description: trimmedDesc,
      item_ids: dedupedInitial,
      created_at: nowIso,
      updated_at: nowIso,
    };
    this.playlists.unshift(created);
    if (!this.dispatchRelationalRpc({ op: "upsert_playlist", playlist: created })) {
      this.persistToStorage();
    }
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
    target.updated_at = new Date().toISOString();
    if (!this.dispatchRelationalRpc({ op: "upsert_playlist", playlist: target })) {
      this.persistToStorage();
    }
    return structuredClone(target);
  }

  public deletePlaylist(playlistId: string): Playlist | null {
    const idx = this.playlists.findIndex((pl) => pl.id === playlistId);
    if (idx < 0) return null;
    const [removed] = this.playlists.splice(idx, 1);
    if (!this.dispatchRelationalRpc({ op: "delete_playlist", id: playlistId })) {
      this.persistToStorage();
    }
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
    if (!this.dispatchRelationalRpc({ op: "upsert_playlist", playlist: restored })) {
      this.persistToStorage();
    }
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
    target.updated_at = new Date().toISOString();
    if (!this.dispatchRelationalRpc({ op: "upsert_playlist", playlist: target })) {
      this.persistToStorage();
    }
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
    target.updated_at = new Date().toISOString();
    if (!this.dispatchRelationalRpc({ op: "upsert_playlist", playlist: target })) {
      this.persistToStorage();
    }
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
      const disabledFolders = this.disabledSourceFolderIds();
      const visibleMediaIds = new Set(
        this.media
          .filter(
            (m) =>
              !m.source_folder_id || !disabledFolders.has(m.source_folder_id),
          )
          .map((m) => m.id),
      );
      target.item_ids.splice(currIdx, 1);
      const remainingVisibleRawIndices: number[] = [];
      for (let i = 0; i < target.item_ids.length; i += 1) {
        if (visibleMediaIds.has(target.item_ids[i]!)) {
          remainingVisibleRawIndices.push(i);
        }
      }
      let rawInsertIdx: number;
      if (remainingVisibleRawIndices.length === 0) {
        rawInsertIdx = Math.max(0, Math.min(currIdx, target.item_ids.length));
      } else {
        const boundedVisibleIdx = Math.max(
          0,
          Math.min(targetIndex, remainingVisibleRawIndices.length),
        );
        if (boundedVisibleIdx >= remainingVisibleRawIndices.length) {
          rawInsertIdx =
            remainingVisibleRawIndices[remainingVisibleRawIndices.length - 1]! + 1;
        } else {
          rawInsertIdx = remainingVisibleRawIndices[boundedVisibleIdx]!;
        }
      }
      target.item_ids.splice(rawInsertIdx, 0, mediaId);
      target.updated_at = new Date().toISOString();
      if (!this.dispatchRelationalRpc({ op: "upsert_playlist", playlist: target })) {
        this.persistToStorage();
      }
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
    const useShuffle = Boolean(options?.shuffle);
    let orderedQueue = [...queueIds];
    let firstId =
      options?.startMediaId && queueIds.includes(options.startMediaId)
        ? options.startMediaId
        : queueIds[0]!;

    if (useShuffle) {
      this.unshuffledQueue = [...queueIds];
      if (options?.startMediaId && queueIds.includes(options.startMediaId)) {
        const rest = queueIds.filter((id) => id !== options.startMediaId);
        orderedQueue = [options.startMediaId, ...shufflePermutation(rest)];
        firstId = options.startMediaId;
      } else {
        orderedQueue = shufflePermutation(queueIds);
        firstId = orderedQueue[0]!;
      }
    } else {
      this.unshuffledQueue = null;
    }

    this.snapshot.queue = orderedQueue;
    this.snapshot.shuffle = useShuffle;
    this.shuffleOrder = useShuffle ? [...orderedQueue] : [];
    this.selectMediaById(firstId, {
      startFromBeginning: true,
      preserveShuffleOrder: true,
    });
    this.persistProgressAndSession();
    return this.playbackCommand({ action: "play" });
  }

  /**
   * Generates a redacted, user-reviewable diagnostic report (`docs/SECURITY.md`).
   * Guarantees that zero file paths, filenames, playlist titles, or note contents appear in the report.
   */
  public generateDiagnosticReport(): DiagnosticReport {
    const plat = getNativePlatformInfo();
    return {
      app_version: this.getCurrentVersion(),
      os_family: plat.os,
      arch: plat.arch,
      schema_version: 4,
      media_count: this.media.length,
      missing_media_count: this.media.filter((m) => m.availability === "missing").length,
      total_note_count: this.notes.length,
      playlist_count: this.playlists.length,
      playback_state: this.snapshot.state,
      last_error_code: this.snapshot.error?.code ?? null,
      generated_at: new Date().toISOString(),
    };
  }

  public async generateDiagnosticReportNative(): Promise<DiagnosticReport> {
    if (
      this.hasNativeRelationalRpc &&
      isBrowserRuntime() &&
      (window.location.hostname === "127.0.0.1" ||
        window.location.hostname === "localhost")
    ) {
      if (this.rpcFlushPromise) {
        await this.rpcFlushPromise;
      }
      try {
        const res = await nativeFetch("/__audora_rpc", {
          method: "POST",
          headers: { "Content-Type": "application/json" },
          body: JSON.stringify({ op: "diagnostic_report" }),
        });
        if (res.ok) {
          const parsed = (await res.json()) as { ok?: boolean; data?: DiagnosticReport };
          if (parsed.ok && parsed.data) {
            return {
              ...parsed.data,
              app_version: this.getCurrentVersion(),
              playback_state: this.snapshot.state,
              last_error_code: this.snapshot.error?.code ?? parsed.data.last_error_code,
            };
          }
        }
      } catch {
        // Fall back to local report when running outside native shell
      }
    }
    return this.generateDiagnosticReport();
  }

  public getCurrentVersion(): string {
    const bootVersion = getNativePlatformInfo().appVersion;
    if (getNativeSessionToken() && bootVersion) {
      if (this.stagedRestartVersion && compareSemver(this.stagedRestartVersion, bootVersion) > 0) {
        return this.stagedRestartVersion;
      }
      return bootVersion;
    }
    const base = bootVersion ?? BASE_APP_VERSION;
    const installed = this.settings.installed_version;
    if (installed && compareSemver(installed, base) > 0) {
      return installed;
    }
    return base;
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
    const plat = getNativePlatformInfo();
    const osSuffixes =
      plat.os === "windows"
        ? ["-setup.exe", ".msi", "-portable.zip", ".exe"]
        : plat.os === "linux"
          ? [".deb", ".appimage", ".tar.gz"]
          : [".app.tar.gz", ".dmg"];

    let chosen: { url: string; size: number; sha: string; score: number } | null = null;

    for (const asset of parsed.assets ?? []) {
      const name = (asset.name ?? "").toLowerCase();
      if (name.endsWith(".json") || name.endsWith(".sha256") || name.endsWith(".sig")) {
        continue;
      }
      const url = asset.browser_download_url ?? "";
      if (!url) continue;
      if (plat.os === "linux" && name.endsWith(".app.tar.gz")) {
        continue;
      }
      if (plat.os === "macos" && (name.includes("linux") || name.includes("windows"))) {
        continue;
      }
      if (plat.os === "linux" && (name.includes("macos") || name.includes("darwin") || name.includes("windows"))) {
        continue;
      }
      if (plat.os === "windows" && (name.includes("macos") || name.includes("darwin") || name.includes("linux"))) {
        continue;
      }
      const size = asset.size ?? 0;
      let assetSha = "";
      if (asset.digest?.startsWith("sha256:")) {
        const hex = asset.digest.slice("sha256:".length);
        if (/^[0-9a-f]{64}$/i.test(hex)) {
          assetSha = hex.toLowerCase();
        }
      }
      const suffixIdx = osSuffixes.findIndex((ext) => name.endsWith(ext));
      if (suffixIdx >= 0) {
        const isUniversal = name.includes("universal");
        const hasArm64 = /aarch64|arm64/.test(name);
        const hasX64 = /x86_64|amd64|x64/.test(name);
        if (!isUniversal) {
          if (plat.arch === "aarch64" && hasX64 && !hasArm64) {
            continue;
          }
          if (plat.arch === "x86_64" && hasArm64 && !hasX64) {
            continue;
          }
        }
        const exactArchMatch = plat.arch === "aarch64" ? hasArm64 : hasX64;
        // Higher score wins: exact arch match (+100) / universal (+50) + earlier suffix priority (`.app.tar.gz` > `.dmg`) (`C-5`).
        const score =
          (exactArchMatch ? 100 : isUniversal ? 50 : 25) +
          (osSuffixes.length - suffixIdx);
        if (!chosen || score > chosen.score) {
          chosen = { url, size, sha: assetSha, score };
        }
      }
    }

    const downloadUrl = chosen?.url ?? parsed.html_url ?? "";
    const packageSize = chosen?.size ?? 0;
    // Never fabricate a fake SHA-256 when `digest` is absent or no OS+arch-compatible asset exists (`C-2`, `H-5`).
    const sha256 = chosen?.sha ?? "";

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
    if (!clean) {
      throw this.makeError(
        "AV-301",
        "ValidationFailed",
        "Invalid update version",
        "Target update version must not be empty.",
      );
    }
    const status = this.checkForUpdates(true);
    if (!status.verified) {
      throw this.makeError(
        "AV-301",
        "ValidationFailed",
        "Unverified update package",
        "SHA-256 signature verification failed for this release.",
      );
    }
    if (!status.update_available || status.latest_version !== clean) {
      throw this.makeError(
        "AV-301",
        "ValidationFailed",
        "Update version mismatch",
        "Requested update version does not match a verified staged release.",
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
    if (!this.dispatchRelationalRpc({ op: "save_settings", settings: this.settings })) {
      this.persistToStorage();
    }
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
    const preCheck = this.checkForUpdates(true);
    if (!preCheck.verified || !preCheck.update_available || preCheck.latest_version !== clean) {
      // Trigger standard validation error path
      this.installUpdate(clean);
    }

    if (isBrowserRuntime()) {
      let handledByNativeBridge = false;
      if (
        window.location.hostname === "127.0.0.1" ||
        window.location.hostname === "localhost"
      ) {
        try {
          const res = await nativeFetch("/__audora_update", {
            method: "POST",
            headers: { "Content-Type": "application/json" },
            body: JSON.stringify({
              version: clean,
              sha256: preCheck.sha256_checksum,
              download_url: preCheck.download_url,
            }),
          });
          if (isJsonResponse(res)) {
            const text = await res.text();
            if (text.trim().startsWith("{")) {
              const payload = JSON.parse(text) as {
                ok?: boolean;
                status?: string;
                message?: string;
                deferred_install?: boolean;
              };
              if (res.status !== 404 || payload.status) {
                handledByNativeBridge = true;
                if (!payload.ok) {
                  throw this.makeError(
                    "AV-301",
                    "ValidationFailed",
                    "Update verification failed",
                    payload.message ?? "Native update integrity check failed.",
                  );
                }
                if (payload.status === "restarting") {
                  if (
                    payload.deferred_install === true ||
                    getNativePlatformInfo().os === "windows"
                  ) {
                    return {
                      status: {
                        ...preCheck,
                        staged_for_restart: true,
                      },
                      message:
                        payload.message ??
                        `Audora v${clean} verified. Closing to install and reopen…`,
                    };
                  }
                  this.stagedRestartVersion = clean;
                  const status = this.installUpdate(clean);
                  return {
                    status,
                    message:
                      payload.message ??
                      `Audora v${clean} verified and restarting…`,
                  };
                }
                return {
                  status: {
                    ...preCheck,
                    staged_for_restart: false,
                  },
                  message:
                    payload.message ??
                    `Opened Audora v${clean} installer. Complete installation manually.`,
                };
              }
            }
          }
        } catch (err) {
          if (err && typeof err === "object" && "code" in err) {
            throw err;
          }
          // Running in Vite dev server or standalone browser; fall through.
        }
      }

      const status = this.installUpdate(clean);
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
      return {
        status,
        message: `Audora v${clean} verified (SHA-256) and staged. Restart Audora to complete update.`,
      };
    }

    const status = this.installUpdate(clean);
    return {
      status,
      message: `Audora v${clean} verified (SHA-256) and staged. Restart Audora to complete update.`,
    };
  }

  public dismissUpdate(version: string): AppUpdateStatusDto {
    const clean = version.trim().replace(/^v/i, "");
    this.settings.dismissed_update_version = clean;
    if (!this.dispatchRelationalRpc({ op: "save_settings", settings: this.settings })) {
      this.persistToStorage();
    }
    return this.checkForUpdates(false);
  }

  public cleanupApplicationData(): CleanupSummaryDto {
    this.stateEpoch += 1;
    const plat = getNativePlatformInfo();
    const sep = plat.os === "windows" ? "\\" : "/";
    const removedPaths = [
      `${plat.dataDir}${sep}audora.sqlite`,
      `${plat.dataDir}${sep}updates${sep}release-manifest.json`,
    ];
    for (const m of this.media) {
      revokeIfBlobUrl(m.stream_url);
    }
    this.settleRpcEntry(this.inFlightRpcEntry, false);
    this.inFlightRpcEntry = null;
    for (const pending of this.rpcQueue) {
      this.settleRpcEntry(pending, false);
    }
    this.rpcQueue = [];
    this.pendingNativeSave = null;
    this.preHydrationDirtySettings.clear();
    this.nativeHydrationState = "hydrated";
    this.hydrationPromise = Promise.resolve();
    if (this.seekDebounceTimer !== null) {
      clearTimeout(this.seekDebounceTimer);
      this.seekDebounceTimer = null;
    }
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
      this.useStorage &&
      isBrowserRuntime() &&
      (window.location.hostname === "127.0.0.1" ||
        window.location.hostname === "localhost")
    ) {
      void nativeFetch("/__audora_state", { method: "DELETE" }).catch(() => {
        // Ignore when native server is not running.
      });
    }
    this.media = [];
    this.notes = [];
    this.playlists = [];
    this.shuffleOrder = [];
    this.unshuffledQueue = null;
    this.stagedRestartVersion = null;
    this.settings = structuredClone(INITIAL_SETTINGS);
    this.snapshot = createIdleSnapshot(INITIAL_SETTINGS.default_playback_speed);
    this.stagedManifest = null;
    postNativeIpcMessage("window_mode:full");
    return {
      removed_paths: removedPaths,
      bytes_reclaimed: 262_144,
    };
  }

  public async cleanupApplicationDataNative(): Promise<CleanupSummaryDto> {
    this.stateEpoch += 1;
    const plat = getNativePlatformInfo();
    const sep = plat.os === "windows" ? "\\" : "/";
    let removedPaths = [
      `${plat.dataDir}${sep}audora.sqlite`,
      `${plat.dataDir}${sep}updates${sep}release-manifest.json`,
    ];
    let bytesReclaimed = 0;

    for (const m of this.media) {
      revokeIfBlobUrl(m.stream_url);
    }
    this.settleRpcEntry(this.inFlightRpcEntry, false);
    this.inFlightRpcEntry = null;
    for (const pending of this.rpcQueue) {
      this.settleRpcEntry(pending, false);
    }
    this.rpcQueue = [];
    this.pendingNativeSave = null;
    this.preHydrationDirtySettings.clear();
    this.nativeHydrationState = "hydrated";
    this.hydrationPromise = Promise.resolve();
    if (this.seekDebounceTimer !== null) {
      clearTimeout(this.seekDebounceTimer);
      this.seekDebounceTimer = null;
    }
    if (this.rpcFlushPromise) {
      await this.rpcFlushPromise;
    }
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
      this.useStorage &&
      isBrowserRuntime() &&
      (window.location.hostname === "127.0.0.1" ||
        window.location.hostname === "localhost")
    ) {
      try {
        const res = await nativeFetch("/__audora_state", { method: "DELETE" });
        if (res.ok && isJsonResponse(res)) {
          const parsed = (await res.json()) as {
            ok?: boolean;
            removed_paths?: string[];
            bytes_reclaimed?: number;
          };
          if (parsed.ok) {
            if (Array.isArray(parsed.removed_paths) && parsed.removed_paths.length > 0) {
              removedPaths = parsed.removed_paths;
            }
            if (typeof parsed.bytes_reclaimed === "number") {
              bytesReclaimed = parsed.bytes_reclaimed;
            }
          }
        }
      } catch {
        // Ignore when native server is not running.
      }
    }

    this.media = [];
    this.notes = [];
    this.playlists = [];
    this.shuffleOrder = [];
    this.unshuffledQueue = null;
    this.stagedRestartVersion = null;
    this.settings = structuredClone(INITIAL_SETTINGS);
    this.snapshot = createIdleSnapshot(INITIAL_SETTINGS.default_playback_speed);
    this.stagedManifest = null;
    postNativeIpcMessage("window_mode:full");
    return {
      removed_paths: removedPaths,
      bytes_reclaimed: bytesReclaimed,
    };
  }

  private makeError(
    code: AppErrorPayload["code"],
    category: string,
    title: string,
    message: string,
    customRecoverySteps?: string[],
  ): AppErrorPayload {
    let recoverySteps: string[];
    if (customRecoverySteps && customRecoverySteps.length > 0) {
      recoverySteps = customRecoverySteps;
    } else {
      switch (code) {
        case "AV-101":
          recoverySteps = [
            "1. Click 'Locate file…' to relink the moved or renamed file.",
            "2. Reconnect the external drive, or remove the item from your library.",
          ];
          break;
        case "AV-102":
          recoverySteps = [
            "1. Check that the file exists and Audora has permission to read it.",
            "2. Choose another file or inspect its folder permissions.",
          ];
          break;
        case "AV-103":
          recoverySteps = [
            "1. Check that the file is not empty (0 bytes) or truncated on disk.",
            "2. Choose another file or re-export the recording.",
          ];
          break;
        case "AV-104":
          recoverySteps = [
            "1. Convert the file to MP4 (H.264) or WebM.",
            "2. Try opening it with your system’s default player.",
          ];
          break;
        case "AV-201":
          recoverySteps = [
            "1. Choose another file that is not already tracked in your library.",
            "2. Cancel relinking and open the existing library item directly.",
          ];
          break;
        case "AV-202":
          recoverySteps = [
            "1. Review the original vs. selected file duration and media type.",
            "2. Click 'Relink anyway' to preserve your notes, or 'Cancel' to choose another file.",
          ];
          break;
        case "AV-301":
          recoverySteps = [
            "1. Review the highlighted field requirement and keep your input within valid bounds.",
          ];
          break;
        case "AV-401":
          recoverySteps = [
            "1. Free up disk space and verify write permissions in your Audora data directory.",
            "2. Retry the operation; your automatic SQLite backup remains preserved.",
          ];
          break;
        default:
          recoverySteps = [
            "1. Retry the action.",
            "2. Preview or copy a redacted diagnostic report from Settings → Privacy.",
          ];
          break;
      }
    }
    return {
      code,
      category,
      title,
      message,
      recovery_steps: recoverySteps,
      recoverable: true,
    };
  }
}

export const ipcClient = new AudoraIpcClient();
