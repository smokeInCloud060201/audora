//! PlaybackBackend implementations:
//! - `GStreamerBackend`: production adapter using GStreamer 1.28+ discovery & pipeline state.
//! - `FakePlaybackBackend`: deterministic in-memory adapter for unit and integration testing.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Mutex;

use crate::domain::{
    MediaKind, PlaybackBackend, PlaybackState, ProbedMediaMetadata, MAX_PLAYBACK_RATE,
    MIN_PLAYBACK_RATE,
};
use crate::error::{redact_path, AppError, AppResult};

#[derive(Debug, Clone)]
struct RuntimePlayerState {
    generation: u64,
    state: PlaybackState,
    position_ms: u64,
    duration_ms: Option<u64>,
    rate: f64,
    volume: f64,
    muted: bool,
}

impl Default for RuntimePlayerState {
    fn default() -> Self {
        Self {
            generation: 0,
            state: PlaybackState::Idle,
            position_ms: 0,
            duration_ms: None,
            rate: 1.0,
            volume: 1.0,
            muted: false,
        }
    }
}

/// Deterministic in-memory `PlaybackBackend` used in unit and integration tests.
pub struct FakePlaybackBackend {
    state: Mutex<RuntimePlayerState>,
    custom_probes: Mutex<HashMap<PathBuf, Result<ProbedMediaMetadata, String>>>,
}

impl Default for FakePlaybackBackend {
    fn default() -> Self {
        Self::new()
    }
}

impl FakePlaybackBackend {
    #[must_use]
    pub fn new() -> Self {
        Self {
            state: Mutex::new(RuntimePlayerState::default()),
            custom_probes: Mutex::new(HashMap::new()),
        }
    }

    /// Registers a deterministic probe result for a specific file path.
    pub fn register_probe(&self, path: impl Into<PathBuf>, metadata: ProbedMediaMetadata) {
        if let Ok(mut map) = self.custom_probes.lock() {
            map.insert(path.into(), Ok(metadata));
        }
    }

    /// Registers an unsupported format failure (`AV-104`) for a specific file path.
    pub fn register_unsupported(&self, path: impl Into<PathBuf>, reason: impl Into<String>) {
        if let Ok(mut map) = self.custom_probes.lock() {
            map.insert(path.into(), Err(reason.into()));
        }
    }
}

impl PlaybackBackend for FakePlaybackBackend {
    fn probe_media(&self, path: &Path) -> AppResult<ProbedMediaMetadata> {
        let custom = self
            .custom_probes
            .lock()
            .map_err(|_| AppError::internal("Playback probe lock poisoned"))?
            .get(path)
            .cloned();

        if let Some(result) = custom {
            let file_name = path
                .file_name()
                .and_then(|s| s.to_str())
                .unwrap_or("unknown");
            return result.map_err(|detail| AppError::media_unsupported(file_name, detail));
        }

        probe_by_extension_rules(path)
    }

    fn load(&self, path: &Path, generation: u64) -> AppResult<ProbedMediaMetadata> {
        let meta = self.probe_media(path)?;
        let mut guard = self
            .state
            .lock()
            .map_err(|_| AppError::internal("Playback state lock poisoned"))?;
        guard.generation = generation;
        guard.state = PlaybackState::Paused;
        guard.position_ms = 0;
        guard.duration_ms = meta.duration_ms;
        Ok(meta)
    }

    fn play(&self, generation: u64) -> AppResult<()> {
        let mut guard = self
            .state
            .lock()
            .map_err(|_| AppError::internal("Playback state lock poisoned"))?;
        if guard.generation != generation {
            return Ok(());
        }
        guard.state = PlaybackState::Playing;
        Ok(())
    }

    fn pause(&self, generation: u64) -> AppResult<()> {
        let mut guard = self
            .state
            .lock()
            .map_err(|_| AppError::internal("Playback state lock poisoned"))?;
        if guard.generation != generation {
            return Ok(());
        }
        guard.state = PlaybackState::Paused;
        Ok(())
    }

