# Development and delivery plan

## Workspace shape

```text
crates/audora-core/       Rust domain, services, playback adapters, and SQLite migrations
apps/desktop/src-tauri/   Native desktop shell, installer hooks, and macOS entitlements
apps/desktop/ui/          React + TypeScript (strict) UI and semantic CSS token system
assets/icons/             Official Audora SVG master and rasterized application icons
.docs/                    Product, UX, architecture, and platform specifications
```

Keep the workspace simple until code boundaries demonstrate a need for more crates. Domain rules and persistence invariants are testable without launching a window or hardware decoder.

## Local development prerequisites

- **Rust:** Stable toolchain (`rust-toolchain.toml`, `rustc` ≥ 1.85) with `rustfmt` and `clippy`.
- **Node.js & pnpm:** Node.js ≥ 22 (`v24` recommended) and `pnpm` (`11.19.0`).
- **GStreamer (for live GStreamer probe/playback adapter):**
  - **macOS:** `GStreamer.framework 1.28+` at `/Library/Frameworks/GStreamer.framework/Versions/1.0` (or `gst-discoverer-1.0` / `gst-launch-1.0` in `PATH`).
  - **Linux:** `libgstreamer1.0-dev`, `libgstreamer-plugins-base1.0-dev`, `gstreamer1.0-plugins-base`, `gstreamer1.0-plugins-good`, `libwebkit2gtk-4.1-dev`.
  - **Windows:** GStreamer MSVC 64-bit runtime and development installer + WebView2 runtime.

## Standard workspace commands

Run from the repository root:

- **Install frontend dependencies:** `pnpm install`
- **Run all checks (Frontend + Rust fmt, Clippy & tests):** `pnpm check`
- **Format Rust code:** `pnpm fmt:rust`
- **Run Rust unit & integration tests:** `pnpm test:rust`
- **Run frontend typecheck & unit tests:** `pnpm typecheck && pnpm test:ui`
- **Build frontend production bundle:** `pnpm build:ui`
- **Launch UI development server:** `pnpm dev:ui`

## CI gates

For every pull request (`.github/workflows/ci.yml`):

- Format Rust and frontend code; run Clippy (`-D warnings`) and TypeScript strict type checking.
- Build supported target configurations across macOS, Linux, and Windows runners.
- Run unit and integration checks for domain, persistence, migration backup/recovery, playback state transitions, and command validation.
- Enforce license and vulnerability rules via [`deny.toml`](../deny.toml).

Release workflows additionally build clean packages, produce SBOM/checksums, verify notices, sign/notarize as configured, and attach provenance and release notes. CI must not expose signing keys to untrusted pull-request code.

## Quality strategy

- **Domain tests:** note timestamp validation, quick markers, search filtering, media identity/relink conflict detection, playback state transitions, and load generation suppression.
- **Persistence tests:** migrations, transaction rollback, pre-migration backup preservation on failure, cascade delete, and concurrent access.
- **Adapter tests:** `FakePlaybackBackend` deterministic command sequencing and `GStreamerBackend` discovery parser tests.
- **UI tests:** token/theme switching, keyboard shortcut guards, timestamp formatting, and state transitions.

## Observability

Local structured logs (`tracing`) use severity and operation identifiers, with sensitive values (note bodies, search queries, user home paths) redacted per [ERROR_CATALOG.md](ERROR_CATALOG.md). No remote telemetry is enabled by default.

## Support policy

Minimum OS versions, supported Linux targets, CPU architectures, media format matrix, and installer formats are published in [PLATFORM_SUPPORT.md](PLATFORM_SUPPORT.md) and [FORMAT_AND_LICENSE_MATRIX.md](FORMAT_AND_LICENSE_MATRIX.md).
