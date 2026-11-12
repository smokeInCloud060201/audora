# Audora product and engineering documentation

This is the source of truth for the product and engineering baseline. Normative terms **MUST**, **MUST NOT**, **SHOULD**, and **MAY** are used in their usual requirements sense.

## Product

- [Product requirements](PRODUCT.md): audience, goals, supported behavior, accessibility, and acceptance criteria.
- [User workflows and glossary](WORKFLOWS.md): product glossary, edge-case rules, user-flow diagrams, happy/failure paths, and per-flow acceptance criteria.
- [Feature modules](FEATURES.md): boundaries and responsibilities of each product subsystem.
- [UI design handoff](UI_DESIGN.md): canonical Figma frame inventory, extracted design tokens, discrepancy resolutions, and accessibility/keyboard specification.
- [Architecture](ARCHITECTURE.md): technology choices, runtime boundaries, data flow, persistence model, and Phase 1 validation ownership.
- [Security and privacy](SECURITY.md): threat model, privacy defaults, secure development, and disclosure process.
- [Licensing](LICENSING.md): Apache 2.0 policy, dependency compatibility, media codec and plugin obligations.

## Delivery and engineering

- [Roadmap](ROADMAP.md): staged work from design validation through stable releases.
- [Phase and sprint plan](SPRINT_PLAN.md): sequenced work packages, dependencies, and phase gates.
- [Development plan](DEVELOPMENT.md): build, test, quality, CI, and platform support practices.
- [Coding conventions](CODING_CONVENTIONS.md): Rust, frontend, API, error, and documentation rules.
- [Git and commit conventions](GIT_CONVENTIONS.md): branches, commits, reviews, and versioning.
- [Contribution workflow](CONTRIBUTING.md): contributor expectations and project governance baseline.
- [Architecture decision records](adr/README.md): recorded architectural, licensing, and privacy decisions ([ADR 0001](adr/0001-desktop-shell-and-playback-architecture.md), [ADR 0002](adr/0002-project-licensing-and-codec-policy.md), [ADR 0003](adr/0003-local-first-privacy-and-untrusted-boundary.md)).

## Design status

**Phase 0 (Product and design baseline) is complete.** The documents define the reviewed product scope, Figma UI/accessibility baseline, security/licensing policies, and initial ADRs. **Phase 1 (Technical validation and detailed design)** is the current active phase to validate architecture choices with cross-platform spikes before Phase 2 feature implementation begins. Any change to supported platforms, persistence guarantees, licensing, media backend, or security boundaries requires a documented decision and corresponding updates to these documents.
