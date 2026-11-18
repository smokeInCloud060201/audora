import React, { useEffect, useRef, useState } from "react";
import { createPortal } from "react-dom";
import { ipcClient } from "../api/ipc";
import type {
  AppSettings,
  AppUpdateStatusDto,
  DiagnosticReport,
  ThemeMode,
  TimestampPrecision,
  TrustedSourceFolder,
} from "../types/domain";
import { useModalFocusTrap } from "../utils/useModalFocusTrap";
import { AudoraLogo } from "./AudoraLogo";
import { IconCheck, IconClose, IconPlus, IconRepeat } from "./Icons";

export type SettingsTab =
  | "appearance"
  | "sources"
  | "playback"
  | "notes"
  | "keyboard"
  | "privacy"
  | "about";

interface SettingsViewProps {
  settings: AppSettings;
  onUpdateSettings: (next: AppSettings) => void;
  settingsTab: SettingsTab;
  onSelectSettingsTab: (tab: SettingsTab) => void;
  playbackSpeeds: number[];
  sourceFolders: TrustedSourceFolder[];
  isPickingSourceFolder: boolean;
  sourceFolderError: string | null;
  rescanningFolderId: string | null;
  onAddSource: () => void;
  onToggleSource: (folder: TrustedSourceFolder) => void;
  onRescanSource: (folder: TrustedSourceFolder) => void;
  onDeleteSource: (folder: TrustedSourceFolder) => void;
  backupStatusMessage: string | null;
  onSetBackupStatusMessage: (msg: string | null) => void;
  diagnosticReport: DiagnosticReport | null;
  onSetDiagnosticReport: (report: DiagnosticReport | null) => void;
  onCleanupAppData: () => void;
  currentVersion: string;
  onSetUpdateStatus: React.Dispatch<React.SetStateAction<AppUpdateStatusDto | null>>;
  onCheckForUpdates: () => void;
  onModalOpenChange?: (open: boolean) => void;
}

