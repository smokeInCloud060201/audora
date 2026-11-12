# Architecture

## Proposed baseline

Use a Rust application core with Tauri 2 for the desktop shell and a TypeScript UI (React is the initial UI proposal). Use a SQLite database through a Rust library for local metadata, playback progress, and notes. Put playback behind a Rust `PlaybackBackend` interface, with GStreamer as the initial backend candidate through `gstreamer-rs`.

This is a design decision to validate before feature implementation ([ADR 0001](adr/0001-desktop-shell-and-playback-architecture.md)). GStreamer is selected as a cross-platform framework candidate because it exposes an LGPL core and a plugin architecture; actual codec availability and redistribution obligations depend on the exact plugin set and build. It does not imply every format is supported. Phase 1 must build and package a minimal playback spike on all three OS families, verify video surface embedding inside/alongside Tauri 2, inventory codecs/plugins, and confirm installer behavior before committing to the backend. Record a new ADR if the outcome changes the choice.

## Runtime topology

```text
┌──────────────────────────────────────────┐
│ Tauri window (Full & Minimum modes)      │
│ Hybrid 56px OS header + TypeScript UI    │
└──────────────────┬───────────────────────┘
                   │ typed commands/events
┌──────────────────▼───────────────────────┐
│ Rust application services                │
│ playback · library · notes · settings    │
└─────────┬──────────────────────┬─────────┘
          │                      │
┌─────────▼─────────┐  ┌─────────▼──────────┐
│ Playback adapter  │  │ Persistence        │
│ GStreamer (initial│  │ SQLite + migrations│
│ candidate)        │  │ local app data     │
└─────────┬─────────┘  └────────────────────┘
          │ OS audio/video & route APIs
     macOS / Windows / Linux
```

The frontend is not trusted ([ADR 0003](adr/0003-local-first-privacy-and-untrusted-boundary.md)). Tauri capabilities and command allowlists are narrow. Commands accept explicit arguments, validate them, and return typed results. The frontend does not receive unrestricted filesystem or shell access.

## Window shell and presentation modes

- **Hybrid 56 px header:** Uses native OS window controls in their platform-standard locations (macOS traffic lights inset on the left via overlay titlebar; Windows and Linux caption controls on the right) paired with an explicit `data-tauri-drag-region`.
- **Full mode vs. Minimum mode:** Managed as window layout/size states on the active window (`min 960×600` for Full mode; `min 480×260` for Minimum mode) so toggling modes never tears down the active `PlaybackBackend` pipeline or interrupts audio/video output.

## Threading and state

- UI rendering remains on the frontend event loop. Blocking file probes, database work, and decoder setup run asynchronously or on worker threads.
- Playback service owns playback state, session queue (`repeat`, `shuffle`), and playback rate (`0.5×–2.0×`), serializing commands per active player instance.
- Backend callbacks are translated into app events and dispatched without blocking decoder threads.
- Database writes (including debounced `last_position_ms` progress checkpoints) use short transactions; no database lock is held while waiting for playback or UI.
- Shutdown requests orderly backend teardown, commits final playback progress, and flushes persistence operations within a bounded time.

## Playback state model

`Idle → Loading → Playing ↔ Paused`; `Playing/Paused → Seeking → prior state`; `Playing → Ended`; any active state may transition to `Failed`; opening another item begins a new `Loading` generation. Each load receives a generation identifier so stale asynchronous events from the prior item cannot update current state. Seek requests are ordered/coalesced where the backend supports it, and acknowledged position is sourced from backend clock rather than optimistic UI state.

## Data and storage

Use SQLite in the per-user application data directory, with foreign keys enabled, WAL where supported, schema migrations, and transactions. Database path and backup location follow OS conventions. Do not store media bytes or note contents in logs.

Initial logical schema:

- `media(id, path, display_name, media_kind, duration_ms nullable, last_position_ms, completed, fingerprint nullable, availability, added_at, last_opened_at nullable, last_seen_at)`
- `notes(id, media_id, position_ms, is_quick_marker, body, created_at, updated_at)`
- `settings(key, value_json, updated_at)`
- `schema_migrations(version, applied_at)`

Paths are platform-specific and are never assumed portable across machines. `last_position_ms` supports Library progress display (`%`) and `Resume where I left off`. A fingerprint (or size + mtime + duration tuple) assists re-linking as an advisory signal; automatic re-association requires a strong match, and mismatches prompt the user ([WORKFLOWS.md](WORKFLOWS.md)). Do not hash entire large files during open without consent. Schema changes must preserve notes and be tested against backups and prior supported versions.

