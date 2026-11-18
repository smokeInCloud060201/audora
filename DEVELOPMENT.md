# Audora Development Guide

This guide is for developers who want to build and run Audora from source. For downloads and a product introduction, see the [main README](README.md); for using the app, see the [User Guide](.docs/USER_GUIDE.md).

## Prerequisites

- **Rust:** 1.89 or newer, with `rustfmt` and `clippy` (the stable toolchain is configured in `rust-toolchain.toml`).
- **Node.js:** 22 or newer.
- **pnpm:** 11.19.0, as declared in the root `package.json`.
- **Desktop WebView dependencies:**
  - **macOS:** Xcode Command Line Tools (`WKWebView` and `AVFoundation`/`VideoToolbox` are built into macOS).
  - **Windows:** Rust MSVC build tools and the Microsoft Edge `WebView2` Evergreen runtime (preinstalled on Windows 10 22H2+ and Windows 11).
  - **Linux:** `WebKitGTK 4.1` and `GTK 3` development headers for building the `tao`/`wry` shell, plus system GStreamer plugins used by `WebKitGTK` at runtime for media playback.
- **Optional CLI metadata probers:** Audora includes a built-in pure-Rust container header prober for all 12 supported audio/video formats; if `gst-discoverer-1.0` (GStreamer) or `ffprobe` (FFmpeg) is installed in `PATH`, `audora-core`'s optional CLI discovery adapter can also use them for headless inspection. No C GStreamer SDK is required to compile Audora.

### Linux dependencies (Ubuntu/Debian)

Install the WebKitGTK/GTK development packages and runtime codec plugins:

```bash
sudo apt-get update
sudo apt-get install -y \
  libwebkit2gtk-4.1-dev libgtk-3-dev libayatana-appindicator3-dev \
  librsvg2-dev patchelf \
  gstreamer1.0-plugins-base gstreamer1.0-plugins-good
```

The detailed platform setup and support matrix are in [`.docs/DEVELOPMENT.md`](.docs/DEVELOPMENT.md) and [`.docs/PLATFORM_SUPPORT.md`](.docs/PLATFORM_SUPPORT.md).

## Get the source

```bash
git clone https://github.com/smokeInCloud060201/audora.git
cd audora
pnpm install --frozen-lockfile
```

## Build and run

Build the frontend bundle, then start the native desktop application:

```bash
pnpm build:ui
cargo run -p audora-desktop --release
```

For UI-only development in a browser with Vite (includes local dev bridge endpoints for file/folder picking and byte-range streaming):

```bash
pnpm dev:ui
```

## Run checks

Run the project checks before submitting changes:

```bash
pnpm check
pnpm build:ui
pnpm verify:release
```

`pnpm check` runs the frontend typecheck and unit tests, Rust formatting and Clippy (`-D warnings`), and Rust workspace unit/integration tests. `pnpm verify:release` checks required release artifacts, manifest/SBOM version parity, and runtime Content Security Policy enforcement after the UI build.

## Project documentation

- [Development and delivery plan](.docs/DEVELOPMENT.md)
- [Architecture](.docs/ARCHITECTURE.md)
- [Architecture decision records](.docs/adr/README.md)
- [Contribution workflow](.docs/CONTRIBUTING.md)
- [Coding conventions](.docs/CODING_CONVENTIONS.md)
- [Security and privacy](.docs/SECURITY.md)
- [Platform support](.docs/PLATFORM_SUPPORT.md)
- [Media formats and licensing](.docs/FORMAT_AND_LICENSE_MATRIX.md)
- [Third-party notices](THIRD_PARTY_NOTICES.md)

## Contribution notes

Keep changes consistent with the project’s local-first privacy goals and architectural boundaries ([ADR 0003](.docs/adr/0003-local-first-privacy-and-untrusted-boundary.md), [ADR 0004](.docs/adr/0004-native-webview-loopback-architecture.md)). Treat media metadata and file paths as untrusted input. Do not log note contents or add telemetry, account requirements, or remote media playback without an explicit product and privacy review.
