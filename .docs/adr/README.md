# Architecture decision records

ADRs preserve why a consequential choice was made. Store one numbered Markdown file per decision, for example `0001-desktop-shell-and-playback-architecture.md`.

Each ADR contains: title/status/date; context and constraints; decision; alternatives considered; consequences and risks; evidence or validation required; and links to affected docs. Status is `Proposed`, `Accepted`, `Superseded`, or `Rejected`.

Create an ADR for choices that affect platform support, architecture/dependencies, licensing, security/privacy, persisted formats, user data, or long-term compatibility. Small implementation details do not need an ADR. When superseding a decision, retain the old record and link the replacement.

## Decision register

| ADR | Title | Status | Summary |
|---|---|---|---|
| [ADR 0001](0001-desktop-shell-and-playback-architecture.md) | Desktop shell, persistence, and playback architecture | `Superseded in part` | Initial Phase 1 baseline (Tauri 2 + React/TS UI, Rust services, SQLite WAL persistence, `PlaybackBackend` trait); desktop shell & media transport superseded by [ADR 0004](0004-native-webview-loopback-architecture.md). |
| [ADR 0002](0002-project-licensing-and-codec-policy.md) | Project licensing, LGPL dynamic linking, and codec policy | `Accepted` | Apache-2.0 project license, permissive dependency preference, no GPL/AGPL in binaries, dynamic linking + SBOM audit for system/LGPL plugins. |
| [ADR 0003](0003-local-first-privacy-and-untrusted-boundary.md) | Local-first privacy and untrusted boundary model | `Accepted` | Zero default network/telemetry, read-only source media, untrusted webview IPC validation in Rust, plain-text note rendering, and redacted local logs. |
| [ADR 0004](0004-native-webview-loopback-architecture.md) | Native `tao` + `wry` shell, authenticated loopback media server, and pure-Rust container prober | `Accepted` | Direct `tao` + `wry` window shell, CSPRNG token-authenticated `127.0.0.1:0` HTTP Range server with `MediaAllowlist`, OS WebView HTML5 `<video>`/`<audio>` playback, and pure-Rust 12-format container duration prober. |