export const SettingsView = React.memo(function SettingsView({
  settings,
  onUpdateSettings,
  settingsTab,
  onSelectSettingsTab,
  playbackSpeeds,
  sourceFolders,
  isPickingSourceFolder,
  sourceFolderError,
  rescanningFolderId,
  onAddSource,
  onToggleSource,
  onRescanSource,
  onDeleteSource,
  backupStatusMessage,
  onSetBackupStatusMessage,
  diagnosticReport,
  onSetDiagnosticReport,
  onCleanupAppData,
  currentVersion,
  onSetUpdateStatus,
  onCheckForUpdates,
  onModalOpenChange,
}: SettingsViewProps): React.ReactElement {
  const [isCleanupConfirmOpen, setIsCleanupConfirmOpen] = useState(false);
  const [copiedDiagnosticJson, setCopiedDiagnosticJson] = useState(false);
  const copyResetTimerRef = useRef<number | null>(null);
  const cleanupDialogRef = useRef<HTMLDivElement | null>(null);
  const cancelCleanupBtnRef = useRef<HTMLButtonElement | null>(null);
  useModalFocusTrap(isCleanupConfirmOpen, cleanupDialogRef, cancelCleanupBtnRef);

  useEffect(() => {
    onModalOpenChange?.(isCleanupConfirmOpen);
    return () => {
      onModalOpenChange?.(false);
    };
  }, [isCleanupConfirmOpen, onModalOpenChange]);

  useEffect(() => {
    return () => {
      if (copyResetTimerRef.current !== null) {
        window.clearTimeout(copyResetTimerRef.current);
      }
    };
  }, []);

  useEffect(() => {
    if (!isCleanupConfirmOpen) return undefined;
    const onKeyDown = (e: KeyboardEvent) => {
      if (e.key === "Escape") {
        e.stopPropagation();
        setIsCleanupConfirmOpen(false);
      }
    };
    window.addEventListener("keydown", onKeyDown, true);
    return () => window.removeEventListener("keydown", onKeyDown, true);
  }, [isCleanupConfirmOpen]);

  const handleCopyDiagnosticJson = () => {
    if (!diagnosticReport) return;
    const formatted = JSON.stringify(diagnosticReport, null, 2);
    const markCopied = () => {
      if (copyResetTimerRef.current !== null) {
        window.clearTimeout(copyResetTimerRef.current);
      }
      setCopiedDiagnosticJson(true);
      copyResetTimerRef.current = window.setTimeout(() => {
        copyResetTimerRef.current = null;
        setCopiedDiagnosticJson(false);
      }, 2500);
    };
    if (typeof navigator !== "undefined" && navigator.clipboard?.writeText) {
      void navigator.clipboard.writeText(formatted).then(markCopied).catch(() => {
        onSetBackupStatusMessage("Unable to access system clipboard.");
      });
    } else {
      markCopied();
    }
  };
  return (
    <div className="content-page">
      <div style={{ marginBottom: 24 }}>
        <h1 style={{ margin: "0 0 4px 0", fontSize: 24 }}>Settings</h1>
        <p style={{ margin: 0, fontSize: 13, color: "var(--text-tertiary)" }}>
          Customize appearance, playback behavior, notes, and local storage
        </p>
      </div>

      <div className="settings-layout">
        <nav className="settings-subnav" aria-label="Settings categories">
          {(
            [
              ["appearance", "Appearance"],
              ["sources", "Local sources"],
              ["playback", "Playback"],
              ["notes", "Notes"],
              ["keyboard", "Keyboard"],
              ["privacy", "Privacy"],
              ["about", "About"],
            ] as const
          ).map(([id, label]) => (
            <button
              key={id}
              type="button"
              className="settings-subnav-btn"
              aria-current={settingsTab === id ? "true" : undefined}
              onClick={() => onSelectSettingsTab(id)}
            >
              <span>{label}</span>
            </button>
          ))}
        </nav>

        <div style={{ flex: 1, maxWidth: 680 }}>
          {settingsTab === "appearance" && (
            <section
              style={{
                padding: 24,
                borderRadius: 10,
                background: "var(--bg-surface-1)",
                border: "1px solid var(--border-control)",
              }}
            >
              <h2 style={{ margin: "0 0 6px 0", fontSize: 16 }}>Theme</h2>
              <p
                style={{
                  margin: "0 0 16px 0",
                  fontSize: 12,
                  color: "var(--text-tertiary)",
                }}
              >
                Choose a dark studio appearance, a daylight surface, or follow your OS
                preference
              </p>
              <div className="theme-preview-grid">
                {(["dark", "light", "system"] as ThemeMode[]).map((t) => (
                  <button
                    key={t}
                    type="button"
                    className="theme-preview-card"
                    aria-pressed={settings.theme === t}
                    onClick={() =>
                      onUpdateSettings(ipcClient.updateSettings({ ...settings, theme: t }))
                    }
                  >
                    <div
                      className="theme-mini-window"
                      style={{
                        background:
                          t === "light"
                            ? "#f6f8fa"
                            : t === "dark"
                              ? "#090b0e"
                              : "linear-gradient(90deg, #090b0e 50%, #f6f8fa 50%)",
                      }}
                    >
                      <span
                        style={{
                          width: 24,
                          height: 4,
                          borderRadius: 2,
                          background: "var(--accent-primary)",
                        }}
                      />
                      <span
                        style={{
                          width: "70%",
                          height: 6,
                          borderRadius: 3,
                          background: "var(--border-control)",
                        }}
                      />
                    </div>
                    <span style={{ fontSize: 13, fontWeight: 600 }}>
                      {t.charAt(0).toUpperCase() + t.slice(1)}
                    </span>
                  </button>
                ))}
              </div>
            </section>
          )}

          {settingsTab === "sources" && (
            <section
              data-testid="settings-sources-panel"
              style={{
                padding: 24,
                borderRadius: 10,
                background: "var(--bg-surface-1)",
                border: "1px solid var(--border-control)",
                display: "flex",
                flexDirection: "column",
                gap: 18,
              }}
            >
              <div
                style={{
                  display: "flex",
                  justifyContent: "space-between",
                  alignItems: "flex-start",
                  gap: 16,
                }}
              >
                <div>
                  <h2 style={{ margin: "0 0 6px 0", fontSize: 16 }}>
                    Local media sources
                  </h2>
                  <p
                    style={{
                      margin: 0,
                      fontSize: 12.5,
                      color: "var(--text-tertiary)",
                      lineHeight: 1.45,
                    }}
                  >
                    Add trusted local folders containing your video or audio files. Audora
                    imports media from enabled folders and lets you disable or delete any
                    source folder after importing—your files on disk are never modified.
                  </p>
                </div>
                <button
                  type="button"
                  className="btn-primary"
                  style={{ flexShrink: 0 }}
                  disabled={isPickingSourceFolder}
                  onClick={onAddSource}
                >
                  {isPickingSourceFolder ? (
                    <span>Opening disk…</span>
                  ) : (
                    <>
                      <IconPlus size={14} />
                      <span>Add a source</span>
                    </>
                  )}
                </button>
              </div>

              {sourceFolderError && (
                <div
                  role="alert"
                  style={{ fontSize: 12, color: "var(--status-error-text)" }}
                >
                  {sourceFolderError}
                </div>
              )}

              <div
                style={{
                  paddingTop: 14,
                  borderTop: "1px solid var(--border-subtle)",
                  display: "flex",
                  flexDirection: "column",
                  gap: 12,
                }}
              >
                <div
                  style={{
                    fontSize: 12,
                    fontWeight: 600,
                    color: "var(--text-secondary)",
                  }}
                >
                  Show media from ({sourceFolders.length})
                </div>

                {sourceFolders.length === 0 ? (
                  <div
                    style={{
                      padding: "22px 16px",
                      borderRadius: 8,
                      border: "1px dashed var(--border-control)",
                      background: "var(--bg-inset)",
                      textAlign: "center",
                      fontSize: 12.5,
                      color: "var(--text-tertiary)",
                    }}
                  >
                    No trusted source folders configured yet. Click{" "}
                    <strong>Add a source</strong> above to import audio and video files.
                  </div>
                ) : (
                  <div className="source-folder-list">
                    {sourceFolders.map((folder) => (
                      <div
                        key={folder.id}
                        className="source-folder-row"
                        data-testid={`source-folder-${folder.id}`}
                      >
                        <div className="source-folder-meta">
                          <div className="source-folder-title-row">
                            <span className="source-folder-title">{folder.label}</span>
                            <span className="source-folder-count tabular-nums">
                              {folder.media_count}{" "}
                              {folder.media_count === 1 ? "file" : "files"}
                            </span>
                          </div>
                          <code className="source-folder-path" title={folder.path}>
                            {folder.path}
                          </code>
                        </div>

                        <div className="source-folder-actions">
                          <button
                            type="button"
                            role="switch"
                            aria-checked={folder.enabled}
                            aria-label={`Show media from ${folder.label}`}
                            title={
                              folder.enabled
                                ? "Showing media from this folder"
                                : "Hidden from library"
                            }
                            className="source-toggle-switch"
                            onClick={() => onToggleSource(folder)}
                          >
                            <span className="source-toggle-thumb" />
                          </button>

                          <button
                            type="button"
                            className="btn-secondary"
                            style={{ height: 30, padding: "0 10px", fontSize: 12 }}
                            disabled={rescanningFolderId === folder.id}
                            aria-label={`Rescan ${folder.label}`}
                            onClick={() => onRescanSource(folder)}
                          >
                            {rescanningFolderId === folder.id ? (
                              <span>Scanning…</span>
                            ) : (
                              <>
                                <IconRepeat size={13} />
                                <span>Sync</span>
                              </>
                            )}
                          </button>

                          <button
                            type="button"
                            className="btn-danger-subtle"
                            aria-label={`Delete source folder ${folder.label}`}
                            onClick={() => onDeleteSource(folder)}
                          >
                            Delete
                          </button>
                        </div>
                      </div>
                    ))}
                  </div>
                )}
              </div>
            </section>
          )}

          {settingsTab === "playback" && (
            <section
              style={{
                padding: 24,
                borderRadius: 10,
                background: "var(--bg-surface-1)",
                border: "1px solid var(--border-control)",
                display: "flex",
                flexDirection: "column",
                gap: 18,
              }}
            >
              <h2 style={{ margin: 0, fontSize: 16 }}>Playback behavior</h2>

              <label
                style={{
                  display: "flex",
                  justifyContent: "space-between",
                  alignItems: "center",
                }}
              >
                <div>
                  <div style={{ fontSize: 13, fontWeight: 600 }}>
                    Resume where I left off
                  </div>
                  <div style={{ fontSize: 12, color: "var(--text-tertiary)" }}>
                    Remember playback position for each file in your library
                  </div>
                </div>
                <input
                  type="checkbox"
                  checked={settings.resume_where_left_off}
                  onChange={(e) =>
                    onUpdateSettings(
                      ipcClient.updateSettings({
                        ...settings,
                        resume_where_left_off: e.target.checked,
                      }),
                    )
                  }
                />
              </label>

              <label
                style={{
                  display: "flex",
                  justifyContent: "space-between",
                  alignItems: "center",
                }}
              >
                <div>
                  <div style={{ fontSize: 13, fontWeight: 600 }}>
                    Pause when headphones disconnect
                  </div>
                  <div style={{ fontSize: 12, color: "var(--text-tertiary)" }}>
                    Automatically pause playback if audio output changes
                  </div>
                </div>
                <input
                  type="checkbox"
                  checked={settings.pause_on_headphones_disconnect}
                  onChange={(e) =>
                    onUpdateSettings(
                      ipcClient.updateSettings({
                        ...settings,
                        pause_on_headphones_disconnect: e.target.checked,
                      }),
                    )
                  }
                />
              </label>

              <div
                style={{
                  display: "flex",
                  justifyContent: "space-between",
                  alignItems: "center",
                }}
              >
                <div>
                  <div style={{ fontSize: 13, fontWeight: 600 }}>
                    Default playback speed
                  </div>
                  <div style={{ fontSize: 12, color: "var(--text-tertiary)" }}>
                    Applied when opening a new media file
                  </div>
                </div>
                <select
                  aria-label="Default playback speed"
                  value={settings.default_playback_speed}
                  onChange={(e) =>
                    onUpdateSettings(
                      ipcClient.updateSettings({
                        ...settings,
                        default_playback_speed: Number(e.target.value),
                      }),
                    )
                  }
                  style={{
                    padding: "6px 10px",
                    borderRadius: 8,
                    border: "1px solid var(--border-control)",
                    background: "var(--bg-surface-2)",
                    color: "var(--text-primary)",
                  }}
                >
                  {playbackSpeeds.map((s) => (
                    <option key={s} value={s}>
                      {s}×
                    </option>
                  ))}
                </select>
              </div>
            </section>
          )}

          {settingsTab === "notes" && (
            <section
              style={{
                padding: 24,
                borderRadius: 10,
                background: "var(--bg-surface-1)",
                border: "1px solid var(--border-control)",
              }}
            >
              <h2 style={{ margin: "0 0 14px 0", fontSize: 16 }}>Timestamped notes</h2>
              <div
                style={{
                  display: "flex",
                  justifyContent: "space-between",
                  alignItems: "center",
                }}
              >
                <div>
                  <div style={{ fontSize: 13, fontWeight: 600 }}>
                    Note timestamp precision
                  </div>
                  <div style={{ fontSize: 12, color: "var(--text-tertiary)" }}>
                    Format used when pressing N to add a note
                  </div>
                </div>
                <div style={{ display: "flex", gap: 8 }}>
                  {(
                    [
                      ["nearest_second", "Nearest second"],
                      ["milliseconds", "Milliseconds"],
                    ] as [TimestampPrecision, string][]
                  ).map(([val, label]) => (
                    <button
                      key={val}
                      type="button"
                      className="btn-secondary"
                      aria-pressed={settings.timestamp_precision === val}
                      onClick={() =>
                        onUpdateSettings(
                          ipcClient.updateSettings({
                            ...settings,
                            timestamp_precision: val,
                          }),
                        )
                      }
                    >
                      {label}
                    </button>
                  ))}
                </div>
              </div>
            </section>
          )}

          {settingsTab === "keyboard" && (
            <section
              style={{
                padding: 24,
                borderRadius: 10,
                background: "var(--bg-surface-1)",
                border: "1px solid var(--border-control)",
              }}
            >
              <h2 style={{ margin: "0 0 14px 0", fontSize: 16 }}>Keyboard shortcuts</h2>
              <div style={{ display: "flex", flexDirection: "column", gap: 10 }}>
                {[
                  ["Play / Pause", "Space"],
                  ["Seek backward / forward 5s", "← / →"],
                  ["Seek backward / forward 15s", "Shift + ← / →"],
                  ["Adjust volume ±5%", "↑ / ↓"],
                  ["Add note at current timestamp", "N"],
                  ["Drop quick marker", "Shift + N"],
                  ["Save note in composer", "⌘/Ctrl + Enter"],
                  ["Search library & notes", "⌘/Ctrl + K"],
                  ["Open media file", "⌘/Ctrl + O"],
                  ["Toggle Playlists bar", "["],
                  ["Toggle Notes & Queue bar", "]"],
                  ["Toggle Minimum / Full mode", "M"],
                ].map(([actionLabel, shortcut]) => (
                  <div
                    key={actionLabel}
                    style={{
                      display: "flex",
                      justifyContent: "space-between",
                      padding: "8px 0",
                      borderBottom: "1px solid var(--border-subtle)",
                      fontSize: 13,
                    }}
                  >
                    <span>{actionLabel}</span>
                    <code
                      style={{
                        padding: "2px 8px",
                        borderRadius: 6,
                        background: "var(--bg-surface-2)",
                        color: "var(--accent-text)",
                      }}
                    >
                      {shortcut}
                    </code>
                  </div>
                ))}
              </div>
            </section>
          )}

          {settingsTab === "privacy" && (
            <section
              style={{
                padding: 24,
                borderRadius: 10,
                background: "var(--bg-surface-1)",
                border: "1px solid var(--border-control)",
                display: "flex",
                flexDirection: "column",
                gap: 12,
              }}
            >
              <h2 style={{ margin: 0, fontSize: 16 }}>Local-first privacy &amp; storage</h2>
              <p
                style={{
                  margin: 0,
                  fontSize: 13,
                  color: "var(--text-secondary)",
                }}
              >
                Audora stores your library, playlists, and notes locally in a SQLite WAL
                database with automatic pre-migration backups (`.pre-vN.bak`). Audora sends
                zero telemetry or analytics; network requests occur only when checking for or
                downloading GitHub release updates (configurable in About), and your media
                files on disk are never modified.
              </p>
              <div
                style={{
                  padding: 12,
                  borderRadius: 8,
                  background: "var(--bg-inset)",
                  border: "1px solid var(--border-control)",
                  fontSize: 12,
                }}
              >
                <div style={{ color: "var(--text-tertiary)", marginBottom: 4 }}>
                  Local database path
                </div>
                <code>
                  {ipcClient.getAppDataDirectoryDisplay()}
                  {ipcClient.getAppDataDirectoryDisplay().includes("\\") ? "\\" : "/"}
                  audora.sqlite
                </code>
              </div>

              <div style={{ display: "flex", flexWrap: "wrap", gap: 10, marginTop: 4 }}>
                <button
                  type="button"
                  className="btn-secondary"
                  onClick={() => {
                    void ipcClient.createManualBackupNative().then((backupPath) => {
                      onSetBackupStatusMessage(`Backup snapshot saved to ${backupPath}`);
                    });
                  }}
                >
                  Create backup snapshot
                </button>
                <button
                  type="button"
                  className="btn-secondary"
                  onClick={() => {
                    void ipcClient
                      .generateDiagnosticReportNative()
                      .then((rep) => onSetDiagnosticReport(rep));
                  }}
                >
                  Preview redacted diagnostic report
                </button>
                <button
                  type="button"
                  className="btn-danger-subtle"
                  style={{ height: 34, padding: "0 12px" }}
                  onClick={() => setIsCleanupConfirmOpen(true)}
                >
                  Clean up app data &amp; cache
                </button>
              </div>

              {backupStatusMessage && (
                <div
                  role="status"
                  style={{
                    display: "inline-flex",
                    alignItems: "center",
                    gap: 6,
                    fontSize: 12,
                    color: "var(--accent-text)",
                  }}
                >
                  <IconCheck size={13} />
                  <span>{backupStatusMessage}</span>
                </div>
              )}

              {diagnosticReport && (
                <div style={{ display: "flex", flexDirection: "column", gap: 8 }}>
                  <div
                    style={{
                      display: "flex",
                      justifyContent: "space-between",
                      alignItems: "center",
                      gap: 10,
                    }}
                  >
                    <span style={{ fontSize: 12, fontWeight: 600, color: "var(--text-secondary)" }}>
                      Redacted diagnostic report
                    </span>
                    <button
                      type="button"
                      className="btn-secondary"
                      style={{ height: 28, padding: "0 10px", fontSize: 11.5 }}
                      onClick={handleCopyDiagnosticJson}
                    >
                      {copiedDiagnosticJson ? "Copied!" : "Copy JSON"}
                    </button>
                  </div>
                  <pre
                    data-testid="diagnostic-report-preview"
                    style={{
                      margin: 0,
                      padding: 12,
                      borderRadius: 8,
                      background: "var(--bg-inset)",
                      border: "1px solid var(--border-control)",
                      fontSize: 11,
                      color: "var(--text-secondary)",
                      overflowX: "auto",
                    }}
                  >
                    {JSON.stringify(diagnosticReport, null, 2)}
                  </pre>
                </div>
              )}
            </section>
          )}

          {settingsTab === "about" && (
            <section
              style={{
                padding: 24,
                borderRadius: 10,
                background: "var(--bg-surface-1)",
                border: "1px solid var(--border-control)",
                display: "flex",
                flexDirection: "column",
                gap: 16,
              }}
            >
              <div
                style={{
                  display: "flex",
                  alignItems: "center",
                  justifyContent: "space-between",
                  gap: 14,
                  flexWrap: "wrap",
                }}
              >
                <div style={{ display: "flex", alignItems: "center", gap: 14 }}>
                  <AudoraLogo variant="icon" size={48} />
                  <div>
                    <h2 style={{ margin: "0 0 4px 0", fontSize: 16 }}>
                      Audora v{currentVersion}
                    </h2>
                    <p
                      style={{
                        margin: 0,
                        fontSize: 13,
                        color: "var(--text-secondary)",
                      }}
                    >
                      Cross-platform local-first audio and video player with timestamped notes.
                    </p>
                  </div>
                </div>
                <button
                  type="button"
                  className="btn-secondary"
                  onClick={onCheckForUpdates}
                >
                  Check for updates…
                </button>
              </div>

              <div
                style={{
                  paddingTop: 14,
                  borderTop: "1px solid var(--border-subtle)",
                  display: "flex",
                  flexDirection: "column",
                  gap: 14,
                }}
              >
                <label
                  style={{
                    display: "flex",
                    justifyContent: "space-between",
                    alignItems: "center",
                    gap: 12,
                  }}
                >
                  <div>
                    <div style={{ fontSize: 13, fontWeight: 600 }}>
                      Automatically check for updates
                    </div>
                    <div style={{ fontSize: 12, color: "var(--text-tertiary)" }}>
                      Notify in the sidebar when a verified new Audora release is available
                    </div>
                  </div>
                  <input
                    type="checkbox"
                    aria-label="Automatically check for updates"
                    checked={settings.auto_check_updates !== false}
                    onChange={(e) => {
                      const checked = e.target.checked;
                      const next = ipcClient.updateSettings({
                        ...settings,
                        auto_check_updates: checked,
                      });
                      onUpdateSettings(next);
                      if (!checked) {
                        onSetUpdateStatus((prev) =>
                          prev ? { ...prev, update_available: false } : null,
                        );
                      } else {
                        void ipcClient
                          .checkForUpdatesOnline(false)
                          .then(onSetUpdateStatus);
                      }
                    }}
                  />
                </label>
              </div>

              <div
                style={{
                  paddingTop: 12,
                  borderTop: "1px solid var(--border-subtle)",
                  fontSize: 12,
                  color: "var(--text-tertiary)",
                }}
              >
                Licensed under Apache License 2.0 · Native OS WebView media playback &amp;
                pure-Rust container prober (optional GStreamer LGPL-2.1+ CLI discovery on
                Linux)
              </div>
            </section>
          )}
        </div>
      </div>

      {isCleanupConfirmOpen &&
        typeof document !== "undefined" &&
        createPortal(
          <div
            className="modal-backdrop"
            role="dialog"
            aria-modal="true"
            aria-label="Confirm clean up app data and cache"
            data-testid="confirm-cleanup-appdata-modal"
          >
            <div ref={cleanupDialogRef} className="modal-dialog">
              <div
                style={{
                  display: "flex",
                  justifyContent: "space-between",
                  alignItems: "center",
                }}
              >
                <h2 style={{ margin: 0, fontSize: 17 }}>
                  Clean up app data &amp; cache?
                </h2>
                <button
                  type="button"
                  className="btn-secondary"
                  style={{ width: 30, height: 30, padding: 0 }}
                  aria-label="Close confirmation dialog"
                  onClick={() => setIsCleanupConfirmOpen(false)}
                >
                  <IconClose size={14} />
                </button>
              </div>
              <p style={{ margin: 0, fontSize: 13, color: "var(--text-secondary)" }}>
                This will permanently clear your Audora library records, playlists,
                timestamped notes, update cache, and local SQLite backups. Your original
                audio and video files on disk stay completely untouched.
              </p>
              <div style={{ display: "flex", justifyContent: "flex-end", gap: 10 }}>
                <button
                  ref={cancelCleanupBtnRef}
                  type="button"
                  className="btn-secondary"
                  onClick={() => setIsCleanupConfirmOpen(false)}
                >
                  Cancel
                </button>
                <button
                  type="button"
                  className="btn-danger-subtle"
                  style={{ height: 34, padding: "0 14px" }}
                  onClick={() => {
                    setIsCleanupConfirmOpen(false);
                    onCleanupAppData();
                  }}
                >
                  Clean up app data
                </button>
              </div>
            </div>
          </div>,
          document.body,
        )}
    </div>
  );
});
