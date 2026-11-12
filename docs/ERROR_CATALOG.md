# User-facing error catalog

This document completes Sprint 1.5 and defines the canonical `AV-*` error codes, categories, user-facing messages, recovery actions, and logging redaction rules mapped across the Rust core and TypeScript UI.

## Error categories and stable codes

| Error Code | Category | Trigger Condition | User Heading & Message | Primary Recovery Actions |
|---|---|---|---|---|
| **`AV-101`** | `MediaUnavailable` | File path no longer exists on disk or external drive is disconnected (`Missing`). | **We can’t find this file** — It may have been moved, renamed, or stored on a disconnected drive. Your notes are safe and will remain here. | `Find file…`, `Remove from library` (Frame `4:339`) |
| **`AV-102`** | `MediaUnreadable` | OS permission denied, path outside authorized scope, or filesystem I/O read failure. | **We can’t read this file** — Check that the file exists and Audora has permission to read it. | `Choose another file`, `Show in folder` |
| **`AV-103`** | `MediaCorrupt` | File header is truncated, zero-byte, or stream fails mid-playback. | **This file appears damaged** — Audora couldn’t read a playable audio or video stream from this file. | `Choose another file`, `Show in folder` |
| **`AV-104`** | `MediaUnsupported` | Container or codec is not supported by available decoders (e.g., `.prores` or unsupported track). | **This format isn’t supported** — The file uses a codec that isn’t available on this system. | `1. Convert the file to MP4 (H.264) or WebM.` `2. Try opening it with your system’s default player.` Buttons: `Choose another file`, `Show in folder` (Frame `4:399`) |
| **`AV-201`** | `RelinkConflict` | User attempts to relink a `Missing` item to a file path that already belongs to another `MediaId` in the library. | **File already in library** — This file is already tracked as a separate item. Audora will not merge two library records automatically. | `Choose another file`, `Cancel` |
| **`AV-202`** | `RelinkMismatch` | Selected replacement file has a different media kind (`Audio` vs `Video`) or duration differs by more than `500 ms`. | **Confirm file relink** — The selected file’s duration or media type differs from the original record. Existing notes will be kept. | `Relink anyway`, `Cancel` |
| **`AV-301`** | `ValidationFailed` | Empty note text without `Save as quick marker`, note body exceeds `10,000` characters, invalid playback speed outside `[0.5, 2.0]`, or negative timestamp. | Inline validation message explaining the exact field constraint. | Keep user input intact; focus invalid field |
| **`AV-401`** | `PersistenceFailure` | SQLite open error, disk full, or schema migration failure. | **Database recovery needed** — Audora preserved your original database backup before making changes. | `Open backup folder`, `Retry` |
| **`AV-501`** | `InternalError` | Unexpected IPC or service failure not covered above. | **Something went wrong** — Your saved notes were not affected. | `Retry`, `Export diagnostic bundle` |

## Logging and redaction rules

- **Never log:** Note `body` text, search query strings, or raw media file bytes.
- **Path redaction:** Log the stable `MediaId` and file extension (for example, `media_id="med_..." ext="mp4" code="AV-104"`) rather than full user home directory paths in standard log output.
- **Boundary serialization:** Every Rust command returns `Result<T, AppErrorPayload>` where `AppErrorPayload` contains `{ code, category, title, message, recovery_steps, recoverable }`.
