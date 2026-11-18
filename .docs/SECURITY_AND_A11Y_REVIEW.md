# Security, Accessibility, and Compatibility Audit

This document records the security controls, accessibility verification, and cross-platform compatibility guarantees enforced in Audora.

---

## 1. Security Review Summary

| Area | Control Implemented | Verification Evidence | Status |
| :--- | :--- | :--- | :--- |
| **Local-First & Loopback Boundary** | Runtime `Content-Security-Policy` HTTP header (`default-src 'self'; script-src 'self'; object-src 'none'; frame-ancestors 'none'`), per-launch 256-bit CSPRNG `X-Audora-Token` constant-time auth, strict `Host` header check, bounded 64 KiB HTTP request body allocation, canonical `MediaAllowlist` rejecting unregistered files and symlinks to unsupported extensions, and `AV-301` rejection of remote/network media schemes (`http://`, `https://`, `rtsp://`, `ftp://`, `ws://`, `smb://`, `dav://`, `davs://`, `nfs://`, `afp://`, `file://`, and UNC `\\server\share`) | `tests::host_header_and_session_token_block_dns_rebinding_and_null_origin`, `tests::media_allowlist_blocks_unregistered_files_and_symlinks_to_non_media`, `tests::ipc_blocks_remote_network_urls_and_diagnostic_report_redacts_all_paths_and_notes`, `ipc.test.ts` | Passed |
| **Hardened Native Shell & Entitlements** | Direct `tao` + `wry` shell with narrow validated IPC messages (`set_window_mode`, `drag_window`, `pick_media_*`, `reveal_in_folder`, `https://`-only `open_external_url`, `quit_with_batch`); macOS `Entitlements.plist` contains zero JIT or unsigned-executable-memory entitlements; release builds resolve UI bundles strictly relative to the signed executable | Inspected `apps/desktop/src-tauri/src/main.rs`, `Entitlements.plist`, and `scripts/verify-release.mjs` | Passed |
| **Verified Self-Update Pipeline** | In-app updater fetches HTTPS GitHub Release metadata, matches exact OS + CPU architecture installer assets, requires a published 64-hex SHA-256 digest from `release-manifest-<platform>.json` (or `.sha256` manifest), verifies downloaded bytes before staging, and cleans up rollback/staging artifacts | `tests` in `apps/desktop/src-tauri/src/updater.rs` & `ipc.test.ts` | Passed |
| **Non-Destructive Media Handling** | Pure-Rust container probing (`probe_local_media_duration_ms`) and HTTP Range streaming open files strictly read-only; `0`-byte (`AV-103`) and unsupported (`AV-104`) files fail cleanly without touching source bytes | `tests::open_media_rejects_unsupported_format_with_av_104_and_preserves_source_file` | Passed |
| **Bounded Source Folder Scope** | Native folder scanning skips hidden dot-directories, enforces bounded traversal depth, and revokes removed source folders from `MediaAllowlist` (`revoke_source_folder`) | `tests::batch_folder_open_is_non_recursive_and_queue_respects_repeat_and_shuffle` & `main.rs` allowlist tests | Passed |
| **SQL Injection & XSS Prevention** | All SQLite queries use parameterized `rusqlite::params!` inside explicit transactions with `PRAGMA foreign_keys = ON`; all React note/title rendering uses plain text (zero `dangerouslySetInnerHTML`) | Inspected `crates/audora-core/src/db.rs` & `apps/desktop/ui/src/**` | Passed |
| **Log & Diagnostic Redaction** | `redact_path` strips directory prefixes; `generateDiagnosticReport` omits all file paths, filenames, and note bodies | `tests::ipc_blocks_remote_network_urls_and_diagnostic_report_redacts_all_paths_and_notes` & `ipc.test.ts` | Passed |
| **Database Corruption, Backup & Cleanup Safety** | Pre-migration `.pre-v<N>.bak`, atomic `.manual.bak` via `.tmp` rename after WAL checkpoint, `.corrupt.bak` (+ `-wal`/`-shm`) preservation on open failure, and `VACUUM` + `PRAGMA wal_checkpoint(TRUNCATE)` on full app-data cleanup | `tests::database_corruption_recovery_preserves_corrupt_file_and_restores_from_manual_backup` & `db.rs` tests | Passed |

---

## 2. Accessibility Review Summary

| Requirement | Implementation | Status |
| :--- | :--- | :--- |
| **Keyboard Navigation & Shortcuts** | `Space` (play/pause when focus is not on an interactive button/input/menu), `←/→` (±5s seek), `Shift+←/→` (±15s seek), `↑/↓` (±5% volume), `N` (add note at live playhead), `Shift+N` (instant quick marker + toast), `⌘/Ctrl+Enter` (save note), `⌘/Ctrl+K` (search), `⌘/Ctrl+O` (open media), `M` (toggle Minimum/Full mode), `Esc` (close dialog / exit Minimum mode) | Passed |
| **Shortcut Suppression in Interactive Controls** | Global shortcuts respect `e.defaultPrevented`, suppress single-key bindings inside `<input>`, `<textarea>`, `<select>`, `[contenteditable]`, and open modals, and preserve native `Space` activation on focused buttons, checkboxes, switches, tabs, and menu items | Verified in `format.test.ts` & `App.tsx` |
| **Modal Focus Trap & Background Inertness** | Open modals trap `Tab` / `Shift+Tab`, mark background workspace landmarks `inert` / `aria-hidden="true"`, and restore focus on close; destructive actions (`Remove from library`, `Clean up app data & cache`) require explicit confirmation dialogs | Verified in `App.tsx`, `PlaylistsView.tsx`, & `SettingsView.tsx` |
| **Screen-Reader Labels, Menus & Live Regions** | Explicit `aria-label`, `aria-current`, `aria-pressed`, `role="menu"` / `role="menuitem"`, `role="dialog"`, `role="alert"`, human-readable `aria-valuetext` on scrubbers, and `.sr-only` `aria-live="polite"` status announcer | Passed |
| **Hit Targets & Focus Rings** | Scrubber note markers provide a `24×24px` pointer/focus target; Minimum mode scrubber and all controls render visible `:focus-visible` outlines; WCAG AA contrast tokens across `Dark`, `Light`, and `System` themes | Verified in `tokens.css` & `app.css` |
| **Reduced Motion** | `@media (prefers-reduced-motion: reduce)` disables non-essential transitions and ambient animations | Verified in `app.css` |
