# Audora product and engineering documentation

This is the source of truth for the product and engineering baseline. Normative terms **MUST**, **MUST NOT**, **SHOULD**, and **MAY** are used in their usual requirements sense.

## Product

- [Product requirements](PRODUCT.md): audience, goals, supported behavior, accessibility, and acceptance criteria.
- [User workflows and glossary](WORKFLOWS.md): product glossary, edge-case rules, user-flow diagrams, happy/failure paths, and per-flow acceptance criteria.
- [Feature modules](FEATURES.md): boundaries and responsibilities of each product subsystem.
- [UI design handoff](UI_DESIGN.md): canonical Figma frame inventory, extracted Dark & Light design tokens, discrepancy resolutions, and accessibility/keyboard specification.
- [User-facing error catalog](ERROR_CATALOG.md): stable `AV-*` error codes, categories, user recovery steps, and log redaction rules.
- [Architecture](ARCHITECTURE.md): technology choices, runtime boundaries, data flow, and persistence model.
- [Platform support matrix](PLATFORM_SUPPORT.md): supported OS versions, architectures, installer formats, hybrid window shell, and per-OS directories.
- [Format, plugin, and license matrix](FORMAT_AND_LICENSE_MATRIX.md): verified GStreamer 1.28.1 LGPL plugin inventory, codec matrix, and redistribution rules.
- [Security and privacy](SECURITY.md): threat model, privacy defaults, secure development, and disclosure process.
- [Licensing](LICENSING.md): Apache 2.0 policy, dependency compatibility, media codec and plugin obligations.

## Delivery and engineering

- [Roadmap](ROADMAP.md): staged work from design validation through stable releases.
- [Phase and sprint plan](SPRINT_PLAN.md): sequenced work packages, dependencies, and phase gates.
- [Development plan](DEVELOPMENT.md): workspace structure, local commands, test strategy, CI, and platform support practices.
- [Coding conventions](CODING_CONVENTIONS.md): Rust, frontend, API, error, and documentation rules.
- [Git and commit conventions](GIT_CONVENTIONS.md): branches, commits, reviews, and versioning.
- [Contribution workflow](CONTRIBUTING.md): contributor expectations and project governance baseline.
- [Architecture decision records](adr/README.md): recorded architectural, licensing, and privacy decisions ([ADR 0001](adr/0001-desktop-shell-and-playback-architecture.md), [ADR 0002](adr/0002-project-licensing-and-codec-policy.md), [ADR 0003](adr/0003-local-first-privacy-and-untrusted-boundary.md)).

## Design status

**Phase 0 (Product and design baseline) and Phase 1 (Technical validation and detailed design) are complete.** **Phase 2 (Engineering foundation)** establishes the Rust/Tauri 2 core (`src-tauri/`), React/TypeScript frontend (`ui/`), SQLite WAL persistence and migration backup engine, and narrow typed IPC boundaries.
