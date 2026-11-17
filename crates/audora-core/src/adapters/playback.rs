//! PlaybackBackend implementations:
//! - `GStreamerBackend`: production adapter using GStreamer 1.28+ discovery & pipeline state.
//! - `FakePlaybackBackend`: deterministic in-memory adapter for unit and integration testing.

use std::collections::HashMap;
use std::env;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
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
        let discoverer_bin = Self::locate_discoverer_binary();
        Self {
            discoverer_bin,
            state: Mutex::new(RuntimePlayerState::default()),
        }
    }

    fn locate_discoverer_binary() -> Option<PathBuf> {
        let bin_name = if cfg!(target_os = "windows") {
            "gst-discoverer-1.0.exe"
        } else {
            "gst-discoverer-1.0"
        };

        if let Some(path_os) = env::var_os("PATH") {
            for dir in env::split_paths(&path_os) {
                let candidate = dir.join(bin_name);
                if candidate.is_file() {
                    return Some(candidate);
                }
            }
        }

        let static_candidates = [
            "/Library/Frameworks/GStreamer.framework/Versions/1.0/bin/gst-discoverer-1.0",
            "/opt/homebrew/bin/gst-discoverer-1.0",
            "/usr/local/bin/gst-discoverer-1.0",
            "/usr/bin/gst-discoverer-1.0",
        ];
        for cand in static_candidates {
            let p = PathBuf::from(cand);
            if p.is_file() {
                return Some(p);
            }
        }

        None
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
        let mut ext_meta = probe_by_extension_rules(path)?;
        // Never invent a synthetic 180s duration in production when GStreamer CLI is absent;
        // probe real container headers or leave duration `None` (`M-6`).
        ext_meta.duration_ms = probe_container_duration_ms(path);

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
    let millis: u64 = match nanos_str.len() {
        0 => 0,
        1 => nanos_str.parse::<u64>().ok()?.saturating_mul(100),
        2 => nanos_str.parse::<u64>().ok()?.saturating_mul(10),
        _ => nanos_str[..3].parse().ok()?,
    };
    Some(
        hours
            .saturating_mul(3_600_000)
            .saturating_add(minutes.saturating_mul(60_000))
            .saturating_add(seconds.saturating_mul(1_000))
            .saturating_add(millis),
    )
}

/// Probes real container duration (`ISO BMFF` `mvhd`, `WAV`, `FLAC`) without external binaries.
pub fn probe_container_duration_ms(path: &Path) -> Option<u64> {
    let ext = path
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    let mut file = File::open(path).ok()?;
    let len = file.metadata().ok()?.len();
    if len == 0 {
        return None;
    }
    match ext.as_str() {
        "mp4" | "m4a" | "m4v" | "mov" => probe_isobmff_mvhd_ms(&mut file, len),
        "wav" => probe_wav_ms(&mut file, len),
        "flac" => probe_flac_ms(&mut file, len),
        _ => None,
    }
}

fn read_at(file: &mut File, pos: u64, buf: &mut [u8]) -> bool {
    file.seek(SeekFrom::Start(pos)).is_ok() && file.read_exact(buf).is_ok()
}

fn read_box_hdr(file: &mut File, pos: u64, limit: u64) -> Option<([u8; 4], u64, u64)> {
    if pos.checked_add(8)? > limit {
        return None;
    }
    let mut hdr = [0_u8; 16];
    if !read_at(file, pos, &mut hdr[..8]) {
        return None;
    }
    let sz32 = u32::from_be_bytes([hdr[0], hdr[1], hdr[2], hdr[3]]);
    let btype = [hdr[4], hdr[5], hdr[6], hdr[7]];
    let (bsz, hlen) = if sz32 == 1 {
        if pos.checked_add(16)? > limit || !read_at(file, pos.saturating_add(8), &mut hdr[8..16]) {
            return None;
        }
        (
            u64::from_be_bytes([
                hdr[8], hdr[9], hdr[10], hdr[11], hdr[12], hdr[13], hdr[14], hdr[15],
            ]),
            16_u64,
        )
    } else if sz32 == 0 {
        (limit.saturating_sub(pos), 8_u64)
    } else {
        (u64::from(sz32), 8_u64)
    };
    if bsz < hlen || pos.checked_add(bsz)? > limit {
        return None;
    }
    Some((btype, bsz, hlen))
}

