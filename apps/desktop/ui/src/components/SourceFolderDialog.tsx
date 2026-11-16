import React, { useEffect, useState } from "react";
import { ipcClient } from "../api/ipc";
import type {
  AppErrorPayload,
  LocalDirectoryListing,
  SourceFolderPreview,
  TrustedSourceFolder,
} from "../types/domain";
import { AudoraLogo } from "./AudoraLogo";

interface SourceFolderDialogProps {
  onAdded: (folder: TrustedSourceFolder, importedCount: number) => void;
  onClose: () => void;
}

export function SourceFolderDialog({
  onAdded,
  onClose,
}: SourceFolderDialogProps): React.ReactElement {
  const [listing, setListing] = useState<LocalDirectoryListing | null>(null);
  const [preview, setPreview] = useState<SourceFolderPreview | null>(null);
  const [filterText, setFilterText] = useState<string>("");
  const [isLoading, setIsLoading] = useState<boolean>(true);
  const [isScanning, setIsScanning] = useState<boolean>(false);
  const [errorMessage, setErrorMessage] = useState<string | null>(null);

  const loadDirectory = async (targetPath?: string) => {
    setIsLoading(true);
    setErrorMessage(null);
    setFilterText("");
    try {
      const next = await ipcClient.browseLocalDirectories(targetPath);
      setListing(next);
    } catch (err) {
      const payload = err as AppErrorPayload;
      setErrorMessage(payload.message ?? "Unable to read local directories.");
    } finally {
      setIsLoading(false);
    }
  };

  useEffect(() => {
    void loadDirectory();
  }, []);

  const handleSelectForPreview = async () => {
    if (!listing) return;
    setIsScanning(true);
    setErrorMessage(null);
    try {
      const result = await ipcClient.previewSourceFolder(listing.current_path);
      setPreview(result);
    } catch (err) {
      const payload = err as AppErrorPayload;
      setErrorMessage(payload.message ?? "Unable to scan selected folder.");
    } finally {
      setIsScanning(false);
    }
  };

  const handleConfirmImport = () => {
    if (!preview) return;
    try {
      const res = ipcClient.confirmSourceFolderImport(preview);
      onAdded(res.folder, res.importedCount);
    } catch (err) {
      const payload = err as AppErrorPayload;
      setErrorMessage(payload.message ?? "Unable to import source folder.");
    }
  };

  // Step 2: Custom In-App Trust & Import Confirmation Popup (replaces native browser alert)
  if (preview) {
    const totalCount = preview.files.length;
    const breakdownParts: string[] = [];
    if (preview.video_count > 0) {
      breakdownParts.push(`${preview.video_count} video`);
    }
    if (preview.audio_count > 0) {
      breakdownParts.push(`${preview.audio_count} audio`);
    }
    const breakdownText =
      totalCount > 0
        ? `${totalCount} ${totalCount === 1 ? "media file" : "media files"} found (${breakdownParts.join(" · ")})`
        : "No media files in this folder yet — future audio or video files will sync automatically";

    return (
      <div
        className="modal-backdrop"
        role="dialog"
        aria-modal="true"
        aria-label="Confirm local media source"
        data-testid="source-confirm-dialog"
      >
        <div className="update-dialog-card">
          <button
            type="button"
            className="btn-secondary update-dialog-close"
            onClick={onClose}
            aria-label="Close source confirmation dialog"
          >
            ✕
          </button>

          <div className="update-app-mark-well" aria-hidden="true">
            <AudoraLogo variant="icon" size={52} />
          </div>

          <p className="update-eyebrow">TRUSTED LOCAL SOURCE</p>
          <h2 className="update-dialog-title">
            Allow Audora to read “{preview.folder_name}”?
          </h2>
          <p className="update-dialog-subtitle">
            Audora will index supported audio and video files in this folder for your local
            library. Your files stay on this device, are never uploaded anywhere, and you can
            delete this source anytime in Settings.
          </p>

          <div className="update-version-status-box">
            <div className="update-version-info" style={{ minWidth: 0 }}>
              <span className="update-version-number">{breakdownText}</span>
              <code
                className="source-folder-path"
                style={{ fontSize: 11 }}
                title={preview.folder_path}
              >
                {preview.folder_path}
              </code>
            </div>
            <span className="update-verified-mark" aria-hidden="true">
              ✓
            </span>
          </div>

          {errorMessage && (
            <div role="alert" style={{ fontSize: 12, color: "var(--status-error-text)" }}>
              {errorMessage}
            </div>
          )}

          <div className="update-dialog-actions">
            <button
              type="button"
              className="btn-secondary"
              onClick={() => setPreview(null)}
            >
              ← Choose another
            </button>
            <button
              type="button"
              className="btn-primary"
              onClick={handleConfirmImport}
            >
              Allow &amp; import{totalCount > 0 ? ` (${totalCount})` : ""}
            </button>
          </div>
        </div>
      </div>
    );
  }

  // Step 1: Custom In-App Local Folder Browser
  const filteredDirs = (listing?.directories ?? []).filter((d) =>
    filterText.trim()
      ? d.name.toLowerCase().includes(filterText.trim().toLowerCase())
      : true,
  );

  return (
    <div
      className="modal-backdrop"
      role="dialog"
      aria-modal="true"
      aria-label="Choose local source folder"
      data-testid="source-browser-dialog"
    >
      <div className="source-picker-dialog">
        <div className="source-picker-header">
          <div style={{ display: "flex", alignItems: "center", gap: 10 }}>
            <AudoraLogo variant="mark" size={28} />
            <div>
              <h2 style={{ margin: 0, fontSize: 16 }}>Add local media source</h2>
              <p style={{ margin: "2px 0 0 0", fontSize: 12, color: "var(--text-tertiary)" }}>
                Select a folder on your device containing video or audio files
              </p>
            </div>
          </div>
          <button
            type="button"
            className="btn-secondary"
            style={{ width: 30, height: 30, padding: 0 }}
            aria-label="Close folder browser"
            onClick={onClose}
          >
            ✕
          </button>
        </div>

        {listing && listing.roots.length > 0 && (
          <div className="source-picker-roots" aria-label="Quick locations">
            {listing.roots.map((root) => {
              const isActive = listing.current_path === root.path;
              return (
                <button
                  key={root.path}
                  type="button"
                  className="filter-pill"
                  aria-pressed={isActive}
                  onClick={() => void loadDirectory(root.path)}
                >
                  {root.label}
                </button>
              );
            })}
          </div>
        )}

        <div className="source-picker-pathbar">
          <button
            type="button"
            className="btn-secondary"
            style={{ height: 30, padding: "0 10px", fontSize: 12, flexShrink: 0 }}
            disabled={!listing?.parent_path || isLoading}
            onClick={() => {
              if (listing?.parent_path) {
                void loadDirectory(listing.parent_path);
              }
            }}
          >
            ↑ Up
          </button>
          <code className="source-picker-current-path" title={listing?.current_path ?? ""}>
            {listing?.current_path ?? "Loading…"}
          </code>
          {listing && (
            <span className="source-folder-count tabular-nums">
              {listing.direct_media_count}{" "}
              {listing.direct_media_count === 1 ? "media file" : "media files"} here
            </span>
          )}
        </div>

        {listing && listing.directories.length > 6 && (
          <input
            type="search"
            aria-label="Filter subfolders"
            placeholder="Filter subfolders…"
            value={filterText}
            onChange={(e) => setFilterText(e.target.value)}
            className="source-picker-filter"
          />
        )}

        <div className="source-picker-dir-list" role="listbox" aria-label="Subfolders">
          {isLoading ? (
            <div className="source-picker-empty">Reading local folders…</div>
          ) : errorMessage ? (
            <div className="source-picker-empty" style={{ color: "var(--status-error-text)" }}>
              {errorMessage}
            </div>
          ) : filteredDirs.length === 0 ? (
            <div className="source-picker-empty">
              {listing?.directories.length === 0
                ? "No subfolders inside this folder. Click Continue below to select this folder."
                : "No subfolders match your filter."}
            </div>
          ) : (
            filteredDirs.map((dir) => (
              <button
                key={dir.path}
                type="button"
                className="source-picker-dir-item"
                onClick={() => void loadDirectory(dir.path)}
              >
                <span className="source-picker-dir-icon" aria-hidden="true">
                  ▣
                </span>
                <span className="source-picker-dir-name">{dir.name}</span>
                <span className="source-picker-dir-chevron" aria-hidden="true">
                  ›
                </span>
              </button>
            ))
          )}
        </div>

        <div className="source-picker-footer">
          <div style={{ minWidth: 0, fontSize: 12, color: "var(--text-tertiary)" }}>
            Selected:{" "}
            <strong style={{ color: "var(--text-primary)" }}>
              {listing?.folder_name ?? "—"}
            </strong>
          </div>
          <div style={{ display: "flex", gap: 10, flexShrink: 0 }}>
            <button type="button" className="btn-secondary" onClick={onClose}>
              Cancel
            </button>
            <button
              type="button"
              className="btn-primary"
              disabled={!listing || isLoading || isScanning}
              onClick={() => void handleSelectForPreview()}
            >
              {isScanning ? "Scanning…" : "Continue →"}
            </button>
          </div>
        </div>
      </div>
    </div>
  );
}
