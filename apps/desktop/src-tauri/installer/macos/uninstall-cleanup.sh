#!/usr/bin/env bash
# Audora macOS uninstall cleanup script
# Removes application bundle and local application support artifacts.
set -euo pipefail

APP_BUNDLE="/Applications/Audora.app"
SUPPORT_DIR="${HOME}/Library/Application Support/com.audora.desktop"
LEGACY_SUPPORT_DIR="${HOME}/Library/Application Support/com.audora.app"
CACHE_DIR="${HOME}/Library/Caches/com.audora.desktop"
PREFS_PLIST="${HOME}/Library/Preferences/com.audora.desktop.plist"

if [[ -x "${APP_BUNDLE}/Contents/MacOS/audora" ]]; then
  "${APP_BUNDLE}/Contents/MacOS/audora" --uninstall-cleanup || true
fi

rm -rf "${SUPPORT_DIR}" "${LEGACY_SUPPORT_DIR}" "${CACHE_DIR}" "${PREFS_PLIST}"

if [[ -d "${APP_BUNDLE}" ]]; then
  rm -rf "${APP_BUNDLE}"
fi

echo "Audora and all local artifacts have been removed."
