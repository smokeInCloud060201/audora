import type { MediaItem, TimestampPrecision } from "../types/domain";

/**
 * Formats integer milliseconds into `mm:ss`, `h:mm:ss`, or `mm:ss.SSS` depending on precision.
 */
export function formatTimestamp(
  ms: number | null | undefined,
  precision: TimestampPrecision = "nearest_second",
): string {
  if (ms === null || ms === undefined || Number.isNaN(ms) || ms < 0) {
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
  return formatTimestamp(ms, precision);
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
export function computeProgressPercent(item: Pick<MediaItem, "completed" | "duration_ms" | "last_position_ms">): number {
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
): boolean {
  if (isModalOpen || isContentEditable) {
    return true;
  }
  if (!targetTagName) {
    return false;
  }
  const normalized = targetTagName.toUpperCase();
  return normalized === "INPUT" || normalized === "TEXTAREA" || normalized === "SELECT";
}
