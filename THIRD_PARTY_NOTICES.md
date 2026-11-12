# Third-Party Notices

Audora is distributed under the **Apache License, Version 2.0** ([LICENSE](LICENSE)).

This file documents the third-party open-source components used by Audora, their licenses, and instructions for replacing dynamically linked **LGPL-2.1+** multimedia libraries in accordance with [ADR 0002](.docs/adr/0002-project-licensing-and-codec-policy.md) and [FORMAT_AND_LICENSE_MATRIX.md](.docs/FORMAT_AND_LICENSE_MATRIX.md).

---

## 1. Rust Application Dependencies (`src-tauri/Cargo.toml`)

| Crate | License | Usage |
| :--- | :--- | :--- |
| `rusqlite` (with bundled `libsqlite3`) | `MIT` (SQLite is Public Domain) | Local SQLite WAL persistence and schema migrations |
| `serde` & `serde_json` | `MIT OR Apache-2.0` | Typed IPC command and settings serialization |
| `thiserror` | `MIT OR Apache-2.0` | Structured `AV-*` domain error definitions |
| `tracing` & `tracing-subscriber` | `MIT` | Redacted local diagnostic logging |

All Rust dependencies are verified in CI via [`deny.toml`](deny.toml). `GPL`, `AGPL`, and `SSPL` crates are strictly denied.

---

## 2. Frontend Dependencies (`ui/package.json`)

| Package | License | Usage |
| :--- | :--- | :--- |
| `react` & `react-dom` | `MIT` | User interface rendering (`ui/`) |
| `vite` & `vitest` (dev/build only) | `MIT` | Frontend bundling and unit testing |
| `typescript` (dev/build only) | `Apache-2.0` | Static type checking |

---

## 3. Dynamically Linked Multimedia Runtime (GStreamer 1.28.1)

Audora uses **GStreamer 1.28.1** (`core`, `gst-plugins-base`, `gst-plugins-good`, and LGPL plugins from `gst-plugins-bad`) under the **GNU Lesser General Public License v2.1 or later (`LGPL-2.1-or-later`)**.

- **Excluded Sets:** `gst-plugins-ugly` and all GPL-licensed plugins are excluded from Audora distributions.
- **Source Code Availability:** Upstream GStreamer source code for version `1.28.1` is available at <https://gstreamer.freedesktop.org/src/>.

### How to Replace or Relink GStreamer Dynamic Libraries (LGPL-2.1 Compliance)

Because GStreamer is linked dynamically as shared libraries (`.dylib` on macOS, `.dll` on Windows, `.so` on Linux), users are free to inspect, modify, or replace the GStreamer shared libraries used by Audora:

1. **macOS (`Audora.app`):**
   - Shared libraries reside in `Audora.app/Contents/Frameworks/GStreamer.framework/Versions/1.0/lib/` (or `/Library/Frameworks/GStreamer.framework/Versions/1.0/lib/`).
   - Replace the target `.dylib` file with your compatible custom-built `1.28.x` `.dylib` and re-sign the local bundle for macOS Gatekeeper using:
     ```bash
     codesign --force --deep --sign - /Applications/Audora.app
     ```
2. **Windows (`.msi` install):**
   - Shared libraries (`gst*.dll`) reside in the application directory (`%ProgramFiles%\Audora\bin\`).
   - Replace the target `.dll` with a compatible `x86_64` GStreamer `1.28.x` build.
3. **Linux (`.deb` / `.AppImage`):**
   - On `.deb` installations, Audora links against the system's shared `/usr/lib/<arch>-linux-gnu/libgstreamer-1.0.so.0`, or you can override the library search path at launch via `LD_LIBRARY_PATH=/path/to/custom/gstreamer/lib audora`.
