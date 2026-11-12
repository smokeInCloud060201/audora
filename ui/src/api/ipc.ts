import type {
  AppErrorPayload,
  AppSettings,
  CreateNoteInput,
  LibraryFilter,
  LibrarySearchResult,
  MediaItem,
  Note,
  PlaybackCommand,
  PlaybackSnapshot,
  UpdateNoteInput,
  WindowMode,
} from "../types/domain";

const STORAGE_KEY = "audora.state.v1";

/**
 * Initial representative local library matching the Figma design frames (`2:3`, `4:3`, `4:206`, `4:339`).
 */
const INITIAL_MEDIA: MediaItem[] = [
  {
    id: "med_listening",
    path: "/Users/local/Media/The Art of Listening.mp4",
    display_name: "The Art of Listening",
    media_kind: "video",
    duration_ms: 2_892_000, // 48:12
    last_position_ms: 864_000, // 14:24
    completed: false,
    file_size_bytes: 420_000_000,
    fingerprint: null,
    availability: "available",
    added_at: "18 minutes ago",
    last_opened_at: "18 minutes ago",
    last_seen_at: "18 minutes ago",
    note_count: 4,
  },
  {
    id: "med_reykjavik",
    path: "/Users/local/Media/Field Notes — Reykjavík.flac",
    display_name: "Field Notes — Reykjavík",
    media_kind: "audio",
    duration_ms: 4_328_000, // 1:12:08
    last_position_ms: 1_341_680,
    completed: false,
    file_size_bytes: 185_000_000,
    fingerprint: null,
    availability: "available",
    added_at: "Yesterday",
    last_opened_at: "Yesterday",
    last_seen_at: "Yesterday",
    note_count: 0,
  },
  {
    id: "med_rhythm",
    path: "/Users/local/Media/Editing Rhythm — Masterclass.mp4",
    display_name: "Editing Rhythm — Masterclass",
    media_kind: "video",
    duration_ms: 2_204_000, // 36:44
    last_position_ms: 0,
    completed: false,
    file_size_bytes: 310_000_000,
    fingerprint: null,
    availability: "available",
    added_at: "Last week",
    last_opened_at: "Last week",
    last_seen_at: "Last week",
    note_count: 0,
  },
  {
    id: "med_interview",
    path: "/Users/local/Media/Monday Interview.wav",
    display_name: "Monday Interview.wav",
    media_kind: "audio",
    duration_ms: 3_260_000, // 54:20
    last_position_ms: 2_868_800,
    completed: false,
    file_size_bytes: 95_000_000,
    fingerprint: null,
    availability: "available",
    added_at: "Last week",
    last_opened_at: "Last week",
    last_seen_at: "Last week",
    note_count: 0,
  },
  {
    id: "med_archive_missing",
    path: "/Volumes/Archive/Sound Design Reel.mp4",
    display_name: "Archive: Sound Design Reel",
    media_kind: "video",
    duration_ms: 1_326_000, // 22:06
    last_position_ms: 420_000,
    completed: false,
    file_size_bytes: null,
    fingerprint: null,
    availability: "missing",
    added_at: "2 weeks ago",
    last_opened_at: "2 weeks ago",
    last_seen_at: "2 weeks ago",
    note_count: 4,
  },
];

