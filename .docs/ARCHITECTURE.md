# Architecture

## Shipped baseline ([ADR 0004](adr/0004-native-webview-loopback-architecture.md))

Audora combines a **Rust application core** (`audora-core`) and a **native `tao` + `wry` desktop shell** (`audora-desktop`) with a **React + TypeScript (`strict`) UI** (`apps/desktop/ui`). Local metadata, source folders, playlists, playback progress, settings, and timestamped notes are persisted in a **SQLite WAL database** via `rusqlite`.

Media playback renders through the platform OS WebView's hardware-accelerated HTML5 `<video>` / `<audio>` stack streamed from a per-launch, 256-bit CSPRNG token-authenticated **`127.0.0.1:0` HTTP Range server**, paired with a **pure-Rust 12-format container prober** for fast read-only header/duration inspection ([ADR 0004](adr/0004-native-webview-loopback-architecture.md), superseding the initial Tauri 2 macro + linked `gstreamer-rs` spike in [ADR 0001](adr/0001-desktop-shell-and-playback-architecture.md)). `audora-core` also retains a `PlaybackBackend` trait (`FakePlaybackBackend` + optional CLI `GStreamerBackend` wrapping `gst-discoverer-1.0`/`ffprobe`) for headless service tests and extended CLI inspection.

## Runtime topology

```text
┌───────────────────────────────────────────────────────────┐
│ Native tao + wry window (Full & Minimum modes)            │
│ Hybrid 56px OS header + React/TypeScript UI               │
│ HTML5 <video>/<audio> + relational IPC client             │
└───────────────┬───────────────────────────┬───────────────┘
                │ window.ipc.postMessage    │ 127.0.0.1:0 HTTP + X-Audora-Token
                │ (window mode, pickers,    │ (/__audora_rpc, /__audora_state,
                │  quit_with_batch)         │  /__audora_media Range 206, /__audora_update)
┌───────────────▼───────────────────────────▼───────────────┐
│ Rust desktop shell & application core                     │
│ MediaAllowlist · pure-Rust container prober · self-update │
└─────────────────────────────┬─────────────────────────────┘
                              │ parameterized SQL (WAL + FK)
                ┌─────────────▼──────────────┐
                │ SQLite relational store    │
                │ media · notes · playlists  │
                │ settings · session_state   │
                └────────────────────────────┘
```

The frontend is treated as untrusted ([ADR 0003](adr/0003-local-first-privacy-and-untrusted-boundary.md)). Every loopback HTTP endpoint requires a constant-time match on the per-launch 256-bit `X-Audora-Token`, strict `Host` header validation (`127.0.0.1:<port>`), and `RUNTIME_CSP` (`default-src 'self'; object-src 'none'; frame-ancestors 'none'`). `GET /__audora_media` streams only canonicalized local files explicitly registered in Rust's `MediaAllowlist` via OS file/folder pickers, native drag-and-drop, CLI launch arguments, or persisted library records. `tauri.conf.json` and `capabilities/default.json` are retained as release-gate metadata verified by `scripts/verify-release.mjs`.

## Window shell and presentation modes

- **Hybrid 56 px header:** Uses native OS window decorations in Full Mode paired with native titlebar drag handling (`drag_window`) and frameless always-on-top compact controls in Minimum Mode.
- **Full mode vs. Minimum mode:** Managed as window layout/size states on a single persistent `tao` window + `wry` WebView (`min 1040×640` for Full mode; `360×168` frameless always-on-top for Minimum mode) so toggling modes never interrupts active audio/video playback or loses playhead state.

## Threading, streaming, and shutdown

- UI rendering and HTML5 media element control run on the WebView event loop; Rust handles native OS pickers, bounded multi-threaded HTTP Range streaming (`MAX_CONCURRENT_HTTP_CLIENTS = 64`), container probing, and SQLite transactions on background threads.
- Active playback state, finite-permutation shuffle queue (`repeat`: `off | all | one`), and playback rate (`0.5×–2.0×`) are coordinated by the UI/IPC layer and persisted to SQLite via serialized `POST /__audora_rpc` calls.
- Database writes (including debounced `save_progress` checkpoints and `Immediate` batch transactions on `POST /__audora_rpc`) use short parameterized transactions with `PRAGMA foreign_keys = ON` and `WAL` mode.
- Graceful shutdown (`CloseRequested` → `__AUDORA_ON_CLOSE_REQUESTED__` → `quit_with_batch`, with a bounded native fallback timeout) stops accepting new loopback connections, drains in-flight HTTP/RPC workers, commits the final session/progress batch transactionally, and checkpoints the WAL (`TRUNCATE`) before exit.

