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
    const client = new AudoraIpcClient(false);
    expect(() =>
      client.relinkMedia(
        "med_archive_missing",
        "/Users/local/Media/The Art of Listening.mp4",
      ),
    ).toThrow();

    expect(() => client.openMediaPath("Camera_Original.prores")).toThrow();
  });

  it("prompts with AV-202 when relinking to a file with different duration or media kind, and preserves notes on confirm", () => {
    const client = new AudoraIpcClient(false);
    const notesBefore = client.listNotes("med_archive_missing");
    expect(notesBefore.length).toBe(4);

    // Relinking to a shorter cut without confirmation throws AV-202
    expect(() =>
      client.relinkMedia(
        "med_archive_missing",
        "/Users/local/Media/Sound Design Reel_short_cut.mp4",
      ),
    ).toThrow();

    // Confirming mismatch succeeds, marks available, and preserves all 4 notes
    const relinked = client.relinkMedia(
      "med_archive_missing",
      "/Users/local/Media/Sound Design Reel_short_cut.mp4",
      { confirmMismatch: true, probedDurationMs: 600_000 },
    );
    expect(relinked.availability).toBe("available");
    expect(relinked.duration_ms).toBe(600_000);

    const notesAfter = client.listNotes("med_archive_missing");
    expect(notesAfter.length).toBe(4);
  });

  it("supports batch local file open, queue repeat/shuffle, tickPlayback auto-advance, and headphone disconnect pause", () => {
    const client = new AudoraIpcClient(false);
    const opened = client.openLocalFiles([
      { name: "Session_01.mp4", size: 12_000_000, durationMs: 10_000 },
      { name: "Session_02.wav", size: 8_000_000, durationMs: 20_000 },
    ]);
    expect(opened.length).toBe(2);
    expect(client.getPlaybackSnapshot().media_id).toBe(opened[0].id);

    // Play and tick playback
    client.playbackCommand({ action: "play" });
    const afterTick = client.tickPlayback(1_000);
    expect(afterTick.position_ms).toBe(1_000);

    // Headphone disconnect pauses playback when enabled
    const afterDisconnect = client.handleAudioRouteDisconnected();
    expect(afterDisconnect.state).toBe("paused");

    // Repeat One loops from 0 on media ended
    client.playbackCommand({ action: "set_repeat", mode: "one" });
    client.playbackCommand({ action: "play" });
    const looped = client.handleMediaEnded();
    expect(looped.state).toBe("playing");
    expect(looped.position_ms).toBe(0);

    // Repeat Off advances to the next queued item (`opened[1]`)
    client.playbackCommand({ action: "set_repeat", mode: "off" });
    const advanced = client.handleMediaEnded();
    expect(advanced.media_id).toBe(opened[1].id);
    expect(advanced.state).toBe("playing");
  });

  it("searches media titles and note text offline and supports note edit/delete/restore", () => {
    const client = new AudoraIpcClient(false);
    const result = client.searchLibrary("ambience", "all");
    expect(result.media_items.length).toBe(1);
    expect(result.matching_notes.length).toBe(1);
    expect(result.matching_notes[0].body).toContain("three-layer ambience");

    const noteId = result.matching_notes[0].id;
    const updated = client.updateNote({
      id: noteId,
      body: "Updated three-layer ambience note.",
    });
    expect(updated.body).toBe("Updated three-layer ambience note.");

    const deleted = client.deleteNote(noteId);
    expect(deleted?.id).toBe(noteId);
    expect(client.listNotes("med_listening").length).toBe(3);

    if (deleted) {
      client.restoreNote(deleted);
    }
    expect(client.listNotes("med_listening").length).toBe(4);
  });
});
