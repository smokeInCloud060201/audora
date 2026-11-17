# Platform and support matrix

This document completes Sprint 1.1 and defines Audora's supported operating systems, architectures, installer formats, window-shell integration, and CI targets for the v1.0 release line.

## Supported operating systems and architectures (v1.0)

| OS Family | Minimum Supported Version | Architectures | Package / Installer Formats | WebView Runtime |
|---|---|---|---|---|
| **macOS** | macOS 13.0 (Ventura) or newer | `aarch64-apple-darwin` (Apple Silicon), `x86_64-apple-darwin` (Intel) | `.dmg` disk image & `.app.tar.gz` update bundle containing signed `Audora.app` | `WKWebView` (OS built-in) |
| **Windows** | Windows 10 version 22H2 (Build 19045) or Windows 11 | `x86_64-pc-windows-msvc` (ARM64 evaluated post-1.0) | Per-user NSIS `-setup.exe` installer and `-portable.zip` archive | Microsoft Edge `WebView2` (Evergreen) |
| **Linux** | Ubuntu 22.04 LTS / 24.04 LTS, Fedora 40+ (glibc ≥ 2.35) | `x86_64-unknown-linux-gnu` | Debian/Ubuntu `.deb` and `.tar.gz` portable bundle | `WebKitGTK` (`webkit2gtk-4.1`) |

## Window shell and OS integration

### Hybrid 56 px header
- **macOS, Windows & Linux:** Renders the 56 px header with `data-tauri-drag-region` and native OS window decorations in Full Mode, and frameless always-on-top compact controls in Minimum Mode.
- **Window modes:**
  - **Full mode:** Default `1440 × 900`, freely resizable down to minimum `1040 × 640`.
  - **Minimum mode:** Compact bottom-right floating widget `360 × 168`.

### Per-user data and backup locations

| OS Family | SQLite Database, WebView & Backups Directory | Environment Override |
|---|---|---|
| **macOS** | `~/Library/Application Support/com.audora.desktop/` | `AUDORA_DATA_DIR` / `AUDORA_DB_PATH` |
| **Windows** | `%APPDATA%\com.audora.desktop\data\` | `AUDORA_DATA_DIR` / `AUDORA_DB_PATH` |
| **Linux** | `$XDG_DATA_HOME/com.audora.desktop/` (`~/.local/share/com.audora.desktop/`) | `AUDORA_DATA_DIR` / `AUDORA_DB_PATH` |

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
