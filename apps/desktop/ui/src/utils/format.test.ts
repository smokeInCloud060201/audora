import { describe, expect, it } from "vitest";
import { AudoraIpcClient, DEFAULT_GITHUB_RELEASE_REPO } from "../api/ipc";
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
  it("starts with an empty library and idle playback state with zero mock data", () => {
    const client = new AudoraIpcClient(false);
    const search = client.searchLibrary("", "all");
    expect(search.media_items).toEqual([]);
    expect(search.matching_notes).toEqual([]);

    const snap = client.getPlaybackSnapshot();
    expect(snap.state).toBe("idle");
    expect(snap.media_id).toBeNull();
    expect(snap.position_ms).toBe(0);
    expect(snap.duration_ms).toBeNull();
    expect(snap.queue).toEqual([]);
  });

  it("enforces AV-301 on empty non-marker notes and supports quick markers + jump-to-moment", () => {
    const client = new AudoraIpcClient(false);
    const media = client.openMediaPath("/tmp/Lecture_01.mp4", { durationMs: 120_000 });

    expect(() =>
      client.createNote({
        media_id: media.id,
        position_ms: 15_000,
        is_quick_marker: false,
        body: "   ",
      }),
    ).toThrow();

    const marker = client.createNote({
      media_id: media.id,
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
    const existing = client.openMediaPath("/tmp/Existing_Session.mp4", {
      durationMs: 90_000,
    });
    const missing = client.openMediaPath("/tmp/Moved_Archive.mp4", {
      durationMs: 60_000,
    });
    client.markMediaMissing(missing.id);

    expect(() => client.relinkMedia(missing.id, existing.path)).toThrow();
    expect(() => client.openMediaPath("Camera_Original.prores")).toThrow();
  });

  it("prompts with AV-202 when relinking to a file with different duration or media kind, and preserves notes on confirm", () => {
    const client = new AudoraIpcClient(false);
    const item = client.openMediaPath("/tmp/Interview_Reel.mp4", {
      durationMs: 1_200_000,
    });
    client.createNote({
      media_id: item.id,
      position_ms: 60_000,
      is_quick_marker: false,
      body: "First timestamped observation",
    });
    client.createNote({
      media_id: item.id,
      position_ms: 180_000,
      is_quick_marker: false,
      body: "Second timestamped observation",
    });
    client.markMediaMissing(item.id);

    expect(client.listNotes(item.id).length).toBe(2);

    // Relinking to a shorter cut without confirmation throws AV-202
    expect(() =>
      client.relinkMedia(item.id, "/tmp/Interview_Reel_short_cut.mp4"),
    ).toThrow();

    // Confirming mismatch succeeds, marks available, and preserves all notes
    const relinked = client.relinkMedia(
      item.id,
      "/tmp/Interview_Reel_short_cut.mp4",
      { confirmMismatch: true, probedDurationMs: 600_000 },
    );
    expect(relinked.availability).toBe("available");
    expect(relinked.duration_ms).toBe(600_000);

    const notesAfter = client.listNotes(item.id);
    expect(notesAfter.length).toBe(2);
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
    const media = client.openMediaPath("/tmp/Acoustic_Study.flac", {
      durationMs: 300_000,
    });
    const created = client.createNote({
      media_id: media.id,
      position_ms: 42_000,
      is_quick_marker: false,
      body: "Reference: the three-layer ambience technique.",
    });

    const result = client.searchLibrary("ambience", "all");
    expect(result.media_items.length).toBe(1);
    expect(result.matching_notes.length).toBe(1);
    expect(result.matching_notes[0].body).toContain("three-layer ambience");

    const updated = client.updateNote({
      id: created.id,
      body: "Updated three-layer ambience note.",
    });
    expect(updated.body).toBe("Updated three-layer ambience note.");

    const deleted = client.deleteNote(created.id);
    expect(deleted?.id).toBe(created.id);
    expect(client.listNotes(media.id).length).toBe(0);

    if (deleted) {
      client.restoreNote(deleted);
    }
    expect(client.listNotes(media.id).length).toBe(1);
  });

  it("blocks remote network URLs with AV-301 and generates a redacted diagnostic report without file paths or note bodies", () => {
    const client = new AudoraIpcClient(false);
    const active = client.openMediaPath("/tmp/Private_Recording.wav", {
      durationMs: 180_000,
    });
    const missing = client.openMediaPath("/tmp/Disconnected_Drive_Clip.mp4", {
      durationMs: 90_000,
    });
    client.markMediaMissing(missing.id);
    client.createNote({
      media_id: active.id,
      position_ms: 12_000,
      is_quick_marker: false,
      body: "Confidential studio session note",
    });

    expect(() => client.openMediaPath("https://example.com/remote.mp4")).toThrow();
    expect(() =>
      client.relinkMedia(missing.id, "rtsp://camera.local/stream"),
    ).toThrow();

    const backupPath = client.createManualBackup();
    expect(backupPath).toContain(".sqlite.manual.bak");

    const report = client.generateDiagnosticReport();
    expect(report.media_count).toBe(2);
    expect(report.missing_media_count).toBe(1);
    expect(report.total_note_count).toBe(1);
    const json = JSON.stringify(report);
    expect(json).not.toContain("Private_Recording");
    expect(json).not.toContain("Confidential studio");
  });

  it("stages GitHub release JSON, respects dismiss vs manual check, installs update, and cleans up app data", async () => {
    const client = new AudoraIpcClient(false);
    const initial = client.checkForUpdates();
    expect(initial.current_version).toBe("0.1.0");
    expect(initial.github_repo).toBe(DEFAULT_GITHUB_RELEASE_REPO);
    expect(initial.update_available).toBe(false);

    const staged = client.stageGitHubReleaseJson(
      JSON.stringify({
        tag_name: "v0.2.0",
        name: "Audora 0.2.0 is ready",
        body: "### Highlights\n- Native self-updater parity with **Readora**\n- Signed `Audora.app.tar.gz` bundle",
        published_at: "2026-11-12T10:00:00Z",
        html_url: "https://github.com/smokeInCloud060201/audora/releases/tag/v0.2.0",
        assets: [
          {
            name: "Audora_0.2.0_macos-aarch64.app.tar.gz",
            browser_download_url:
              "https://github.com/smokeInCloud060201/audora/releases/download/v0.2.0/Audora_0.2.0_macos-aarch64.app.tar.gz",
            size: 16_777_216,
            digest:
              "sha256:abcdef0123456789abcdef0123456789abcdef0123456789abcdef0123456789",
          },
        ],
      }),
    );
    expect(staged.update_available).toBe(true);
    expect(staged.latest_version).toBe("0.2.0");
    expect(staged.verified).toBe(true);

    const dismissed = client.dismissUpdate("0.2.0");
    expect(dismissed.update_available).toBe(false);
    expect(client.checkForUpdates(true).update_available).toBe(true);

    const { status } = await client.installNativeUpdate("0.2.0");
    expect(status.current_version).toBe("0.2.0");
    expect(status.staged_for_restart).toBe(true);
    expect(client.getCurrentVersion()).toBe("0.2.0");

    const cleanup = client.cleanupApplicationData();
    expect(cleanup.removed_paths.length).toBeGreaterThan(0);
    expect(client.getCurrentVersion()).toBe("0.1.0");
  });
});
