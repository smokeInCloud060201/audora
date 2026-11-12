# Development and delivery plan

## Workspace shape (proposed)

```text
src-tauri/          Rust application, services, adapters, migrations
ui/                 TypeScript UI and frontend assets
docs/               Product and engineering design
packaging/          Installer resources, icons, platform metadata
```

Keep the workspace simple until code boundaries demonstrate a need for more crates. Domain rules should be testable without launching a window or a decoder.

## Local development

Document pinned toolchain/runtime versions and platform prerequisites in the developer setup guide before implementation. Provide one documented command for formatting/lint checks and one for launching a development build. Avoid requiring secrets for ordinary development. Test media fixtures must be legally redistributable, small, and documented with their licenses.

## CI gates

For every pull request:

- Format Rust and frontend code; run static analysis and type checking.
- Build supported target configurations where runners permit.
- Run unit and integration checks for domain, persistence, command validation, and packaging metadata.
- Scan dependencies for vulnerabilities and license changes; review exceptions explicitly.
- Verify migrations from the previous supported schema and preserve a database backup on failure.
- Check secret leakage, generated artifact drift, and documentation for changed interfaces.

Release workflows additionally build clean packages, produce SBOM/checksums, verify notices, sign/notarize as configured, and attach provenance and release notes. CI must not expose signing keys to untrusted pull-request code.

## Quality strategy

- Domain tests: note timestamp validation, search, media identity/relink decisions, playback state transitions.
- Persistence tests: migrations, transaction rollback, backup/restore, corruption handling, concurrent access.
- Adapter tests: backend event translation, command sequencing, stale generation suppression.
- UI tests: keyboard and focus flow, note lifecycle, error/loading states, responsive layout.
- Platform acceptance: file dialogs, drag/drop, audio/video output, installers, permissions, uninstall, and open-with.
- Manual accessibility and supported-format verification per release.

Use deterministic fixtures and fake playback backends for most tests; a smaller end-to-end suite uses known media assets on actual OS runners/devices.

## Observability

Local structured logs use severity and operation identifiers, with sensitive values redacted. Provide a user-controlled diagnostic export. No remote telemetry is enabled by default. Performance measurement focuses on startup, file-open responsiveness, seek latency, UI responsiveness during playback, and memory use over long sessions.

## Support policy

Publish minimum OS versions, supported Linux targets, CPU architectures, media format matrix, installation/update path, and support window before first stable release. State known limitations and distinguish application bugs from absent OS/plugin codec support.
