# Product roadmap

This roadmap is outcome-driven and intentionally has no calendar promises. A phase closes only when its exit criteria are met across supported operating systems.

## 0. Product and design baseline — completed

**Outcome:** reviewed requirements, user workflows, Figma UI/accessibility specification, module boundaries, licensing, security, and delivery standards.

Exit: product scope and Figma discrepancies resolved ([PRODUCT.md](PRODUCT.md), [WORKFLOWS.md](WORKFLOWS.md), [UI_DESIGN.md](UI_DESIGN.md)); unresolved technical choices assigned to Phase 1 owners ([ARCHITECTURE.md](ARCHITECTURE.md)); Apache 2.0, dependency policy, and untrusted boundaries recorded in [ADR 0001](adr/0001-desktop-shell-and-playback-architecture.md)–[ADR 0003](adr/0003-local-first-privacy-and-untrusted-boundary.md).

## 1. Technical validation and detailed design — current

**Outcome:** prove platform and licensing feasibility before building product features.

- Select exact UI framework setup and frontend token/styling strategy.
- Build disposable playback/package experiments for macOS, Windows, and representative Linux targets.
- Validate GStreamer video embedding, audio output, seek behavior, playback speed (`0.5×–2.0×`), OS audio-route disconnect events, shutdown, codecs, plugin distribution, and LGPL obligations.
- Choose supported architectures, Linux distributions, native installer formats, and minimum OS versions.
- Validate SQLite location, backup/restore, migration, and relocation workflows.
- Complete capability matrix, user-facing error catalog (`AV-*`), and Light theme token mapping.
- Update ADRs and support/license matrices with validation evidence.

Exit: architecture choices have evidence on all target OS families, package/license path is viable, and scope is measurable.

## 2. Engineering foundation

**Outcome:** a buildable, reviewable application foundation.

- Establish Rust workspace, UI shell, typed command layer, formatting/linting, CI, dependency/license inventory, and contribution protections.
- Implement settings and versioned SQLite migrations.
- Add error/reporting boundaries, logging redaction, and platform service interfaces.
- Produce development builds on all supported CI targets.

Exit: clean checkout builds; CI checks all target platforms; persistence migration and failure policies are exercised.

## 3. Playback and library

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
