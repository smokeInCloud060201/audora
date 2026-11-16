#!/usr/bin/env node
/**
 * Audora Phase 5 & Phase 6 Release Gate Verifier (`pnpm verify:release`).
 *
 * Verifies that all required legal, licensing, security, and build artifacts exist,
 * and outputs SHA-256 checksums for built frontend and migration assets.
 */

import { createHash } from "node:crypto";
import { existsSync, readdirSync, readFileSync, statSync } from "node:fs";
import { join, resolve } from "node:path";

const rootDir = resolve(import.meta.dirname, "..");

const requiredFiles = [
  "LICENSE",
  "THIRD_PARTY_NOTICES.md",
  "CHANGELOG.md",
  "sbom.cdx.json",
  "deny.toml",
  "rust-toolchain.toml",
  "Cargo.toml",
  ".docs/PRODUCT.md",
  ".docs/WORKFLOWS.md",
  ".docs/UI_DESIGN.md",
  ".docs/ARCHITECTURE.md",
  ".docs/PLATFORM_SUPPORT.md",
  ".docs/FORMAT_AND_LICENSE_MATRIX.md",
  ".docs/ERROR_CATALOG.md",
  ".docs/SECURITY.md",
  ".docs/LICENSING.md",
  ".docs/SECURITY_AND_A11Y_REVIEW.md",
  ".docs/USER_GUIDE.md",
  "crates/audora-core/Cargo.toml",
  "crates/audora-core/migrations/0001_initial.sql",
  "crates/audora-core/migrations/0002_playlists.sql",
  "crates/audora-core/migrations/0003_ui_state.sql",
  "apps/desktop/src-tauri/Cargo.toml",
  "apps/desktop/src-tauri/tauri.conf.json",
  "apps/desktop/src-tauri/Entitlements.plist",
  "apps/desktop/src-tauri/installer/macos/uninstall-cleanup.sh",
  "apps/desktop/src-tauri/installer/windows/nsis-hooks.nsh",
  "apps/desktop/src-tauri/installer/linux/postrm",
  "apps/desktop/src-tauri/capabilities/default.json",
  "assets/icons/icon-1024.svg",
  "assets/icons/icon-32.svg",
  "assets/icons/icon.png",
  "assets/icons/32x32.png",
  "assets/icons/16x16.png",
  "apps/desktop/ui/public/favicon.svg",
  "apps/desktop/ui/dist/index.html",
];

let missing = 0;
for (const rel of requiredFiles) {
  const full = join(rootDir, rel);
  if (!existsSync(full)) {
    console.error(`[FAIL] Missing required release artifact: ${rel}`);
    missing += 1;
  }
}

if (missing > 0) {
  process.exit(1);
}

// Verify Tauri CSP enforces object-src 'none' and has no wildcard connect-src
const tauriConf = JSON.parse(
  readFileSync(join(rootDir, "apps/desktop/src-tauri/tauri.conf.json"), "utf8"),
);
const csp = tauriConf?.app?.security?.csp ?? "";
if (
  !csp.includes("object-src 'none'") ||
  !csp.includes("connect-src 'self'") ||
  csp.includes("connect-src *")
) {
  console.error("[FAIL] Tauri CSP does not enforce strict connect-src/object-src policy");
  process.exit(1);
}

function sha256File(filePath) {
  const bytes = readFileSync(filePath);
  return createHash("sha256").update(bytes).digest("hex");
}

console.log("=== Audora Release Gate Verification Passed ===");
console.log("SHA-256 Checksums:");

const checksumTargets = [
  "LICENSE",
  "THIRD_PARTY_NOTICES.md",
  "sbom.cdx.json",
  "crates/audora-core/migrations/0001_initial.sql",
  "crates/audora-core/migrations/0002_playlists.sql",
  "crates/audora-core/migrations/0003_ui_state.sql",
  "apps/desktop/ui/dist/index.html",
];

const assetsDir = join(rootDir, "apps/desktop/ui/dist/assets");
if (existsSync(assetsDir)) {
  for (const name of readdirSync(assetsDir)) {
    const p = join(assetsDir, name);
    if (statSync(p).isFile()) {
      checksumTargets.push(`apps/desktop/ui/dist/assets/${name}`);
    }
  }
}

for (const rel of checksumTargets) {
  const digest = sha256File(join(rootDir, rel));
  console.log(`  ${digest}  ${rel}`);
}
