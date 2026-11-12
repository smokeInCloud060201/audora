# Coding conventions

These are proposed project rules; implementation should add automated enforcement where practical.

## General

- Prefer clear, explicit names and small functions with one ownership boundary.
- Keep user-facing behavior and errors consistent across platforms.
- Avoid hidden global mutable state. Make I/O, clocks, and OS services explicit dependencies where testability benefits.
- Do not use `unsafe` Rust without a documented safety argument and focused review.
- Never suppress a warning or lint without a scoped explanation.

## Rust

- Use stable Rust and commit the toolchain configuration once phase 1 pins it.
- Format with `rustfmt`; lint with Clippy. CI treats warnings as errors for project code after baseline is clean.
- Return typed errors at module boundaries; add context without leaking sensitive paths or content to UI/logs.
- Use `Result` for recoverable failures; do not panic on user input, files, database state, or backend errors.
- Validate all serialized input at entry points. Define bounds for strings, collections, timestamps, and IDs.
- Keep domain models independent of Tauri, SQL, and GStreamer types. Convert at adapters.
- Use integer milliseconds for persisted timestamps; convert to backend clock types only at the playback boundary.
- Prefer immutable data and message passing for concurrent playback state. Document lock ordering and callback ownership if synchronization is necessary.
- Document public modules and non-obvious invariants. Safety comments are mandatory for `unsafe` blocks.

## Frontend

- TypeScript strict mode; avoid `any` and unchecked casts. Validate IPC responses as boundary data where possible.
- UI components render state and dispatch typed commands; they do not access SQL, filesystem, or privileged APIs directly.
- Represent loading, empty, error, unavailable, and success states explicitly.
- Use semantic HTML, accessible names, keyboard operation, visible focus, and reduced-motion-aware effects.
- Render note text as text. Do not inject HTML from notes, filenames, or media metadata.
- Keep state updates resilient to out-of-order asynchronous responses using request/load generations.

## APIs and errors

- Name commands with verbs, define typed inputs/results, and keep payloads bounded.
- Make operations idempotent where feasible; use stable IDs for mutations.
- Distinguish validation, not-found, unsupported, unavailable, I/O, persistence, and internal errors.
- User messages explain the next useful action; logs may include diagnostic context after redaction.

## Documentation

- Document user-visible behavior, platform deviations, persistence changes, security implications, and licensing impacts.
- Update architecture/docs in the same change when a contract or policy changes.
- Add an ADR for significant tradeoffs instead of relying on code comments or chat history.
