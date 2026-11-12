# Product roadmap

This roadmap is outcome-driven and intentionally has no calendar promises. A phase closes only when its exit criteria are met across supported operating systems.

## 0. Product and design baseline — completed

**Outcome:** reviewed requirements, user workflows, Figma UI/accessibility specification, module boundaries, licensing, security, and delivery standards.

Exit: product scope and Figma discrepancies resolved ([PRODUCT.md](PRODUCT.md), [WORKFLOWS.md](WORKFLOWS.md), [UI_DESIGN.md](UI_DESIGN.md)); unresolved technical choices assigned to Phase 1 owners ([ARCHITECTURE.md](ARCHITECTURE.md)); Apache 2.0, dependency policy, and untrusted boundaries recorded in [ADR 0001](adr/0001-desktop-shell-and-playback-architecture.md)–[ADR 0003](adr/0003-local-first-privacy-and-untrusted-boundary.md).

## 1. Technical validation and detailed design — completed

**Outcome:** prove platform and licensing feasibility before building product features.

- Selected React + TypeScript (`strict`) + Vite + semantic CSS variables (`Dark` & `Light` themes).
- Validated GStreamer 1.28.1 plugin inventory (`LGPL`) and media discovery/playback pipeline on macOS (`arm64`) with synthetic `.wav` and `.mp4` (H.264 + Opus) fixtures ([FORMAT_AND_LICENSE_MATRIX.md](FORMAT_AND_LICENSE_MATRIX.md)).
- Defined supported OS versions, architectures, installer formats, and hybrid 56 px header rules ([PLATFORM_SUPPORT.md](PLATFORM_SUPPORT.md)).
- Specified SQLite WAL schema (`0001_initial.sql`), pre-migration atomic backups, and relocation workflows.
- Completed user-facing error catalog ([ERROR_CATALOG.md](ERROR_CATALOG.md)) and accepted [ADR 0001](adr/0001-desktop-shell-and-playback-architecture.md).

Exit: architecture choices have evidence, package/license path is documented, and scope is measurable.

## 2. Engineering foundation — completed

**Outcome:** a buildable, reviewable application foundation.

- Established Rust workspace (`src-tauri/`), React + TypeScript (`strict`) UI shell (`ui/`), semantic CSS token system, typed command layer (`commands.rs`), formatting/linting, CI (`.github/workflows/ci.yml`), and dependency/license inventory (`deny.toml`).
- Implemented settings (`SqliteSettingsRepository`) and versioned SQLite WAL migrations (`0001_initial.sql`) with automatic pre-migration backups.
- Added `AV-*` error/reporting boundaries (`error.rs`), path/note redaction, and `PlaybackBackend` (`GStreamerBackend` + `FakePlaybackBackend`) & repository traits.

Exit: clean checkout builds; CI checks target platforms; persistence migration and failure policies are exercised by automated tests.

## 3. Playback and library — completed

**Outcome:** useful, reliable local playback.

- Implemented single-file, multi-file, non-recursive folder open (`open_media_batch`), drag-and-drop, CLI open-with (`main.rs`), and library row actions (`Play now`, `Add to queue`, `Mark missing`, `Remove from library`).
- Implemented playback state machine with monotonic load-generation IDs, `<video>`/`<audio>` element sync + real-time clock fallback, interactive volume slider (`0–100%`), speed multiplier (`0.5×–2.0×` with pitch preservation), status badges (`Playing`, `Paused`, `Seeking`, `Ended`, `Loading`, `Error`), dedicated audio-only waveform artwork canvas (`Frame 4:3`), and Full/Minimum modes (`? Help` popover `Frame 2:115`).
- Implemented per-media progress tracking (`%`), `Resume where I left off`, session queue (`Repeat Off/All/One`, `Shuffle`, item removal, auto-advance on `Ended`), and missing/unsupported/corrupt media states (`AV-101`–`AV-104`).

Exit: acceptance flows work on each OS; playback remains responsive and handles backend failure without data loss.

## 4. Timestamped notes and organization — completed

**Outcome:** the product's defining note-and-return workflow.

- Implemented quick markers (`Shift+N`), text notes (`N`, `⌘/Ctrl+Enter`), edit/delete with 6-second `Undo` toast (`Frame 4:578`), chronological list, scrubber note marker ticks, out-of-range note badges, local search (`⌘/Ctrl+K`) with direct `▶ mm:ss — Jump to moment` results, and seek back.
- Persisted notes transactionally in SQLite WAL (`src-tauri/`) and `localStorage` (`ui/`); implemented Missing Media view (`Frame 4:339`) with preserved notes list (`Frame 4:378`), duplicate conflict blocking (`AV-201`), and Relink Mismatch Confirmation Modal (`AV-202`).
- Completed all 6 Settings sub-navigation categories (`Appearance` with miniature Dark/Light/System window previews and live `prefers-color-scheme` listener, `Playback` with headphone disconnect pause listener, `Notes`, `Keyboard`, `Privacy`, `About`).

Exit: notes survive restart, migration, and tested relocation scenarios; core accessibility review passes.

## 5. Hardening and release candidate — current

**Outcome:** complete installable product candidate.

- Installer UX, signing/notarization, SBOM/notices, checksums, crash recovery, backup/restore, and support docs.
- Security review of Tauri capabilities, media input, dependencies, logging, and update strategy.
- Performance, accessibility, localization readiness, and cross-platform manual review.
- Release candidate and issue triage.

Exit: stable acceptance criteria in PRODUCT.md pass; no blocking data loss, security, licensing, or platform defects.

## 6. Stable release and maintenance

**Outcome:** sustainable public distribution.

- Publish signed or verifiable packages, source, checksums, notices, format matrix, support policy, and release notes.
- Triage issues, security reports, and dependency updates under published policy.
- Consider subtitles, export/import, update delivery, and other post-release features through explicit RFC/ADR review.

## Prioritization rules

Correct playback and preservation of notes outrank cosmetic features. Cross-platform parity, accessibility, privacy, packaging, and license compliance are release requirements, not optional polish. Any feature that introduces network access, executable extensions, media modification, or personal data collection requires design and security review before implementation.
