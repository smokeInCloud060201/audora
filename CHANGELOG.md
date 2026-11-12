# Changelog

All notable changes to Audora are documented in this file.

## [0.1.0] - Release Candidate (Phases 0–6 Complete)

### Added
- **Desktop Player & Modes:**
  - Full workspace (`2:3`) with 56 px hybrid header (`data-tauri-drag-region`), 76 px navigation rail, 128 px transport bar, and 365 px `Queue | Notes` side panel.
  - Dedicated **Audio-Only Canvas** (`4:3`) with concentric ambient rings and 18-bar waveform artwork card.
  - Compact **Minimum Mode** (`2:103`) with expandable `? Help` popover (`2:115`).
  - Playback speed selector (`0.5×–2.0×` with pitch preservation), interactive volume slider + mute, and session queue with `Repeat (Off / All / One)` and `Shuffle`.
- **Timestamped Notes & Quick Markers:**
  - Single-keystroke note capture (`N`) that freezes the captured timestamp while allowing playback to continue.
  - Instant quick markers (`Shift+N`) and empty-body marker checkbox (`4:88`).
  - Visual note markers along the timeline scrubber, chronological note list, `⋯` edit/delete menu with 6-second `Undo` toast (`4:578`), and `Out of range` badges when relinked media is shorter.
- **Local Library, Offline Search & Relinking:**
  - Instant offline search (`⌘/Ctrl+K`) across media titles and note bodies with direct `▶ mm:ss — Jump to moment` results.
  - Missing media recovery (`4:339`) with preserved notes preview (`4:378`), duplicate merge protection (`AV-201`), and mismatch confirmation dialog (`AV-202`).
- **Persistence, Security & Settings:**
  - SQLite WAL storage (`0001_initial.sql`) with automatic pre-migration backups (`.pre-vN.bak`), manual backup snapshots (`.manual.bak`), and corrupt file preservation (`.corrupt.bak`).
  - Six Settings categories (`Appearance`, `Playback`, `Notes`, `Keyboard`, `Privacy`, `About`) with `Dark / Light / System` themes, headphone disconnect pause listener, and redacted diagnostic report preview.
