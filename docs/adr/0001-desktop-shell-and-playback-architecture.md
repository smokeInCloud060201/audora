# ADR 0001: Desktop shell, persistence, and playback architecture

- **Status:** Proposed (Phase 1 technical validation required)
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

## Validation required (Phase 1 exit criteria)

This ADR moves from `Proposed` to `Accepted` only when Sprints 1.1–1.5 demonstrate:

1. Smooth video rendering, audio output, playback speed (`0.5×–2.0×`), accurate seeking, and clean teardown on macOS, Windows, and a representative Linux target.
2. Working Full ↔ Minimum mode window transitions and hybrid 56 px OS window controls.
3. Installable test packages on all three OSes with a verified LGPL-compliant dynamic plugin inventory.

## Related documents

- [ARCHITECTURE.md](../ARCHITECTURE.md)
- [FEATURES.md](../FEATURES.md)
- [UI_DESIGN.md](../UI_DESIGN.md)
