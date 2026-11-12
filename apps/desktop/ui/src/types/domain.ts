/**
 * Strict TypeScript contracts mirroring `src-tauri/src/domain/models.rs` and `error.rs`.
 */

export type ErrorCode =
  | "AV-101"
  | "AV-102"
  | "AV-103"
  | "AV-104"
  | "AV-201"
  | "AV-202"
  | "AV-301"
  | "AV-401"
  | "AV-501";

export interface AppErrorPayload {
  code: ErrorCode;
  category: string;
  title: string;
  message: string;
  recovery_steps: string[];
  recoverable: boolean;
}

export type MediaKind = "video" | "audio";

export type Availability = "available" | "missing" | "unreadable" | "unsupported";

export interface MediaItem {
  id: string;
  path: string;
  display_name: string;
  media_kind: MediaKind;
  duration_ms: number | null;
  last_position_ms: number;
  completed: boolean;
  file_size_bytes: number | null;
  fingerprint: string | null;
  availability: Availability;
  added_at: string;
  last_opened_at: string | null;
  last_seen_at: string;
  note_count: number;
  stream_url?: string;
}

export type PlaybackState =
  | "idle"
  | "loading"
  | "playing"
  | "paused"
  | "seeking"
  | "ended"
  | "failed";

export type RepeatMode = "off" | "all" | "one";

export interface PlaybackSnapshot {
  generation: number;
  state: PlaybackState;
  media_id: string | null;
  position_ms: number;
  duration_ms: number | null;
  rate: number;
  volume: number;
  muted: boolean;
  repeat_mode: RepeatMode;
  shuffle: boolean;
  queue: string[];
  error: AppErrorPayload | null;
}

export type PlaybackCommand =
  | { action: "play" }
  | { action: "pause" }
  | { action: "toggle_play_pause" }
  | { action: "seek"; position_ms: number }
  | { action: "skip_relative"; delta_ms: number }
  | { action: "set_rate"; rate: number }
  | { action: "set_volume"; volume: number }
  | { action: "set_muted"; muted: boolean }
  | { action: "set_repeat"; mode: RepeatMode }
  | { action: "set_shuffle"; shuffle: boolean }
  | { action: "next" }
  | { action: "previous" }
  | { action: "stop" };

export interface Note {
  id: string;
  media_id: string;
  position_ms: number;
  is_quick_marker: boolean;
  body: string;
  created_at: string;
  updated_at: string;
}

export interface CreateNoteInput {
  media_id: string;
  position_ms: number;
  is_quick_marker: boolean;
  body: string;
}

export interface UpdateNoteInput {
  id: string;
  body: string;
}

export type LibraryFilter = "all" | "video" | "audio";

export interface LibrarySearchResult {
  media_items: MediaItem[];
  matching_notes: Note[];
}

export type ThemeMode = "dark" | "light" | "system";

export type TimestampPrecision = "nearest_second" | "milliseconds";

export type WindowMode = "full" | "minimum";

export interface AppSettings {
  theme: ThemeMode;
  resume_where_left_off: boolean;
  pause_on_headphones_disconnect: boolean;
  default_playback_speed: number;
  timestamp_precision: TimestampPrecision;
  window_mode: WindowMode;
  auto_check_updates?: boolean;
  github_release_repo?: string;
  installed_version?: string | null;
  dismissed_update_version?: string | null;
}

export interface ReleaseManifestDto {
  version: string;
  title: string;
  notes: string;
  package_size_bytes: number;
  sha256: string;
  published_at: string;
  download_url?: string;
}

export interface AppUpdateStatusDto {
  current_version: string;
  update_available: boolean;
  latest_version: string;
  release_title: string;
  release_notes: string;
  package_size_bytes: number;
  sha256_checksum: string;
  download_url: string;
  github_repo: string;
  verified: boolean;
  staged_for_restart: boolean;
  checked_at_unix_ms: number;
}

export interface CleanupSummaryDto {
  removed_paths: string[];
  bytes_reclaimed: number;
}

export type PrimarySection = "library" | "now_playing" | "settings";

export type SidePanelTab = "queue" | "notes";

export interface DiagnosticReport {
  app_version: string;
  os_family: string;
  arch: string;
  schema_version: number;
  media_count: number;
  missing_media_count: number;
  total_note_count: number;
  playback_state: PlaybackState;
  last_error_code: ErrorCode | null;
  generated_at: string;
}
