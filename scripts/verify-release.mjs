#!/usr/bin/env node
/**
 * Audora Release Gate Verifier (`pnpm verify:release`).
 *
 * Verifies that all required legal, licensing, security, and build artifacts exist,
 * enforces manifest & SBOM version synchronization, validates runtime & bundle CSP,
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
  "crates/audora-core/migrations/0004_relational_unification.sql",
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
  "assets/icons/icon.ico",
  "assets/icons/32x32.png",
  "assets/icons/32x32.rgba",
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

// Enforce version parity across Cargo.toml, package.json, apps/desktop/ui/package.json, tauri.conf.json, and sbom.cdx.json (`M-8`)
const cargoToml = readFileSync(join(rootDir, "Cargo.toml"), "utf8");
const cargoVersionMatch = cargoToml.match(/\[workspace\.package\][\s\S]*?^\s*version\s*=\s*"([^"]+)"/m);
const cargoVersion = cargoVersionMatch?.[1] ?? "";
const rootPkg = JSON.parse(readFileSync(join(rootDir, "package.json"), "utf8"));
const uiPkg = JSON.parse(readFileSync(join(rootDir, "apps/desktop/ui/package.json"), "utf8"));
const tauriConf = JSON.parse(
  readFileSync(join(rootDir, "apps/desktop/src-tauri/tauri.conf.json"), "utf8"),
);
const sbom = JSON.parse(readFileSync(join(rootDir, "sbom.cdx.json"), "utf8"));
const sbomVersion = sbom?.metadata?.component?.version ?? "";

const versions = {
  "Cargo.toml": cargoVersion,
  "package.json": rootPkg.version ?? "",
  "apps/desktop/ui/package.json": uiPkg.version ?? "",
  "apps/desktop/src-tauri/tauri.conf.json": tauriConf.version ?? "",
  "sbom.cdx.json": sbomVersion,
};

const uniqueVersions = new Set(Object.values(versions));
if (!cargoVersion || uniqueVersions.size !== 1) {
  console.error("[FAIL] Version mismatch across workspace manifests:", versions);
  process.exit(1);
}
if (!Array.isArray(sbom.components) || sbom.components.length < 50) {
  console.error("[FAIL] sbom.cdx.json is incomplete; expected full resolved dependency list");
  process.exit(1);
}

// Verify every non-workspace Cargo.lock package is represented in sbom.cdx.json (`R2-M12`)
const cargoLock = readFileSync(join(rootDir, "Cargo.lock"), "utf8");
const sbomCargoKeys = new Set(
  sbom.components
    .filter((c) => typeof c.purl === "string" && c.purl.startsWith("pkg:cargo/"))
    .map((c) => `${c.name}@${c.version}`),
);
const missingSbomCargo = [];
for (const m of cargoLock.matchAll(/\[\[package\]\]\nname = "([^"]+)"\nversion = "([^"]+)"/g)) {
  const pkgName = m[1];
  const pkgVer = m[2];
  if (pkgName === "audora-core" || pkgName === "audora-desktop") continue;
  const key = `${pkgName}@${pkgVer}`;
  if (!sbomCargoKeys.has(key)) {
    missingSbomCargo.push(key);
  }
}
if (missingSbomCargo.length > 0) {
  console.error("[FAIL] Cargo.lock packages missing from sbom.cdx.json:", missingSbomCargo);
  process.exit(1);
}

// Verify CSP in BOTH tauri.conf.json AND `RUNTIME_CSP` in main.rs (`M-8`)
const csp = tauriConf?.app?.security?.csp ?? "";
const mainRs = readFileSync(
  join(rootDir, "apps/desktop/src-tauri/src/main.rs"),
  "utf8",
);
const runtimeCspMatch = mainRs.match(/const\s+RUNTIME_CSP:\s*&str\s*=\s*"([^"]+)"/);
const runtimeCsp = runtimeCspMatch?.[1] ?? "";

const hasWildcardDirective = (policy) => {
  for (const dir of ["default-src", "script-src", "connect-src", "img-src", "media-src"]) {
    const match = policy.match(new RegExp(`${dir}\\s+([^;]+)`, "i"));
    if (match && match[1].split(/\s+/).some((tok) => tok === "*" || tok.includes("*"))) {
      return true;
    }
  }
  const connectMatch = policy.match(/connect-src\s+([^;]+)/i);
  return !connectMatch;
};

if (
  !csp.includes("object-src 'none'") ||
  !csp.includes("connect-src 'self'") ||
  hasWildcardDirective(csp)
) {
  console.error("[FAIL] Tauri CSP does not enforce strict non-wildcard connect-src/object-src policy");
  process.exit(1);
}

if (
  !runtimeCsp ||
  !runtimeCsp.includes("default-src 'self'") ||
  !runtimeCsp.includes("script-src 'self'") ||
  !runtimeCsp.includes("object-src 'none'") ||
  !runtimeCsp.includes("frame-ancestors 'none'") ||
  hasWildcardDirective(runtimeCsp) ||
  !mainRs.includes("Content-Security-Policy: {RUNTIME_CSP}") ||
  !/x-audora-token/i.test(mainRs)
) {
  console.error(
    "[FAIL] Runtime loopback server in main.rs is missing strict RUNTIME_CSP or session token enforcement",
  );
  process.exit(1);
}

function sha256File(filePath) {
  const bytes = readFileSync(filePath);
  return createHash("sha256").update(bytes).digest("hex");
}

console.log(`=== Audora v${cargoVersion} Release Gate Verification Passed ===`);
console.log("SHA-256 Checksums:");

const checksumTargets = [
  "LICENSE",
  "THIRD_PARTY_NOTICES.md",
  "sbom.cdx.json",
  "crates/audora-core/migrations/0001_initial.sql",
  "crates/audora-core/migrations/0002_playlists.sql",
  "crates/audora-core/migrations/0003_ui_state.sql",
  "crates/audora-core/migrations/0004_relational_unification.sql",
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
