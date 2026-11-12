# ADR 0003: Local-first privacy and untrusted boundary model

- **Status:** Accepted
- **Date:** 2026-11-12
- **Deciders:** Project Maintainers

## Context and constraints

Users rely on Audora to annotate personal study materials, interviews, research recordings, and work-in-progress media. Both malicious/malformed media files and potential webview XSS bugs represent realistic desktop attack vectors.

## Decision

1. **Zero network / local-first operation by default:** Audora requires no account, makes **zero** network calls during media probing, playback, note editing, or search, and collects no telemetry or crash uploads by default. Any future opt-in update check or external documentation handoff must be explicitly initiated by the user.
2. **Non-destructive media handling:** Source media files are opened strictly read-only. Metadata, progress (`last_position_ms`), and notes live exclusively in Audora's SQLite database.
3. **Untrusted frontend boundary:** Treat the TypeScript webview as an untrusted client:
   - Enforce a strict Content Security Policy (no remote scripts, styles, or frames).
   - Disable general-purpose Tauri filesystem, shell, and HTTP plugins for the frontend.
   - Expose only narrow, domain-specific Rust commands (`open_media`, `create_note`, etc.) that validate payload sizes, types, IDs, and user-authorized path scopes in Rust.
   - Render all note bodies, filenames, and probed media metadata strictly as plain text (never `innerHTML` or unescaped HTML).
4. **Untrusted media & redacted diagnostics:** Treat filenames, paths, container metadata, and media streams as untrusted inputs. Parameterize all SQL queries. Redact note bodies and sensitive path details from application logs; diagnostic bundle export must be user-initiated with a preview of included files.

## Alternatives considered

- **Frontend-driven SQLite / filesystem plugins (`@tauri-apps/plugin-sql`, `@tauri-apps/plugin-fs`):** Reduces Rust boilerplate, but exposes broad SQL/file capabilities directly to the webview runtime and duplicates domain invariants in TypeScript. Rejected.
- **Mandatory at-rest database encryption in v1.0:** Protects against offline disk inspection, but introduces password/keychain lock-out data-loss risk for local notes. Deferred; OS user-account permissions protect the SQLite file, and this boundary is documented transparently in [SECURITY.md](../SECURITY.md) and Settings → Privacy.

## Consequences and risks

- Every feature requires a typed Rust command/DTO rather than ad-hoc frontend file/DB calls, slightly increasing boilerplate while keeping domain rules testable without a UI.
- If Phase 1 threat review identifies high-risk native decoder plugins, playback subprocess isolation may be evaluated via a follow-up ADR.

## Related documents

- [SECURITY.md](../SECURITY.md)
- [CODING_CONVENTIONS.md](../CODING_CONVENTIONS.md)
- [WORKFLOWS.md](../WORKFLOWS.md)
