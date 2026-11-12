# Packaging and third-party distribution assets

This directory holds platform installer metadata, application icons, macOS entitlements, and third-party license notices bundled during release builds.

## Compliance checklist (per [LICENSING.md](../docs/LICENSING.md) & [FORMAT_AND_LICENSE_MATRIX.md](../docs/FORMAT_AND_LICENSE_MATRIX.md))

- Root [`LICENSE`](../LICENSE) (Apache License 2.0) is included in all installer targets via `src-tauri/tauri.conf.json`.
- Only LGPLv2.1+ GStreamer core/base/good/platform plugins listed in `docs/FORMAT_AND_LICENSE_MATRIX.md` may be bundled, dynamically linked as shared libraries.
- `gst-plugins-ugly` and any GPL/AGPL plugins are excluded.
