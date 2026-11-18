# Changelog

All notable changes to Audora are documented in this file.

## [1.1.2] - 2026-11-18

### Fixed
- **Automatic Queue Advancement on Track Completion:** Fixed end-of-stream playback transitions so finishing an audio or video file automatically advances to and plays the next available item in the session queue (`00:00` start, honoring **Repeat Off / All / One** and **Shuffle**).
  - Guarded `timeupdate`, `ended`, `loadedmetadata`, `canplay`, and `error` media element callbacks by `mediaId` and load `generation` to eliminate a double-end race on `.mp4` / `.m4a` / `.m4v` / `.mov` streams that previously skipped or immediately ended the next queued track.
  - Keyed `<audio>` and `<video>` elements by `activeMedia.id` with `autoPlay` and restored `defaultPlaybackRate`, `playbackRate`, `preservesPitch`, `volume`, and `muted` on metadata load for reliable `WKWebView` transitions without speed resets.
  - Updated queue navigation (`Ended`, `Next`, `Previous`, and `removeFromQueue`) across both UI IPC and `audora-core` to automatically skip missing/offline or disabled-folder items to reach the next playable track, and clamped near-EOF `Resume where I left off` positions on direct Library opens.

### Documentation
- **User-Friendly README & Developer Guide:** Refreshed `README.md` for end users and added a dedicated `DEVELOPMENT.md` guide covering local setup, architecture, testing, and release packaging.

## [1.1.1] - 2026-11-17

### Fixed
- **Dark Mode Seek Timeline Visibility:** Replaced native WebView range slider groove styling on `.scrubber-slider` with explicit theme tokens (`--scrubber-track-bg: #2c333d`, `#363f4b` on hover) and a live progress gradient, and subtly lightened Minimum Mode's dark `--min-track-bg` (`#38413f`) so the unfilled timeline bar remains clearly visible against dark backgrounds in production builds.

## [1.1.0] - 2026-11-17

### Added
- **Unified Relational SQLite v4 Persistence (`0004_relational_unification.sql`):** Replaced whole-state JSON blob snapshots with O(1) indexed relational RPC mutations (`/__audora_rpc`) across `media`, `notes`, `playlists`, `playlist_items`, and `settings`, including automatic non-destructive migration from legacy `app_ui_state` (`v3`) blobs, ID/path collision reconciliation, and atomic shutdown batch flushing.
- **Pure-Rust 12-Format Container Duration Prober:** Instant offline stream duration inspection for `.mp4`, `.m4a`, `.m4v`, `.mov` (`mvhd` / `mehd` / `mdhd` / multi-`sidx`), `.mkv` & `.webm` (EBML `Segment` → `Info`), `.mp3` (`Xing` / `Info` / `VBRI` + multi-frame sync verification), `.aac` (ADTS + ISO-BMFF), `.ogg` & `.opus` (BOS serial + granule framing), `.wav` (RIFF + `RF64`/`BW64` `ds64`), and `.flac` (`STREAMINFO`), with bounded-timeout CLI fallbacks.
- **Native OS Drag-and-Drop & Active Playback Animations:** Direct native `wry` OS file/folder drag-and-drop ingestion into the Library and Player, plus animated equalizer/waveform indicators when tracks are actively playing.

### Changed
- **Modularized Frontend Architecture:** Extracted `PlayerStageView`, `LibraryView`, `SettingsView`, `MinimumWidgetView`, `PlaylistsSidebar`, `RightInspectorDrawer`, and `UpdateBanner` into dedicated typed view/component modules with modal keyboard focus traps and WCAG AA Light Minimum Mode contrast.
- **Core Toolchain & Native Shell Upgrades:** Upgraded `tao` (`0.37.2`), `wry` (`0.57.2`), `rusqlite` (`0.43.0` / `libsqlite3-sys 0.40.0`), `vite` (`6.4.6`), and `vitest` (`4.1.12`), synchronizing CycloneDX 1.5 `sbom.cdx.json` and `THIRD_PARTY_NOTICES.md`.

### Security
- **Patched Bundled SQLite & Frontend Build Advisories:** Resolved `RUSTSEC-2026-0459` (`CVE-2026-119375` in `libsqlite3-sys` via SQLite `>= 3.54.1`), eliminated unmaintained `kuchikiki`/`fxhash` (`RUSTSEC-2025-0057`), and cleared all 15 npm advisories (`pnpm audit`).
- **Loopback Server & Native Self-Updater Hardening:** Enforced ephemeral IPv4 `127.0.0.1`-only `Host`/`Origin` validation (blocking `localhost` IPv6 port shadowing and `Origin: null` iframe bypasses), 256-bit OS CSPRNG `X-Audora-Token` authentication, OS-picker-gated two-tier media/folder allowlists with immediate revocation, macOS `codesign` identity & `ditto` bundle verification, Windows Authenticode signature checks, and GitHub Sigstore SLSA build provenance attestations.

### Fixed
- **Playback, Scrubbing & Error Classification:** Eliminated timeline scrubber dead zones while dragging, preserved authoritative ISO-BMFF container durations on fragmented `.m4a` files, separated unsupported codec errors (`AV-104`) from missing-file states (`AV-101`), fixed even-length shuffle wraparound, and kept Full Mode windows freely resizable above minimum dimensions.
- **Cross-Platform Persistence & Installer Safety:** Fixed SQLite Unicode case-insensitive search and `%`/`_` wildcard escaping, preserved corrupt databases during `PRAGMA quick_check` recovery, handled Windows open-handle SQLite cleanup (`VACUUM` + `wal_checkpoint(TRUNCATE)`) and 8.3 short-path canonicalization, and scoped NSIS uninstaller cleanup strictly to Audora-owned files.

## [0.1.0] - Initial Public Release

### Highlights
- **Local-First Audio & Video Player:** Play `.mp3`, `.wav`, `.flac`, `.m4a`, `.aac`, `.ogg`, `.opus`, `.mp4`, `.m4v`, `.mov`, `.webm`, and `.mkv` files completely offline on macOS, Windows, and Linux with **Dark**, **Light**, and **System** themes.
- **Persistent Playback & Compact Minimum Mode:** Audio continues seamlessly while navigating across Library and Settings, with variable playback speed (`0.5×`–`2×`), timeline markers, **Repeat** (`Off` / `All` / `One`), **Shuffle**, and a floating `360 × 168` **Minimum Mode** (`M`).
- **Timestamped Notes & Instant Markers:** Capture notes (`N`) or quick markers (`Shift + N`) without interrupting playback, click any note to seek and auto-play immediately, and search across media titles and note text offline (`⌘K` / `Ctrl + K`).
- **3-Column Workspace & Multi-Playlists:** Collapsible **Playlists** left drawer (`[`) and **Notes & Queue** right drawer (`]`) with custom playlists, queue-to-playlist snapshots, and track reordering.
- **Trusted Local Media Sources:** Link local folders in **Settings → Local sources** to auto-import and sync media, toggle folder visibility without losing notes, and automatically deduplicate manually opened files.
- **Resilient Local Storage & Verified Self-Updates:** SQLite WAL persistence with automatic pre-migration and manual backups, missing-file relinking that preserves every note, redacted diagnostic reports, clean app-data removal, and SHA-256 verified GitHub Releases updates.
