# Audora User Guide & Troubleshooting

Welcome to **Audora**, a local-first desktop audio and video player built for capturing timestamped notes and returning to exact moments in your media.

---

## 1. Installation

| Platform | Package Format | Installation Steps |
| :--- | :--- | :--- |
| **macOS 13+** (`arm64` / `x86_64`) | `Audora_<version>_macos-aarch64.dmg` | Open the `.dmg` and drag `Audora.app` to `/Applications`. |
| **Windows 10/11** (`x86_64`) | `Audora_<version>_windows-x86_64-setup.exe` / `-portable.zip` | Run the `-setup.exe` per-user installer and launch Audora from the Start Menu, or extract `-portable.zip`. |
| **Linux** (`Ubuntu 22.04+`, `Debian 12+`) | `Audora_<version>_linux-x86_64.deb` / `.tar.gz` | Install via `sudo apt install ./Audora_<version>_linux-x86_64.deb` or extract `.tar.gz`. |

---

## 2. Core Workflows & Keyboard Shortcuts

| Action | Shortcut | Notes |
| :--- | :--- | :--- |
| **Open media file(s)** | `⌘O` (macOS) / `Ctrl+O` | Also supports dragging & dropping files onto the window |
| **Play / Pause** | `Space` | Works in Full mode and Minimum mode (when focus is not on another button or input) |
| **Seek backward / forward** | `←` / `→` (`±5s`), `Shift+←` / `Shift+→` (`±15s`) | Or click/drag the timeline scrubber |
| **Adjust volume** | `↑` / `↓` (`±5%`) | Or drag the transport volume slider |
| **Add note at current timestamp** | `N` | Captures the exact timestamp immediately; playback continues while you type |
| **Save note in composer** | `⌘Enter` / `Ctrl+Enter` | Or check **Save as quick marker** to save a timestamp without text |
| **Drop instant quick marker** | `Shift + N` | Drops a marker at the current playhead without opening the modal |
| **Search library & notes** | `⌘K` / `Ctrl+K` | Matches both media titles and note text offline |
| **Toggle Minimum / Full mode** | `M` | Press `Esc` in Minimum mode to return to Full mode |

---

## 3. Data Storage, Backups, and Recovery

Audora stores your library records, playback progress, and timestamped notes locally in a SQLite WAL database:

- **macOS:** `~/Library/Application Support/com.audora.desktop/audora.sqlite`
- **Windows:** `%APPDATA%\com.audora.desktop\data\audora.sqlite`
- **Linux:** `$XDG_DATA_HOME/com.audora.desktop/audora.sqlite` (`~/.local/share/com.audora.desktop/audora.sqlite`)

### Automatic & Manual Backups
- **Pre-Migration Backups:** Before applying any database schema upgrade, Audora automatically creates `audora.sqlite.pre-v<N>.bak`.
- **Manual Snapshot:** In **Settings → Privacy**, click **Create backup snapshot** at any time to save `audora.sqlite.manual.bak`.
- **Corruption Recovery:** If the database file is ever damaged by an unexpected power loss or disk error, Audora preserves the damaged file at `audora.sqlite.corrupt.bak` and automatically restores from your latest backup snapshot.

---

## 4. Troubleshooting Error Codes (`AV-*`)

| Code | Meaning | How to Resolve |
| :--- | :--- | :--- |
| **`AV-101`** | **File unavailable / missing** | The media file was moved, renamed, or is on an unplugged drive. Click **Locate file…** in the Library to relink your preserved notes to the file's new location. |
| **`AV-102`** | **File unreadable** | Check OS file permissions or ensure the external drive is mounted. |
| **`AV-103`** | **Corrupt or empty file** | Verify the media file is not `0 bytes` or truncated. |
| **`AV-104`** | **Unsupported format/codec** | Transcode the file to a Baseline Tier 1 format (`MP4` with H.264/AAC, `WebM` with VP9/Opus, `WAV`, `FLAC`, `MP3`, or `Ogg/Opus`). |
| **`AV-201`** | **Relink conflict** | The replacement file path is already tracked as another item in your library. Audora blocks silent merges so you never lose notes. |
| **`AV-202`** | **Relink mismatch warning** | The replacement file has a different duration or media type than the original. Confirm **Relink anyway** to update the path while keeping all notes (notes past the new duration are badged `Out of range`). |
