import { describe, expect, it } from "vitest";
import { AudoraIpcClient } from "../api/ipc";
import {
  computeProgressPercent,
  formatTimestamp,
  shouldSuppressGlobalShortcut,
} from "./format";

describe("formatTimestamp", () => {
  it("formats nearest_second and milliseconds accurately", () => {
    expect(formatTimestamp(222_000, "nearest_second")).toBe("03:42");
    expect(formatTimestamp(864_000, "nearest_second")).toBe("14:24");
    expect(formatTimestamp(4_328_000, "nearest_second")).toBe("1:12:08");
    expect(formatTimestamp(3_425, "milliseconds")).toBe("00:03.425");
  });
});

describe("computeProgressPercent", () => {
  it("calculates progress percentage and handles completed/not-started items", () => {
    expect(
      computeProgressPercent({
        completed: false,
        duration_ms: 100_000,
        last_position_ms: 0,
      }),
    ).toBe(0);
    expect(
      computeProgressPercent({
        completed: false,
        duration_ms: 100_000,
        last_position_ms: 62_000,
      }),
    ).toBe(62);
    expect(
      computeProgressPercent({
        completed: true,
        duration_ms: 100_000,
        last_position_ms: 20_000,
      }),
    ).toBe(100);
  });
});

describe("shouldSuppressGlobalShortcut", () => {
  it("suppresses shortcuts when focused in form inputs or when a modal is open", () => {
    expect(shouldSuppressGlobalShortcut("TEXTAREA", false, false)).toBe(true);
    expect(shouldSuppressGlobalShortcut("INPUT", false, false)).toBe(true);
    expect(shouldSuppressGlobalShortcut("DIV", false, true)).toBe(true);
    expect(shouldSuppressGlobalShortcut("BUTTON", false, false)).toBe(false);
  });
});

describe("AudoraIpcClient workflows", () => {
  it("enforces AV-301 on empty non-marker notes and supports quick markers + jump-to-moment", () => {
    const client = new AudoraIpcClient();
    expect(() =>
      client.createNote({
        media_id: "med_listening",
        position_ms: 15_000,
        is_quick_marker: false,
        body: "   ",
      }),
    ).toThrow();

    const marker = client.createNote({
      media_id: "med_listening",
      position_ms: 15_400,
      is_quick_marker: true,
      body: "",
    });
    expect(marker.is_quick_marker).toBe(true);
    expect(marker.position_ms).toBe(15_000);

    const snap = client.activateNote(marker.id);
    expect(snap.position_ms).toBe(15_000);
  });

  it("blocks duplicate relink paths with AV-201 and rejects unsupported formats with AV-104", () => {
    const client = new AudoraIpcClient();
    expect(() =>
      client.relinkMedia(
        "med_archive_missing",
        "/Users/local/Media/The Art of Listening.mp4",
      ),
    ).toThrow();

    expect(() => client.openMediaPath("Camera_Original.prores")).toThrow();
  });
});
