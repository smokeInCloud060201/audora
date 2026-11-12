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

## 3. Playback and library — current

**Outcome:** useful, reliable local playback.

- Open/drop files; metadata probing; playback state machine; session queue with repeat/shuffle; transport, seek, speed, volume controls, and Full/Minimum modes.
- Per-media progress tracking (`%`) and `Resume where I left off`; missing/unsupported/corrupt media states; OS open-with behavior.
- Format matrix, backend capability reporting, and platform package verification.

Exit: acceptance flows work on each OS; playback remains responsive and handles backend failure without data loss.

## 4. Timestamped notes and organization

**Outcome:** the product's defining note-and-return workflow.

- Create quick markers and text notes, edit/delete, list, search (`⌘/Ctrl+K`), and seek back.
- Persist notes transactionally; support unavailable media and explicit relinking (`Find file…`).
- Keyboard shortcuts, accessibility, theme selection (`Dark / Light / System`), and timestamp precision settings.

Exit: notes survive restart, migration, and tested relocation scenarios; core accessibility review passes.

## 5. Hardening and release candidate

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
