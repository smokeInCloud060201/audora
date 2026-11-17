<div align="center">
  <img src="assets/icons/icon-1024.svg" alt="Audora logo" width="88" height="88" />

  # Audora

  **Listen, watch, and capture the moments that matter.**

  Audora is a local-first desktop audio and video player that lets you add notes to exact moments in your media. Your library and notes stay on your computer.

  [Download Audora](https://github.com/smokeInCloud060201/audora/releases) · [User guide](.docs/USER_GUIDE.md) · [Report a problem](https://github.com/smokeInCloud060201/audora/issues) · [Developer guide](DEVELOPMENT.md)

  [![CI](https://github.com/smokeInCloud060201/audora/actions/workflows/ci.yml/badge.svg)](https://github.com/smokeInCloud060201/audora/actions/workflows/ci.yml)
  [![Latest release](https://img.shields.io/github/v/release/smokeInCloud060201/audora)](https://github.com/smokeInCloud060201/audora/releases/latest)
  [![Rust 2021](https://img.shields.io/badge/Rust-Edition%202021-191715?logo=rust)](Cargo.toml)
  [![Native Shell: tao + wry](https://img.shields.io/badge/Shell-tao%200.37%20%2B%20wry%200.57-2a6650)](apps/desktop/src-tauri/Cargo.toml)
  [![React 19 + TypeScript](https://img.shields.io/badge/UI-React%2019%20%C2%B7%20TypeScript-3d6f5c?logo=react)](apps/desktop/ui/package.json)
  [![License: Apache-2.0](https://img.shields.io/badge/License-Apache--2.0-8a8278)](LICENSE)
  [![Privacy: Local-First](https://img.shields.io/badge/Privacy-Local--First%20%C2%B7%20No%20Telemetry-2a6650)](.docs/SECURITY.md)

  **macOS** · **Windows** · **Linux**
</div>

---

## Your media, your notes, your place

Audora is for people who want to stay engaged with what they are listening to or watching—whether that is a lecture, an interview, a rehearsal, or a favorite album. Add a note while something is playing, then return to that exact point later.

### Made for listening, watching, and remembering

- **Keep your place.** Audora remembers playback progress so you can pick up where you left off.
- **Capture a thought at the right moment.** Add a detailed timestamped note or drop a quick marker without interrupting playback.
- **Jump back to a note.** Select a note or timeline marker to return to its moment in the media.
- **Keep your media organized.** Create playlists, arrange your queue, and search your library and notes.
- **Make it fit your day.** Adjust playback speed, use shuffle or repeat, and switch to a compact mini-player when you need more room.
- **Use your own local files.** Add individual files or choose folders as media sources.

Audora supports common audio and video formats including MP3, WAV, FLAC, M4A, AAC, Ogg/Opus, MP4, MOV, WebM, and MKV. Playback support can vary by operating system and available media codecs; see the [format guide](.docs/FORMAT_AND_LICENSE_MATRIX.md) for details.

## Private by design

Audora is designed for local media and local notes. You do not need an account, and the project does not collect telemetry. Your library details, playback progress, playlists, and notes are stored on your device. Audora does not upload your media files.

Update checks and downloads use GitHub Releases. Media playback is intended for local files; remote streaming URLs are not supported. Read more in the [privacy and security guide](.docs/SECURITY.md).

## Download Audora

Visit [Audora Releases](https://github.com/smokeInCloud060201/audora/releases) and choose an available package for your operating system. The release page contains the current version, download files, and release notes.

Audora provides release packages for Apple silicon macOS, 64-bit Windows, and 64-bit Linux. Minimum operating-system requirements and Linux dependencies are listed in the [platform support guide](.docs/PLATFORM_SUPPORT.md). Windows currently provides a portable ZIP when an installer is not available in the release.

After opening Audora, select local audio or video files—or add a media folder—to start your library. Audora keeps your original media files in place.

## An open-source project

Audora is an independent open-source project, licensed under the [Apache License 2.0](LICENSE). It is part of the wider community of open-source media tools, alongside established projects such as [VLC](https://www.videolan.org/vlc/) and [Audacity](https://www.audacityteam.org/). Audora has its own focused goal: pairing local media playback with notes linked to timestamps. It is not affiliated with or a replacement for those projects.

Have an idea or found a problem? [Open an issue](https://github.com/smokeInCloud060201/audora/issues). Please do not attach private media or include sensitive note text in reports.

## For developers

To build Audora from source, run it locally, or contribute, start with the separate [Development Guide](DEVELOPMENT.md). For everyday usage help, see the [User Guide](.docs/USER_GUIDE.md).

## License

Audora is licensed under the [Apache License, Version 2.0](LICENSE). See the [third-party notices](THIRD_PARTY_NOTICES.md), [licensing policy](.docs/LICENSING.md), and [software bill of materials](sbom.cdx.json) for dependency information.
