# Phase and sprint plan

This plan turns [the roadmap](ROADMAP.md) into ordered work packages. A “sprint” is a planning container, not a calendar commitment: no duration, team capacity, or release date is assumed. Keep sprint goals small enough to review as a coherent outcome. Re-plan after each sprint using evidence, blockers, and capacity.

## Planning rules

- Finish phase gates before taking on work that depends on them.
- Keep platform feasibility, privacy, accessibility, licensing, and data safety visible in every phase.
- Assign an owner and measurable acceptance criteria to each sprint before it starts.
- Split work so cross-platform integrations produce reviewable evidence early.
- Track risks and decisions in the relevant design doc or ADR; do not let unresolved assumptions quietly become implementation behavior.
- A phase can take multiple sprints. Do not mark it complete because its sprint list is exhausted; use the exit criteria.

## Phase 0 — Product and design baseline (Completed)

**Goal:** agree on what Audora is and what quality means before implementation.

### Sprint 0.1 — Requirements and user workflows (Completed)

- [x] Review the product statement, target users, scope, non-goals, and stable-release acceptance criteria ([PRODUCT.md](PRODUCT.md)).
- [x] Define the primary flows: open media, play/seek/speed/modes, create a note or quick marker, edit/delete notes, search & return to timestamp, handle missing media ([WORKFLOWS.md](WORKFLOWS.md)).
- [x] Resolve ambiguities in timestamp semantics, quick markers, playback progress/resume, note search, and media relocation ([WORKFLOWS.md](WORKFLOWS.md)).
- [x] Produce user-flow diagrams and a glossary for product terms ([WORKFLOWS.md](WORKFLOWS.md)).

**Acceptance:** each core flow has a happy path, failure path, and acceptance criteria; scope conflicts between Figma and requirements are resolved and recorded.

### Sprint 0.2 — UX and accessibility specification (Completed)