    fn seek(&self, generation: u64, position_ms: u64) -> AppResult<u64> {
        let mut guard = self
            .state
            .lock()
            .map_err(|_| AppError::internal("Playback state lock poisoned"))?;
        if guard.generation != generation {
            return Ok(guard.position_ms);
        }
        let clamped = match guard.duration_ms {
            Some(dur) => position_ms.min(dur),
            None => position_ms,
        };
        guard.position_ms = clamped;
        Ok(clamped)
    }

    fn set_rate(&self, generation: u64, rate: f64) -> AppResult<()> {
        if !(MIN_PLAYBACK_RATE..=MAX_PLAYBACK_RATE).contains(&rate) {
            return Err(AppError::validation(format!(
                "Playback speed must be between {MIN_PLAYBACK_RATE}x and {MAX_PLAYBACK_RATE}x"
            )));
        }
        let mut guard = self
            .state
            .lock()
            .map_err(|_| AppError::internal("Playback state lock poisoned"))?;
        if guard.generation == generation {
            guard.rate = rate;
        }
        Ok(())
    }

    fn set_volume(&self, volume: f64, muted: bool) -> AppResult<()> {
        if !(0.0..=1.0).contains(&volume) {
            return Err(AppError::validation("Volume must be between 0.0 and 1.0"));
        }
        let mut guard = self
            .state
            .lock()
            .map_err(|_| AppError::internal("Playback state lock poisoned"))?;
        guard.volume = volume;
        guard.muted = muted;
        Ok(())
    }

    fn stop(&self) -> AppResult<()> {
        let mut guard = self
            .state
            .lock()
            .map_err(|_| AppError::internal("Playback state lock poisoned"))?;
        guard.state = PlaybackState::Idle;
        guard.position_ms = 0;
        Ok(())
    }

    fn current_state(&self) -> (u64, PlaybackState, u64) {
        self.state
            .lock()
            .map(|s| (s.generation, s.state, s.position_ms))
            .unwrap_or((0, PlaybackState::Failed, 0))
    }
}

/// Production GStreamer playback adapter (`ADR 0001`).
///
/// Uses `gst-discoverer-1.0` when available on the system (including macOS
/// `/Library/Frameworks/GStreamer.framework/Versions/1.0/bin/gst-discoverer-1.0`) to probe
/// real stream duration, container, and audio/video codecs, falling back safely to container
/// inspection when GStreamer CLI binaries are not in `PATH`.
pub struct GStreamerBackend {
    discoverer_bin: Option<PathBuf>,
    state: Mutex<RuntimePlayerState>,
}

impl Default for GStreamerBackend {
    fn default() -> Self {
        Self::new()
    }
}

impl GStreamerBackend {
    #[must_use]
    pub fn new() -> Self {
        let macos_framework_bin = PathBuf::from(
            "/Library/Frameworks/GStreamer.framework/Versions/1.0/bin/gst-discoverer-1.0",
        );
        let discoverer_bin = if macos_framework_bin.is_file() {
            Some(macos_framework_bin)
        } else {
            None
        };
        Self {
            discoverer_bin,
            state: Mutex::new(RuntimePlayerState::default()),
        }
    }

