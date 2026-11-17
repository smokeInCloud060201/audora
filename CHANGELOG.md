# Changelog

All notable changes to Audora are documented in this file.

## [1.0.3] - 2026-11-17

### Added & Improved
- **Unified Relational SQLite v4 Storage (`0004_relational_unification.sql`):** All media items, timestamped notes, playlists, trusted source folders, and playback sessions now persist directly in normalized SQLite WAL tables with non-destructive legacy `app_ui_state` migration, `id`/`path` collision reconciliation, and atomic shutdown batch flushing.
- **12-Format Pure-Rust Container Duration Prober:** Instant, offline duration discovery across `.mp4`, `.m4a`, `.m4v`, `.mov` (`mvhd` / `mehd` / `mdhd` / multi-`sidx`), `.mkv` & `.webm` (EBML `Segment` -> `Info`), `.mp3` (`Xing`/`Info`/`VBRI` + multi-frame sync verification), `.aac` (ADTS + ISO-BMFF), `.ogg` & `.opus` (BOS serial + granule framing), `.wav` (RIFF + `RF64`/`BW64` `ds64`), and `.flac` (`STREAMINFO`), with bounded-timeout CLI fallbacks on Linux.
- **Loopback & Native Self-Updater Hardening:** Ephemeral `127.0.0.1`-only origin binding, 256-bit OS CSPRNG session tokens, OS-picker-gated two-tier media/folder allowlists with instant revocation, macOS `codesign` identity & `ditto` verification, and detached Windows Authenticode installer handoff.
- **Playback, Drag-Drop & Accessibility Polish:** Zero-dead-zone timeline scrubbing, native OS window drag-and-drop file/folder ingestion, decode-error (`AV-104`) vs missing-file (`AV-101`) separation, modal keyboard focus traps, WCAG AA Light Minimum Mode contrast, and full CycloneDX 1.5 SBOM synchronization.

## [0.1.0] - Initial Public Release

### Highlights
- **Local-First Audio & Video Player:** Play `.mp3`, `.wav`, `.flac`, `.m4a`, `.aac`, `.ogg`, `.opus`, `.mp4`, `.m4v`, `.mov`, `.webm`, and `.mkv` files completely offline on macOS, Windows, and Linux with **Dark**, **Light**, and **System** themes.
- **Persistent Playback & Compact Minimum Mode:** Audio continues seamlessly while navigating across Library and Settings, with variable playback speed (`0.5×`–`2×`), timeline markers, **Repeat** (`Off` / `All` / `One`), **Shuffle**, and a floating `360 × 168` **Minimum Mode** (`M`).
- **Timestamped Notes & Instant Markers:** Capture notes (`N`) or quick markers (`Shift + N`) without interrupting playback, click any note to seek and auto-play immediately, and search across media titles and note text offline (`⌘K` / `Ctrl + K`).
- **3-Column Workspace & Multi-Playlists:** Collapsible **Playlists** left drawer (`[`) and **Notes & Queue** right drawer (`]`) with custom playlists, queue-to-playlist snapshots, and track reordering.
- **Trusted Local Media Sources:** Link local folders in **Settings → Local sources** to auto-import and sync media, toggle folder visibility without losing notes, and automatically deduplicate manually opened files.
- **Resilient Local Storage & Verified Self-Updates:** SQLite WAL persistence with automatic pre-migration and manual backups, missing-file relinking that preserves every note, redacted diagnostic reports, clean app-data removal, and SHA-256 verified GitHub Releases updates.
