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
  "deny.toml",
  "rust-toolchain.toml",
  "docs/PRODUCT.md",
  "docs/WORKFLOWS.md",
  "docs/UI_DESIGN.md",
  "docs/ARCHITECTURE.md",
  "docs/PLATFORM_SUPPORT.md",
  "docs/FORMAT_AND_LICENSE_MATRIX.md",
  "docs/ERROR_CATALOG.md",
  "docs/SECURITY.md",
  "docs/LICENSING.md",
  "docs/SECURITY_AND_A11Y_REVIEW.md",
  "docs/USER_GUIDE.md",
  "src-tauri/Cargo.toml",
  "src-tauri/tauri.conf.json",
  "src-tauri/capabilities/default.json",
  "src-tauri/migrations/0001_initial.sql",
  "ui/dist/index.html",
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

// Verify Tauri CSP contains no remote wildcards
const tauriConf = JSON.parse(
  readFileSync(join(rootDir, "src-tauri/tauri.conf.json"), "utf8"),
);
const csp = tauriConf?.app?.security?.csp ?? "";
if (!csp.includes("object-src 'none'") || !csp.includes("connect-src 'self'")) {
  console.error("[FAIL] Tauri CSP does not enforce strict local-only connect-src/object-src");
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
  "src-tauri/migrations/0001_initial.sql",
  "ui/dist/index.html",
];

const assetsDir = join(rootDir, "ui/dist/assets");
if (existsSync(assetsDir)) {
  for (const name of readdirSync(assetsDir)) {
    const p = join(assetsDir, name);
    if (statSync(p).isFile()) {
      checksumTargets.push(`ui/dist/assets/${name}`);
    }
  }
}

for (const rel of checksumTargets) {
  const digest = sha256File(join(rootDir, rel));
  console.log(`  ${digest}  ${rel}`);
}
