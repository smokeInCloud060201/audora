# Format, prober, and license matrix

This document records Audora's supported 12-format media matrix, pure-Rust container prober + OS WebView decoder architecture ([ADR 0004](adr/0004-native-webview-loopback-architecture.md)), optional Linux/CLI GStreamer plugin inventory, and Apache-2.0 / LGPL compliance obligations.

## Playback & probing architecture

1. **Container header & duration probing (pure Rust, built-in):**
   Audora inspects container headers read-only in Rust (`probe_local_media_duration_ms`) without modifying source files or linking native C codec libraries:
   - **ISO BMFF (`mp4`, `m4v`, `mov`, `m4a`):** Parses `mvhd` (`timescale` + `duration`), fragmented `mvex`/`mehd`, `sidx` segment indexes, and track-level `trak` → `mdia` → `mdhd` boxes (supporting both front-`moov` and tail-`moov` files).
   - **Matroska / WebM (`webm`, `mkv`):** Parses EBML `Segment` → `Info` (`TimestampScale` + float32/float64 `Duration`), skipping known-size `Cluster` payloads cleanly.
   - **RIFF WAVE (`wav`):** Parses `fmt ` byte rate and `data` chunk size (including RF64/extensible headers).
   - **FLAC (`flac`):** Parses `STREAMINFO` sample rate and 36-bit total sample count.
   - **Ogg (`ogg`, `opus`):** Parses initial identification header (`OpusHead` 48 kHz pre-skip or `\x01vorbis` sample rate) and scans the final `OggS` page granule position.
   - **MP3 (`mp3`) & AAC (`aac`):** Skips `ID3v2` tags, reads Xing/Info/VBRI VBR frame headers when present, or estimates duration from MPEG frame / ADTS headers.

2. **Media decoding & rendering (OS WebView media stack):**
   Active media streams over authenticated `127.0.0.1:0` HTTP `206 Partial Content` byte ranges into the OS WebView's hardware-accelerated HTML5 `<video>` / `<audio>` pipeline:
   - **macOS (`WKWebView`):** Apple `AVFoundation` / `VideoToolbox` / `AudioToolbox`.
   - **Windows (`WebView2`):** Microsoft Edge Chromium media stack + Windows `Media Foundation` / `Direct3D11`.
   - **Linux (`WebKitGTK 4.1`):** System `GStreamer 1.x` (`gstreamer1.0-plugins-base` + `gstreamer1.0-plugins-good`, dynamically loaded by `WebKitGTK`).

## Supported media format matrix

| Media Kind | Container / Extension | Baseline Codecs Supported | Prober & Decoder Source |
|---|---|---|---|
| **Video** *(or audio-only container)* | `.mp4`, `.m4v`, `.mov` | H.264 (AVC) video + AAC / MP3 / Opus audio | Pure-Rust ISO BMFF prober + OS WebView hardware decoder (`VideoToolbox` / `Media Foundation` / `WebKitGTK` GStreamer) |
| **Video** *(or audio-only container)* | `.webm`, `.mkv` | VP8, VP9, AV1 (where platform decoder present) + Opus / Vorbis | Pure-Rust EBML prober + OS WebView media decoder *(note: `.mkv` container playback depends on platform WebView Matroska demuxer support; unsupported streams surface `AV-104`)* |
| **Audio** | `.wav` | Uncompressed PCM / IEEE float | Pure-Rust RIFF WAVE prober + OS WebView audio decoder |
| **Audio** | `.flac` | FLAC lossless audio | Pure-Rust `STREAMINFO` prober + OS WebView audio decoder |
| **Audio** | `.ogg`, `.opus` | Opus, Ogg Vorbis | Pure-Rust `OggS` granule prober + OS WebView audio decoder |
| **Audio** | `.mp3`, `.m4a`, `.aac` | MP3, AAC-LC | Pure-Rust MPEG/BMFF/ADTS prober + OS WebView audio decoder |

> **Audio-only `.mp4` / `.mov` / `.webm` auto-detection:** If a video container carries only audio tracks (`videoWidth === 0 && videoHeight === 0` on metadata load), Audora automatically switches to the audio-only presentation stage and updates the library item's `media_kind` to `audio`.
>
> **Unsupported formats or codecs (e.g., Apple ProRes on systems without ProRes decode, HEVC/AC-3 without system codec support, raw camera formats, `0`-byte files, or DRM-protected streams):** Fail gracefully without altering source files and route to the error screen with stable error codes **`AV-103`** (`Corrupt or empty file`) or **`AV-104`** (`Decoder unavailable`).

## Linux system GStreamer plugin inventory (recommended for `WebKitGTK`)

Audora's `.deb` package recommends standard LGPL-licensed system GStreamer packages (`gstreamer1.0-plugins-base`, `gstreamer1.0-plugins-good`) used dynamically by `WebKitGTK` on Linux (and optionally by `gst-discoverer-1.0` when present):

| Plugin Name | Source Module | License | Responsibility |
|---|---|---|---|
| `coreelements`, `playback`, `typefindfunctions` | `gstreamer` / `gst-plugins-base` | LGPLv2.1+ | Pipeline queues, stream selection, seeking, MIME detection |
| `audioconvert`, `audioresample`, `videoconvertscale` | `gst-plugins-base` | LGPLv2.1+ | Sample-rate conversion (`0.5×–2.0×` playback rate) and video scaling |
| `isomp4`, `matroska`, `ogg`, `wavparse` | `gst-plugins-base` / `gst-plugins-good` | LGPLv2.1+ | MP4/MOV/M4A, WebM/MKV, Ogg, and WAV demuxing |
| `opus`, `vorbis`, `flac` | `gst-plugins-base` / `gst-plugins-good` | LGPLv2.1+ | Opus, Vorbis, and lossless FLAC audio decoding |

### Excluded modules and plugins
- **`gst-plugins-ugly` & any GPL/AGPL plugins:** Never bundled or required by Audora ([ADR 0002](adr/0002-project-licensing-and-codec-policy.md)).

## Apache 2.0 and third-party license compliance

1. **Zero bundled GPL/copyleft static code:** Audora binaries statically link only permissive Rust crates audited by `cargo-deny` (`MIT`, `Apache-2.0`, `BSD-2-Clause`, `BSD-3-Clause`, `ISC`, `Unicode-3.0`, `Zlib`, `MPL-2.0`) and dynamically use OS-provided WebView/media frameworks.
2. **Third-party notices & SBOM:** Every release bundle includes `LICENSE` (Apache-2.0), `THIRD_PARTY_NOTICES.md`, and a version-synchronized CycloneDX `sbom.cdx.json` verified by `pnpm verify:release`.
3. **Automated CI enforcement:** Enforced across macOS, Linux, and Windows target triples via [`deny.toml`](../deny.toml) and `pnpm audit --prod`.
