import { describe, expect, it } from "vitest";
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
});
