# Architecture decision records

ADRs preserve why a consequential choice was made. Store one numbered Markdown file per decision, for example `0001-desktop-shell-and-playback-architecture.md`.

Each ADR contains: title/status/date; context and constraints; decision; alternatives considered; consequences and risks; evidence or validation required; and links to affected docs. Status is `Proposed`, `Accepted`, `Superseded`, or `Rejected`.

Create an ADR for choices that affect platform support, architecture/dependencies, licensing, security/privacy, persisted formats, user data, or long-term compatibility. Small implementation details do not need an ADR. When superseding a decision, retain the old record and link the replacement.

## Decision register

| ADR | Title | Status | Summary |
|---|---|---|---|
| [ADR 0001](0001-desktop-shell-and-playback-architecture.md) | Desktop shell, persistence, and playback architecture | `Accepted` | Tauri 2 + React/TS UI, hybrid 56 px OS header, Rust services, SQLite persistence, and `PlaybackBackend` trait with GStreamer 1.28.1 adapter (validated in Phase 1). |
| [ADR 0002](0002-project-licensing-and-codec-policy.md) | Project licensing, LGPL dynamic linking, and codec policy | `Accepted` | Apache-2.0 project license, permissive dependency preference, no GPL/AGPL in binaries, dynamic linking + SBOM audit for LGPL GStreamer plugins in Sprint 1.3. |
| [ADR 0003](0003-local-first-privacy-and-untrusted-boundary.md) | Local-first privacy and untrusted boundary model | `Accepted` | Zero default network/telemetry, read-only source media, untrusted webview IPC validation in Rust, plain-text note rendering, and redacted local logs. |