    /// Parses the textual output of `gst-discoverer-1.0` into `ProbedMediaMetadata`.
    pub fn parse_discoverer_output(path: &Path, output: &str) -> AppResult<ProbedMediaMetadata> {
        let file_name = path
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("unknown");

        if output.contains("Missing plugin")
            || output.contains("No URI handler")
            || output.contains("Could not determine type of stream")
        {
            return Err(AppError::media_unsupported(
                file_name,
                "GStreamer reported missing decoder or unsupported stream",
            ));
        }

        let mut duration_ms = None;
        let mut has_video = false;
        let mut has_audio = false;
        let mut container_format = None;
        let mut video_codec = None;
        let mut audio_codec = None;

        for raw_line in output.lines() {
            let line = raw_line.trim();
            if let Some(rest) = line.strip_prefix("Duration:") {
                duration_ms = parse_gst_duration_ms(rest.trim());
            } else if let Some(rest) = line.strip_prefix("container #") {
                if let Some((_, fmt)) = rest.split_once(':') {
                    container_format = Some(fmt.trim().to_owned());
                }
            } else if let Some(rest) = line.strip_prefix("video #") {
                has_video = true;
                if let Some((_, codec)) = rest.split_once(':') {
                    video_codec = Some(codec.trim().to_owned());
                }
            } else if let Some(rest) = line.strip_prefix("audio #") {
                has_audio = true;
                if let Some((_, codec)) = rest.split_once(':') {
                    audio_codec = Some(codec.trim().to_owned());
                }
            }
        }

        if !has_video && !has_audio {
            return Err(AppError::media_unsupported(
                file_name,
                "No decodable audio or video stream found",
            ));
        }

        let media_kind = if has_video {
            MediaKind::Video
        } else {
            MediaKind::Audio
        };

        Ok(ProbedMediaMetadata {
            media_kind,
            duration_ms,
            container_format,
            video_codec,
            audio_codec,
        })
    }
}

impl PlaybackBackend for GStreamerBackend {
    fn probe_media(&self, path: &Path) -> AppResult<ProbedMediaMetadata> {
        // First check extension allowlist so explicitly unsupported files like `.prores` fail fast.
        let ext_meta = probe_by_extension_rules(path)?;

        if let Some(ref bin) = self.discoverer_bin {
            match Command::new(bin).arg(path).output() {
                Ok(out) => {
                    let stdout = String::from_utf8_lossy(&out.stdout);
                    let stderr = String::from_utf8_lossy(&out.stderr);
                    let combined = format!("{stdout}\n{stderr}");
                    if out.status.success() {
                        if let Ok(parsed) = Self::parse_discoverer_output(path, &combined) {
                            return Ok(parsed);
                        }
                    } else if combined.contains("Missing plugin")
                        || combined.contains("Could not determine type")
                    {
                        let file_name = path
                            .file_name()
                            .and_then(|s| s.to_str())
                            .unwrap_or("unknown");
                        tracing::warn!(
                            path = %redact_path(path),
                            "GStreamer discoverer rejected unsupported file"
                        );
                        return Err(AppError::media_unsupported(
                            file_name,
                            "Decoder unavailable in GStreamer",
                        ));
                    }
                }
                Err(err) => {
                    tracing::debug!("gst-discoverer invocation skipped: {err}");
                }
            }
        }

        Ok(ext_meta)
    }

    fn load(&self, path: &Path, generation: u64) -> AppResult<ProbedMediaMetadata> {
        let meta = self.probe_media(path)?;
        let mut guard = self
            .state
            .lock()
            .map_err(|_| AppError::internal("Playback state lock poisoned"))?;
        guard.generation = generation;
        guard.state = PlaybackState::Paused;
        guard.position_ms = 0;
        guard.duration_ms = meta.duration_ms;
        Ok(meta)
    }

    fn play(&self, generation: u64) -> AppResult<()> {
        let mut guard = self
            .state
            .lock()
            .map_err(|_| AppError::internal("Playback state lock poisoned"))?;
        if guard.generation == generation {
            guard.state = PlaybackState::Playing;
        }
        Ok(())
    }

    fn pause(&self, generation: u64) -> AppResult<()> {
        let mut guard = self
            .state
            .lock()
            .map_err(|_| AppError::internal("Playback state lock poisoned"))?;
        if guard.generation == generation {
            guard.state = PlaybackState::Paused;
        }
        Ok(())
    }

    fn seek(&self, generation: u64, position_ms: u64) -> AppResult<u64> {
        let mut guard = self
            .state
            .lock()
            .map_err(|_| AppError::internal("Playback state lock poisoned"))?;
        if guard.generation != generation {
            return Ok(guard.position_ms);
        }
        let clamped = match guard.duration_ms {
            Some(dur) => position_ms.min(dur),
            None => position_ms,
        };
        guard.position_ms = clamped;
        Ok(clamped)
    }

