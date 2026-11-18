# Development and delivery plan

## Workspace shape

```text
crates/audora-core/       Rust domain, SQLite WAL persistence, migrations, pure-Rust container prober, and playback service traits
apps/desktop/src-tauri/   Native desktop shell (tao + wry), authenticated loopback HTTP/Range/RPC server, self-updater, and installer hooks
apps/desktop/ui/          React + TypeScript (strict) UI, relational IPC client, and semantic CSS token system
assets/icons/             Official Audora SVG master and rasterized application icons
.docs/                    Product, UX, architecture, ADRs, security, and platform specifications
```

Keep the workspace simple until code boundaries demonstrate a need for more crates. Domain rules, container header probing, and SQLite persistence invariants are testable without launching a window or hardware decoder.

## Local development prerequisites

- **Rust:** Stable toolchain (`rust-toolchain.toml`, `rustc` ≥ 1.89) with `rustfmt` and `clippy`.
- **Node.js & pnpm:** Node.js ≥ 22 (`v24` recommended) and `pnpm` (`11.19.0`).
- **Platform WebView & runtime media dependencies ([ADR 0004](adr/0004-native-webview-loopback-architecture.md)):**
  - **macOS:** Xcode Command Line Tools (`WKWebView` + built-in `AVFoundation`/`VideoToolbox`).
  - **Linux:** `libwebkit2gtk-4.1-dev`, `libgtk-3-dev`, `libayatana-appindicator3-dev`, `librsvg2-dev`, `patchelf`, plus runtime `gstreamer1.0-plugins-base` and `gstreamer1.0-plugins-good` for `WebKitGTK` HTML5 media decoding.
  - **Windows:** Rust MSVC 64-bit toolchain + Microsoft Edge `WebView2` Evergreen runtime.
- **Optional CLI discovery tools:** `gst-discoverer-1.0` or `ffprobe` in `PATH` are optional fallbacks for `GStreamerBackend` CLI inspection; Rust compilation does not link against C GStreamer headers.

## Standard workspace commands

Run from the repository root:

- **Install frontend dependencies:** `pnpm install --frozen-lockfile`
- **Run all checks (Frontend + Rust fmt, Clippy & tests):** `pnpm check`
- **Format Rust code:** `pnpm fmt:rust`
- **Run Rust unit & integration tests:** `pnpm test:rust`
- **Run frontend typecheck & unit tests:** `pnpm typecheck && pnpm test:ui`
- **Build frontend production bundle:** `pnpm build:ui`
- **Verify release gate invariants:** `pnpm verify:release`
- **Launch UI development server:** `pnpm dev:ui`

## CI gates

For every pull request (`.github/workflows/ci.yml`):

- Format Rust (`cargo fmt --check`) and run Clippy (`cargo clippy --workspace --all-targets -- -D warnings`) and TypeScript strict type checking (`pnpm typecheck`).
- Build and test across macOS, Linux, and Windows runners (`cargo test --workspace` and `pnpm test:ui`).
- Run unit and integration checks for domain invariants, SQLite relational RPCs, migration backup/recovery, loopback `X-Audora-Token` / `MediaAllowlist` security boundaries, pure-Rust container duration probing, and self-updater SHA-256 verification.
- Enforce Rust crate license/advisory rules across all supported targets via [`deny.toml`](../deny.toml) (`cargo-deny`) and audit production npm dependencies (`pnpm audit --prod`).
- Build the production UI bundle and verify release gate invariants (`pnpm verify:release`).

Release workflows (`.github/workflows/release.yml`) additionally verify tag/manifest version synchronization, build native installers (`.dmg` + `.app.tar.gz` on macOS, `.deb` + `.tar.gz` on Linux, NSIS `-setup.exe` + `-portable.zip` on Windows), sign/notarize when credentials are configured, generate per-platform SHA-256 manifests, and attest cryptographic SLSA build provenance. CI never exposes signing keys to untrusted pull-request code.

## Quality strategy

- **Domain & service tests (`audora-core`):** note timestamp validation, quick markers, search filtering, media identity/relink conflict detection (`AV-201`/`AV-202`), playback state transitions, queue wrap/auto-advance, and load generation suppression.
- **Persistence & RPC tests (`audora-core` + `audora-desktop`):** SQLite WAL migrations (`0001`–`0004`), atomic `Immediate` batch RPC transactions, pre-migration backup preservation, corruption recovery, cascade delete, and concurrent access.
- **Loopback & prober tests (`audora-desktop`):** pure-Rust container duration probing (WAV, MP4, WebM, FLAC, Ogg/Opus, MP3, AAC), RFC 9110 HTTP Range (`206 Partial Content` / `416`), `X-Audora-Token` + `Host` DNS-rebinding defenses, `MediaAllowlist` canonical symlink checks, and self-update SHA-256 validation.
- **UI & IPC tests (`apps/desktop/ui`):** relational IPC synchronization, finite-permutation shuffle queue, keyboard shortcut guards, relink duration/type validation, timestamp formatting, and accessibility attributes.

## Observability

Local structured logs (`tracing`) use severity and operation identifiers, with sensitive values (note bodies, search queries, session tokens, and full user directory paths) redacted per [ERROR_CATALOG.md](ERROR_CATALOG.md). No remote telemetry is enabled.

## Support policy

Minimum OS versions, supported Linux targets, CPU architectures, media format matrix, and installer formats are published in [PLATFORM_SUPPORT.md](PLATFORM_SUPPORT.md) and [FORMAT_AND_LICENSE_MATRIX.md](FORMAT_AND_LICENSE_MATRIX.md).