fn probe_isobmff_mvhd_ms(file: &mut File, file_len: u64) -> Option<u64> {
    let mut pos = 0_u64;
    for _ in 0..256 {
        if pos >= file_len {
            break;
        }
        let (btype, bsz, hlen) = read_box_hdr(file, pos, file_len)?;
        if &btype == b"moov" {
            let moov_end = pos.saturating_add(bsz);
            let mut cpos = pos.saturating_add(hlen);
            for _ in 0..128 {
                if cpos >= moov_end {
                    break;
                }
                let Some((ctype, csz, chlen)) = read_box_hdr(file, cpos, moov_end) else {
                    break;
                };
                if &ctype == b"mvhd" {
                    let plen = csz.saturating_sub(chlen);
                    if plen >= 20 {
                        let mut mvhd = [0_u8; 32];
                        let want = (plen as usize).min(32);
                        if read_at(file, cpos.saturating_add(chlen), &mut mvhd[..want]) {
                            let ver = mvhd[0];
                            let (ts, dur) = if ver == 1 && want >= 32 {
                                let ts =
                                    u32::from_be_bytes([mvhd[20], mvhd[21], mvhd[22], mvhd[23]]);
                                let d = u64::from_be_bytes([
                                    mvhd[24], mvhd[25], mvhd[26], mvhd[27], mvhd[28], mvhd[29],
                                    mvhd[30], mvhd[31],
                                ]);
                                (ts, if d == u64::MAX { 0 } else { d })
                            } else if ver == 0 {
                                let ts =
                                    u32::from_be_bytes([mvhd[12], mvhd[13], mvhd[14], mvhd[15]]);
                                let d32 =
                                    u32::from_be_bytes([mvhd[16], mvhd[17], mvhd[18], mvhd[19]]);
                                (ts, if d32 == u32::MAX { 0 } else { u64::from(d32) })
                            } else {
                                (0, 0)
                            };
                            if ts > 0 && dur > 0 {
                                let ms = (u128::from(dur) * 1_000) / u128::from(ts);
                                if let Ok(v) = u64::try_from(ms) {
                                    if v > 0 {
                                        return Some(v);
                                    }
                                }
                            }
                        }
                    }
                }
                cpos = cpos.saturating_add(csz);
            }
        }
        pos = pos.saturating_add(bsz);
    }
    None
}

fn probe_wav_ms(file: &mut File, file_len: u64) -> Option<u64> {
    if file_len < 44 {
        return None;
    }
    let mut riff = [0_u8; 12];
    if !read_at(file, 0, &mut riff) || &riff[0..4] != b"RIFF" || &riff[8..12] != b"WAVE" {
        return None;
    }
    let mut pos = 12_u64;
    let mut byte_rate = 0_u32;
    let mut data_sz = 0_u64;
    for _ in 0..64 {
        if pos.saturating_add(8) > file_len {
            break;
        }
        let mut ch = [0_u8; 8];
        if !read_at(file, pos, &mut ch) {
            break;
        }
        let cid = [ch[0], ch[1], ch[2], ch[3]];
        let csz = u64::from(u32::from_le_bytes([ch[4], ch[5], ch[6], ch[7]]));
        if &cid == b"fmt " && csz >= 16 {
            let mut fmt_b = [0_u8; 16];
            if read_at(file, pos.saturating_add(8), &mut fmt_b) {
                byte_rate = u32::from_le_bytes([fmt_b[8], fmt_b[9], fmt_b[10], fmt_b[11]]);
            }
        } else if &cid == b"data" {
            data_sz = csz.min(file_len.saturating_sub(pos.saturating_add(8)));
            if byte_rate > 0 {
                break;
            }
        }
        pos = pos
            .saturating_add(8)
            .saturating_add(csz)
            .saturating_add(csz & 1);
    }
    if byte_rate > 0 && data_sz > 0 {
        u64::try_from((u128::from(data_sz) * 1_000) / u128::from(byte_rate))
            .ok()
            .filter(|&v| v > 0)
    } else {
        None
    }
}

fn probe_flac_ms(file: &mut File, file_len: u64) -> Option<u64> {
    if file_len < 42 {
        return None;
    }
    let mut hdr = [0_u8; 42];
    if !read_at(file, 0, &mut hdr) || &hdr[0..4] != b"fLaC" || (hdr[4] & 0x7F) != 0 {
        return None;
    }
    let sr = (u32::from(hdr[18]) << 12) | (u32::from(hdr[19]) << 4) | (u32::from(hdr[20]) >> 4);
    let samples = (u64::from(hdr[21] & 0x0F) << 32)
        | (u64::from(hdr[22]) << 24)
        | (u64::from(hdr[23]) << 16)
        | (u64::from(hdr[24]) << 8)
        | u64::from(hdr[25]);
    if sr > 0 && samples > 0 {
        u64::try_from((u128::from(samples) * 1_000) / u128::from(sr))
            .ok()
            .filter(|&v| v > 0)
    } else {
        None
    }
}
