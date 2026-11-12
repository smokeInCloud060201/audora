# Git, commits, and reviews

## Branches

Use short-lived branches named by intent, such as `feat/note-search`, `fix/relink-path`, or `docs/security-policy`. Keep branches focused and rebase/merge according to repository hosting policy once established. Do not commit generated installers or private signing material.

## Commit messages

Use Conventional Commits:

```text
<type>(optional-scope): imperative summary

[optional body explaining why and relevant constraints]
```

Common types: `feat`, `fix`, `docs`, `refactor`, `perf`, `test`, `build`, `ci`, `chore`, `revert`. Keep the summary concise and imperative. Use `!` and a `BREAKING CHANGE:` footer for incompatible public behavior or persisted/API changes. Each commit should represent one coherent change and leave the tree buildable when practical.

Examples:

```text
docs: define local media and note requirements
feat(notes): seek to a selected timestamp
fix(storage): retain notes after media is moved
```

## Pull requests

- Explain user impact, design choice, tests/checks run, platform coverage, migration behavior, and security/licensing impact.
- Link to an issue or ADR for nontrivial work.
- Include screenshots or short recordings for visual changes when useful, with no private media.
- Require review by a maintainer; security-sensitive, persistence, playback backend, and licensing changes need focused review.
- CI must pass. Reviewers verify accessibility, error states, data-loss risk, and documentation updates.
- Do not merge knowingly failing builds or unresolved release-blocking security/license findings.

## Versioning

Use Semantic Versioning for public releases once the first stable API/data policy is published. Before 1.0, release numbers may change, but persisted data must still be migrated or safely rejected with recovery instructions. Treat the database schema and import/export formats as compatibility surfaces.
