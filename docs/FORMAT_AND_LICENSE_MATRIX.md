# Format, plugin, and license matrix

This document completes Sprint 1.3 and records the verified GStreamer 1.28.1 plugin inventory, LGPL dynamic-linking requirements, and supported media format matrix for Audora v1.0.

## Verified GStreamer plugin inventory (GStreamer 1.28.1)

Empirical inspection on macOS (`GStreamer.framework 1.28.1`) confirmed the following required plugin set. All included plugins are licensed under **LGPLv2.1+** (reported as `LGPL` by `gst-inspect-1.0`):

| Plugin Name | Source Module | License | Responsibility in Audora |
|---|---|---|---|
| `coreelements` | `gstreamer` | LGPL | Core pipeline queues, capsfilter, filesrc, identity |
| `playback` | `gst-plugins-base` | LGPL | `playbin3`, `uridecodebin3`, stream selection & seeking |
| `typefindfunctions` | `gst-plugins-base` | LGPL | Container and elementary stream MIME/type detection |
| `audioconvert` | `gst-plugins-base` | LGPL | Audio sample format conversion |
| `audioresample` | `gst-plugins-base` | LGPL | Sample-rate conversion and pitch/rate support |
| `videoconvertscale` | `gst-plugins-base` | LGPL | Video colorspace conversion and aspect-ratio scaling |
| `autodetect` | `gst-plugins-good` | LGPL | Auto-detection of platform audio/video sinks |
| `isomp4` | `gst-plugins-good` | LGPL | MP4 / QuickTime / M4A (`qtdemux`) container demuxing |
| `matroska` | `gst-plugins-good` | LGPL | WebM and Matroska (`.webm`, `.mkv`) demuxing |
| `ogg` | `gst-plugins-base` | LGPL | Ogg container (`.ogg`, `.oga`, `.ogv`) demuxing |
| `opus` | `gst-plugins-base` | LGPL | Opus audio decoding |
| `vorbis` | `gst-plugins-base` | LGPL | Vorbis audio decoding |
| `flac` | `gst-plugins-good` | LGPL | Lossless FLAC audio decoding |
| `wavparse` | `gst-plugins-good` | LGPL | PCM `.wav` parsing and playback |
| `applemedia` *(macOS)* | `gst-plugins-bad` | LGPL | Apple VideoToolbox / AudioToolbox hardware H.264/AAC decoding |
| `d3d11` / `mediafoundation` *(Windows)* | `gst-plugins-bad` | LGPL | Windows Media Foundation / Direct3D11 hardware decoding |
| `va` *(Linux)* | `gst-plugins-bad` | LGPL | VA-API hardware decode where supported by system drivers |

### Excluded modules and plugins
- **`gst-plugins-ugly`:** Excluded in entirety due to GPL-licensed components and patent risk.
- **`libgstpython`:** Excluded from bundled packages (unneeded runtime scripting plugin).
- **Any GPL / AGPL plugin:** Strictly excluded per [ADR 0002](adr/0002-project-licensing-and-codec-policy.md).

## Supported media format matrix (v1.0)

| Media Kind | Container / Extension | Codecs Supported | Decoder Source |
|---|---|---|---|
| **Video** | `.mp4`, `.m4v`, `.mov` | H.264 (AVC) video + AAC / Opus / MP3 audio | OS hardware decoder (`applemedia` / `mediafoundation` / `va`) + `isomp4` |
| **Video** | `.webm`, `.mkv` | VP8, VP9, AV1 (where platform decoder present) + Opus / Vorbis | Bundled LGPL plugins (`matroska`, `opus`, `vorbis`) + platform WebView/decoder |
| **Audio** | `.wav` | Uncompressed PCM (8/16/24/32/64-bit) | Bundled LGPL `wavparse` + `audioconvert` |
| **Audio** | `.flac` | FLAC lossless audio | Bundled LGPL `flac` |
| **Audio** | `.ogg`, `.opus` | Opus, Ogg Vorbis | Bundled LGPL `ogg`, `opus`, `vorbis` |
| **Audio** | `.mp3`, `.m4a`, `.aac` | MP3, AAC-LC | Platform audio decoder + `isomp4` |

> **Unsupported formats (e.g., Apple ProRes `.prores` on systems without ProRes decode, raw camera formats, DRM-protected streams):** Fail gracefully during probe/load and route to the `Unsupported media` screen with stable error code **`AV-104`** (`Decoder unavailable`).

## LGPLv2.1+ and Apache 2.0 compliance obligations

1. **Dynamic linking only:** GStreamer core and plugins are loaded as shared libraries (`.dylib` / `.framework` on macOS, `.dll` on Windows, `.so` on Linux) so users can inspect, modify, or replace the LGPL libraries.
2. **Third-party notices:** Every release archive includes `LICENSE` (Apache-2.0 for Audora code), `NOTICE`, `THIRD_PARTY_LICENSES.txt` (including full LGPL-2.1 text), and an automated CycloneDX/SPDX SBOM.
3. **Rust crate license enforcement:** Enforced in CI via [`deny.toml`](../deny.toml) using `cargo-deny`.
