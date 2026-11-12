# Platform and support matrix

This document completes Sprint 1.1 and defines Audora's supported operating systems, architectures, installer formats, window-shell integration, and CI targets for the v1.0 release line.

## Supported operating systems and architectures (v1.0)

| OS Family | Minimum Supported Version | Architectures | Package / Installer Formats | WebView Runtime |
|---|---|---|---|---|
| **macOS** | macOS 13.0 (Ventura) or newer | `aarch64-apple-darwin` (Apple Silicon), `x86_64-apple-darwin` (Intel) | `.dmg` disk image containing signed & notarized `Audora.app` | `WKWebView` (OS built-in) |
| **Windows** | Windows 10 version 22H2 (Build 19045) or Windows 11 | `x86_64-pc-windows-msvc` (ARM64 evaluated post-1.0) | Per-user NSIS `.exe` installer and `.msi` package | Microsoft Edge `WebView2` (Evergreen) |
| **Linux** | Ubuntu 22.04 LTS / 24.04 LTS, Fedora 40+ (glibc ≥ 2.35) | `x86_64-unknown-linux-gnu` | `.AppImage` and Debian/Ubuntu `.deb` | `WebKitGTK` (`webkit2gtk-4.1`) |

## Window shell and OS integration

### Hybrid 56 px header
- **macOS:** Uses Tauri `titleBarStyle: "Overlay"` with native traffic-light window controls (`Close`, `Minimize`, `Zoom`) positioned at the top-left of the 56 px header; the Audora brand mark shifts right (`padding-left: 76px`) to clear the native traffic lights.
- **Windows & Linux:** Renders the 56 px header with `data-tauri-drag-region` and right-aligned window controls (`Minimize`, `Maximize/Restore`, `Close`) matching native platform semantics.
- **Window modes:**
  - **Full mode:** Default `1440 × 900`, minimum `960 × 600`.
  - **Minimum mode:** Default `680 × 320`, minimum `480 × 260`.

### Per-user data and backup locations

| OS Family | SQLite Database & Backups Directory | Logs & Diagnostics Directory |
|---|---|---|
| **macOS** | `~/Library/Application Support/com.audora.app/` | `~/Library/Logs/com.audora.app/` |
| **Windows** | `%APPDATA%\com.audora.app\data\` | `%LOCALAPPDATA%\com.audora.app\logs\` |
| **Linux** | `$XDG_DATA_HOME/audora/` (`~/.local/share/audora/`) | `$XDG_STATE_HOME/audora/logs/` (`~/.local/state/audora/logs/`) |

### Audio-output disconnect behavior (`Pause when headphones disconnect`)
- **macOS:** Supported via CoreAudio default output device property listener + WebView `mediaDevices.ondevicechange`.
- **Windows:** Supported via WASAPI `IMMNotificationClient` endpoint state changes + WebView `mediaDevices.ondevicechange`.
- **Linux:** Supported via PipeWire / PulseAudio device notifications and WebView `mediaDevices.ondevicechange` where desktop session permissions expose audio-sink routing events. If a minimal Linux window manager does not expose route events, Settings displays an informational subtitle noting that hardware route detection depends on PipeWire/PulseAudio.

## CI and release verification matrix

- **Pull request CI runners:** `macos-latest` (`aarch64-apple-darwin`), `ubuntu-22.04` (`x86_64-unknown-linux-gnu`), and `windows-latest` (`x86_64-pc-windows-msvc`).
- **Signing and notarization:**
  - macOS builds sign with Developer ID Application certificate and Apple notary service when release credentials are present; unsigned developer builds document `xattr -cr` local testing instructions.
  - Windows builds sign with Authenticode certificate in release workflows; per-user installation does not require administrator elevation.
  - Linux packages publish SHA-256 checksums and detached signatures.
