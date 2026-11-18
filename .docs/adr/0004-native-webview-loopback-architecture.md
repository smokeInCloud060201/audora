# ADR 0004: Native `tao` + `wry` shell, authenticated loopback media server, and pure-Rust container prober

- **Status:** Accepted (Supersedes desktop shell & playback surface choices in [ADR 0001](0001-desktop-shell-and-playback-architecture.md))
- **Date:** 2026-11-18
- **Deciders:** Project Maintainers

## Context and constraints

During implementation and cross-platform packaging across macOS (`WKWebView`), Windows (`WebView2`), and Linux (`WebKitGTK 4.1`), two practical constraints emerged from the initial [ADR 0001](0001-desktop-shell-and-playback-architecture.md) proposal (Tauri 2 macro runtime + linked `gstreamer-rs` `playbin3` video sink):

1. **DOM overlay compositing & window mode switching:** Rendering hardware-decoded video frames via an external native GStreamer sink beneath or alongside a webview introduces z-order clipping against translucent DOM overlays (`Add a note` modal, scrubber tooltips, toast bars) and complicates seamless Full mode ↔ frameless Minimum mode (`360×168` always-on-top) transitions.
2. **Build portability & redistribution surface:** Linking `gstreamer-rs` C libraries at compile time requires heavy per-OS SDK installations (`GStreamer.framework`, MSVC `.lib` installers, pkg-config dev headers) even for contributors working on UI, SQLite persistence, or domain logic, and expands installer bundling complexity when platform WebViews already integrate hardware-accelerated media pipelines (`AVFoundation`/`VideoToolbox` on macOS, `Media Foundation` on Windows, `GStreamer` inside `WebKitGTK` on Linux).

At the same time, Audora must preserve all [ADR 0002](0002-project-licensing-and-codec-policy.md) (Apache-2.0 / no GPL) and [ADR 0003](0003-local-first-privacy-and-untrusted-boundary.md) (local-first privacy, untrusted webview boundary, read-only media access) guarantees.

## Decision

Adopt and standardize the shipped native desktop runtime architecture:

1. **Direct `tao` + `wry` native shell (`apps/desktop/src-tauri`):**
   - Use Tauri's underlying windowing (`tao`) and OS WebView (`wry`) crates directly without the heavy Tauri plugin/macro layer. Retain `tauri.conf.json` and `capabilities/default.json` as audited release-gate metadata verified by `scripts/verify-release.mjs`.
   - Manage Full mode (`≥ 1040×640` decorated) and Minimum mode (`360×168` frameless always-on-top) on a single persistent window + WebView instance so mode switches never interrupt active playback.
   - Handle native OS file associations (`tao::event::Event::Opened` on macOS and CLI argv on Windows/Linux), native drag-and-drop (`with_drag_drop_handler`), and native OS file/folder pickers in Rust.

2. **Authenticated `127.0.0.1:0` loopback HTTP server:**
   - Bind an ephemeral loopback listener (`127.0.0.1:0`) per process launch protected by a 256-bit CSPRNG `X-Audora-Token` secret, strict `Host` header validation (blocking DNS rebinding), and a non-wildcard `Content-Security-Policy` (`RUNTIME_CSP`).
   - Serve compiled UI static assets, relational SQLite RPC mutations (`POST /__audora_rpc`), bootstrap/backup/cleanup endpoints (`/__audora_state`), verified self-update staging (`/__audora_update`), and RFC 9110 HTTP byte-range media streaming (`GET /__audora_media?path=...&token=...`).
   - Enforce a canonicalized `MediaAllowlist` in Rust so `/__audora_media` streams **only** files explicitly opened via OS pickers, native drag-and-drop, CLI launch arguments, or persisted library/source-folder records (non-recursive / bounded depth, dot-directories skipped, symlinks to unsupported extensions rejected).

3. **HTML5 `<video>` / `<audio>` playback + pure-Rust container prober (`audora-core`):**
   - Render active audio and video through the OS WebView's hardware-accelerated HTML5 `<video>` / `<audio>` element over HTTP `206 Partial Content` byte ranges, keeping DOM modals, note markers, and Full ↔ Minimum mode layout transitions 100% glitch-free.
   - Probe container headers and durations in pure Rust (`probe_local_media_duration_ms` across WAV, MP4/M4V/MOV/M4A, WebM/MKV, FLAC, Ogg/Opus, MP3, and ADTS AAC) without requiring external C libraries at build time.
   - Retain `PlaybackBackend` (`FakePlaybackBackend` and optional CLI `GStreamerBackend` wrapping `gst-discoverer-1.0` / `ffprobe`) in `audora-core` for headless domain/service tests and optional extended codec discovery.

## Consequences and risks

- **Zero native SDK friction for Rust builds:** `cargo test --workspace` and `cargo build --release` compile cleanly with standard Rust toolchains (plus `libwebkit2gtk-4.1-dev` / `libgtk-3-dev` on Linux), with zero mandatory GStreamer C headers.
- **Codec parity follows OS WebView media stacks:** macOS (`WKWebView` / VideoToolbox), Windows (`WebView2` / Media Foundation), and Linux (`WebKitGTK` / system `gstreamer1.0-plugins-base` & `good`) decode Baseline Tier 1 formats natively; unsupported streams surface `MediaError` in the WebView and map deterministically to `AV-104` (`Decoder unavailable`).
- **Loopback token discipline:** Because `<video src>` cannot attach custom HTTP headers on sub-range requests, `/__audora_media` validates a constant-time query token paired with strict `Host`, `Sec-Fetch-Site`, and `MediaAllowlist` checks, while never writing the token to logs or disk.

## Related documents

- [ADR 0001](0001-desktop-shell-and-playback-architecture.md)
- [ADR 0002](0002-project-licensing-and-codec-policy.md)
- [ADR 0003](0003-local-first-privacy-and-untrusted-boundary.md)
- [ARCHITECTURE.md](../ARCHITECTURE.md)
- [FORMAT_AND_LICENSE_MATRIX.md](../FORMAT_AND_LICENSE_MATRIX.md)
- [PLATFORM_SUPPORT.md](../PLATFORM_SUPPORT.md)
- [SECURITY_AND_A11Y_REVIEW.md](../SECURITY_AND_A11Y_REVIEW.md)
