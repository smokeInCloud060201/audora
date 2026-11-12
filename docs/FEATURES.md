# Feature modules

Module boundaries are logical boundaries; the implementation may begin in a single workspace with separate crates only where separation yields clear ownership.

| Module | Owns | Must not own |
|---|---|---|
| Application shell | Window lifecycle, hybrid 56 px OS header, Full/Minimum mode sizing, menus, startup/shutdown, OS integration | Playback engine internals or note persistence rules |
| UI | Player, library, note editor/list, queue panel, settings, accessibility, view state | Direct filesystem or SQL access |
| Command/API layer | Validated commands and events between UI and Rust services; request correlation and error mapping | Business rules duplicated in UI |
| Playback service | Playback state machine, load generations, session queue (repeat/shuffle), seek, playback speed (`0.5×–2.0×`), volume/mute, track selection, backend abstraction | Database writes or UI toolkit types |
| Media backend adapter | Decoder/output integration, video surface binding, backend event translation, platform-specific backend setup | Product policy, note semantics |
| Library service | Media references, metadata refresh, playback progress (`last_position_ms`), missing/relink states, search queries | Copying/editing source media |
| Notes service | Note and quick-marker creation/update/delete, timestamp precision/validation, note lookup/search | UI presentation or playback implementation |
| Persistence | Database connection, migrations, transactions, backups, repositories | Feature-specific policy beyond persistence invariants |
| Platform services | File dialogs, show-in-folder, open-with/deep links, app directories, audio-route disconnect events, packaging hooks | Cross-platform domain logic |
| Configuration | Typed settings (theme, resume, headphone-disconnect pause, default speed, timestamp precision), defaults, validation | Arbitrary secrets (none expected) |
| Diagnostics | Redacted logs, stable error codes (`AV-*`), local diagnostic bundle generation | Silent upload or media/note content collection |

## Dependency direction

UI → command/API → application services → domain interfaces. Services depend on repository and playback traits, not concrete UI or OS types. Platform and backend adapters implement those interfaces. Persistence depends on domain data contracts but the domain does not depend on SQL details. No module may call another module's private storage directly.

## Core contracts

- `MediaId`: stable application-generated identifier.
- `MediaReference`: canonicalized path plus best-effort size/mtime/duration metadata; never treated as proof of content identity by itself.
- `MediaInfo`: display metadata, `media_kind` (`Video` | `Audio`), `duration_ms` (optional), `last_position_ms`, `completed`, `availability`, available tracks, and backend capability summary.
- `PlaybackSnapshot`: state, generation ID, `media_id`, `position_ms`, `duration_ms`, `rate` (`0.5–2.0`), `volume`, `muted`, `repeat_mode`, `shuffle`, and capabilities.
- `Note`: `id`, `media_id`, `position_ms` (integer milliseconds), `is_quick_marker`, `body`, `created_at`, and `updated_at`.
- Commands return typed results; events are notifications and may not silently replace authoritative reads.
- All cross-boundary values are serializable, versioned where persisted or exposed to frontend, and validated at the receiving boundary.

## Module completion definition

Each module documents its public contract, error cases, ownership of state, and test strategy. No feature is complete while its persistence behavior, accessibility implications, platform behavior, and user-facing failure cases remain unspecified.
