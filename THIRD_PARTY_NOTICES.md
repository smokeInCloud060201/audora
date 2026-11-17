# Third-Party Notices

Audora is distributed under the **Apache License, Version 2.0** ([LICENSE](LICENSE)).

This file documents the third-party open-source components used by Audora and their licenses in accordance with [ADR 0002](.docs/adr/0002-project-licensing-and-codec-policy.md) and [FORMAT_AND_LICENSE_MATRIX.md](.docs/FORMAT_AND_LICENSE_MATRIX.md).

---

## 1. Rust Application Dependencies (`Cargo.toml` / `Cargo.lock`)

| Crate | Version | License | Usage |
| :--- | :--- | :--- | :--- |
| `rusqlite` & `libsqlite3-sys` | `0.32.1` / `0.30.1` | `MIT` (SQLite is Public Domain) | Local SQLite WAL persistence, backups, and schema migrations |
| `serde` & `serde_json` | `1.0.229` / `1.0.154` | `MIT OR Apache-2.0` | Typed IPC command, state, and manifest serialization |
| `sha2` | `0.10.9` | `MIT OR Apache-2.0` | Cryptographic SHA-256 verification and session token generation |
| `tao` | `0.31.1` | `Apache-2.0` | Cross-platform native OS window creation and event loop |
| `wry` | `0.48.1` | `Apache-2.0 OR MIT` | Cross-platform system WebView binding (`WKWebView`, `WebView2`, `WebKitGTK`) |
| `thiserror` | `2.0.21` | `MIT OR Apache-2.0` | Structured `AV-*` domain error definitions |
| `tracing` & `tracing-subscriber` | `0.1.44` / `0.3.23` | `MIT` | Redacted local diagnostic logging |

All Rust dependencies are audited in CI via [`deny.toml`](deny.toml) (`cargo-deny`). `GPL`, `AGPL`, and `SSPL` crates are strictly denied.

---

## 2. Frontend Dependencies (`apps/desktop/ui/package.json`)

| Package | Version | License | Usage |
| :--- | :--- | :--- | :--- |
| `react` & `react-dom` | `19.3.1` | `MIT` | User interface rendering (`apps/desktop/ui/`) |
| `vite` & `vitest` (dev/build only) | `6.4.6` / `2.1.9` | `MIT` | Frontend bundling and unit testing |
| `typescript` (dev/build only) | `5.9.3` | `Apache-2.0` | Static type checking |

---

## 3. System WebView & Optional Dynamically Linked Multimedia Runtime

Audora renders media playback through the operating system's native WebView engine (`WKWebView` on macOS, Microsoft Edge `WebView2` on Windows, and `WebKitGTK 4.1` on Linux) and optionally probes local stream metadata via `gst-discoverer-1.0` when GStreamer (`LGPL-2.1-or-later`) is installed on the host system:

- **Excluded Sets:** `gst-plugins-ugly` and all GPL-licensed codec plugins are excluded from Audora distributions.
- **Dynamic Linking / User Replacement (LGPL-2.1):**
  - On **Linux (`.deb`)**, `WebKitGTK` and GStreamer shared libraries (`/usr/lib/<arch>-linux-gnu/libgstreamer-1.0.so.0`) are dynamically linked from system packages and can be overridden via `LD_LIBRARY_PATH`.
  - On **macOS** and **Windows**, Audora does not statically link GStreamer; if GStreamer is installed on the host (`/Library/Frameworks/GStreamer.framework` or in `PATH`), users remain free to inspect, upgrade, or replace those shared libraries independently.
