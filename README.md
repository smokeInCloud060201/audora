<div align="center">
  <img src="assets/icons/icon-1024.svg" alt="Audora Logo" width="96" height="96" />

  <h1>Audora</h1>

  <p>
    <strong>Local-first desktop audio &amp; video player with effortless timestamped notes.</strong><br />
    Listen, watch, bookmark key moments, organize playlists, and jump straight back — 100% offline on macOS, Windows, and Linux.
  </p>

  <p>
    <a href="https://github.com/smokeInCloud060201/audora/actions/workflows/ci.yml"><img src="https://github.com/smokeInCloud060201/audora/actions/workflows/ci.yml/badge.svg" alt="CI Status" /></a>
    <a href="https://github.com/smokeInCloud060201/audora/releases"><img src="https://img.shields.io/github/v/release/smokeInCloud060201/audora?include_prereleases&label=release" alt="Latest Release" /></a>
    <a href="LICENSE"><img src="https://img.shields.io/badge/license-Apache--2.0-0d5c4c" alt="License: Apache 2.0" /></a>
    <img src="https://img.shields.io/badge/platform-macOS%20%7C%20Windows%20%7C%20Linux-1f2937" alt="Platforms: macOS, Windows, Linux" />
  </p>

  <p>
    <a href="https://github.com/smokeInCloud060201/audora/releases">Download</a> ·
    <a href="#features">Features</a> ·
    <a href="#keyboard-shortcuts">Shortcuts</a> ·
    <a href="#installation">Install</a> ·
    <a href="#architecture">Architecture</a> ·
    <a href=".docs/USER_GUIDE.md">User Guide</a>
  </p>
</div>

---

## Overview

**Audora** pairs a clean, distraction-free desktop media player with structured, timestamped note-taking. Whether you are reviewing lectures, analyzing interviews, practicing music, studying language recordings, or auditing video cuts, Audora lets you mark exact timestamps without breaking flow — and return to any moment with a single click.

Every library entry, playlist, and note stays on your device inside a local SQLite database. Audora requires **no account**, collects **zero telemetry**, and blocks remote streaming URLs by design.

---

## Features

- **Focused Audio & Video Playback**
  - Dedicated video stage and waveform-styled audio canvas with **Dark**, **Light**, and **System** themes.
  - Persistent playback across navigation so audio never stops while browsing your Library or Settings.
  - Variable playback speed (`0.5×` – `2×`), timeline note markers, **Repeat** (`off` / `all` / `one`), **Shuffle**, headphone-disconnect auto-pause, and resume-where-you-left-off.
- **Timestamped Notes & Quick Markers**
  - Capture detailed timestamped notes (`N`) or zero-friction quick markers (`Shift + N`) without losing your place.
  - Click any note or timeline marker to **seek and auto-play** from that exact timestamp immediately.
  - Configurable timestamp precision (`nearest second` or `milliseconds`) and instant offline search across titles and note text (`⌘K` / `Ctrl + K`).
- **3-Column Workspace & Multi-Playlists**
  - Docked, collapsible **Playlists** sidebar on the left (`[`) and **Notes & Queue** sidebar on the right (`]`).
  - Create unlimited playlists, snapshot your active playback queue into a playlist, reorder tracks, and launch playlist playback in normal or shuffled order.
- **Trusted Local Media Sources**
  - Link local folders in **Settings → Local sources** (Spotify Local Files workflow) to automatically discover and sync audio/video files.
  - Toggle individual folders on or off in your Library at any time, or remove a source folder while automatically preserving any media with saved notes.
  - Smart canonical-path deduplication ensures opening a file manually never duplicates an item already imported from a source folder.
- **Compact Minimum Mode (`360 × 168`)**
  - Press `M` to switch into a frameless, always-on-top mini player while taking notes in your editor, browser, or slides.
- **Data Safety, Relinking & Verified Updates**
  - **Missing-File Recovery:** Moved or renamed files never orphan your notes — relink to the new path with duration/type safety checks while keeping every note intact.
  - **SQLite WAL Resilience:** Automatic pre-migration backups, one-click manual backups, and redacted diagnostic reports containing zero file paths or note text.
  - **Verified Self-Updates:** Built-in GitHub Releases updater with SHA-256 verification and one-click local app data cleanup.

---

## Supported Formats

| Media Kind | Supported Extensions |
| :--- | :--- |
| **Audio** | `.mp3`, `.wav`, `.flac`, `.m4a`, `.aac`, `.ogg`, `.opus` |
| **Video** | `.mp4`, `.m4v`, `.mov`, `.webm`, `.mkv` |

> Format decoding uses native platform media pipelines; see [Format & License Matrix](.docs/FORMAT_AND_LICENSE_MATRIX.md) for codec details per OS.

---

## Installation

### Prebuilt Installers

