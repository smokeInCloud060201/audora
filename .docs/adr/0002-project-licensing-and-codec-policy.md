# ADR 0002: Project licensing, LGPL dynamic linking, and codec policy

- **Status:** Accepted (with Phase 1 Sprint 1.3 packaging verification gate)
- **Date:** 2026-11-12
- **Deciders:** Project Maintainers

## Context and constraints

Audora is intended as a sustainable open-source desktop product distributed as both source code and prebuilt platform installers. Desktop media players touch complex legal surfaces: open-source copyleft obligations (LGPL/GPL) in media frameworks and patent/redistribution restrictions on audio/video codecs.

## Decision

1. **Project license:** License Audora's original source code and documentation under the **Apache License 2.0** (canonical [LICENSE](../../LICENSE) in repository root; `NOTICE` and third-party license bundle generated before first release).
2. **Third-party Rust & JS dependencies:** Prefer permissive licenses compatible with Apache-2.0 (`Apache-2.0`, `MIT`, `BSD-2-Clause`, `BSD-3-Clause`, `ISC`, `Zlib`, `Unicode-3.0`). Automate license scanning in CI (`cargo-deny` / equivalent frontend audit).
3. **Copyleft exclusions:** Exclude **GPL** and **AGPL** libraries and plugins (such as `gst-plugins-ugly` or GPL-configured decoder builds) from distributed Audora binaries.
4. **LGPL media backend rules:** LGPLv2.1+ components (such as GStreamer core, `gst-plugins-base`, and reviewed LGPL plugins) **MUST** be dynamically linked (shared libraries/frameworks) so end users can inspect or replace the LGPL libraries, accompanied by complete license texts, copyright attributions, and corresponding source pointers in release artifacts.
5. **Transparent codec policy:** Publish an explicit per-OS format matrix distinguishing bundled royalty-free/permissive decoders (e.g., Opus, Vorbis, FLAC, WAV, VP8/VP9/AV1 where applicable) from OS-provided media foundation/platform decoders. Never market universal codec support or make unverified patent claims.

## Alternatives considered

- **GPLv3 project license:** Would allow bundling GPL codec libraries directly, but conflicts with the maintainer goal of permissive Apache-2.0 licensing and complicates macOS notarization/distribution models.
- **OS-native media APIs only (`AVFoundation`, `Media Foundation`, `GStreamer` on Linux):** Avoids bundling GStreamer on macOS/Windows, at the cost of maintaining three separate `PlaybackBackend` adapters with divergent seek, track, and format semantics. Kept as a fallback if LGPL bundling proves impractical in Sprint 1.3.

## Consequences and risks

- Every new native library or GStreamer plugin bundled via `apps/desktop/src-tauri/` or release workflows requires explicit license and codec classification before merge.
- Certain proprietary or patent-encumbered formats (for example Apple ProRes on non-macOS systems, or unbundled codecs) will fail to decode and must surface clear user guidance (`Error AV-104 · Decoder unavailable` in [UI_DESIGN.md](../UI_DESIGN.md)).

## Validation required

- **Sprint 1.3:** Produce a complete SBOM and plugin/license inventory for macOS, Windows, and Linux test installers and review codec/LGPL compliance before public binary distribution.

## Related documents

- [LICENSING.md](../LICENSING.md)
- [ARCHITECTURE.md](../ARCHITECTURE.md)
