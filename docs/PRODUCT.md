# Product requirements

## Product statement

Audora is a dependable desktop audio and video player with first-class timestamped notes. A listener or viewer can mark a useful moment without losing playback context, add a short explanation, and return to that moment from a searchable library.

## Intended users and use cases

- People studying lectures, interviews, language recordings, music, or long videos.
- Researchers and creators who need to mark passages in local media.
- People who want a straightforward, keyboard-friendly local media player.

Primary flow: open or drag in media → play and seek → create a note at the current position → find it later → activate the note to seek back to the marked moment. Detailed happy paths, failure paths, edge-case rules, and a product glossary live in [User workflows and glossary](WORKFLOWS.md).

## Product principles

1. Local-first: media and notes stay on the device. No account, analytics, network service, or cloud sync is required.
2. Preserve source media: Audora MUST NOT modify media files as a side effect of playback or annotation.
3. Predictable navigation: a note refers to a specific media identity and timestamp, and visibly reports when its source is unavailable.
4. Useful controls: playback, seeking, volume, speed, window modes, and notes work from keyboard and pointer, with accessible labels and focus states.
5. Transparent compatibility: users can see supported formats and understand when playback depends on installed or bundled codecs.

## UI design source

The canonical UI design reference is the [Audora Figma file](https://www.figma.com/design/PaNgbiT8RSycIk1QBzPiqf/Audora?node-id=4-522&m=dev), inspected in Dev Mode during Phase 0. Use its screens, components, and design tokens as the visual baseline. Product behavior and accessibility requirements in this document and [WORKFLOWS.md](WORKFLOWS.md) remain the acceptance criteria; when the Figma design and these requirements differ, record the discrepancy and resolve it in [UI design handoff](UI_DESIGN.md) before implementation rather than silently choosing one.

## Product scope

### Required for the first stable release

- Installable, signed or clearly verifiable desktop packages for macOS, Windows, and Linux.
- Open local files through file picker, drag and drop, and supported command-line/open-with entry points.
- Audio and video playback with play/pause, seek/skip, playback speed (`0.5×`–`2.0×`), volume/mute, playback position/duration, per-file progress tracking/resume, and error feedback.
- Two window presentation modes: **Full mode** (navigation rail, video/audio canvas, transport, and `Queue` / `Notes` side panel) and **Minimum mode** (distraction-free time/progress player with quick return to Full mode).
- Session playlist/queue with persistent `Queue | Notes` panel switching, plus repeat and shuffle controls.
- Timestamped notes: create text notes or quick markers at current playhead, edit text, delete with confirmation/undo, list chronologically by media, seek to note, and search notes/media titles locally (`⌘/Ctrl+K`) with `All / Video / Audio` filtering.
- Persistent local library, playback progress, and notes across restarts; resilient schema migration and backup/recovery behavior.
- Settings for theme (`Dark`, `Light`, `System`), playback behavior (`Resume where I left off`, default playback speed, and OS-supported `Pause when headphones disconnect` validated in Phase 1), and note timestamp precision (`Nearest second` or `Milliseconds`); sensible defaults.
- Keyboard navigation, baseline keyboard shortcuts, screen-reader names for controls, visible focus, and scalable UI.
- Privacy and license notices, crash-safe persistence, update/release documentation, and user-facing troubleshooting.

### Planned capabilities after the stable baseline

- Subtitle selection and rendering where backend and platform support are consistent.
- Frame stepping and user-configurable custom keyboard shortcut remapping.
- Import/export of notes in a documented, versioned format.
- Optional portable mode on platforms where it can be implemented safely.
- Optional extensions only after a capability and permission model is designed.

### Explicit non-goals

- Streaming service, DRM circumvention, online media catalog, or account system.
- Editing, transcoding, recording, or modifying media source files.
- Cloud sync or telemetry enabled by default.
- Promising every codec/container combination on every operating system. Supported formats and bundled components must be published accurately.
- Loading arbitrary third-party plugins in the initial product.

## Functional requirements

### Library and file access

- Users can open one or multiple supported files and folders through native file selection or drag and drop.
- The library stores references, probed metadata, and playback progress (`last_position_ms`), not copies of media.
- Library items display title, media type (`Video` / `Audio`), duration, progress percentage (`Not started`, `%`, or `Missing`), and relative last-opened time.
- A missing, moved, unreadable, or unsupported file has a distinct status and actionable recovery path; notes are retained.
- File paths and metadata are treated as untrusted input. The application must not recursively index a folder without explicit user action.

### Playback and presentation modes

- Playback state is explicit: `Idle`, `Loading`, `Playing`, `Paused`, `Seeking`, `Ended`, or `Failed`.
- Seeking and note activation operate in media presentation time and clamp or reject invalid positions safely.
- Playback speed supports standard presets (`0.5×`, `0.75×`, `1.0×`, `1.25×`, `1.5×`, `2.0×`) with pitch-preserved audio where the backend supports it.
- Switching between **Full mode** and **Minimum mode** preserves active media, playback position, speed, and queue without interrupting playback.
- UI shows loading and failure state rather than freezing when a decoder or file operation is slow.
- Backend-specific differences are normalized into documented app behavior; unsupported tracks and codecs produce understandable messages and recovery suggestions (for example, `Error AV-104 · Decoder unavailable`).

### Notes

- A note contains an immutable identifier, media reference, timestamp in integer milliseconds (`position_ms`), text (`body`), and created/updated times.
- Creating a note captures the playhead once when triggered. Later playback or seeking while editing does not change its timestamp.
- Empty/whitespace-only text is allowed only if the user explicitly selects `Save as quick marker`; quick markers display a clear `Quick marker` label in lists and search results.
- A note can be edited, deleted (with confirmation or undo), searched, and activated to seek directly to its timestamp (overriding `Resume where I left off`).
- Notes remain available and readable if media is offline. Re-linking media (`Find file…`) preserves note associations where identity can be established; ambiguous matches require user confirmation and duplicate library records are never silently merged.
- Note timestamps are clamped to known duration when valid; unknown-duration streams allow non-negative capture without upper clamp as specified in [WORKFLOWS.md](WORKFLOWS.md).

### Search and organization

- Local search (`⌘/Ctrl+K`) covers media display name and note text offline.
- Library and search results can be filtered by media type (`All`, `Video`, `Audio`) and notes can be viewed chronologically by timestamp or by recent update.
- Search is strictly local and does not transmit query terms.

## Non-functional requirements

- Responsive UI: routine controls respond without waiting for long-running I/O; file and database work runs off the UI thread.
- Reliability: a crash or power loss must not corrupt the entire notes database; use transactions and atomic migration/backup practices.
- Privacy: no network requests during ordinary playback except an explicitly initiated update check or user-requested external link.
- Accessibility: keyboard operation and semantic labels for all core flows; automated checks supplement manual assistive-technology review.
- Portability: core playback and note rules are platform-independent; window chrome integration, filesystem integration, audio-route events, and packaging are platform adapters.
- Maintainability: modules have explicit ownership and versioned interfaces; avoid coupling user-visible behavior to an individual decoder implementation.

## First stable acceptance criteria

The stable release is ready only when all are true:

1. Supported macOS, Windows, and Linux packages can be installed and uninstalled using platform-appropriate steps.
2. A clean install can open representative supported audio and video files, play, pause, seek, adjust speed, switch between Full and Minimum modes, track/resume progress, and display useful failures for unsupported inputs.
3. A user can create (text notes and quick markers), edit, find, activate, and delete notes; notes survive application restarts and media relocation/relinking.
4. Persistence migration, recovery, and backup behavior are documented and exercised on all supported operating systems.
5. Accessibility review covers the complete open/play/note/find/return flow across Dark and Light themes.
6. Release artifacts have checksums, provenance/release notes, license notices, and a published format/dependency list.
7. No unresolved critical/high security issue, known data-loss defect, or license incompatibility remains.
