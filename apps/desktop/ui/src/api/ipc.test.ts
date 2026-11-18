import { afterEach, describe, expect, it, vi } from "vitest";
import type { AppErrorPayload } from "../types/domain";
import { AudoraIpcClient } from "./ipc";

describe("AudoraIpcClient queue auto-advance on playback completion", () => {
  it("prevents double-advance on M4A/MP4 when timeupdate reaches container duration right before onEnded", () => {
    const client = new AudoraIpcClient(false);
    const [track1, track2, track3] = client.openLocalFiles([
      { name: "01_Intro.m4a", path: "/tmp/01_Intro.m4a", size: 2_000_000, durationMs: 10_000 },
      { name: "02_Body.m4a", path: "/tmp/02_Body.m4a", size: 4_000_000, durationMs: 25_000 },
      { name: "03_Outro.m4a", path: "/tmp/03_Outro.m4a", size: 3_000_000, durationMs: 15_000 },
    ]);

    client.playbackCommand({ action: "play" });
    const startSnap = client.getPlaybackSnapshot();
    expect(startSnap.media_id).toBe(track1!.id);
    expect(startSnap.state).toBe("playing");

    // 1. Browser fires `timeupdate` at 10_000ms (authoritative ISO-BMFF container duration)
    const afterTimeUpdate = client.syncMediaElementPosition(
      10_000,
      10_000,
      track1!.id,
      startSnap.generation,
    );
    expect(afterTimeUpdate.media_id).toBe(track2!.id);
    expect(afterTimeUpdate.state).toBe("playing");
    expect(afterTimeUpdate.position_ms).toBe(0);
    expect(client.getMediaById(track1!.id)?.completed).toBe(true);

    // 2. Microseconds later, the outgoing <audio> element fires `ended` with Track 1's id & generation -> MUST be ignored!
    const afterDuplicateEnded = client.handleMediaEnded(track1!.id, startSnap.generation);
    expect(afterDuplicateEnded.media_id).toBe(track2!.id);
    expect(afterDuplicateEnded.state).toBe("playing");
    expect(afterDuplicateEnded.position_ms).toBe(0);
    expect(client.getMediaById(track2!.id)?.completed).toBe(false);

    // 3. Stale `timeupdate` from Track 1 must also be ignored
    const afterStaleTimeUpdate = client.syncMediaElementPosition(
      10_000,
      10_000,
      track1!.id,
      startSnap.generation,
    );
    expect(afterStaleTimeUpdate.media_id).toBe(track2!.id);
    expect(afterStaleTimeUpdate.position_ms).toBe(0);

    // 4. Track 2 finishes -> advances to Track 3
    const track2Gen = afterDuplicateEnded.generation;
    const afterTrack2 = client.handleMediaEnded(track2!.id, track2Gen);
    expect(afterTrack2.media_id).toBe(track3!.id);
    expect(afterTrack2.state).toBe("playing");
    expect(afterTrack2.position_ms).toBe(0);

    // 5. Track 3 (final track, Repeat Off) finishes -> enters `ended` state and ignores duplicate `ended`
    const afterTrack3 = client.handleMediaEnded(track3!.id, afterTrack2.generation);
    expect(afterTrack3.media_id).toBe(track3!.id);
    expect(afterTrack3.state).toBe("ended");
    expect(afterTrack3.position_ms).toBe(15_000);

    const afterDuplicateFinalEnded = client.handleMediaEnded(track3!.id, afterTrack2.generation);
    expect(afterDuplicateFinalEnded.state).toBe("ended");
  });

  it("skips missing/unavailable items mid-queue and starts queued tracks from 00:00 while preserving direct Library resume", () => {
    const client = new AudoraIpcClient(false);
    const [t1, t2, t3] = client.openLocalFiles([
      { name: "Track1.mp3", path: "/tmp/Track1.mp3", size: 1_000_000, durationMs: 60_000 },
      { name: "Track2.mp3", path: "/tmp/Track2.mp3", size: 1_000_000, durationMs: 60_000 },
      { name: "Track3.mp3", path: "/tmp/Track3.mp3", size: 1_000_000, durationMs: 60_000 },
    ]);

    // Partially play Track 3 to 30_000ms, then return to Track 1
    client.selectMediaById(t3!.id, { autoPlay: true });
    client.playbackCommand({ action: "seek", position_ms: 30_000 });
    client.playbackCommand({ action: "pause" });
    expect(client.getMediaById(t3!.id)?.last_position_ms).toBe(30_000);

    // Mark Track 2 as missing
    client.markMediaMissing(t2!.id);

    // Select Track 1 and play to completion -> must skip missing Track 2 and auto-play Track 3 from 00:00
    client.selectMediaById(t1!.id, { autoPlay: true });
    const activeSnap = client.getPlaybackSnapshot();
    const nextSnap = client.handleMediaEnded(t1!.id, activeSnap.generation);
    expect(nextSnap.media_id).toBe(t3!.id);
    expect(nextSnap.state).toBe("playing");
    expect(nextSnap.position_ms).toBe(0);

    // Direct Library selection still honors `resume_where_left_off` unless within 500ms of EOF
    client.playbackCommand({ action: "seek", position_ms: 22_000 });
    client.selectMediaById(t1!.id);
    client.selectMediaById(t3!.id);
    expect(client.getPlaybackSnapshot().position_ms).toBe(22_000);

    client.playbackCommand({ action: "seek", position_ms: 59_800 });
    client.selectMediaById(t1!.id);
    client.selectMediaById(t3!.id);
    expect(client.getPlaybackSnapshot().position_ms).toBe(0);
  });

  it("wraps around on Repeat All, loops current track with incremented generation on Repeat One, and restarts from 0 after Ended", () => {
    const client = new AudoraIpcClient(false);
    const [t1, t2] = client.openLocalFiles([
      { name: "A.wav", path: "/tmp/A.wav", size: 500_000, durationMs: 8_000 },
      { name: "B.wav", path: "/tmp/B.wav", size: 500_000, durationMs: 12_000 },
    ]);

    // Repeat One increments generation and stays playing at 0ms
    client.playbackCommand({ action: "set_repeat", mode: "one" });
    client.playbackCommand({ action: "play" });
    const genBefore = client.getPlaybackSnapshot().generation;
    const looped = client.handleMediaEnded(t1!.id, genBefore);
    expect(looped.media_id).toBe(t1!.id);
    expect(looped.state).toBe("playing");
    expect(looped.position_ms).toBe(0);
    expect(looped.generation).toBeGreaterThan(genBefore);

    // Duplicate ended with prior generation is ignored even on Repeat One
    const ignoredRepeatEnded = client.handleMediaEnded(t1!.id, genBefore);
    expect(ignoredRepeatEnded.generation).toBe(looped.generation);

    // Repeat All wraps from last item (`t2`) back to `t1`
    client.playbackCommand({ action: "set_repeat", mode: "all" });
    const toSecond = client.handleMediaEnded(t1!.id, looped.generation);
    expect(toSecond.media_id).toBe(t2!.id);
    expect(toSecond.state).toBe("playing");

    const wrappedToFirst = client.handleMediaEnded(t2!.id, toSecond.generation);
    expect(wrappedToFirst.media_id).toBe(t1!.id);
    expect(wrappedToFirst.state).toBe("playing");
    expect(wrappedToFirst.position_ms).toBe(0);

    // Repeat Off ends at `t2`, and TogglePlayPause restarts from 0ms
    client.playbackCommand({ action: "set_repeat", mode: "off" });
    const step2 = client.handleMediaEnded(t1!.id, wrappedToFirst.generation);
    const finalEnded = client.handleMediaEnded(t2!.id, step2.generation);
    expect(finalEnded.state).toBe("ended");
    expect(finalEnded.position_ms).toBe(12_000);

    const replayed = client.playbackCommand({ action: "toggle_play_pause" });
    expect(replayed.state).toBe("playing");
    expect(replayed.position_ms).toBe(0);
    expect(replayed.generation).toBeGreaterThan(finalEnded.generation);
  });

  afterEach(() => {
    vi.restoreAllMocks();
    delete (globalThis as { window?: unknown }).window;
  });

  it("handles NativeDroppedEntry objects directly without redundant probes, rejects 0-byte files with AV-103, and scans dropped directories", async () => {
    const fetchSpy = vi.fn();
    vi.stubGlobal("fetch", fetchSpy);

    const client = new AudoraIpcClient(false);
    const res = await client.ingestNativeDroppedPaths([
      {
        kind: "file",
        path: "/Users/demo/Clips/Take_01.mp4",
        name: "Take_01.mp4",
        size: 14_500_000,
        duration_ms: 45_200,
      },
      {
        path: "/Users/demo/Clips/VoiceMemo.m4a",
        name: "VoiceMemo.m4a",
        size: 2_100_000,
        duration_ms: 18_000,
      },
    ]);

    expect(fetchSpy).not.toHaveBeenCalled();
    expect(res.openedItems).toHaveLength(2);
    expect(res.openedItems[0]?.file_size_bytes).toBe(14_500_000);
    expect(res.openedItems[0]?.duration_ms).toBe(45_200);
    expect(res.openedItems[0]?.duration_probed).toBe(true);

    // 0-byte dropped file throws AV-103 with recovery steps and firstFileName
    await expect(
      client.ingestNativeDroppedPaths([
        {
          kind: "file",
          path: "/Users/demo/Clips/CorruptZero.mp4",
          name: "CorruptZero.mp4",
          size: 0,
          duration_ms: null,
        },
      ]),
    ).rejects.toMatchObject({
      code: "AV-103",
      category: "MediaCorrupt",
      title: "Empty file",
      firstFileName: "CorruptZero.mp4",
    });

    // Unsupported dropped file throws AV-104 with firstFileName
    await expect(
      client.ingestNativeDroppedPaths([
        {
          kind: "file",
          path: "/Users/demo/Clips/Archive.prores",
          name: "Archive.prores",
          size: 1024,
        },
      ]),
    ).rejects.toMatchObject({
      code: "AV-104",
      firstFileName: "Archive.prores",
    });

    // Directory entry delegates to scanAndAddSourceFolderPath
    const dirRes = await client.ingestNativeDroppedPaths([
      { kind: "dir", path: "/Users/demo/MusicFolder" },
    ]);
    expect(dirRes.openedItems).toHaveLength(0);
    expect(client.getSourceFolders().some((f) => f.path === "/Users/demo/MusicFolder")).toBe(
      true,
    );
  });

  it("visits every playable item once on Shuffle + Repeat Off then ends, wraps on Repeat All, and avoids double-skipping in playPlaylist", () => {
    const client = new AudoraIpcClient(false);
    const tracks = client.openLocalFiles([
      { name: "S1.mp3", path: "/tmp/S1.mp3", size: 1000, durationMs: 10_000 },
      { name: "S2.mp3", path: "/tmp/S2.mp3", size: 1000, durationMs: 10_000 },
      { name: "S3.mp3", path: "/tmp/S3.mp3", size: 1000, durationMs: 10_000 },
      { name: "S4.mp3", path: "/tmp/S4.mp3", size: 1000, durationMs: 10_000 },
    ]);

    client.playbackCommand({ action: "set_repeat", mode: "off" });
    client.playbackCommand({ action: "set_shuffle", shuffle: true });
    client.playbackCommand({ action: "play" });

    const playedIds: string[] = [];
    for (let step = 0; step < 10; step += 1) {
      const snap = client.getPlaybackSnapshot();
      playedIds.push(snap.media_id!);
      const afterEnd = client.handleMediaEnded(snap.media_id!, snap.generation);
      if (afterEnd.state === "ended") {
        break;
      }
    }

    expect(playedIds).toHaveLength(4);
    expect(new Set(playedIds).size).toBe(4);
    expect(client.getPlaybackSnapshot().state).toBe("ended");

    // Repeat All wraps around after visiting all 4 tracks
    client.selectMediaById(tracks[0]!.id, { autoPlay: true, startFromBeginning: true });
    client.playbackCommand({ action: "set_repeat", mode: "all" });
    const passIds: string[] = [];
    for (let i = 0; i < 5; i += 1) {
      const snap = client.getPlaybackSnapshot();
      passIds.push(snap.media_id!);
      const next = client.handleMediaEnded(snap.media_id!, snap.generation);
      expect(next.state).toBe("playing");
    }
    expect(new Set(passIds.slice(0, 4)).size).toBe(4);

    // playPlaylist with shuffle=true plays exact queue order without double-skipping and ends on Repeat Off
    const pl = client.createPlaylist({
      name: "Shuffle Set",
      initial_item_ids: tracks.map((t) => t.id),
    });
    client.playbackCommand({ action: "set_repeat", mode: "off" });
    const plSnap = client.playPlaylist(pl.id, { shuffle: true });
    const expectedOrder = [...plSnap.queue];
    const actualOrder: string[] = [plSnap.media_id!];
    let curr = plSnap;
    while (curr.state !== "ended") {
      curr = client.handleMediaEnded(curr.media_id!, curr.generation);
      if (curr.state !== "ended") {
        actualOrder.push(curr.media_id!);
      }
    }
    expect(actualOrder).toEqual(expectedOrder);

    // Disabling shuffle restores the original unshuffled playlist order
    const unshuffled = client.playbackCommand({ action: "set_shuffle", shuffle: false });
    expect(unshuffled.queue).toEqual(tracks.map((t) => t.id));
  });

  it("resets unprobed duration on production relink, formats old vs new AV-202 details, and upgrades audio-only video containers", () => {
    const store = new Map<string, string>();
    vi.stubGlobal("window", {
      location: { hostname: "localhost" },
      localStorage: {
        getItem: (k: string) => store.get(k) ?? null,
        setItem: (k: string, v: string) => {
          store.set(k, v);
        },
        removeItem: (k: string) => {
          store.delete(k);
        },
      },
    });

    const prodClient = new AudoraIpcClient(true);
    const original = prodClient.openMediaPath("/tmp/Lecture_Short.mp4", {
      size: 5_000_000,
      durationMs: 60_000,
    });
    expect(original.duration_probed).toBe(true);
    prodClient.markMediaMissing(original.id);

    // Relink without probedDurationMs in production resets duration_ms=null & duration_probed=false
    const relinkedUnprobed = prodClient.relinkMedia(
      original.id,
      "/tmp/Lecture_Extended_2h.mp4",
      { fileSizeBytes: 55_000_000 },
    );
    expect(relinkedUnprobed.duration_ms).toBeNull();
    expect(relinkedUnprobed.duration_probed).toBe(false);
    expect(relinkedUnprobed.file_size_bytes).toBe(55_000_000);

    // Subsequent <video> metadata load with 180_000ms adopts the real longer duration instead of cutting off at 60s!
    prodClient.playbackCommand({ action: "play" });
    const synced = prodClient.syncMediaElementPosition(90_000, 180_000);
    expect(synced.state).toBe("playing");
    expect(synced.duration_ms).toBe(180_000);
    expect(synced.position_ms).toBe(90_000);

    // AV-202 message clearly displays old vs new media type and duration
    prodClient.markMediaMissing(original.id);
    let mismatchErr: AppErrorPayload | null = null;
    try {
      prodClient.relinkMedia(original.id, "/tmp/Lecture_Audio_Edit.m4a", {
        probedDurationMs: 120_000,
      });
    } catch (err) {
      mismatchErr = err as AppErrorPayload;
    }
    expect(mismatchErr?.code).toBe("AV-202");
    expect(mismatchErr?.message).toContain("video → audio");
    expect(mismatchErr?.message).toContain("03:00 → 02:00");
    expect(mismatchErr?.recovery_steps[0]).toContain("duration");

    // Audio-only .mp4 container upgrades cleanly to media_kind === "audio"
    const upgraded = prodClient.upgradeMediaKindToAudio(original.id);
    expect(upgraded?.media_kind).toBe("audio");
    expect(prodClient.getMediaById(original.id)?.media_kind).toBe("audio");
  });

  it("blocks network/file/WebDAV paths, prevents cross-OS/arch update asset mismatches, pre-validates openLocalFiles batches, clears completed on rewind, and preserves custom queues", () => {
    const client = new AudoraIpcClient(false);
    for (const blockedPath of [
      "smb://nas.local/share/track.mp3",
      "dav://webdav.local/clip.mp4",
      "davs://webdav.local/clip.mp4",
      "nfs://storage.local/export/a.wav",
      "afp://macserver.local/vol/b.flac",
      "file:///Users/demo/Desktop/video.mp4",
      "\\\\evil.example.com@SSL\\DavWWWRoot\\payload.mp4",
      "\\\\fileserver\\davwwwroot\\clip.mp3",
    ]) {
      expect(() => client.openMediaPath(blockedPath)).toThrow();
    }

    // Batch open with an unsupported 2nd file leaves library untouched (0 partial mutations)
    expect(() =>
      client.openLocalFiles([
        { name: "Good.mp4", size: 1000 },
        { name: "Bad.prores", size: 1000 },
      ]),
    ).toThrow();
    expect(client.searchLibrary("", "all").media_items).toHaveLength(0);

    // Rewinding via skip_relative or previous clears completed = false
    const [t1] = client.openLocalFiles([
      { name: "Clip.mp3", path: "/tmp/Clip.mp3", size: 1000, durationMs: 10_000 },
    ]);
    client.playbackCommand({ action: "play" });
    client.handleMediaEnded(t1!.id, client.getPlaybackSnapshot().generation);
    expect(client.getMediaById(t1!.id)?.completed).toBe(true);

    client.playbackCommand({ action: "skip_relative", delta_ms: -5_000 });
    expect(client.getMediaById(t1!.id)?.completed).toBe(false);

    client.playbackCommand({ action: "play" });
    client.handleMediaEnded(t1!.id, client.getPlaybackSnapshot().generation);
    expect(client.getMediaById(t1!.id)?.completed).toBe(true);
    client.playbackCommand({ action: "previous" });
    expect(client.getMediaById(t1!.id)?.completed).toBe(false);

    // stageGitHubReleaseJson never picks .exe or wrong-arch assets on macOS aarch64
    const windowsOnly = client.stageGitHubReleaseJson(
      JSON.stringify({
        tag_name: "v2.0.0",
        assets: [
          {
            name: "Audora_2.0.0_windows-x86_64-setup.exe",
            browser_download_url: "https://example.com/Audora-setup.exe",
            size: 12_000_000,
            digest:
              "sha256:2222222222222222222222222222222222222222222222222222222222222222",
          },
          {
            name: "Audora_2.0.0_macos-x86_64.dmg",
            browser_download_url: "https://example.com/Audora-x86_64.dmg",
            size: 15_000_000,
            digest:
              "sha256:3333333333333333333333333333333333333333333333333333333333333333",
          },
        ],
      }),
    );
    expect(windowsOnly.verified).toBe(false);
    expect(windowsOnly.update_available).toBe(false);
    expect(windowsOnly.download_url).not.toContain(".exe");
    expect(windowsOnly.download_url).not.toContain("x86_64.dmg");

    // Restoring persisted custom queue does NOT append every other library item behind it
    const store = new Map<string, string>();
    store.set(
      "audora.state.v2",
      JSON.stringify({
        media: [
          {
            id: "med_custom_1",
            path: "/tmp/One.mp3",
            display_name: "One.mp3",
            media_kind: "audio",
            duration_ms: 30_000,
            last_position_ms: 0,
            completed: false,
            file_size_bytes: 1000,
            fingerprint: null,
            availability: "available",
            added_at: "2026-01-01T00:00:00Z",
            last_opened_at: "2026-01-01T00:00:00Z",
            last_seen_at: "2026-01-01T00:00:00Z",
            note_count: 0,
          },
          {
            id: "med_custom_2",
            path: "/tmp/Two.mp3",
            display_name: "Two.mp3",
            media_kind: "audio",
            duration_ms: 30_000,
            last_position_ms: 0,
            completed: false,
            file_size_bytes: 1000,
            fingerprint: null,
            availability: "available",
            added_at: "2026-01-01T00:00:00Z",
            last_opened_at: "2026-01-01T00:00:00Z",
            last_seen_at: "2026-01-01T00:00:00Z",
            note_count: 0,
          },
          {
            id: "med_unqueued_3",
            path: "/tmp/Three.mp3",
            display_name: "Three.mp3",
            media_kind: "audio",
            duration_ms: 30_000,
            last_position_ms: 0,
            completed: false,
            file_size_bytes: 1000,
            fingerprint: null,
            availability: "available",
            added_at: "2026-01-01T00:00:00Z",
            last_opened_at: "2026-01-01T00:00:00Z",
            last_seen_at: "2026-01-01T00:00:00Z",
            note_count: 0,
          },
        ],
        notes: [],
        playlists: [],
        settings: client.getSettings(),
        playback_session: {
          active_media_id: "med_custom_1",
          queue: ["med_custom_1", "med_custom_2"],
          volume: 0.8,
          muted: false,
          rate: 1.0,
          repeat_mode: "off",
          shuffle: false,
        },
      }),
    );
    vi.stubGlobal("window", {
      location: { hostname: "example.local" },
      localStorage: {
        getItem: (k: string) => store.get(k) ?? null,
        setItem: (k: string, v: string) => {
          store.set(k, v);
        },
        removeItem: (k: string) => {
          store.delete(k);
        },
      },
    });
    const hydratedClient = new AudoraIpcClient(true);
    expect(hydratedClient.getPlaybackSnapshot().queue).toEqual([
      "med_custom_1",
      "med_custom_2",
    ]);
  });

  it("preserves localStorage when SQLite seed fails, surfaces AV-401 on failed saveNoteVerified, and fences in-flight RPCs on cleanup", async () => {
    const store = new Map<string, string>();
    store.set(
      "audora.state.v2",
      JSON.stringify({
        media: [],
        notes: [],
        playlists: [],
        settings: {
          theme: "light",
          resume_where_left_off: true,
          pause_on_headphones_disconnect: true,
          default_playback_speed: 1.5,
          timestamp_precision: "milliseconds",
          window_mode: "full",
        },
      }),
    );

    let postStateShouldSucceed = false;
    let rpcShouldSucceed = false;
    const callOrder: string[] = [];

    vi.stubGlobal("navigator", { userAgent: "Mozilla/5.0 AppleWebKit/605.1.15" });
    vi.stubGlobal("window", {
      __AUDORA_SESSION_TOKEN__: "tok_test_123",
      location: { hostname: "127.0.0.1" },
      localStorage: {
        getItem: (k: string) => store.get(k) ?? null,
        setItem: (k: string, v: string) => {
          store.set(k, v);
        },
        removeItem: (k: string) => {
          store.delete(k);
        },
      },
    });

    vi.stubGlobal(
      "fetch",
      vi.fn(async (url: string, init?: RequestInit) => {
        const method = init?.method ?? "GET";
        callOrder.push(`${method} ${url}`);
        if (url === "/__audora_state" && method === "GET") {
          return new Response("{}", {
            status: 200,
            headers: { "Content-Type": "application/json" },
          });
        }
        if (url === "/__audora_state" && method === "POST") {
          return new Response(JSON.stringify({ ok: postStateShouldSucceed }), {
            status: postStateShouldSucceed ? 200 : 500,
            headers: { "Content-Type": "application/json" },
          });
        }
        if (url === "/__audora_state" && method === "DELETE") {
          return new Response(
            JSON.stringify({ ok: true, removed_paths: ["/tmp/audora.sqlite"], bytes_reclaimed: 4096 }),
            { status: 200, headers: { "Content-Type": "application/json" } },
          );
        }
        if (url === "/__audora_rpc") {
          return new Response(JSON.stringify({ ok: rpcShouldSucceed }), {
            status: rpcShouldSucceed ? 200 : 500,
            headers: { "Content-Type": "application/json" },
          });
        }
        return new Response("{}", { status: 404 });
      }),
    );

    const client = new AudoraIpcClient(true);
    const persistenceErrors: AppErrorPayload[] = [];
    client.onPersistenceError = (err) => persistenceErrors.push(err);

    // 1. Seed of non-default settings fails (500) -> localStorage MUST NOT be deleted!
    await client.ensureHydratedFromNative();
    expect(store.has("audora.state.v2")).toBe(true);

    // 2. saveNoteVerified fails on /__audora_rpc -> rejects with AV-401, triggers onPersistenceError, and leaves 0 saved notes
    rpcShouldSucceed = true;
    const media = client.openMediaPath("/tmp/Session.mp3", { size: 2048, durationMs: 30_000 });
    await client.generateDiagnosticReportNative(); // flushes openMediaPath RPC

    rpcShouldSucceed = false;
    await expect(
      client.saveNoteVerified({
        mode: "create",
        media_id: media.id,
        position_ms: 5_000,
        is_quick_marker: false,
        body: "Draft thought that must stay in composer",
      }),
    ).rejects.toMatchObject({ code: "AV-401" });
    expect(client.listNotes(media.id)).toHaveLength(0);
    expect(persistenceErrors.some((e) => e.code === "AV-401")).toBe(true);

    // 3. cleanupApplicationDataNative flushes in-flight RPC before DELETE /__audora_state and skips empty save_session on exit
    rpcShouldSucceed = true;
    postStateShouldSucceed = true;
    callOrder.length = 0;
    client.playbackCommand({ action: "set_volume", volume: 0.5 });
    await client.cleanupApplicationDataNative();
    const rpcIdx = callOrder.indexOf("POST /__audora_rpc");
    const delIdx = callOrder.indexOf("DELETE /__audora_state");
    expect(delIdx).toBeGreaterThanOrEqual(0);
    if (rpcIdx >= 0) {
      expect(rpcIdx).toBeLessThan(delIdx);
    }

    const quitWin = globalThis.window as unknown as {
      __AUDORA_COLLECT_QUIT_BATCH__?: () => string;
    };
    expect(JSON.parse(quitWin.__AUDORA_COLLECT_QUIT_BATCH__!())).toEqual([]);
  });

  it("fences Event::Opened drops behind GET /__audora_state hydration, merges pre-hydration settings, and refuses to overwrite SQLite on 500", async () => {
    const store = new Map<string, string>();
    vi.stubGlobal("navigator", { userAgent: "Mozilla/5.0 AppleWebKit/605.1.15" });
    vi.stubGlobal("window", {
      __AUDORA_SESSION_TOKEN__: "tok_hydration_race",
      location: { hostname: "127.0.0.1" },
      localStorage: {
        getItem: (k: string) => store.get(k) ?? null,
        setItem: (k: string, v: string) => {
          store.set(k, v);
        },
        removeItem: (k: string) => {
          store.delete(k);
        },
      },
    });

    // Case 1: GET /__audora_state returns 500 -> must NOT POST /__audora_state or save_settings
    const errCalls: string[] = [];
    vi.stubGlobal(
      "fetch",
      vi.fn(async (url: string, init?: RequestInit) => {
        const method = init?.method ?? "GET";
        errCalls.push(`${method} ${url}`);
        if (url === "/__audora_state" && method === "GET") {
          return new Response(JSON.stringify({ ok: false }), {
            status: 500,
            headers: { "Content-Type": "application/json" },
          });
        }
        return new Response(JSON.stringify({ ok: true }), {
          status: 200,
          headers: { "Content-Type": "application/json" },
        });
      }),
    );

    const errClient = new AudoraIpcClient(true);
    await errClient.ensureHydratedFromNative();
    errClient.updateSettings({ ...errClient.getSettings(), theme: "light" });
    await new Promise((r) => setTimeout(r, 0));
    expect(errCalls.some((c) => c === "POST /__audora_state")).toBe(false);
    expect(errCalls.some((c) => c === "POST /__audora_rpc")).toBe(false);

    // Case 2: Slow GET /__audora_state races against pre-hydration settings toggle AND Event::Opened drop
    let resolveStateFetch!: (res: Response) => void;
    const rpcBodies: Array<Record<string, unknown>> = [];
    vi.stubGlobal(
      "fetch",
      vi.fn(async (url: string, init?: RequestInit) => {
        const method = init?.method ?? "GET";
        if (url === "/__audora_state" && method === "GET") {
          return new Promise<Response>((resolve) => {
            resolveStateFetch = resolve;
          });
        }
        if (url === "/__audora_rpc" && init?.body) {
          rpcBodies.push(JSON.parse(String(init.body)) as Record<string, unknown>);
          return new Response(JSON.stringify({ ok: true }), {
            status: 200,
            headers: { "Content-Type": "application/json" },
          });
        }
        return new Response(JSON.stringify({ ok: true }), {
          status: 200,
          headers: { "Content-Type": "application/json" },
        });
      }),
    );

    const client = new AudoraIpcClient(true);
    const hydrationPromise = client.ensureHydratedFromNative();

    // User toggles side panel `]` BEFORE hydration completes -> must NOT clobber SQLite source_folders!
    client.updateSettings({ ...client.getSettings(), show_side_panel: false });
    expect(rpcBodies.some((b) => b.op === "save_settings")).toBe(false);

    // macOS Event::Opened arrives while hydration is still pending
    const dropPromise = client.ingestNativeDroppedPaths(
      [
        {
          kind: "file",
          path: "/tmp/Opened_Via_OS.mp3",
          name: "Opened_Via_OS.mp3",
          size: 4096,
          duration_ms: 42_000,
        },
      ],
      { autoPlay: true },
    );

    // Now SQLite returns persisted library + source_folders + theme="light"
    resolveStateFetch(
      new Response(
        JSON.stringify({
          media: [
            {
              id: "med_hydrated_1",
              path: "/tmp/Hydrated_Track.wav",
              display_name: "Hydrated_Track.wav",
              media_kind: "audio",
              duration_ms: 60_000,
              last_position_ms: 5_000,
              completed: false,
              file_size_bytes: 10_000,
              fingerprint: null,
              availability: "available",
              added_at: "2026-01-01T00:00:00Z",
              last_opened_at: "2026-01-01T00:00:00Z",
              last_seen_at: "2026-01-01T00:00:00Z",
              note_count: 0,
              source_folder_id: "src_hydrated",
            },
          ],
          notes: [],
          playlists: [],
          settings: {
            theme: "light",
            resume_where_left_off: true,
            pause_on_headphones_disconnect: true,
            default_playback_speed: 1.25,
            timestamp_precision: "nearest_second",
            window_mode: "full",
            auto_check_updates: true,
            installed_version: "1.1.2",
            dismissed_update_version: null,
            source_folders: [
              {
                id: "src_hydrated",
                path: "/tmp/HydratedFolder",
                label: "HydratedFolder",
                enabled: true,
                added_at: "2026-01-01T00:00:00Z",
                last_scanned_at: "2026-01-01T00:00:00Z",
                media_count: 1,
              },
            ],
            show_playlists_bar: true,
            show_side_panel: true,
          },
        }),
        { status: 200, headers: { "Content-Type": "application/json" } },
      ),
    );

    const [_, dropRes] = await Promise.all([hydrationPromise, dropPromise]);
    expect(dropRes.openedItems).toHaveLength(1);
    // Hydrated library track + source folder preserved AND pre-hydration side-panel toggle merged!
    expect(client.searchLibrary("", "all").media_items.map((m) => m.id)).toContain(
      "med_hydrated_1",
    );
    expect(client.getSourceFolders()).toHaveLength(1);
    expect(client.getSettings().theme).toBe("light");
    expect(client.getSettings().show_side_panel).toBe(false);
    // Event::Opened autoPlay starts playback on the opened track
    expect(client.getPlaybackSnapshot().media_id).toBe(dropRes.openedItems[0]!.id);
    expect(client.getPlaybackSnapshot().state).toBe("playing");
  });

  it("preserves volume/muted/rate/repeat/shuffle and monotonic generation across idle transitions, syncs shuffle queue & preserveShuffleOrder, and isolates open errors", () => {
    const client = new AudoraIpcClient(false);
    const [t1, t2, t3] = client.openLocalFiles([
      { name: "Track1.mp3", path: "/tmp/Track1.mp3", size: 1000, durationMs: 30_000 },
      { name: "Track2.mp3", path: "/tmp/Track2.mp3", size: 1000, durationMs: 30_000 },
      { name: "Track3.mp3", path: "/tmp/Track3.mp3", size: 1000, durationMs: 30_000 },
    ]);

    client.playbackCommand({ action: "set_volume", volume: 0.35 });
    client.playbackCommand({ action: "set_muted", muted: true });
    client.playbackCommand({ action: "set_rate", rate: 1.5 });
    client.playbackCommand({ action: "set_repeat", mode: "all" });

    // Enabling shuffle syncs snapshot.queue to the shuffled order and stores unshuffledQueue
    const shuffled = client.playbackCommand({ action: "set_shuffle", shuffle: true });
    expect(shuffled.shuffle).toBe(true);
    expect(shuffled.queue).toHaveLength(3);

    // Selecting a track with preserveShuffleOrder keeps shuffled queue intact
    const snapshotOrderBefore = [...shuffled.queue];
    client.selectMediaById(t2!.id, { autoPlay: true, preserveShuffleOrder: true });
    expect(client.getPlaybackSnapshot().queue).toEqual(snapshotOrderBefore);
    expect(client.getPlaybackSnapshot().state).toBe("playing");

    // Pre-open validation failure while playing does NOT corrupt active playback state
    const genPlaying = client.getPlaybackSnapshot().generation;
    expect(() => client.openMediaPath("/tmp/BadCodec.prores")).toThrow();
    expect(client.getPlaybackSnapshot().state).toBe("playing");
    expect(client.getPlaybackSnapshot().media_id).toBe(t2!.id);
    expect(client.getPlaybackSnapshot().error).toBeNull();
    expect(client.getPlaybackSnapshot().generation).toBe(genPlaying);

    // Removing all tracks transitions to idle while preserving volume, muted, rate, repeat, shuffle & monotonic generation
    client.removeMedia(t1!.id);
    client.removeMedia(t3!.id);
    const genBeforeLastRemove = client.getPlaybackSnapshot().generation;
    client.removeMedia(t2!.id);
    const idleSnap = client.getPlaybackSnapshot();
    expect(idleSnap.state).toBe("idle");
    expect(idleSnap.media_id).toBeNull();
    expect(idleSnap.volume).toBe(0.35);
    expect(idleSnap.muted).toBe(true);
    expect(idleSnap.rate).toBe(1.5);
    expect(idleSnap.repeat_mode).toBe("all");
    expect(idleSnap.shuffle).toBe(true);
    expect(idleSnap.generation).toBeGreaterThan(genBeforeLastRemove);

    // Persisted idle snapshot with 0 available items also restores audio preferences instead of resetting to 100% unmuted
    const store = new Map<string, string>();
    store.set(
      "audora.state.v2",
      JSON.stringify({
        media: [],
        notes: [],
        playlists: [],
        settings: client.getSettings(),
        playback_session: {
          active_media_id: null,
          queue: [],
          volume: 0.25,
          muted: true,
          rate: 1.75,
          repeat_mode: "one",
          shuffle: true,
        },
      }),
    );
    vi.stubGlobal("window", {
      location: { hostname: "example.local" },
      localStorage: {
        getItem: (k: string) => store.get(k) ?? null,
        setItem: (k: string, v: string) => {
          store.set(k, v);
        },
        removeItem: (k: string) => {
          store.delete(k);
        },
      },
    });
    const restoredClient = new AudoraIpcClient(true);
    const restoredSnap = restoredClient.getPlaybackSnapshot();
    expect(restoredSnap.state).toBe("idle");
    expect(restoredSnap.volume).toBe(0.25);
    expect(restoredSnap.muted).toBe(true);
    expect(restoredSnap.rate).toBe(1.75);
    expect(restoredSnap.repeat_mode).toBe("one");
    expect(restoredSnap.shuffle).toBe(true);
    expect(restoredSnap.generation).toBeGreaterThanOrEqual(1);
  });

  it("blocks playback on missing media with AV-101, increments generation on upgradeMediaKindToAudio, skips retry on 4xx RPC, and flushes inFlightRpcEntry on exit", async () => {
    const client = new AudoraIpcClient(false);
    const clip = client.openMediaPath("/tmp/AudioOnly.mp4", {
      size: 2048,
      durationMs: 20_000,
    });
    const genBeforeUpgrade = client.getPlaybackSnapshot().generation;
    client.upgradeMediaKindToAudio(clip.id);
    expect(client.getPlaybackSnapshot().generation).toBeGreaterThan(genBeforeUpgrade);

    client.markMediaMissing(clip.id);
    expect(() => client.playbackCommand({ action: "play" })).toThrowError(/moved, renamed/i);
    expect(() => client.playbackCommand({ action: "toggle_play_pause" })).toThrowError(
      /moved, renamed/i,
    );
    expect(client.getPlaybackSnapshot().state).toBe("paused");
    expect(() =>
      client.relinkMedia(clip.id, "/tmp/ZeroBytes.mp4", { fileSizeBytes: 0 }),
    ).toThrowError(/0 bytes/i);

    // 4xx RPC error skips retry; in-flight RPC is captured by __AUDORA_COLLECT_QUIT_BATCH__ and settles false
    vi.stubGlobal("navigator", { userAgent: "Mozilla/5.0 AppleWebKit/605.1.15" });
    vi.stubGlobal("window", {
      __AUDORA_SESSION_TOKEN__: "tok_rpc_test",
      location: { hostname: "127.0.0.1" },
      localStorage: {
        getItem: () => null,
        setItem: () => {},
        removeItem: () => {},
      },
    });
    let rpcMode: "ok" | "fail400" | "hang" = "ok";
    let noteRpcCalls = 0;
    let releaseInFlight: ((res: Response) => void) | null = null;
    const setReleaseInFlight = (fn: (res: Response) => void) => {
      releaseInFlight = fn;
    };
    vi.stubGlobal(
      "fetch",
      vi.fn(async (url: string, init?: RequestInit) => {
        if (url === "/__audora_state" && (init?.method ?? "GET") === "GET") {
          return new Response("{}", {
            status: 200,
            headers: { "Content-Type": "application/json" },
          });
        }
        if (url === "/__audora_rpc") {
          const body = init?.body ? (JSON.parse(String(init.body)) as { op?: string }) : {};
          if (body.op === "upsert_note") {
            noteRpcCalls += 1;
            if (rpcMode === "fail400") {
              return new Response(JSON.stringify({ ok: false }), {
                status: 400,
                headers: { "Content-Type": "application/json" },
              });
            }
            if (rpcMode === "hang") {
              return new Promise<Response>((resolve) => {
                setReleaseInFlight(resolve);
              });
            }
          }
          return new Response(JSON.stringify({ ok: true }), {
            status: 200,
            headers: { "Content-Type": "application/json" },
          });
        }
        return new Response("{}", { status: 404 });
      }),
    );

    const rpcClient = new AudoraIpcClient(true);
    await rpcClient.ensureHydratedFromNative();
    const m = rpcClient.openMediaPath("/tmp/Live.mp3", { size: 1000, durationMs: 10_000 });
    await rpcClient.generateDiagnosticReportNative(); // flushes openMediaPath RPCs

    // 4xx HTTP status skips retry (exactly 1 attempt)
    rpcMode = "fail400";
    noteRpcCalls = 0;
    await expect(
      rpcClient.saveNoteVerified({
        mode: "create",
        media_id: m.id,
        position_ms: 1_000,
        is_quick_marker: false,
        body: "Test 400 note",
      }),
    ).rejects.toMatchObject({ code: "AV-401" });
    expect(noteRpcCalls).toBe(1);

    // inFlightRpcEntry stays visible to __AUDORA_COLLECT_QUIT_BATCH__ until settled and settles drained promises false
    rpcMode = "hang";
    const pendingNotePromise = rpcClient.saveNoteVerified({
      mode: "create",
      media_id: m.id,
      position_ms: 2_000,
      is_quick_marker: false,
      body: "In-flight quit flush note",
    });
    await new Promise((r) => setTimeout(r, 0));
    const quitWin = globalThis.window as unknown as {
      __AUDORA_COLLECT_QUIT_BATCH__?: () => string;
    };
    const exitBatch = JSON.parse(quitWin.__AUDORA_COLLECT_QUIT_BATCH__!()) as Array<{
      op?: string;
    }>;
    expect(exitBatch.some((op) => op.op === "upsert_note")).toBe(true);
    await expect(pendingNotePromise).rejects.toMatchObject({ code: "AV-401" });
    if (releaseInFlight) {
      (releaseInFlight as (res: Response) => void)(
        new Response(JSON.stringify({ ok: true }), {
          status: 200,
          headers: { "Content-Type": "application/json" },
        }),
      );
    }
  });

  it("defers installed_version persistence when native update uses deferred_install (Windows/Linux deb)", async () => {
    const manifestJson = JSON.stringify({
      tag_name: "v2.1.0",
      name: "Audora 2.1.0",
      body: "Release notes",
      assets: [
        {
          name: "Audora_2.1.0_macos-aarch64.dmg",
          browser_download_url:
            "https://github.com/smokeInCloud060201/audora/releases/download/v2.1.0/Audora_2.1.0_macos-aarch64.dmg",
          size: 12_000_000,
          digest:
            "sha256:4444444444444444444444444444444444444444444444444444444444444444",
        },
      ],
    });

    let deferredFlag = true;
    vi.stubGlobal("navigator", { userAgent: "Mozilla/5.0 AppleWebKit/605.1.15" });
    vi.stubGlobal("window", {
      location: { hostname: "127.0.0.1", origin: "http://127.0.0.1:41234" },
      __AUDORA_SESSION_TOKEN__: "test-token",
      __AUDORA_PLATFORM__: {
        os: "linux",
        arch: "x86_64",
        data_dir: "/tmp/audora-test",
      },
      localStorage: {
        getItem: () => null,
        setItem: () => {},
        removeItem: () => {},
      },
    });
    vi.stubGlobal(
      "fetch",
      vi.fn(async (input: RequestInfo | URL) => {
        const url = String(input);
        if (url === "/__audora_state") {
          return new Response("{}", {
            status: 200,
            headers: { "Content-Type": "application/json" },
          });
        }
        if (url === "/__audora_update") {
          return new Response(
            JSON.stringify({
              ok: true,
              status: "restarting",
              deferred_install: deferredFlag,
              message: "Closing to install and reopen…",
            }),
            {
              status: 200,
              headers: { "Content-Type": "application/json" },
            },
          );
        }
        return new Response(JSON.stringify({ ok: true }), {
          status: 200,
          headers: { "Content-Type": "application/json" },
        });
      }),
    );

    const client = new AudoraIpcClient(true);
    await client.ensureHydratedFromNative();
    const initialInstalledVer = client.getSettings().installed_version;
    client.stageGitHubReleaseJson(
      manifestJson.replaceAll(
        "Audora_2.1.0_macos-aarch64.dmg",
        "Audora_2.1.0_linux-x86_64.deb",
      ),
    );
    const deferredRes = await client.installNativeUpdate("2.1.0");
    expect(deferredRes.status.staged_for_restart).toBe(true);
    expect(client.getSettings().installed_version).toBe(initialInstalledVer);
    expect(client.getSettings().installed_version).not.toBe("2.1.0");

    deferredFlag = false;
    const immediateRes = await client.installNativeUpdate("2.1.0");
    expect(immediateRes.status.staged_for_restart).toBe(true);
    expect(client.getSettings().installed_version).toBe("2.1.0");
    vi.unstubAllGlobals();
  });
});
