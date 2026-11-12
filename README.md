# Audora

Audora is a cross-platform, local-first desktop media player for macOS, Windows, and Linux. It plays local audio and video and lets people create named, timestamped notes and quick markers while media is playing, then return to those moments later.

## Project status

**Phases 0–6 are complete.** The workspace includes:

- **Core crate (`crates/audora-core/`)** — Domain models, GStreamer + fake playback adapters, SQLite WAL persistence with automatic pre-migration/manual/corrupt backups, offline library/note search, media relinking, and GitHub Releases update verification.
- **Desktop shell (`apps/desktop/src-tauri/`)** — Native `tao` + `wry` window, loopback UI & state bridge, native self-updater (`/__audora_update`), and OS installer cleanup hooks (`--uninstall-cleanup`).
- **Desktop UI (`apps/desktop/ui/`)** — React 19 + TypeScript (`strict`) + Vite 6 + semantic CSS token system (`Dark`, `Light`, `System`), Minimum Mode, and verified software update dialog.
- **Documentation (`.docs/`)** — Read [the documentation index](.docs/README.md) for product workflows, Figma UI/accessibility baseline, architecture, security model, and release roadmap.

## Development & verification

```bash
pnpm install
pnpm check
pnpm build:ui
pnpm verify:release
```

## Intended platforms

- macOS (Apple Silicon and Intel, macOS 13+)
- Windows (64-bit)
- Linux (64-bit `.deb` and `.tar.gz`)

## License

Licensed under the [Apache License 2.0](LICENSE). See [licensing policy](.docs/LICENSING.md) and [third-party notices](THIRD_PARTY_NOTICES.md).
