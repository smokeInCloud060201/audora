# Audora Development Guide

This guide is for developers who want to build and run Audora from source. For downloads and a product introduction, see the [main README](README.md); for using the app, see the [User Guide](.docs/USER_GUIDE.md).

## Prerequisites

- **Rust:** 1.89 or newer, with `rustfmt` and `clippy` (the stable toolchain is configured in `rust-toolchain.toml`).
- **Node.js:** 22 or newer.
- **pnpm:** 11.19.0, as declared in the root `package.json`.
- **GStreamer:** development/runtime packages are needed for the playback backend. Install a supported GStreamer 1.28+ distribution for your OS and ensure its runtime tools and libraries are available to Audora.
- **Desktop WebView dependencies:** WebView2 on Windows; WebKitGTK 4.1 and GTK 3 on Linux. macOS uses the system WebKit framework.

### Linux dependencies (Ubuntu/Debian)

Install the development and runtime packages used by the project:

```bash
sudo apt-get update
sudo apt-get install -y \
  libwebkit2gtk-4.1-dev libgtk-3-dev libayatana-appindicator3-dev \
  librsvg2-dev patchelf libgstreamer1.0-dev libgstreamer-plugins-base1.0-dev \
  gstreamer1.0-plugins-base gstreamer1.0-plugins-good
```

For macOS, install the Xcode Command Line Tools and GStreamer.framework 1.28 or newer. For Windows, install the Rust MSVC build tools, WebView2 runtime, and 64-bit GStreamer runtime/development packages. The detailed platform setup and support matrix are in [`.docs/DEVELOPMENT.md`](.docs/DEVELOPMENT.md) and [`.docs/PLATFORM_SUPPORT.md`](.docs/PLATFORM_SUPPORT.md).

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

For UI-only development in a browser with Vite:

```bash
pnpm dev:ui
```

The browser development server is for frontend work; it does not launch the complete native app or its local playback backend.

## Run checks

Run the project checks before submitting changes:

```bash
pnpm check
pnpm build:ui
pnpm verify:release
```

`pnpm check` runs the frontend typecheck and tests, Rust formatting and Clippy, and Rust workspace tests. `pnpm verify:release` checks release-related assets and metadata after the UI build.

## Project documentation

- [Development and delivery plan](.docs/DEVELOPMENT.md)
- [Architecture](.docs/ARCHITECTURE.md)
- [Contribution workflow](.docs/CONTRIBUTING.md)
- [Coding conventions](.docs/CODING_CONVENTIONS.md)
- [Security and privacy](.docs/SECURITY.md)
- [Platform support](.docs/PLATFORM_SUPPORT.md)
- [Media formats and licensing](.docs/FORMAT_AND_LICENSE_MATRIX.md)
- [Third-party notices](THIRD_PARTY_NOTICES.md)

## Contribution notes

Keep changes consistent with the project’s local-first privacy goals and architectural boundaries. Treat media metadata and file paths as untrusted input. Do not log note contents or add telemetry, account requirements, or remote media playback without an explicit product and privacy review.