const INITIAL_NOTES: Note[] = [
  {
    id: "note_1",
    media_id: "med_listening",
    position_ms: 222_000, // 03:42
    is_quick_marker: false,
    body: "Great explanation of how room tone shapes the transition.",
    created_at: "18 minutes ago",
    updated_at: "18 minutes ago",
  },
  {
    id: "note_2",
    media_id: "med_listening",
    position_ms: 738_000, // 12:18
    is_quick_marker: false,
    body: "Try this approach on the interview sequence.",
    created_at: "15 minutes ago",
    updated_at: "15 minutes ago",
  },
  {
    id: "note_3",
    media_id: "med_listening",
    position_ms: 1_625_000, // 27:05
    is_quick_marker: false,
    body: "Reference: the three-layer ambience technique.",
    created_at: "10 minutes ago",
    updated_at: "10 minutes ago",
  },
  {
    id: "note_4",
    media_id: "med_listening",
    position_ms: 2_496_000, // 41:36
    is_quick_marker: false,
    body: "Key takeaway — restraint creates more perceived depth.",
    created_at: "5 minutes ago",
    updated_at: "5 minutes ago",
  },
  {
    id: "note_archive_1",
    media_id: "med_archive_missing",
    position_ms: 222_000, // 03:42
    is_quick_marker: false,
    body: "Opening sub-bass swell layered with Foley cloth rustle.",
    created_at: "2 weeks ago",
    updated_at: "2 weeks ago",
  },
  {
    id: "note_archive_2",
    media_id: "med_archive_missing",
    position_ms: 484_000, // 08:04
    is_quick_marker: false,
    body: "Metallic resonance cut right before the dialogue entrance.",
    created_at: "2 weeks ago",
    updated_at: "2 weeks ago",
  },
  {
    id: "note_archive_3",
    media_id: "med_archive_missing",
    position_ms: 738_000, // 12:18
    is_quick_marker: false,
    body: "Exterior rain perspective shifts smoothly to interior reverb.",
    created_at: "2 weeks ago",
    updated_at: "2 weeks ago",
  },
  {
    id: "note_archive_4",
    media_id: "med_archive_missing",
    position_ms: 1_190_000, // 19:50
    is_quick_marker: false,
    body: "Final tail decay — leave 2 seconds of silence before credits.",
    created_at: "2 weeks ago",
    updated_at: "2 weeks ago",
  },
];

const INITIAL_SETTINGS: AppSettings = {
  theme: "dark",
  resume_where_left_off: true,
  pause_on_headphones_disconnect: true,
  default_playback_speed: 1.0,
  timestamp_precision: "nearest_second",
  window_mode: "full",
};

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
  private media: MediaItem[] = structuredClone(INITIAL_MEDIA);
  private notes: Note[] = structuredClone(INITIAL_NOTES);
  private settings: AppSettings = structuredClone(INITIAL_SETTINGS);
  private snapshot: PlaybackSnapshot = {
    generation: 1,
    state: "paused",
    media_id: "med_listening",
    position_ms: 864_000, // 14:24
    duration_ms: 2_892_000, // 48:12
    rate: 1.0,
    volume: 1.0,
    muted: false,
    repeat_mode: "off",
    shuffle: false,
    queue: ["med_listening", "med_reykjavik", "med_rhythm"],
    error: null,
  };

  public constructor(useStorage = true) {
    if (useStorage) {
      this.hydrateFromStorage();
    }
  }

  private hydrateFromStorage(): void {
    try {
      if (typeof window === "undefined" || !window.localStorage) return;
      const raw = window.localStorage.getItem(STORAGE_KEY);
      if (!raw) return;
      const parsed = JSON.parse(raw) as Partial<PersistedState>;
      if (Array.isArray(parsed.media) && parsed.media.length > 0) {
        this.media = parsed.media;
      }
      if (Array.isArray(parsed.notes)) {
        this.notes = parsed.notes;
      }
      if (parsed.settings) {
        this.settings = { ...INITIAL_SETTINGS, ...parsed.settings };
      }
    } catch {
      // Fall back to clean defaults if storage is corrupted or unavailable.
    }
  }

  private persistToStorage(): void {
    try {
      if (typeof window === "undefined" || !window.localStorage) return;
      const payload: PersistedState = {
        // Strip transient blob object URLs before persisting to localStorage
        media: this.media.map(({ stream_url: _stream, ...rest }) => rest),
        notes: this.notes,
        settings: this.settings,
      };
      window.localStorage.setItem(STORAGE_KEY, JSON.stringify(payload));
    } catch {
      // Ignore quota errors in restricted webviews.
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
    return structuredClone(this.settings);
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

  public openMediaPath(rawPath: string, options?: { streamUrl?: string; size?: number; durationMs?: number }): MediaItem {
    const trimmed = rawPath.trim();
    if (!trimmed) {
      throw this.makeError(
        "AV-301",
        "ValidationFailed",
        "Invalid path",
        "Please enter or choose a local media file path.",
      );
    }
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
        duration_ms: options?.durationMs ?? 1_800_000,
        last_position_ms: 0,
        completed: false,
        file_size_bytes: options?.size ?? 50_000_000,
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
      const virtualPath = f.path ?? `/Users/local/Media/${f.name}`;
      const item = this.openMediaPath(virtualPath, {
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
    const trimmed = newPath.trim();
    if (!trimmed) {
      throw this.makeError(
        "AV-301",
        "ValidationFailed",
        "Path required",
        "Please enter or select the relocated media file path.",
      );
    }
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