## Playback state model

`Idle → Loading → Playing ↔ Paused`; `Playing/Paused → Seeking → prior state`; `Playing → Ended`; any active state may transition to `Failed`; opening another item begins a new load generation so stale asynchronous metadata/error events from a prior item cannot overwrite active state. Audio-only `.mp4`/`.mov`/`.webm` files whose `loadedmetadata` reports `videoWidth === 0 && videoHeight === 0` automatically switch to the dedicated audio stage while syncing `media_kind = "audio"` to SQLite.

## Data and storage

SQLite lives in the per-user application data directory (`com.audora.desktop`), with foreign keys enabled, WAL journaling, versioned migrations (`0001_initial.sql` through `0004_relational_unification.sql`), automatic `.pre-v<N>.bak` snapshots before schema upgrades, manual `.manual.bak` snapshots, and `.corrupt.bak` quarantine on recovery. Media bytes and note contents are never written to logs.

Core relational tables (`0001`–`0004` migrations):

- `media(id, path, display_name, media_kind, duration_ms nullable, last_position_ms, completed, file_size_bytes nullable, fingerprint nullable, availability, added_at, last_opened_at nullable, last_seen_at, source_folder_id nullable, duration_probed)`
- `notes(id, media_id, position_ms, is_quick_marker, body, created_at, updated_at)`
- `playlists(id, name, description, created_at, updated_at)` and `playlist_items(playlist_id, media_id, position, added_at)`
- `settings(key, value_json, updated_at)` (stores user settings plus serialized `'playback_session_v1'`)
- `app_ui_state(key, payload_json, updated_at)` (legacy v3 snapshot table migrated automatically into v4 relational tables)
- `schema_migrations(version, applied_at)`

## Media identity and relocation

Application IDs (`MediaId`) are stable UUIDs. Paths are locators and can become stale. When an item disappears on disk, Audora retains its metadata, progress, playlists, and notes in a `Missing` state (`AV-101`). Relinking (`relinkMedia`) validates local path safety (`AV-301`), rejects silent merges when a selected replacement path already belongs to another `MediaId` (`AV-201`), warns with old-vs-new details before relinking across mismatched media kind or duration (`AV-202`), updates `file_size_bytes` and probed `duration_ms`, and preserves every timestamped note (badging notes past a shorter replacement duration as `Out of range`).

## Native IPC and loopback endpoints

- **Window IPC (`window.ipc.postMessage`):** `set_window_mode`, `drag_window`, `pick_media_files`, `pick_media_folder`, `reveal_in_folder`, `open_external_url` (`https://` allowlisted), `quit_with_batch`.
- **Loopback HTTP (`127.0.0.1:<port>` with `X-Audora-Token`):**
  - `GET /__audora_state`: Relational SQLite bootstrap (`media`, `notes`, `playlists`, `source_folders`, `settings`, `session`) with automatic legacy `ui_kv_store` one-time migration.
  - `POST /__audora_rpc`: Validated relational mutations (`upsert_media_batch`, `delete_media_ids`, `update_progress`, `upsert_note`, `delete_note`, `upsert_playlist`, `delete_playlist`, `save_settings`, `revoke_source_folder`, `save_session`, `diagnostic_report`, `batch`).
  - `POST /__audora_state` (`X-Audora-Action: backup`): Checkpoints WAL and atomically writes `audora.sqlite.manual.bak`.
  - `DELETE /__audora_state`: Clears library/notes/playlists/settings/session, runs `VACUUM` + `PRAGMA wal_checkpoint(TRUNCATE)`, removes manual/pre-migration backup artifacts, and clears `MediaAllowlist`.
  - `GET /__audora_media`: Read-only streaming (`200 OK` / `206 Partial Content` / `416 Range Not Satisfiable`) restricted to `MediaAllowlist`.
  - `POST /__audora_update`: Stages GitHub Release metadata, downloads HTTPS installer archive with mandatory SHA-256 verification against the release manifest, and applies OS-appropriate update handoff.

## Failure handling

Backend, filesystem, database, network-path, and decoder errors map into stable `AV-*` error codes ([ERROR_CATALOG.md](ERROR_CATALOG.md)) with actionable user recovery guidance and redacted local diagnostics (`Export diagnostic report`). A corrupt or unsupported media file (`AV-103` / `AV-104`) never blocks playback of other items in the library or queue.
