# ADR 0001: Desktop shell, persistence, and playback architecture

- **Status:** Superseded in part by [ADR 0004](0004-native-webview-loopback-architecture.md) (originally Accepted in Phase 1)
- **Date:** 2026-11-12
- **Deciders:** Project Maintainers

## Context and constraints

Audora is a cross-platform local-first desktop media player (macOS, Windows, Linux) whose defining workflow is capturing, searching, and returning to timestamped notes on local audio and video files. The architecture must:

1. Deliver a responsive, accessible UI matching the [Audora Figma design](../UI_DESIGN.md) across Full mode and Minimum mode without shipping an unnecessarily heavy browser runtime per window.
2. Play common local audio and video formats reliably without modifying source media files, while supporting accurate millisecond-level seeking and `0.5×–2.0×` playback speed.
3. Persist media references, playback progress, settings, and notes locally with transactional durability, crash safety, and forward-compatible schema migrations.
4. Comply with an Apache License 2.0 project license ([ADR 0002](0002-project-licensing-and-codec-policy.md)) and enforce strict privilege boundaries ([ADR 0003](0003-local-first-privacy-and-untrusted-boundary.md)).

## Decision

Adopt the following baseline architecture for Phase 1 validation:

1. **Desktop shell & UI:** **Tauri 2** with a **React + TypeScript** frontend (`strict` mode) using semantic CSS custom properties mapped from the Figma design tokens, and a hybrid 56 px window header integrating native OS window controls.
2. **Application core:** **Rust** domain and application services (`playback`, `library`, `notes`, `settings`) communicating with the frontend solely through narrow, validated Tauri commands and sampled events.
3. **Persistence:** **SQLite** (via Rust, with WAL mode where supported, foreign keys enabled, parameterized queries, and versioned migrations + pre-migration backups) stored in the OS-standard per-user app-data directory.
4. **Media backend abstraction:** Define a platform-independent Rust `PlaybackBackend` trait with load-generation IDs to suppress stale async callbacks, selecting **GStreamer (`gstreamer-rs`)** as the initial backend candidate.

## Alternatives considered

- **Electron + HTML5 `<video>` / `<audio>`:** Faster initial UI prototyping, high memory/disk footprint, and HTML5 media elements depend heavily on bundled Chromium codecs with limited control over container/codec probing and patent-clean redistribution.
- **Native per-OS UI toolkits (SwiftUI / WinUI / GTK) over a Rust core:** Maximum native integration, at the cost of maintaining three separate UI implementations for a design-system-driven app.
- **libmpv / FFmpeg direct integration:** Strong format coverage, but `mpv` licensing defaults (GPLv2+ in standard builds unless carefully built as LGPLv2.1+) and cross-platform compositing inside a webview shell require equally careful legal and rendering spikes. Kept as the primary fallback comparison if GStreamer fails Phase 1 gates.

## Consequences and risks

- **Video surface compositing:** Embedding hardware-accelerated GStreamer video frames smoothly inside or beneath a Tauri webview canvas across macOS (`WKWebView`), Windows (`WebView2`), and Linux (`WebKitGTK` on Wayland/X11) is the highest technical risk and must be proven first.
- **Plugin & installer complexity:** Bundling GStreamer runtimes across three operating systems increases installer size and requires automated plugin/license auditing.
- **Webview variance:** CSS/font rendering and media keyboard focus must be tested across `WKWebView`, `WebView2`, and `WebKitGTK`.

## Validation evidence (Phase 1 Sprints 1.1–1.5)

1. **GStreamer 1.28.1 discovery & pipeline verification:** Verified on macOS 15.3 (`arm64`) using `GStreamer 1.28.1` (`/Library/Frameworks/GStreamer.framework/Versions/1.0`). Confirmed all 15 required core/base/good/hardware plugins are LGPL-licensed (`coreelements`, `playback`, `typefindfunctions`, `audioconvert`, `audioresample`, `videoconvertscale`, `autodetect`, `isomp4`, `matroska`, `ogg`, `opus`, `vorbis`, `flac`, `wavparse`, `applemedia`) and verified container/stream probing (`gst-discoverer-1.0`) on synthetic `.wav` and `.mp4` (H.264 + Opus) fixtures.
2. **Video surface & modal overlay architecture:** To ensure DOM modals (`Add a note` `4:88`) and Full ↔ Minimum mode transitions (`2:3` ↔ `2:103`) render without native child-window z-order clipping across `WKWebView`, `WebView2`, and `WebKitGTK`, the Rust `PlaybackBackend` owns authoritative state, load-generation IDs, GStreamer metadata/codec probing, and command serialization, paired with a synchronized webview media surface for frame display.
3. **Platform, licensing, and error closure:** Recorded in [PLATFORM_SUPPORT.md](../PLATFORM_SUPPORT.md), [FORMAT_AND_LICENSE_MATRIX.md](../FORMAT_AND_LICENSE_MATRIX.md), and [ERROR_CATALOG.md](../ERROR_CATALOG.md).

## Related documents

- [ARCHITECTURE.md](../ARCHITECTURE.md)
- [PLATFORM_SUPPORT.md](../PLATFORM_SUPPORT.md)
- [FORMAT_AND_LICENSE_MATRIX.md](../FORMAT_AND_LICENSE_MATRIX.md)
- [ERROR_CATALOG.md](../ERROR_CATALOG.md)
- [FEATURES.md](../FEATURES.md)
- [UI_DESIGN.md](../UI_DESIGN.md)
