import React, { useEffect, useRef, useState } from "react";
import type { AppUpdateStatusDto } from "../types/domain";
import { useModalFocusTrap } from "../utils/useModalFocusTrap";
import { AudoraLogo } from "./AudoraLogo";
import { IconCheck, IconClose } from "./Icons";

interface UpdateDialogProps {
  updateStatus: AppUpdateStatusDto;
  onConfirmInstall: (version: string) => void;
  onDismiss: (version: string) => void;
  onCheckAgain: () => void;
  onClose: () => void;
}

function formatPackageMeta(status: AppUpdateStatusDto): string {
  if (!status.update_available) {
    return status.staged_for_restart
      ? "Staged to apply on next application restart"
      : "Installed and verified";
  }
  const sizeBytes = status.package_size_bytes ?? 0;
  const mb = sizeBytes > 0 ? Math.max(1, Math.round(sizeBytes / (1024 * 1024))) : null;
  const verificationLabel = status.verified
    ? "Downloaded and verified"
    : "Pending checksum verification";
  return mb !== null ? `${mb} MB · ${verificationLabel}` : verificationLabel;
}

function renderInlineMarkdown(text: string): React.ReactNode[] {
  const parts = text.split(/(\*\*[^*]+\*\*|`[^`]+`)/g);
  return parts.map((part, idx) => {
    if (part.startsWith("**") && part.endsWith("**") && part.length > 4) {
      return <strong key={idx}>{part.slice(2, -2)}</strong>;
    }
    if (part.startsWith("`") && part.endsWith("`") && part.length > 2) {
      return (
        <code key={idx} className="update-inline-code">
          {part.slice(1, -1)}
        </code>
      );
    }
    return <React.Fragment key={idx}>{part}</React.Fragment>;
  });
}

function isStructuredChangelog(notes: string): boolean {
  return (
    notes.includes("\n") ||
    /^(#{1,4}\s|[-*•]\s)/m.test(notes) ||
    notes.length > 160
  );
}

export function UpdateDialog({
  updateStatus,
  onConfirmInstall,
  onDismiss,
  onCheckAgain,
  onClose,
}: UpdateDialogProps): React.ReactElement {
  const [isInstalling, setIsInstalling] = useState(false);
  const dialogRef = useRef<HTMLDivElement | null>(null);
  useModalFocusTrap(true, dialogRef);

  useEffect(() => {
    setIsInstalling(false);
  }, [updateStatus.checked_at_unix_ms, updateStatus.latest_version]);
  const targetVersion =
    updateStatus.latest_version || updateStatus.current_version;
  const isAvailable = updateStatus.update_available;
  const titleText = isAvailable
    ? updateStatus.release_title || `Audora ${targetVersion} is ready`
    : `Audora ${updateStatus.current_version} is up to date`;
  const rawNotes = isAvailable
    ? updateStatus.release_notes ||
      `Version ${targetVersion} has been verified and is ready to install.`
    : `You are running the latest verified build of Audora (${updateStatus.current_version}).`;
  const hasChangelogBlocks = isAvailable && isStructuredChangelog(rawNotes);
  const subtitleText = hasChangelogBlocks
    ? `Version ${targetVersion} has been verified and is ready to install.`
    : rawNotes;
  const metaLabel = formatPackageMeta(updateStatus);

  const changelogLines = hasChangelogBlocks
    ? rawNotes
        .split(/\r?\n/)
        .map((line) => line.trim())
        .filter((line) => line.length > 0)
    : [];

  return (
    <div
      className="modal-backdrop"
      role="dialog"
      aria-modal="true"
      aria-label="Update Available Dialog"
      data-testid="update-dialog"
    >
      <div
        ref={dialogRef}
        className={`update-dialog-card ${
          hasChangelogBlocks ? "has-changelog" : ""
        }`.trim()}
      >
        <button
          type="button"
          className="btn-secondary update-dialog-close"
          onClick={onClose}
          aria-label="Close update dialog"
        >
          <IconClose size={14} />
        </button>

        <div className="update-app-mark-well" aria-hidden="true">
          <AudoraLogo variant="icon" size={56} />
        </div>

        <p className="update-eyebrow">
          {isAvailable ? "UPDATE AVAILABLE" : "SOFTWARE UPDATE"}
        </p>
        <h2 className="update-dialog-title">{titleText}</h2>
        <p className="update-dialog-subtitle">{subtitleText}</p>

        {hasChangelogBlocks && (
          <div className="update-changelog-box" aria-label="Release Notes">
            {changelogLines.map((line, index) => {
              if (/^#{1,3}\s+/.test(line)) {
                const heading = line.replace(/^#{1,3}\s+/, "");
                return (
                  <h3 key={index} className="update-changelog-heading">
                    {renderInlineMarkdown(heading)}
                  </h3>
                );
              }
              if (/^[-*•]\s+/.test(line)) {
                const item = line.replace(/^[-*•]\s+/, "");
                return (
                  <div key={index} className="update-changelog-item">
                    <span className="update-changelog-bullet" aria-hidden="true">
                      •
                    </span>
                    <span>{renderInlineMarkdown(item)}</span>
                  </div>
                );
              }
              return (
                <p key={index} className="update-changelog-paragraph">
                  {renderInlineMarkdown(line)}
                </p>
              );
            })}
          </div>
        )}

        <div className="update-version-status-box">
          <div className="update-version-info">
            <span className="update-version-number">
              Version {targetVersion}
            </span>
            <span className="update-version-meta">{metaLabel}</span>
          </div>
          {updateStatus.verified && (
            <span className="update-verified-mark" aria-label="Verified">
              <IconCheck size={13} />
            </span>
          )}
        </div>

        <div className="update-dialog-actions">
          {isAvailable ? (
            <>
              <button
                type="button"
                className="btn-secondary"
                disabled={isInstalling}
                onClick={() => onDismiss(targetVersion)}
              >
                Later
              </button>
              <button
                type="button"
                className="btn-primary"
                disabled={isInstalling}
                onClick={() => {
                  setIsInstalling(true);
                  onConfirmInstall(targetVersion);
                }}
              >
                {isInstalling
                  ? "Downloading & installing…"
                  : "Restart and install"}
              </button>
            </>
          ) : (
            <>
              <button
                type="button"
                className="btn-secondary"
                onClick={onCheckAgain}
              >
                Check again
              </button>
              <button
                type="button"
                className="btn-primary"
                onClick={onClose}
              >
                Done
              </button>
            </>
          )}
        </div>
      </div>
    </div>
  );
}