## Media identity and relocation

Application IDs (`MediaId`) are stable. Paths are locators and can become stale. Store normalized path for matching, plus file size, media kind, duration, and modification metadata as advisory signals. When an item disappears, retain its metadata, progress, and notes in a `Missing` state. Relinking updates the locator only after validation (and user confirmation when duration/type differs). Never silently merge two library records if a selected file path already belongs to another `MediaId`.

## Frontend/backend interface

Expose narrow typed commands such as `open_media`, `playback_command` (play, pause, seek, set rate, set volume/mute, queue next/prev, set repeat/shuffle), `set_window_mode`, `get_playback_snapshot`, `create_note`, `update_note`, `delete_note`, `list_notes`, `search_library`, `relink_media`, `remove_media`, and `update_settings`. Events include playback state/position/rate, queue state, media availability changes, audio-route disconnect notifications (where supported by the OS), and operation completion. Avoid high-frequency frontend event floods: position updates are sampled to UI needs (e.g., 4–10 Hz while playing, plus immediate emission on state/seek changes) and can be queried on demand. The API layer validates sizes, IDs, enums, timestamps, and path permissions.

## Packaging model

- Build and bundle per target OS/architecture with reproducible CI inputs.
- Produce native installer/update artifacts (macOS app/disk image or package, Windows installer, Linux AppImage and/or distro package after target selection).
- Bundle only the explicitly approved GStreamer runtime/plugins and required platform libraries via dynamic linking compliant with LGPL ([ADR 0002](adr/0002-project-licensing-and-codec-policy.md)); publish an SBOM and notices.
- Sign/notarize where signing infrastructure and platform requirements allow; document unsigned developer builds.
- Installer must not require administrator privileges for per-user install when platform conventions permit.
- Updates are opt-in/transparent and cryptographically verified if an updater is introduced; updater is not in the initial architecture until its security design is approved.

## Failure handling

Map backend, file, database, and OS errors into stable app error categories (such as `AV-104` for unsupported decoder/codec) with safe user text and detailed redacted local logs. Preserve source error codes internally for diagnosis. Database-open or migration failure must preserve the original file and offer recovery/export instructions. The app must remain usable for unaffected media when one media item is corrupt.

## Phase 1 validation and ownership register

All architectural assumptions from Phase 0 have assigned owners and required Phase 1 validation evidence before Phase 2 implementation begins:

| Assumption / Question | Owner | Phase 1 Sprint | Required Evidence & Exit Criteria |
|---|---|---|---|
| **GStreamer video surface & seek/rate fidelity inside Tauri 2** across macOS, Windows, and Linux (Wayland/X11) | Project Maintainer (Playback) | Sprint 1.2 | Disposable spike plays H.264/AAC & WebM/Opus/WAV/MP3, renders video smoothly in Full mode, seeks accurately, supports `0.5×–2.0×` playback rate, and shuts down cleanly without leaks. |
| **GStreamer bundling & LGPL relinking compliance** | Project Maintainer (Release/Legal) | Sprint 1.3 | Test installers built on all 3 OSes; dynamic linking verified; complete plugin/codec inventory generated; GPL/patent-encumbered plugins excluded unless reviewed. |
| **Target OS versions, architectures & Linux package format** | Project Maintainer (Platform) | Sprint 1.1 | Support matrix published (min macOS, Windows 10/11 x64/ARM64 evaluation, Ubuntu/Fedora baseline, AppImage vs `.deb`/Flatpak decision). |
| **Hybrid 56 px header & Full ↔ Minimum window resize behavior** | Project Maintainer (UI/Shell) | Sprint 1.1 / 1.5 | Verified macOS overlay traffic lights, Windows/Linux window controls, drag regions, and seamless mode resize without playback interruption. |
| **OS headphone / audio-output disconnect detection** | Project Maintainer (Platform) | Sprint 1.2 | Documented per-OS API feasibility (CoreAudio on macOS, WASAPI on Windows, PipeWire/PulseAudio on Linux); fallback UX defined if unavailable on a Linux target. |
| **SQLite WAL, atomic backup & migration recovery** | Project Maintainer (Persistence) | Sprint 1.4 | Verified per-OS app-data paths, foreign-key enforcement, pre-migration backup copy, and simulated corruption/rollback recovery preserving notes. |