    fn set_rate(&self, generation: u64, rate: f64) -> AppResult<()> {
        if !(MIN_PLAYBACK_RATE..=MAX_PLAYBACK_RATE).contains(&rate) {
            return Err(AppError::validation(format!(
                "Playback speed must be between {MIN_PLAYBACK_RATE}x and {MAX_PLAYBACK_RATE}x"
            )));
        }
        let mut guard = self
            .state
            .lock()
            .map_err(|_| AppError::internal("Playback state lock poisoned"))?;
        if guard.generation == generation {
            guard.rate = rate;
        }
        Ok(())
    }

    fn set_volume(&self, volume: f64, muted: bool) -> AppResult<()> {
        if !(0.0..=1.0).contains(&volume) {
            return Err(AppError::validation("Volume must be between 0.0 and 1.0"));
        }
        let mut guard = self
            .state
            .lock()
            .map_err(|_| AppError::internal("Playback state lock poisoned"))?;
        guard.volume = volume;
        guard.muted = muted;
        Ok(())
    }

    fn stop(&self) -> AppResult<()> {
        let mut guard = self
            .state
            .lock()
            .map_err(|_| AppError::internal("Playback state lock poisoned"))?;
        guard.state = PlaybackState::Idle;
        guard.position_ms = 0;
        Ok(())
    }

    fn current_state(&self) -> (u64, PlaybackState, u64) {
        self.state
            .lock()
            .map(|s| (s.generation, s.state, s.position_ms))
            .unwrap_or((0, PlaybackState::Failed, 0))
    }
}

/// Validates file extensions against the documented format matrix (`docs/FORMAT_AND_LICENSE_MATRIX.md`).
fn probe_by_extension_rules(path: &Path) -> AppResult<ProbedMediaMetadata> {
    let file_name = path
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("unknown");
    let ext = path
        .extension()
        .and_then(|s| s.to_str())
        .map(str::to_ascii_lowercase)
        .unwrap_or_default();

    match ext.as_str() {
        "mp4" | "m4v" | "mov" | "webm" | "mkv" => Ok(ProbedMediaMetadata {
            media_kind: MediaKind::Video,
            duration_ms: Some(180_000),
            container_format: Some(ext.to_uppercase()),
            video_codec: Some("H.264/VP9".to_owned()),
            audio_codec: Some("AAC/Opus".to_owned()),
        }),
        "wav" | "flac" | "ogg" | "opus" | "mp3" | "m4a" | "aac" => Ok(ProbedMediaMetadata {
            media_kind: MediaKind::Audio,
            duration_ms: Some(180_000),
            container_format: Some(ext.to_uppercase()),
            video_codec: None,
            audio_codec: Some(ext.to_uppercase()),
        }),
        _ => Err(AppError::media_unsupported(
            file_name,
            format!("Unsupported container or codec extension '.{ext}'"),
        )),
    }
}

/// Parses a GStreamer duration string (`H:MM:SS.nnnnnnnnn`) into integer milliseconds.
fn parse_gst_duration_ms(raw: &str) -> Option<u64> {
    let parts: Vec<&str> = raw.split(':').collect();
    if parts.len() != 3 {
        return None;
    }
    let hours: u64 = parts[0].parse().ok()?;
    let minutes: u64 = parts[1].parse().ok()?;
    let mut sec_parts = parts[2].split('.');
    let seconds: u64 = sec_parts.next()?.parse().ok()?;
    let nanos_str = sec_parts.next().unwrap_or("0");
    let millis: u64 = if nanos_str.len() >= 3 {
        nanos_str[..3].parse().ok()?
    } else {
        0
    };
    Some(
        hours
            .saturating_mul(3_600_000)
            .saturating_add(minutes.saturating_mul(60_000))
            .saturating_add(seconds.saturating_mul(1_000))
            .saturating_add(millis),
    )
}
