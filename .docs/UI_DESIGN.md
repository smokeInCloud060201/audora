# UI design handoff and accessibility specification

## Canonical design reference and review status

- **Figma file:** [Audora Figma Design (`Page 1`, node `0:1`)](https://www.figma.com/design/PaNgbiT8RSycIk1QBzPiqf/Audora?node-id=4-522&m=dev)
- **Review status:** **Completed in Phase 0 (Sprint 0.2)** via authorized Figma Dev Mode MCP inspection (`get_metadata`, `get_variable_defs`, and `get_design_context` across all 11 top-level frames).
- **Source-of-truth contract:** The Figma file is the canonical visual and layout reference; [PRODUCT.md](PRODUCT.md) and [WORKFLOWS.md](WORKFLOWS.md) govern functional and accessibility acceptance criteria. All discrepancies identified during Phase 0 have been resolved and recorded below.

---

## Figma frame inventory (`Page 1` / `0:1`)

All workspace frames are designed at **`1440 × 900 px`**, except **`Audora / Minimum mode · 360×168`** (`2:103` / `20:2`), which is designed at **`360 × 168 px`**.

| Node ID | Frame Name | Purpose & Key Elements | Covered Requirements |
|---|---|---|---|
| `2:3` | **Audora / Full mode** | Primary video player workspace: 56 px header, 76 px nav rail (`Now playing` active), 999×716 video player + 128 px transport (`14:24 / 48:12`, scrubber, `↶ ‹ ▶ › ↷`, `1×` speed, volume), 365 px right `Notes panel` with `Queue 3 \| Notes 4` tabs, `+ Add note`, 4 note cards, and `N` keyboard tip. | Video playback, transport, speed, notes list, seek-to-note, Full/Minimum mode switch. |
| `2:103` (`20:2`) | **Audora / Minimum mode · 360×168** | Compact floating frameless player (`360 × 168 px`, `#101513`, `14px` radius): 52 px draggable title bar (`28×28` Audora mark, track title + `Audora · Local`, `?` shortcuts button, `↗` expand-to-full-mode button) and 116 px transport body (`42×42` mint play/pause button, `22px` current timestamp + `/ duration`, and `272×4` scrubber with `12×12` thumb). | Floating Minimum mode presentation, draggable frameless chrome, keyboard hint, seamless return to Full mode. |
| `4:3` | **Audora / Audio playback** | Audio-only Now Playing layout: square `340×340` waveform/album artwork with concentric glow rings, track metadata (`FIELD NOTES — 02`, title, subtitle), full-width 1364×128 bottom transport, and right `Queue` panel (`3 local items`). | Audio-only playback experience, session queue display, speed & transport. |
| `4:88` | **Audora / Add note** | Modal overlay (`500×420` dialog) over dimmed Full mode: `CAPTURED AT 14:24` badge, `Add a note` heading, close `×`, plain-text `Note text area`, `Save as quick marker` checkbox card, `⌘ Enter to save` hint, `Cancel`, and `Save note`. | Immutable playhead capture, text note & quick marker creation, keyboard submit. |
| `4:206` | **Audora / Media library** | Populated local collection view: `Media library` header, `▣ Open media` CTA, `Search titles and notes…` (`⌘ K`), filter pills (`All`, `Video`, `Audio`), and `Media table` (`TITLE`, `PROGRESS`, `LAST OPENED`) showing video, audio, `Not started`, and inline `Missing` (`3 notes safe`) rows. | Local library, progress %, type filter, offline search entry, missing-file status. |
| `4:300` | **Audora / Library empty state** | First-run / empty library view: dashed `1180×470` container (`Your library is ready`, `Open a local audio or video file to start listening and taking notes.`, `Open your first file` primary CTA). | Empty library state and explicit user-initiated file open. |
| `4:339` | **Audora / Missing media** | Offline media recovery view (`‹ Library` back link): `MEDIA OFFLINE` alert header, `We can’t find this file`, `Last known location` path box, `▣ Find file…` and `Remove from library` buttons, plus right-side `Your notes are preserved` card listing saved notes. | Missing file status, note preservation while offline, explicit relinking & removal. |
| `4:399` | **Audora / Unsupported media** | Playback failure screen: `PLAYBACK ERROR` badge, `This format isn’t supported`, filename/codec message, `What you can do` recovery box, `Choose another file` & `Show in folder` actions, and `Error AV-104 · Decoder unavailable` footer. | Normalized decoder error handling, recovery guidance, stable error code. |
| `4:441` | **Audora / Settings** | Preferences workspace with 6 section tabs (`Appearance`, `Playback`, `Notes`, `Keyboard`, `Privacy`, `About`): `Dark / Light / System` theme preview cards and `Playback` toggles (`Resume where I left off`, `Pause when headphones disconnect`, `Default playback speed: 1.0×`). | Theme selection, resume-position preference, headphone-disconnect setting, default speed. |
| `4:523` | **Audora / Components & widgets** | Design reference sheet: `Primary action / Secondary / Disabled` buttons, `Search field`, `Select` (`Nearest second ⌄`), `Playback` scrubber, `Note` card, `Success toast` (`✓ Note added`), `Status & feedback` pills (`Loading`, `Paused`, `Playing`, `Seeking`, `Ended`, `Error`), `Mode selector`, and `Timestamp` badge (`▶ 12:18`). | Reusable component states, timestamp precision control, playback state badges, toast feedback. |
| `4:609` | **Audora / Queue panel** | Video Full mode with the right-hand `Queue` list (`3 local items`, active `Playing now` highlight, item durations, and per-item `⋯` menu). | Session queue management during video playback. |

---

## Visual system and design tokens

`get_variable_defs` confirmed that the Figma file currently styles layers directly rather than binding Figma Variables (`{}`). Implementation **MUST** codify these extracted values into semantic CSS custom properties (`--bg-canvas`, `--accent-primary`, etc.) rather than scattering raw hex literals across components.

### Color tokens (Dark theme baseline)

| Token Role | Extracted Hex | Usage in Figma |
|---|---|---|
| `--bg-window` | `#090b0e` / `#101318` | Root window background (`#090b0e` player views, `#101318` content views, `#0b0e0d` Minimum mode) |
| `--bg-titlebar` | `#111419` | 56 px top header bar |
| `--bg-nav-rail` | `#0e1115` | 76 px left navigation rail |
| `--bg-surface-1` | `#12151a` / `#15191f` | Panels, note cards, status badges, table container |
| `--bg-surface-2` | `#171b20` / `#181c22` | Secondary buttons, inputs, active queue/note hover cards |
| `--bg-dialog` | `#13171c` / `#0d1014` | Modal dialog (`Add note`) and inset callout boxes (`Recovery steps`, `Last known location`) |
| `--border-subtle` | `#20252c` | 1 px structural dividers (title bar, nav rail, panel tabs, table rows) |
| `--border-control` | `#303640` / `#30353d` | Button, input, card, and outer window borders (`#343a43` dashed empty state) |
| `--accent-primary` | `#73d4bd` | Brand mark, primary CTA fill, active scrubber fill, active nav indicator, toggles |
| `--accent-on-primary` | `#092019` | High-contrast dark teal text/icons on `--accent-primary` buttons |
| `--accent-surface` | `#172a26` / `#18352f` | Active nav item background (`#172a26`) and timestamp pill fill (`#18352f`) |
| `--accent-text` | `#8ce6d0` | Timestamp badge text (`▶ 03:42`) and `Jump to moment` action text |
| `--text-primary` | `#f2f4f6` | Page titles, headings, active labels, modal title |
| `--text-secondary` | `#c8cdd3` / `#929aa6` | Body copy, subtitles, secondary button labels |
| `--text-muted` | `#7f8792` | Inactive nav labels, metadata captions (`— Local`, durations) *(see Accessibility contrast note below)* |
| `--status-error-fg` | `#ec7d77` | Error icon, `PLAYBACK ERROR` / `MEDIA OFFLINE` eyebrow, error status dot |
| `--status-error-bg` | `#231c1d` | Error badge container (border `#3b3535`) |
| `--status-warn-fg` | `#e3b341` | `Seeking` status indicator dot |

### Color tokens (Light theme mapping for `4:492` & `System` `4:499`)

Derived from the `Light` preview surface palette (`#f3f1ed` / `#e8e6e1` in `4:493`) and tuned to meet WCAG 2.1 AA contrast (`≥ 4.5:1` for text):

| Token Role | Light Theme Hex | Usage |
|---|---|---|
| `--bg-window` | `#f5f4f0` | Root window background |
| `--bg-titlebar` | `#eceae4` | 56 px top header bar |
| `--bg-nav-rail` | `#eae8e2` | 76 px left navigation rail |
| `--bg-surface-1` | `#ffffff` | Panels, note cards, table container |
| `--bg-surface-2` | `#efede7` | Secondary buttons, inputs, hover surfaces |
| `--bg-dialog` | `#ffffff` | Modal dialog (`Add note`) and callout boxes |
| `--border-subtle` | `#dcdad2` | 1 px structural dividers |
| `--border-control` | `#c7c4bc` | Button, input, card, and outer window borders |
| `--accent-primary` | `#127864` | Primary CTA fill, active scrubber fill, active nav indicator |
| `--accent-on-primary` | `#ffffff` | High-contrast white text/icons on `--accent-primary` buttons |
| `--accent-surface` | `#dff3ee` | Active nav item background and timestamp pill fill |
| `--accent-text` | `#0d5c4c` | Timestamp badge text (`▶ 03:42`) and `Jump to moment` action text |
| `--text-primary` | `#121619` | Page titles, headings, active labels |
| `--text-secondary` | `#3d454d` | Body copy, subtitles, secondary button labels |
| `--text-muted` | `#5a636e` | Metadata captions (`— Local`, durations; `≥ 5.3:1` on `#f5f4f0`) |
| `--status-error-fg` | `#b42318` | Error icon, `PLAYBACK ERROR` / `MEDIA OFFLINE` text |
| `--status-error-bg` | `#fef3f2` | Error badge container (border `#fecdca`) |
| `--status-warn-fg` | `#b54708` | `Seeking` status indicator dot |

### Typography, radii, and elevation

- **Font family:** `Inter` (`400 Regular`, `500 Medium`, `600 Semi Bold`) with system UI sans-serif fallbacks; timestamps and playback counters **MUST** enable `font-variant-numeric: tabular-nums` so time updates never jitter layout horizontally.
- **Type scale (desktop 100% zoom):**
  - Page title (`Media library`, `Settings`, error headings): `30px` SemiBold
  - Compact time readout (Minimum mode `20:15`): `22px` SemiBold (`10px` `/ duration` `20:16`)
  - Modal / section title (`Add a note`, `Notes`, `Queue`): `21px–25px` SemiBold
  - Card / subsection heading: `17px–19px` SemiBold
  - Body & primary inputs: `13px–15px` Regular / Medium
  - Compact UI controls, metadata & eyebrow labels: `11px–12px` Medium / SemiBold *(note: Figma uses `9px–10px` on eyebrows/nav labels; implementation **SHOULD** use an `11px`–`12px` floor or relative `rem` units that scale cleanly).*
- **Radii:** Window `16px` (when frameless/applicable), Dialog & theme cards `12px`, Empty state `10px`, Nav items & cards `8px`, Buttons & inputs `7px`, Timestamp pills `4px`.
- **Elevation:** Primary shell/modal shadow `0 24px 60px rgba(0, 0, 0, 0.45)`, secondary surface shadow `0 20px 50px rgba(0, 0, 0, 0.35)`.

---

## Recorded discrepancies and Phase 0 resolutions

Per [PRODUCT.md](PRODUCT.md), all differences between the Figma file and initial docs were reviewed and resolved with the project owner during Phase 0:

1. **Minimum mode & Playback speed (`2:103`, `2:60`, `4:520`):** Promoted into **v1.0 stable scope** in [PRODUCT.md](PRODUCT.md) and [ARCHITECTURE.md](ARCHITECTURE.md).
2. **Library progress & audio-route pause (`4:247`, `4:508`, `4:513`):** Per-file progress (`%`) and `Resume where I left off` are included in **v1.0** (`last_position_ms` added to SQLite `media` schema). `Pause when headphones disconnect` is retained in Settings subject to per-OS capability validation in Phase 1 (hidden or clearly marked unavailable on Linux environments that do not expose reliable route-change events).
3. **Queue panel tabs & Repeat/Shuffle controls (`2:65`, `4:73`, `4:609`):** Frames `4:3` and `4:609` omitted the `Queue | Notes` tab bar and `Repeat / Shuffle` buttons. **Resolution:** Keep the 49 px `Queue | Notes` tab bar persistent at the top of the 365 px right panel in **both** Video and Audio views so users can always switch between `Queue` and `Notes`, and add compact icon toggle buttons for `Shuffle` and `Repeat` (`Off / All / One`) in the Queue subheader next to `Queue · N local items`.
4. **Window title bar & OS controls (`2:4`):** Figma shows 3 right-aligned dots inside a 56 px header. **Resolution:** Implement a **hybrid 56 px header** (`data-tauri-drag-region`) that integrates native OS window controls in their platform-standard positions (macOS traffic lights inset on the left shifting the Audora mark right by ~72 px; Windows/Linux native/overlay caption controls on the right).
5. **Canvas coordinate glitch on `4:523` (`Audora / Components & widgets`):** Node `4:573` (`Note 03:42`) sits at `x=0, y=252` overlapping `Primary action` instead of `x=760, y=252` under the `Notes` heading. Treat as a reference-sheet positioning typo only; actual `Note` card layout (`2:74`) is unaffected.

---

## Complete UI state & follow-up specification

Every required product flow is covered either directly by a Figma frame or by the explicit component specification below using existing tokens from `4:523`:

| Area / State | Coverage Source & Required Behavior |
|---|---|
| **Playback states (`Idle`, `Loading`, `Playing`, `Paused`, `Seeking`, `Ended`, `Failed`)** | Status pills defined in `4:583–4:600`. `Idle` routes to Library or shows an `Open media` prompt in `Now playing`. `Loading` disables scrubber drag and shows `Loading` indicator without freezing transport/nav. `Seeking` updates scrubber thumb while coalescing requests, confirming time from backend clock. `Ended` shows replay icon (`↻`) on primary transport button unless Queue advances. `Failed` routes to or overlays the `4:399` error pattern. |
| **Notes empty state** | When active media has `0` notes, render a centered muted state inside `Notes panel` (`2:63`) using the `4:331` pattern: `"No notes yet"` + `"Press N or click + Add note to mark a moment."` |
| **Quick marker item** | Uses the standard `Note` card (`2:74`) with `▶ mm:ss` timestamp pill and muted italic placeholder `"Quick marker — add details…"` (styled distinctly from user text) plus `Jump to moment` and `⋯`. |
| **Edit & Delete note** | `⋯` menu (`2:79`) opens a keyboard-accessible menu (`Edit note`, `Delete note`). `Edit note` reuses the `Add note` modal (`4:189`) titled `"Edit note"` (preserving `CAPTURED AT mm:ss`). `Delete note` removes the item and shows the `4:578` toast bar (`"Note deleted"` + `"Undo"` button for 6 seconds, or an explicit confirmation dialog when reduced-timeout accessibility is active). |
| **Relink ambiguity dialog** | Reuses the `4:189` `500px` modal shell when `Find file…` selects a file with mismatched duration/type (displaying prior vs. selected duration and `Cancel` / `Relink anyway` buttons) or blocks merge if the path already belongs to another `MediaId`. |
| **Remaining Settings sections (`Notes`, `Keyboard`, `Privacy`, `About`)** | Reuse the `4:441` two-column Settings layout (`360px` content x-offset, `650px` row dividers): **Notes** contains `Timestamp precision` (`Nearest second ⌄` select from `4:562` vs. `Milliseconds`); **Keyboard** lists read-only shortcut reference table; **Privacy** states local-only storage path, backup location, and `Export diagnostic bundle…`; **About** displays version, Apache-2.0 license notice, third-party attributions, and supported codec/backend summary. |

### Responsive layout and minimum window dimensions

- **Full mode minimum window size:** **`960 × 600 px`** (recommended default `1280 × 800` or `1440 × 900`).
  - Fixed chrome: `56px` header, `76px` navigation rail, `365px` right `Queue / Notes` panel (`320px` compressed width between `960px–1100px`), `128px` transport bar.
  - Flexible canvas: Video/Audio player stage scales fluidly (`min-width: 556px`, `min-height: 416px`) preserving aspect ratio with letterboxing. Notes list and Library table scroll vertically within their containers (`overflow-y: auto`).
- **Minimum mode window size:** **`360 × 168 px`** (frameless always-on-top floating window per `2:103` / `20:2`, with `52px` draggable header and full-width horizontal scrubber).

---

## Accessibility and keyboard specification

### Focus order and landmarks
1. **Header (`<header>`):** Mode toggle (`— Minimum` / `Open full mode`).
2. **Navigation rail (`<nav aria-label="Primary">`):** `Library`, `Now playing`, `Settings` (`aria-current="page"` on active view).
3. **Main stage (`<main>`):** View-specific primary controls (Player canvas/transport in `Now playing`; Search input → Filter pills → Media table rows in `Library`; Sub-navigation tabs → Setting controls in `Settings`).
4. **Side panel (`<aside aria-label="Queue and Notes">`):** `Queue` / `Notes` `role="tablist"` → `+ Add note` CTA → chronological note cards (`Jump to moment` button, `⋯` actions button).
5. **Dialogs (`role="dialog" aria-modal="true"`):** Focus moves automatically into `Note text area` on open, traps `Tab` / `Shift+Tab` inside the modal, and restores focus to the triggering control (`+ Add note` or player) on close.
6. **Visible focus:** All interactive controls **MUST** show a high-contrast `:focus-visible` ring (`2px solid var(--accent-primary)` with `2px` offset); never `outline: none` without replacement.

### Baseline keyboard shortcuts (v1.0)

Single-key shortcuts **MUST** be suppressed whenever focus is inside `<input>`, `<textarea>`, `<select>`, or an open modal dialog.

| Shortcut | Action |
|---|---|
| `Space` | Toggle Play / Pause |
| `N` | Capture current playhead and open `Add a note` dialog |
| `Shift + N` | Immediately save a Quick Marker at current playhead and show `Note added` toast |
| `⌘/Ctrl + Enter` | Submit `Add a note` / `Edit note` dialog |
| `Escape` | Close open modal/popover, or return from Minimum mode to Full mode |
| `←` / `→` | Seek backward / forward `5 seconds` (`Shift + ←/→` for `15 seconds`) |
| `↑` / `↓` | Increase / decrease volume by `5%` (when player/transport focused) |
| `M` | Toggle Mute |
| `⌘/Ctrl + K` | Focus Library `Search titles and notes…` input |
| `⌘/Ctrl + O` | Open native file picker (`Open media`) |

### Screen-reader semantics, contrast, scaling, and motion
- **Accessible names for glyph controls:** Icon-only controls in Figma (`↶`, `‹`, `▶`, `›`, `↷`, `◖))`, `⋯`, `×`) **MUST** use SVG icons (`aria-hidden="true"`) paired with explicit `aria-label` strings (`"Rewind 10 seconds"`, `"Previous item"`, `"Play"` / `"Pause"`, `"Next item"`, `"Forward 10 seconds"`, `"Mute"` / `"Volume"`, `"More actions for note at 03:42"`, `"Close dialog"`).
- **Seek & volume sliders:** Implement using native `<input type="range">` or WAI-ARIA `role="slider"` with `aria-valuemin`, `aria-valuemax`, `aria-valuenow`, and human-readable `aria-valuetext` (`"14 minutes 24 seconds of 48 minutes 12 seconds"`).
- **Live regions:** Toast notifications (`Note added`, `Note deleted`) use `role="status" aria-live="polite"`; playback/decoder errors (`Error AV-104`) use `role="alert"`.
- **Contrast adjustment rule:** Low-contrast muted tokens in Figma such as `#5f6872` on `#101318` (~3.4:1) are decorative/footer-only; any essential status, error code, or interactive text **MUST** use at least `#8d96a0` / `#929aa6` (≥ 4.5:1 against `#101318` / `#090b0e`) to pass WCAG 2.1 AA.
- **Text scaling & reduced motion:** Layout **MUST** remain usable without clipped controls at `125%–150%` UI/text zoom. Respect `prefers-reduced-motion: reduce` by disabling ambient glow pulse animations and modal/panel slide transitions.
