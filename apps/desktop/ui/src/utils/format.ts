import type { MediaItem, TimestampPrecision } from "../types/domain";

/**
 * Formats integer milliseconds into `mm:ss`, `h:mm:ss`, or `mm:ss.SSS` depending on precision.
 */
export function formatTimestamp(
  ms: number | null | undefined,
  precision: TimestampPrecision = "nearest_second",
): string {
  if (ms === null || ms === undefined || !Number.isFinite(ms) || ms < 0) {
    return "00:00";
  }

  const normalizedMs =
    precision === "nearest_second" ? Math.round(ms / 1000) * 1000 : Math.floor(ms);

  const totalSeconds = Math.floor(normalizedMs / 1000);
  const hours = Math.floor(totalSeconds / 3600);
  const minutes = Math.floor((totalSeconds % 3600) / 60);
  const seconds = totalSeconds % 60;
  const remainderMs = normalizedMs % 1000;

  const mm = String(minutes).padStart(2, "0");
  const ss = String(seconds).padStart(2, "0");
  const base = hours > 0 ? `${hours}:${mm}:${ss}` : `${mm}:${ss}`;

  if (precision === "milliseconds") {
    return `${base}.${String(remainderMs).padStart(3, "0")}`;
  }
  return base;
}

export function formatDurationMs(
  ms: number | null | undefined,
  precision: TimestampPrecision = "nearest_second",
): string {
  if (ms === null || ms === undefined || !Number.isFinite(ms) || ms < 0) {
    return "--:--";
  }
  return formatTimestamp(ms, precision);
}

/**
 * Formats a playback timestamp into screen-reader spoken text (`aria-valuetext`).
 */
export function formatAriaTimestamp(ms: number | null | undefined): string {
  if (ms === null || ms === undefined || !Number.isFinite(ms) || ms < 0) {
    return "0 seconds";
  }
  const totalSeconds = Math.max(0, Math.round(ms / 1000));
  const hours = Math.floor(totalSeconds / 3600);
  const minutes = Math.floor((totalSeconds % 3600) / 60);
  const seconds = totalSeconds % 60;
  const parts: string[] = [];
  if (hours > 0) {
    parts.push(`${hours} ${hours === 1 ? "hour" : "hours"}`);
  }
  if (minutes > 0) {
    parts.push(`${minutes} ${minutes === 1 ? "minute" : "minutes"}`);
  }
  if (seconds > 0 || parts.length === 0) {
    parts.push(`${seconds} ${seconds === 1 ? "second" : "seconds"}`);
  }
  return parts.join(" ");
}

/**
 * Formats playback progress (`position` + optional `duration`) for `aria-valuetext` (`UI_DESIGN.md:161`).
 */
export function formatAriaProgress(
  posMs: number | null | undefined,
  durMs: number | null | undefined,
): string {
  const spokenPos = formatAriaTimestamp(posMs);
  if (durMs !== null && durMs !== undefined && Number.isFinite(durMs) && durMs > 0) {
    return `${spokenPos} of ${formatAriaTimestamp(durMs)}`;
  }
  return spokenPos;
}

/**
 * Formats persisted ISO-8601 UTC timestamps (or legacy display strings) into concise human labels.
 */
export function formatHumanDate(raw: string | null | undefined): string {
  if (!raw) return "";
  const trimmed = raw.trim();
  if (!trimmed) return "";
  if (trimmed === "Just now" || trimmed === "Yesterday") {
    return trimmed;
  }
  const numericEpoch = /^\d+$/.test(trimmed) ? Number.parseInt(trimmed, 10) : Number.NaN;
  const parsedMs = Number.isFinite(numericEpoch)
    ? numericEpoch < 1_000_000_000_000
      ? numericEpoch * 1000
      : numericEpoch
    : Date.parse(trimmed);
  if (!Number.isFinite(parsedMs)) {
    return trimmed;
  }
  const diffMs = Date.now() - parsedMs;
  if (diffMs >= -60_000 && diffMs < 60_000) {
    return "Just now";
  }
  if (diffMs >= 60_000 && diffMs < 3_600_000) {
    return `${Math.floor(diffMs / 60_000)}m ago`;
  }
  if (diffMs >= 3_600_000 && diffMs < 86_400_000) {
    return `${Math.floor(diffMs / 3_600_000)}h ago`;
  }
  const d = new Date(parsedMs);
  const yyyy = d.getFullYear();
  const mm = String(d.getMonth() + 1).padStart(2, "0");
  const dd = String(d.getDate()).padStart(2, "0");
  return `${yyyy}-${mm}-${dd}`;
}

export function progressPercent(
  positionMs: number,
  durationMs: number | null | undefined,
): number {
  if (!durationMs || durationMs <= 0 || positionMs <= 0) {
    return 0;
  }
  const clamped = Math.min(positionMs, durationMs);
  return Math.min(100, Math.floor((clamped * 100) / durationMs));
}

/**
 * Computes progress percentage (0..100) for a `MediaItem` (`Frame 4:206`).
 */
export function computeProgressPercent(
  item: Pick<MediaItem, "completed" | "duration_ms" | "last_position_ms">,
): number {
  if (item.completed) {
    return 100;
  }
  if (!item.duration_ms || item.duration_ms <= 0 || item.last_position_ms <= 0) {
    return 0;
  }
  const clamped = Math.min(item.last_position_ms, item.duration_ms);
  return Math.min(100, Math.floor((clamped * 100) / item.duration_ms));
}

/**
 * Returns true when single-key keyboard shortcuts (`Space`, `N`, `M`, arrows) MUST be suppressed
 * because focus is inside an editable form field or modal dialog (`docs/UI_DESIGN.md`).
 */
export function shouldSuppressGlobalShortcut(
  targetTagName: string | null | undefined,
  isContentEditable: boolean,
  isModalOpen: boolean,
  inputType?: string | null,
): boolean {
  if (isModalOpen || isContentEditable) {
    return true;
  }
  if (!targetTagName) {
    return false;
  }
  const normalized = targetTagName.toUpperCase();
  if (normalized === "INPUT") {
    const t = (inputType ?? "").toLowerCase();
    if (t === "range" || t === "checkbox" || t === "radio" || t === "button") {
      return false;
    }
    return true;
  }
  return normalized === "TEXTAREA" || normalized === "SELECT";
}
