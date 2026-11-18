# Platform and support matrix

This document defines Audora's supported operating systems, architectures, installer formats, window-shell integration, and CI targets.

## Supported operating systems and architectures

| OS Family | Minimum Supported Version | Architectures | Package / Installer Formats | WebView & Media Runtime |
|---|---|---|---|---|
| **macOS** | macOS 13.0 (Ventura) or newer | `aarch64-apple-darwin` (Apple Silicon primary CI release), `x86_64-apple-darwin` (source-supported) | `.dmg` disk image & `.app.tar.gz` update bundle containing signed `Audora.app` | `WKWebView` + `AVFoundation` / `VideoToolbox` (OS built-in) |
| **Windows** | Windows 10 version 22H2 (Build 19045) or Windows 11 | `x86_64-pc-windows-msvc` | Per-user NSIS `-setup.exe` installer and `-portable.zip` archive | Microsoft Edge `WebView2` (Evergreen) + Windows `Media Foundation` |
| **Linux** | Ubuntu 22.04 LTS / 24.04 LTS, Debian 12+, Fedora 40+ (glibc ≥ 2.35) | `x86_64-unknown-linux-gnu` | Debian/Ubuntu `.deb` and `.tar.gz` portable bundle | `WebKitGTK` (`libwebkit2gtk-4.1`) + system `gstreamer1.0-plugins-base/good` |

## Window shell and OS integration

### Hybrid 56 px header & window modes
- **Full mode:** Default `1440 × 900` (clamped to 92% of small displays on launch), freely resizable down to minimum `1040 × 640`, with native OS window controls and custom titlebar drag support (`drag_window`).
- **Minimum mode:** Compact always-on-top frameless floating widget `360 × 168` anchored near the bottom-right work area (`M` to enter/exit, `Esc` to return to Full mode).
- **File associations & native drag-and-drop:**
  - **macOS:** `Info.plist` registers `CFBundleDocumentTypes` for all 12 supported audio/video extensions, forwarding Finder "Open With" and Dock drops via `tao::event::Event::Opened`.
  - **Linux:** `audora.desktop` registers `Exec=/usr/bin/audora %F` and `MimeType` entries.
  - **All platforms:** Native OS drag-and-drop (`with_drag_drop_handler`) supports dropping both individual media files and source folders directly onto the Audora window.

### Per-user data and backup locations

| OS Family | SQLite Database, WebView & Backups Directory | Environment Override |
|---|---|---|
| **macOS** | `~/Library/Application Support/com.audora.desktop/` | `AUDORA_DATA_DIR` / `AUDORA_DB_PATH` |
| **Windows** | `%APPDATA%\com.audora.desktop\data\` (WebView2 profile in `%LOCALAPPDATA%\com.audora.desktop\webview2\`) | `AUDORA_DATA_DIR` / `AUDORA_DB_PATH` |
| **Linux** | `$XDG_DATA_HOME/com.audora.desktop/` (`~/.local/share/com.audora.desktop/`) | `AUDORA_DATA_DIR` / `AUDORA_DB_PATH` |

### Audio-output disconnect behavior (`Pause when headphones disconnect`)
- **All platforms (`WKWebView`, `WebView2`, `WebKitGTK`):** Monitored via the WebView's `navigator.mediaDevices` `devicechange` event and `enumerateDevices()` audio-output count tracking. When the count of connected `audiooutput` endpoints drops while media is actively playing (and `Pause when headphones disconnect` is enabled in **Settings → Playback**), Audora automatically pauses playback and announces `"Playback paused — audio output disconnected"`.
- **Platform note:** On desktop environments or WebView configurations that do not expose distinct `audiooutput` enumeration without microphone permission, route-drop auto-pause degrades gracefully without interrupting playback or prompting for microphone access.

## CI and release verification matrix

- **Pull request & push CI runners (`.github/workflows/ci.yml`):** `macos-latest` (`aarch64-apple-darwin`), `ubuntu-latest` (`x86_64-unknown-linux-gnu`), and `windows-latest` (`x86_64-pc-windows-msvc`), running `cargo fmt --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo-deny` across all target triples, `cargo test --workspace`, `pnpm audit --prod`, `pnpm typecheck`, `pnpm test:ui`, `pnpm build:ui`, and `pnpm verify:release`.
- **Signing, notarization, and provenance (`.github/workflows/release.yml`):**
  - macOS builds sign inner binaries, helper scripts, and `Audora.app` with Developer ID Application + hardened runtime (`Entitlements.plist` with zero JIT/unsigned-executable-memory entitlements) and notarize/staple when Apple credentials are configured; unsigned builds use ad-hoc signing (`codesign --sign -`) and document `xattr -cr` for local testing.
  - Windows builds sign `Audora.exe` and the per-user NSIS `-setup.exe` installer with Authenticode SHA-256 when `WINDOWS_CERTIFICATE` is configured.
  - Every tagged GitHub Release publishes per-platform `release-manifest-<platform>.json` files (carrying mandatory SHA-256 digests enforced by the in-app updater), `.sha256` checksum manifests, and GitHub Sigstore SLSA build provenance attestations.