- [x] Inspect the existing [Audora Figma design](https://www.figma.com/design/PaNgbiT8RSycIk1QBzPiqf/Audora?node-id=4-522&m=dev) in Dev Mode; map its actual pages, frames (`2:3`, `2:103`, `4:3`, `4:88`, `4:206`, `4:300`, `4:339`, `4:399`, `4:441`, `4:523`, `4:609`), components, and tokens to product requirements ([UI_DESIGN.md](UI_DESIGN.md)).
- [x] Record the reviewed frame names/links and summarize the actual visual system and extracted tokens ([UI_DESIGN.md](UI_DESIGN.md)).
- [x] Compare design coverage with required player, audio-only, Minimum mode, library, notes, queue, settings, missing-media, and unsupported-media flows; specify follow-up states ([UI_DESIGN.md](UI_DESIGN.md)).
- [x] Specify loading, empty, playing, paused, seeking, ended, unavailable, and failed states ([UI_DESIGN.md](UI_DESIGN.md)).
- [x] Specify keyboard behavior, focus order, shortcuts policy, screen-reader names, contrast, and scaling behavior ([UI_DESIGN.md](UI_DESIGN.md)).
- [x] Confirm the notes panel and player remain usable at the minimum planned window size (`960×600` Full mode, `480×260` Minimum mode) ([UI_DESIGN.md](UI_DESIGN.md)).

**Acceptance:** existing Figma frames are mapped to product requirements; missing states have explicit specifications; the visual baseline and reusable components/tokens are documented; accessibility requirements are specified across screens.

### Sprint 0.3 — Architecture, security, and license baseline (Completed)

- [x] Review proposed Tauri/Rust/SQLite/GStreamer architecture and dependency boundaries ([ARCHITECTURE.md](ARCHITECTURE.md), [FEATURES.md](FEATURES.md), [ADR 0001](adr/0001-desktop-shell-and-playback-architecture.md)).
- [x] Review the threat model, local-data behavior, Tauri capability restrictions, logging policy, and backup expectations ([SECURITY.md](SECURITY.md), [ADR 0003](adr/0003-local-first-privacy-and-untrusted-boundary.md)).
- [x] Identify third-party license, codec, plugin, and patent questions requiring Phase 1 validation ([LICENSING.md](LICENSING.md), [ADR 0002](adr/0002-project-licensing-and-codec-policy.md)).
- [x] Confirm contribution governance, security contact plan, and release integrity requirements ([CONTRIBUTING.md](CONTRIBUTING.md), [SECURITY.md](SECURITY.md)).

**Acceptance:** architecture assumptions have owners and Phase 1 validation plans; the selected license and dependency policy are explicit; no unresolved design question is hidden in implementation scope.

**Phase 0 gate (Passed):** product and UX requirements are reviewed and aligned with the Figma design; Phase 1 questions, owners, and required evidence are recorded in [ARCHITECTURE.md](ARCHITECTURE.md) and [ADRs 0001–0003](adr/README.md).

## Phase 1 — Technical validation and detailed design (Completed)

**Goal:** demonstrate that the proposed desktop and playback approach can be built and distributed on the intended platforms.

### Sprint 1.1 — Platform and support matrix (Completed)

- [x] Select minimum macOS and Windows versions, CPU architectures, and representative Linux distributions ([PLATFORM_SUPPORT.md](PLATFORM_SUPPORT.md)).
- [x] Decide which Linux package formats and installation paths to support initially (`.AppImage` and `.deb`).
- [x] Define the public support matrix and how platform upgrades are evaluated ([PLATFORM_SUPPORT.md](PLATFORM_SUPPORT.md)).
- [x] Inventory signing/notarization requirements and available release infrastructure ([PLATFORM_SUPPORT.md](PLATFORM_SUPPORT.md)).

**Acceptance:** each platform target has a named CI/build path, package target, and support rationale.

### Sprint 1.2 — Playback backend feasibility (Completed)

- [x] Validate candidate backend audio and video discovery, playback state machine, seeking, pause/resume, duration reporting, playback speed (`0.5×–2.0×`), and clean shutdown ([ADR 0001](adr/0001-desktop-shell-and-playback-architecture.md)).
- [x] Probe redistributable synthetic test media (`.wav`, `.mp4` H.264/Opus) with GStreamer 1.28.1 and classify expected unsupported formats (`AV-104`).
- [x] Verify UI embedding and event behavior, including stale load-generation suppression and failed/corrupt files.

**Acceptance:** evidence exists for required player behaviors, with known limitations and failure states documented.

### Sprint 1.3 — Packaging and licensing feasibility (Completed)

- [x] Inventory exact runtime libraries, GStreamer 1.28.1 plugins, codecs, and their LGPL licenses ([FORMAT_AND_LICENSE_MATRIX.md](FORMAT_AND_LICENSE_MATRIX.md)).
- [x] Assess LGPL dynamic-linking obligations and exclude `gst-plugins-ugly` and GPL plugins ([FORMAT_AND_LICENSE_MATRIX.md](FORMAT_AND_LICENSE_MATRIX.md), [`deny.toml`](../deny.toml)).

**Acceptance:** package contents and license obligations are documented; unsupported or legally unclear components are excluded.

### Sprint 1.4 — Persistence and recovery design (Completed)

- [x] Confirm database location, permissions, schema migration approach (`src-tauri/migrations/0001_initial.sql`), pre-migration backup behavior, and corruption recovery UX ([PLATFORM_SUPPORT.md](PLATFORM_SUPPORT.md), [ERROR_CATALOG.md](ERROR_CATALOG.md)).
- [x] Specify media identity, moved/missing file states, relinking confidence (`AV-201` / `AV-202`), and note retention ([WORKFLOWS.md](WORKFLOWS.md)).

**Acceptance:** migration and recovery design preserves notes; media identity rules cannot silently merge unrelated items; backup behavior is testable.

### Sprint 1.5 — Architecture closure (Completed)

- [x] Finalize UI framework (React + TypeScript `strict` + semantic CSS tokens), backend (`PlaybackBackend` trait + GStreamer adapter), package targets, command/event contracts, and module responsibilities ([ADR 0001](adr/0001-desktop-shell-and-playback-architecture.md)).
- [x] Complete Light theme token specification ([UI_DESIGN.md](UI_DESIGN.md)) and user-facing error catalog ([ERROR_CATALOG.md](ERROR_CATALOG.md)).
- [x] Update architecture, licensing, platform support, and ADR docs with validation findings.

**Phase 1 gate (Passed):** playback, packaging/LGPL, persistence recovery, and security designs are validated and documented.

## Phase 2 — Engineering foundation

**Goal:** create a reliable development and release foundation.

### Sprint 2.1 — Workspace and build setup

- Create the Rust workspace and frontend project according to the accepted architecture.
- Pin toolchain and package manager versions; document prerequisites and commands.
- Establish formatting, linting, type checking, and baseline CI.
- Ensure a clean checkout can build development packages on supported CI targets.

**Acceptance:** reproducible documented local setup; CI reports build status per target; no release secrets are exposed to untrusted changes.

### Sprint 2.2 — Application boundaries and command contracts

- Implement shell startup/shutdown and narrow typed command/event boundaries.
- Enforce Tauri capability and content security policies.
- Add typed error categories and redacted structured logging.
- Add domain interfaces and fake adapters for independent testing.

**Acceptance:** frontend cannot access arbitrary filesystem or shell operations; command inputs are validated in Rust; errors map to stable user-facing categories.

### Sprint 2.3 — Persistence foundation

- Implement SQLite connection, migration runner, repositories, and transaction policy.
- Add schema versioning, foreign keys, backup routine, and recovery error flow.
- Establish dependency, vulnerability, license, and SBOM checks in CI.

**Acceptance:** migration failure preserves recoverable data; database behavior matches the approved design; CI flags dependency/license changes for review.

**Phase 2 gate:** clean builds, documented development workflow, persistence safeguards, security boundaries, and CI are operational on supported targets.

## Phase 3 — Playback and library

**Goal:** deliver responsive and dependable local media playback.

### Sprint 3.1 — Media open and library records

- Implement native picker, drag/drop, and open-with entry points in scope.
- Probe media without blocking UI; persist media references and metadata.
- Add unavailable, unreadable, and unsupported states.

**Acceptance:** user-selected media is represented in the library; errors do not freeze the app or alter source files.

### Sprint 3.2 — Playback state machine and transport

- Implement load/play/pause/seek/ended/failed transitions and generation identifiers.
- Add position/duration, volume/mute, and transport UI.
- Handle backend event translation and orderly shutdown.

**Acceptance:** transport state stays consistent with backend state; stale events cannot replace the current item; controls remain responsive during I/O.

### Sprint 3.3 — Queue, audio-only experience, and platform behavior

- Implement current-session queue and agreed repeat/shuffle behavior.
- Complete audio-only presentation and basic track/capability UI.
- Validate dialogs, file associations, drag/drop, and audio/video output per platform.

**Acceptance:** documented playback scenarios pass on each target OS; known format support is accurately communicated.

**Phase 3 gate:** opening, playing, seeking, queueing, and failure recovery work across targets without blocking the interface or risking library data.

## Phase 4 — Timestamped notes and organization

**Goal:** complete the defining note-and-return workflow.

### Sprint 4.1 — Note persistence and capture

- Implement note model and transactional create/update/delete operations.
- Capture current playback position once when creating a note.
- Add quick marker and text note interactions.

**Acceptance:** notes have stable IDs and integer-millisecond positions; invalid/oversized inputs are rejected; note changes survive restart.

### Sprint 4.2 — Notes panel and seek-back flow

- Implement chronological per-media note list and edit/delete UX.
- Activate a note to seek and show clear completion/seek feedback.
- Handle unknown duration and out-of-range positions according to approved rules.

**Acceptance:** note activation returns to the intended timestamp; UI reports failed seeks without losing the note.

### Sprint 4.3 — Search and media recovery

- Search media titles and note text locally; add filters and sort behavior.
- Preserve notes for missing media and implement explicit relinking.
- Add ambiguity handling; prohibit silent record merges.

**Acceptance:** searches work offline; missing media does not erase notes; relinking preserves associations and asks the user when identity is ambiguous.

### Sprint 4.4 — Accessibility and settings

- Implement theme, playback preferences, and timestamp precision settings.
- Complete keyboard and screen-reader flow for opening, playing, noting, searching, and returning.
- Review contrast, scaling, reduced motion, and minimum window behavior.

**Phase 4 gate:** full note workflow, persistence, search, relocation, settings, and accessibility acceptance criteria pass.

## Phase 5 — Hardening and release candidate

**Goal:** turn feature-complete software into a safe, supportable installable product.

### Sprint 5.1 — Data resilience and performance

- Exercise migration from supported prior versions, backup/restore, corruption handling, and crash recovery.
- Measure startup, open latency, seek response, long-session memory, and UI responsiveness.
- Fix data-loss, responsiveness, and reliability issues before tuning noncritical performance.

**Acceptance:** defined performance targets are met or exceptions are documented; recovery paths retain notes.

### Sprint 5.2 — Security, accessibility, and compatibility review

- Review Tauri capabilities, media parsing inputs, IPC validation, logs, dependencies, and update design.
- Run manual accessibility review with keyboard and assistive technologies.
- Verify supported formats and representative media on all OS targets.
- Resolve release-blocking findings.

**Acceptance:** no unresolved critical/high security or accessibility blocker; documented format matrix reflects tested reality.

### Sprint 5.3 — Installer and release candidate

- Finalize installer branding, upgrade/uninstall behavior, signing/notarization, SBOM, notices, checksums, and provenance.
- Prepare support, privacy, license, troubleshooting, and release-note documents.
- Publish release candidate packages for review and collect structured feedback.

**Acceptance:** a clean-machine reviewer can install, use, update/upgrade where supported, and uninstall using published instructions.

**Phase 5 gate:** all stable-release acceptance criteria in PRODUCT.md pass; remaining issues are triaged and no data-loss, licensing, or security blocker remains.

## Phase 6 — Stable release and maintenance

### Sprint 6.1 — Stable release

- Freeze scope and verify release branch from clean CI.
- Publish source and platform packages with signatures or verification data, checksums, SBOM/notices, format matrix, and release notes.
- Announce support and security reporting policies.

**Acceptance:** release artifacts match approved source and package inventories; installation documentation is complete.

### Ongoing maintenance sprints

- Triage defects and security reports under published response policy.
- Update dependencies and media plugins with regression and license review.
- Maintain supported OS matrix and migration compatibility.
- Prioritize user-impacting playback, note integrity, accessibility, and installer defects.
- Evaluate post-release features through requirements and ADR updates before scheduling.

## Cross-phase Definition of Done

A sprint item is done only when:

- Its acceptance criteria are met and reviewed.
- Relevant OS behavior, errors, keyboard access, and persistence effects are addressed.
- Security and license implications are reviewed for new dependencies, permissions, network access, and package contents.
- User-facing behavior and engineering docs are updated.
- CI and agreed manual checks pass; known gaps are recorded with an owner and follow-up.
- No secrets, private media, or personal user data are included in the change.

## Suggested tracking fields

For each issue or sprint card, track: phase, user outcome, acceptance criteria, dependencies, owner, target platforms, data migration impact, security/license impact, validation evidence, and status. Mark blockers explicitly and link the decision or risk record.
