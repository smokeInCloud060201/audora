#!/usr/bin/env bash
# Audora macOS uninstall cleanup script
# Removes application bundle and local application support artifacts.
set -euo pipefail

APP_BUNDLE="/Applications/Audora.app"
USER_APP_BUNDLE="${HOME}/Applications/Audora.app"
SUPPORT_DIR="${HOME}/Library/Application Support/com.audora.desktop"
LEGACY_SUPPORT_DIR="${HOME}/Library/Application Support/com.audora.app"
CACHE_DIR="${HOME}/Library/Caches/com.audora.desktop"
WEBKIT_DIR="${HOME}/Library/WebKit/com.audora.desktop"
SAVED_STATE_DIR="${HOME}/Library/Saved Application State/com.audora.desktop.savedState"
PREFS_PLIST="${HOME}/Library/Preferences/com.audora.desktop.plist"

if [[ -x "${APP_BUNDLE}/Contents/MacOS/audora" ]]; then
  "${APP_BUNDLE}/Contents/MacOS/audora" --uninstall-cleanup || true
elif [[ -x "${USER_APP_BUNDLE}/Contents/MacOS/audora" ]]; then
  "${USER_APP_BUNDLE}/Contents/MacOS/audora" --uninstall-cleanup || true
fi

rm -rf "${SUPPORT_DIR}" "${LEGACY_SUPPORT_DIR}" "${CACHE_DIR}" "${WEBKIT_DIR}" "${SAVED_STATE_DIR}" "${PREFS_PLIST}"

if [[ -d "${APP_BUNDLE}" ]]; then
  rm -rf "${APP_BUNDLE}"
fi
if [[ -d "${USER_APP_BUNDLE}" ]]; then
  rm -rf "${USER_APP_BUNDLE}"
fi

echo "Audora and all local artifacts have been removed."
