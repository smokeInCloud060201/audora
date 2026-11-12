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
    last_position_ms: 864_000, // 14:24 (62% in demo or active timestamp)
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
];

export class AudoraIpcClient {
  private media: MediaItem[] = structuredClone(INITIAL_MEDIA);
  private notes: Note[] = structuredClone(INITIAL_NOTES);
  private settings: AppSettings = {
    theme: "dark",
    resume_where_left_off: true,
    pause_on_headphones_disconnect: true,
    default_playback_speed: 1.0,
    timestamp_precision: "nearest_second",
    window_mode: "full",
  };
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

  public getSettings(): AppSettings {
    return structuredClone(this.settings);
  }

  public updateSettings(next: AppSettings): AppSettings {
    if (next.default_playback_speed < 0.5 || next.default_playback_speed > 2.0) {
      throw this.makeError("AV-301", "ValidationFailed", "Invalid playback speed", "Speed must be between 0.5x and 2.0x.");
    }
    this.settings = structuredClone(next);
    return structuredClone(this.settings);
  }

  public setWindowMode(mode: WindowMode): AppSettings {
    this.settings.window_mode = mode;
    return structuredClone(this.settings);
  }

  public getPlaybackSnapshot(): PlaybackSnapshot {
    return structuredClone(this.snapshot);
  }

  public playbackCommand(cmd: PlaybackCommand): PlaybackSnapshot {
    switch (cmd.action) {
      case "play":
        if (this.snapshot.media_id) {
          this.snapshot.state = "playing";
        }
        break;
      case "pause":
        this.snapshot.state = "paused";
        break;
      case "toggle_play_pause":
        this.snapshot.state = this.snapshot.state === "playing" ? "paused" : "playing";
        break;
      case "seek": {
        const max = this.snapshot.duration_ms ?? cmd.position_ms;
        this.snapshot.position_ms = Math.max(0, Math.min(cmd.position_ms, max));
        const active = this.media.find((m) => m.id === this.snapshot.media_id);
        if (active) {
          active.last_position_ms = this.snapshot.position_ms;
        }
        break;
      }
      case "skip_relative": {
        const nextPos = Math.max(0, this.snapshot.position_ms + cmd.delta_ms);
        const max = this.snapshot.duration_ms ?? nextPos;
        this.snapshot.position_ms = Math.min(nextPos, max);
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
        if (this.snapshot.media_id) {
          const idx = this.snapshot.queue.indexOf(this.snapshot.media_id);
          const nextIdx =
            idx + 1 < this.snapshot.queue.length
              ? idx + 1
              : this.snapshot.repeat_mode === "all"
                ? 0
                : -1;
          if (nextIdx >= 0) {
            this.selectMediaById(this.snapshot.queue[nextIdx]);
          }
        }
        break;
      }
      case "previous": {
        if (this.snapshot.position_ms > 3000) {
          this.snapshot.position_ms = 0;
        } else if (this.snapshot.media_id) {
          const idx = this.snapshot.queue.indexOf(this.snapshot.media_id);
          if (idx > 0) {
            this.selectMediaById(this.snapshot.queue[idx - 1]);
          }
        }
        break;
      }
      case "stop":
        this.snapshot.state = "idle";
        this.snapshot.position_ms = 0;
        break;
    }
    return structuredClone(this.snapshot);
  }

  public selectMediaById(mediaId: string): MediaItem {
    const item = this.media.find((m) => m.id === mediaId);
    if (!item) {
      throw this.makeError("AV-101", "MediaUnavailable", "We can’t find this file", "Selected library item does not exist.");
    }
    if (item.availability === "missing") {
      throw this.makeError("AV-101", "MediaUnavailable", "We can’t find this file", "It may have been moved, renamed, or stored on a disconnected drive.");
    }
    this.snapshot.generation += 1;
    this.snapshot.media_id = item.id;
    this.snapshot.duration_ms = item.duration_ms;
    this.snapshot.position_ms =
      this.settings.resume_where_left_off && !item.completed ? item.last_position_ms : 0;
    this.snapshot.rate = this.settings.default_playback_speed;
    this.snapshot.state = "paused";
    this.snapshot.error = null;
    if (!this.snapshot.queue.includes(item.id)) {
      this.snapshot.queue.push(item.id);
    }
    return structuredClone(item);
  }

  public openMediaPath(rawPath: string): MediaItem {
    const trimmed = rawPath.trim();
    if (!trimmed) {
      throw this.makeError("AV-301", "ValidationFailed", "Invalid path", "Please enter or choose a local media file path.");
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
        id: `med_${Date.now()}`,
        path: trimmed,
        display_name: fileName,
        media_kind: supportedVideo.includes(ext) ? "video" : "audio",
        duration_ms: 1_800_000,
        last_position_ms: 0,
        completed: false,
        file_size_bytes: 50_000_000,
        fingerprint: null,
        availability: "available",
        added_at: "Just now",
        last_opened_at: "Just now",
        last_seen_at: "Just now",
        note_count: 0,
      };
      this.media.unshift(item);
    }
    return this.selectMediaById(item.id);
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
      ? this.notes.filter((n) => {
          const parent = this.media.find((m) => m.id === n.media_id);
          if (!parent) return false;
          if (filter !== "all" && parent.media_kind !== filter) return false;
          return (
            n.body.toLowerCase().includes(q) ||
            parent.display_name.toLowerCase().includes(q)
          );
        })
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
    return structuredClone(note);
  }

  public updateNote(input: UpdateNoteInput): Note {
    const trimmed = input.body.trim();
    if (!trimmed) {
      throw this.makeError("AV-301", "ValidationFailed", "Note text required", "Note text cannot be empty.");
    }
    const note = this.notes.find((n) => n.id === input.id);
    if (!note) {
      throw this.makeError("AV-301", "ValidationFailed", "Note not found", "Selected note does not exist.");
    }
    note.body = trimmed;
    note.is_quick_marker = false;
    note.updated_at = "Just now";
    return structuredClone(note);
  }

  public deleteNote(noteId: string): Note | null {
    const idx = this.notes.findIndex((n) => n.id === noteId);
    if (idx < 0) return null;
    const [removed] = this.notes.splice(idx, 1);
    return structuredClone(removed);
  }

  public restoreNote(note: Note): Note {
    this.notes.push(structuredClone(note));
    return structuredClone(note);
  }

  public activateNote(noteId: string): PlaybackSnapshot {
    const note = this.notes.find((n) => n.id === noteId);
    if (!note) {
      throw this.makeError("AV-301", "ValidationFailed", "Note not found", "Selected note no longer exists.");
    }
    this.selectMediaById(note.media_id);
    return this.playbackCommand({ action: "seek", position_ms: note.position_ms });
  }

  public relinkMedia(mediaId: string, newPath: string): MediaItem {
    const trimmed = newPath.trim();
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
    target.path = trimmed;
    target.availability = "available";
    target.last_seen_at = "Just now";
    return structuredClone(target);
  }

  public removeMedia(mediaId: string): boolean {
    const idx = this.media.findIndex((m) => m.id === mediaId);
    if (idx < 0) return false;
    this.media.splice(idx, 1);
    this.notes = this.notes.filter((n) => n.media_id !== mediaId);
    this.snapshot.queue = this.snapshot.queue.filter((id) => id !== mediaId);
    return true;
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