Download the latest signed/verified package for your platform from [**GitHub Releases**](https://github.com/smokeInCloud060201/audora/releases):

| Platform | Package | Installation |
| :--- | :--- | :--- |
| **macOS** *(macOS 13+, Apple Silicon & Intel)* | `Audora_<version>_macos-aarch64.dmg` | Open the `.dmg` and drag **Audora.app** to **Applications**. |
| **Linux** *(Ubuntu / Debian x86_64)* | `Audora_<version>_linux-x86_64.deb`<br>`Audora_<version>_linux-x86_64.tar.gz` | `sudo apt install ./Audora_<version>_linux-x86_64.deb` (or extract the portable `.tar.gz`). |
| **Windows** *(Windows 10/11 x86_64)* | `Audora_<version>_windows-x86_64-setup.zip`<br>`Audora_<version>_windows-x86_64.exe` | Extract the archive and launch `Audora.exe`. |

### Build from Source

#### Prerequisites

- **Rust** `1.80+` (`rustup`)
- **Node.js** `22+` and **pnpm** `10+`
- **Linux only** *(Ubuntu/Debian system packages)*:
  ```bash
  sudo apt-get update && sudo apt-get install -y \
    libwebkit2gtk-4.1-dev libgtk-3-dev libayatana-appindicator3-dev \
    librsvg2-dev patchelf libgstreamer1.0-dev libgstreamer-plugins-base1.0-dev \
    gstreamer1.0-plugins-base gstreamer1.0-plugins-good
  ```

#### Run Locally

```bash
# 1. Install frontend dependencies
pnpm install

# 2. Build the frontend bundle
pnpm build:ui

# 3. Launch the native desktop application
cargo run -p audora-desktop --release
```

For hot-reloading UI development in your browser / dev server:

```bash
pnpm dev:ui
```

---

## Keyboard Shortcuts

| Shortcut | Action |
| :--- | :--- |
| `Space` | Play / Pause |
| `←` / `→` | Skip backward / forward `5s` |
| `N` | Add a timestamped note at the current playback position |
| `Shift + N` | Drop an instant quick marker without pausing |
| `M` | Toggle between **Full Mode** and **Minimum Mode** (`360 × 168`) |
| `[` | Show / hide the left **Playlists** bar |
| `]` | Show / hide the right **Notes & Queue** bar |
| `⌘O` / `Ctrl + O` | Open local audio or video files from disk |
| `⌘K` / `Ctrl + K` | Focus Library & Note search |
| `Esc` | Close active modal/menu, or exit Minimum Mode |

---

## Architecture

Audora is structured as a strict, memory-safe Rust workspace paired with a lightweight React + TypeScript interface rendered in the operating system's native WebView (`tao` + `wry` / Tauri v2 architecture):

```text
audora/
├── crates/
│   └── audora-core/          # Pure Rust domain, services, SQLite WAL repos & migrations
├── apps/
│   └── desktop/
│       ├── src-tauri/        # Native OS shell (tao + wry), HTTP Range media streamer,
│       │                     # native file/folder pickers, self-updater & installer hooks
│       └── ui/               # React 19 + TypeScript (strict) + semantic CSS design tokens
├── scripts/
│   └── verify-release.mjs    # Automated CSP, SBOM, license & SHA-256 release gate
└── .docs/                    # Product specs, ADRs, security model & user documentation
```

### Engineering Guarantees

- **`#![forbid(unsafe_code)]`** enforced workspace-wide across all Rust crates, alongside `clippy::unwrap_used = "deny"` and `clippy::expect_used = "deny"`.
- **Zero-Telemetry & Strict CSP:** Remote network URLs (`http://`, `https://`, `rtsp://`, etc.) are rejected for media playback (`AV-301`); Content Security Policy forbids `object-src` and wildcard `connect-src`.
- **Clean Uninstallation:** Supports `audora --uninstall-cleanup`, OS package removal hooks, and **Settings → Privacy → Clean up local app data** to remove SQLite databases and caches cleanly without touching your media files on disk.

---

## Development & Verification

Run the full CI verification suite locally before opening a pull request or cutting a release:

```bash
# TypeScript strict check, Vitest unit suite, rustfmt, Clippy (-D warnings), and Cargo tests
pnpm check

# Build production UI assets and run the SBOM / CSP / checksum release gate
pnpm build:ui && pnpm verify:release
```

---

## Documentation

Detailed product, architecture, and operational guides live under [`.docs/`](.docs/README.md):

- [User Guide](.docs/USER_GUIDE.md) — Day-to-day playback, notes, playlists, and recovery workflows
- [Product Specification](.docs/PRODUCT.md) & [Core Workflows](.docs/WORKFLOWS.md)
- [System Architecture](.docs/ARCHITECTURE.md) & [Error Catalog](.docs/ERROR_CATALOG.md)
- [Security Model](.docs/SECURITY.md) & [Platform Support](.docs/PLATFORM_SUPPORT.md)
- [Contributing Guide](.docs/CONTRIBUTING.md) & [Coding Conventions](.docs/CODING_CONVENTIONS.md)

---

## License

Copyright © 2026 Audora Contributors.

Licensed under the [**Apache License, Version 2.0**](LICENSE). See [Licensing Policy](.docs/LICENSING.md), [Third-Party Notices](THIRD_PARTY_NOTICES.md), and [`sbom.cdx.json`](sbom.cdx.json) for dependency attributions.
