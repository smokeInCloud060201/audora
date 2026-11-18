# Security and privacy

## Security goals

- Opening untrusted media must not grant the file or decoder access to arbitrary user data.
- The UI can invoke only the application operations it needs.
- Local notes and metadata are not disclosed to the network by default.
- Updates, installers, dependencies, and release artifacts can be traced to project sources.
- Failures do not silently lose notes or alter original media.

## Threat model

Protect against malicious or malformed media files, crafted metadata/subtitles, compromised or vulnerable decoder/plugin dependencies, malicious frontend input, path traversal, symlink confusion, hostile files in dropped paths, dependency/supply-chain compromise, unsafe update artifacts, and accidental leakage through logs or diagnostic bundles. Local malware already running as the same user and physical compromise of an unlocked machine are outside the primary boundary; notes are not encrypted at rest by default and that limitation must be documented.

## Required controls

### Media and filesystem

- Treat media, metadata, filenames, subtitles, playlists, and dragged paths as untrusted.
- Do not execute or interpret media as scripts. Do not invoke a shell with user-controlled strings.
- Resolve filesystem access through narrow Rust services; reject paths outside user-selected scope for operations that could traverse directories.
- Opening a file is an explicit user action. No background recursive scan by default.
- Keep decoder and plugin versions current; isolate playback in a subprocess only if threat/risk assessment justifies the complexity and the design is reviewed.

### Native shell, WebView, and loopback boundary

- Expose only narrow, explicitly validated IPC commands and loopback endpoints ([ADR 0004](adr/0004-native-webview-loopback-architecture.md)); never grant unrestricted filesystem, shell, or arbitrary network access to the WebView.
- Authenticate every loopback HTTP request (`127.0.0.1:<port>`) with a per-launch 256-bit CSPRNG `X-Audora-Token` using constant-time comparison, validate the `Host` header to block DNS rebinding, cap request body sizes, and restrict `/__audora_media` byte-range streaming to canonical paths registered in `MediaAllowlist`.
- Enforce a strict non-wildcard Content Security Policy (`default-src 'self'; script-src 'self'; object-src 'none'; frame-ancestors 'none'`); never load remote scripts or remote UI content.
- Validate every RPC payload in Rust (`audora-core` + `audora-desktop`), including text length bounds, finite numeric ranges, timestamp validity, and local path safety (rejecting remote URLs and UNC/WebDAV network shares with `AV-301`). Never trust frontend validation alone.
- Prevent navigation to untrusted origins; external documentation/release links open through an explicit `https://`-only OS handoff.
- Keep IPC payloads small and avoid returning raw secrets, full filesystem inventories, or unnecessary media metadata.

### Data and logging

- Parameterize all SQL queries; enable foreign keys and use transactions.
- Bound text lengths and result sizes. Escape/render note text as text, not HTML.
- Logs exclude note bodies, full paths where practical, and media content. Diagnostic export is user initiated, previews included data, and supports redaction.
- No telemetry or crash upload by default. Any later collection requires an ADR, privacy review, explicit opt-in, clear retention/deletion policy, and minimum necessary fields.
- Backups use restrictive per-user permissions where available and are never uploaded automatically.

### Supply chain and releases

- Pin and review dependencies; automate vulnerability and license inventory checks in CI.
- Review new native dependencies and GStreamer plugins for maintenance, permissions, license terms, and platform availability.
- Generate an SBOM for release artifacts and include third-party notices.
- Protect release credentials; use least-privilege CI tokens, protected release workflows, signed tags/artifacts where supported, and checksums.
- Verify updater metadata and payload signatures before applying any future update.

## Vulnerability reporting and response

Before public release, publish a private security contact and supported-version policy in the repository security policy. Acknowledge reports promptly, reproduce in a controlled environment, assess severity and affected versions, prepare a fix, coordinate disclosure, and publish an advisory and patched release. Do not ask reporters to disclose exploit details publicly before a fix is available.

## Release security gate

Every stable release reviews dependency advisories, plugin inventory, permission/capability changes, data migration risks, installer/update integrity, and unresolved vulnerabilities. Critical/high issues block release until fixed or an explicit, public risk decision is recorded.
