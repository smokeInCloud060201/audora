import React from "react";
import { ipcClient } from "../api/ipc";
import type { AppSettings, MediaItem, PlaybackSnapshot } from "../types/domain";
import { formatDurationMs, progressPercent } from "../utils/format";
import { AudoraLogo } from "./AudoraLogo";
import { IconExpandMode, IconHelpCircle, IconPause, IconPlay } from "./Icons";

interface MinimumPlayerWindowProps {
  activeMedia: MediaItem | null;
  snapshot: PlaybackSnapshot;
  isHelpOpen: boolean;
  onToggleHelp: () => void;
  onCloseHelp: () => void;
  onUpdateSettings: (next: AppSettings) => void;
  onUpdateSnapshot: (next: PlaybackSnapshot) => void;
  onOpenMediaPicker: () => void;
  onDropFiles: (files: FileList | null) => void;
  fileInputRef: React.RefObject<HTMLInputElement | null>;
  mediaElementRef: React.RefObject<HTMLVideoElement | HTMLAudioElement | null>;
  onMediaLoadedMetadata: (el: HTMLVideoElement | HTMLAudioElement) => void;
  onMediaTimeUpdate: (el: HTMLVideoElement | HTMLAudioElement) => void;
}

export function MinimumPlayerWindow({
  activeMedia,
  snapshot,
  isHelpOpen,
  onToggleHelp,
  onCloseHelp,
  onUpdateSettings,
  onUpdateSnapshot,
  onOpenMediaPicker,
  onDropFiles,
  fileInputRef,
  mediaElementRef,
  onMediaLoadedMetadata,
  onMediaTimeUpdate,
}: MinimumPlayerWindowProps): React.ReactElement {
  const pct = progressPercent(snapshot.position_ms, snapshot.duration_ms);

  return (
    <div
      className="minimum-mode-shell"
      data-testid="minimum-mode-shell"
      data-node-id="2:103"
      onDragOver={(e) => e.preventDefault()}
      onDrop={(e) => {
        e.preventDefault();
        onDropFiles(e.dataTransfer?.files ?? null);
      }}
    >
      <input
        ref={fileInputRef}
        type="file"
        multiple
        accept="audio/*,video/*,.mp4,.mov,.webm,.mkv,.mp3,.wav,.flac,.ogg,.opus,.m4a,.aac"
        style={{ display: "none" }}
        onChange={(e) => onDropFiles(e.target.files)}
      />

      {activeMedia?.media_kind === "video" && activeMedia.stream_url && (
        <audio
          ref={mediaElementRef as React.RefObject<HTMLAudioElement>}
          src={activeMedia.stream_url}
          onLoadedMetadata={(e) => onMediaLoadedMetadata(e.currentTarget)}
          onTimeUpdate={(e) => onMediaTimeUpdate(e.currentTarget)}
          onEnded={() => onUpdateSnapshot(ipcClient.handleMediaEnded())}
        />
      )}

      <div className="minimum-player-card" data-node-id="20:2">
        <header
          className="minimum-titlebar"
          data-tauri-drag-region
          data-node-id="20:3"
          onMouseDown={(e) => {
            const target = e.target as HTMLElement | null;
            if (target?.closest("button")) return;
            ipcClient.dragNativeWindow();
          }}
        >
          <div className="minimum-titlebar-left">
            <AudoraLogo variant="mark" size={28} isPlaying={snapshot.state === "playing"} />
            <div className="minimum-titlebar-meta">
              <p className="minimum-media-title" data-node-id="20:9">
                {activeMedia ? activeMedia.display_name : "No media loaded"}
              </p>
              <p className="minimum-media-subtitle" data-node-id="20:10">
                {activeMedia ? "Audora · Local" : "Audora · Drop or open a file"}
              </p>
            </div>
          </div>

          <div className="minimum-titlebar-actions">
            <button
              type="button"
              className="minimum-icon-btn"
              aria-label="Minimum mode shortcuts and help"
              aria-expanded={isHelpOpen}
              title="Shortcuts & help"
              data-node-id="20:11"
              onClick={onToggleHelp}
            >
              <IconHelpCircle size={14} />
            </button>
            <button
              type="button"
              className="minimum-icon-btn expand"
              aria-label="Return to full mode"
              title="Open full mode (Esc / M)"
              data-node-id="20:12"
              onClick={() => {
                onCloseHelp();
                onUpdateSettings(ipcClient.setWindowMode("full"));
              }}
            >
              <IconExpandMode size={13} />
            </button>
          </div>
        </header>

        {isHelpOpen && (
          <div className="minimum-help-popover" data-testid="minimum-help-popover">
            <div style={{ fontSize: 11, fontWeight: 600, color: "var(--text-primary)" }}>
              Space to play · Arrow keys to seek · Shift+N marker
            </div>
            <div style={{ fontSize: 10, color: "var(--text-tertiary)" }}>
              Press N to write a note or Esc to expand.
            </div>
            <button
              type="button"
              className="btn-primary"
              style={{ height: 28, fontSize: 11 }}
              onClick={() => {
                onCloseHelp();
                onUpdateSettings(ipcClient.setWindowMode("full"));
              }}
            >
              <span>Open full mode</span>
              <IconExpandMode size={12} />
            </button>
          </div>
        )}

        <div className="minimum-transport-body">
          <button
            type="button"
            className="minimum-play-btn"
            aria-label={
              !activeMedia
                ? "Open local media file"
                : snapshot.state === "playing"
                  ? "Pause"
                  : "Play"
            }
            title={!activeMedia ? "Open local media file" : undefined}
            data-node-id="20:13"
            onClick={() => {
              if (!activeMedia) {
                onOpenMediaPicker();
                return;
              }
              onUpdateSnapshot(ipcClient.playbackCommand({ action: "toggle_play_pause" }));
            }}
          >
            {snapshot.state === "playing" ? <IconPause size={18} /> : <IconPlay size={18} />}
          </button>

          <div className="minimum-time-column">
            <div className="minimum-time-row">
              <span className="minimum-time-current tabular-nums" data-node-id="20:15">
                {formatDurationMs(snapshot.position_ms)}
              </span>
              <span className="minimum-time-total tabular-nums" data-node-id="20:16">
                / {activeMedia ? formatDurationMs(snapshot.duration_ms) : "00:00"}
              </span>
            </div>

            <div className="minimum-scrubber-wrap">
              <input
                type="range"
                aria-label="Seek timeline"
                className="minimum-scrubber"
                disabled={!activeMedia}
                min={0}
                max={snapshot.duration_ms ?? 100}
                value={snapshot.position_ms}
                style={{
                  background: `linear-gradient(to right, var(--min-accent) 0%, var(--min-accent) ${pct}%, var(--min-track-bg) ${pct}%, var(--min-track-bg) 100%)`,
                }}
                onChange={(e) =>
                  onUpdateSnapshot(
                    ipcClient.playbackCommand({
                      action: "seek",
                      position_ms: Number(e.target.value),
                    }),
                  )
                }
              />
            </div>
          </div>
        </div>
      </div>
    </div>
  );
}
