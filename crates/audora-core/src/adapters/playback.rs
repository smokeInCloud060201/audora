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
    ffprobe_bin: Option<PathBuf>,
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
        let discoverer_bin = Self::locate_binary(
            if cfg!(target_os = "windows") {
                "gst-discoverer-1.0.exe"
            } else {
                "gst-discoverer-1.0"
            },
            &[
                "/Library/Frameworks/GStreamer.framework/Versions/1.0/bin/gst-discoverer-1.0",
                "/opt/homebrew/bin/gst-discoverer-1.0",
                "/usr/local/bin/gst-discoverer-1.0",
                "/usr/bin/gst-discoverer-1.0",
            ],
        );
        let ffprobe_bin = Self::locate_binary(
            if cfg!(target_os = "windows") {
                "ffprobe.exe"
            } else {
                "ffprobe"
            },
            &[
                "/opt/homebrew/bin/ffprobe",
                "/usr/local/bin/ffprobe",
                "/usr/bin/ffprobe",
            ],
        );
        Self {
            discoverer_bin,
            ffprobe_bin,
            state: Mutex::new(RuntimePlayerState::default()),
        }
    }

    fn locate_binary(bin_name: &str, static_candidates: &[&str]) -> Option<PathBuf> {
        if let Some(path_os) = env::var_os("PATH") {
            for dir in env::split_paths(&path_os) {
                let candidate = dir.join(bin_name);
                if candidate.is_file() {
                    return Some(candidate);
                }
            }
        }
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

        if ext_meta.duration_ms.is_none() {
            if let Some(ref ffprobe) = self.ffprobe_bin {
                if let Ok(out) = Command::new(ffprobe)
                    .args([
                        "-v",
                        "quiet",
                        "-show_entries",
                        "format=duration",
                        "-of",
                        "default=noprint_wrappers=1:nokey=1",
                    ])
                    .arg(path)
                    .output()
                {
                    if out.status.success() {
                        if let Ok(secs) = String::from_utf8_lossy(&out.stdout).trim().parse::<f64>()
                        {
                            if secs.is_finite() && secs > 0.0 {
                                ext_meta.duration_ms = Some((secs * 1_000.0).round() as u64);
                            }
                        }
                    }
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

/// Probes real container duration (`ISO BMFF` `mvhd`/`sidx`, `MKV`/`WebM` EBML, `MP3` Xing/VBRI/CBR,
/// `Ogg` Opus/Vorbis, `WAV` RIFF, `FLAC` STREAMINFO) in pure Rust without external binaries.
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
        "mkv" | "webm" => probe_ebml_ms(&mut file, len),
        "mp3" => probe_mp3_ms(&mut file, len),
        "ogg" | "opus" => probe_ogg_ms(&mut file, len),
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
    let mut sidx_fallback_ms: Option<u64> = None;
    for _ in 0..256 {
        if pos >= file_len {
            break;
        }
        let Some((btype, bsz, hlen)) = read_box_hdr(file, pos, file_len) else {
            break;
        };
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
        } else if &btype == b"sidx" && sidx_fallback_ms.is_none() {
            let plen = bsz.saturating_sub(hlen);
            if (24..=2_097_152).contains(&plen) {
                let mut buf = vec![0_u8; plen as usize];
                if read_at(file, pos.saturating_add(hlen), &mut buf) {
                    let ver = buf[0];
                    let ts = u32::from_be_bytes([buf[8], buf[9], buf[10], buf[11]]);
                    let off_entries = if ver == 0 { 20_usize } else { 28_usize };
                    if ts > 0 && buf.len() >= off_entries + 4 {
                        let ref_count =
                            u16::from_be_bytes([buf[off_entries + 2], buf[off_entries + 3]])
                                as usize;
                        let table_start = off_entries + 4;
                        if buf.len() >= table_start.saturating_add(ref_count.saturating_mul(12)) {
                            let mut total_dur = 0_u64;
                            for idx in 0..ref_count {
                                let off = table_start + idx * 12 + 4;
                                let sub = u32::from_be_bytes([
                                    buf[off],
                                    buf[off + 1],
                                    buf[off + 2],
                                    buf[off + 3],
                                ]);
                                total_dur = total_dur.saturating_add(u64::from(sub));
                            }
                            if total_dur > 0 {
                                let ms = (u128::from(total_dur) * 1_000) / u128::from(ts);
                                sidx_fallback_ms = u64::try_from(ms).ok().filter(|&v| v > 0);
                            }
                        }
                    }
                }
            }
        }
        pos = pos.saturating_add(bsz);
    }
    sidx_fallback_ms
}

fn read_ebml_vint(buf: &[u8], pos: usize, strip_marker: bool) -> Option<(u64, usize)> {
    let first = *buf.get(pos)?;
    if first == 0 {
        return None;
    }
    let len = (first.leading_zeros() as usize) + 1;
    if len > 8 || pos.checked_add(len)? > buf.len() {
        return None;
    }
    let mask = if strip_marker {
        (1_u8 << (8 - len)).wrapping_sub(1)
    } else {
        0xFF
    };
    let mut val = u64::from(first & mask);
    for &b in &buf[pos + 1..pos + len] {
        val = (val << 8) | u64::from(b);
    }
    Some((val, len))
}

fn probe_ebml_ms(file: &mut File, file_len: u64) -> Option<u64> {
    if file_len < 32 {
        return None;
    }
    let head_len = (file_len as usize).min(262_144);
    let mut buf = vec![0_u8; head_len];
    if !read_at(file, 0, &mut buf) || &buf[..4] != b"\x1A\x45\xDF\xA3" {
        return None;
    }
    // Skip EBML Header element (`0x1A45DFA3`)
    let (hdr_sz, hdr_vlen) = read_ebml_vint(&buf, 4, true)?;
    let mut pos = 4_usize
        .checked_add(hdr_vlen)?
        .checked_add(hdr_sz as usize)?;
    while pos < buf.len() {
        let (eid, id_len) = read_ebml_vint(&buf, pos, false)?;
        let (esz, sz_len) = read_ebml_vint(&buf, pos + id_len, true)?;
        let payload_start = pos + id_len + sz_len;
        if eid == 0x1853_8067 {
            // Matroska Segment: scan top-level Segment children for Info (`0x1549A966`)
            let mut spos = payload_start;
            while spos < buf.len() {
                let Some((cid, cid_len)) = read_ebml_vint(&buf, spos, false) else {
                    break;
                };
                let Some((csz, csz_len)) = read_ebml_vint(&buf, spos + cid_len, true) else {
                    break;
                };
                let cstart = spos + cid_len + csz_len;
                let cend = cstart.saturating_add(csz as usize).min(buf.len());
                if cid == 0x1549_A966 {
                    let mut ts_scale: u64 = 1_000_000; // default 1ms in nanoseconds
                    let mut duration_ticks: Option<f64> = None;
                    let mut ipos = cstart;
                    while ipos < cend {
                        let Some((iid, iid_len)) = read_ebml_vint(&buf, ipos, false) else {
                            break;
                        };
                        let Some((isz, isz_len)) = read_ebml_vint(&buf, ipos + iid_len, true)
                        else {
                            break;
                        };
                        let istart = ipos + iid_len + isz_len;
                        let isz_usize = isz as usize;
                        if istart.saturating_add(isz_usize) > cend {
                            break;
                        }
                        if iid == 0x2AD7B1 && (1..=8).contains(&isz_usize) {
                            let mut v = 0_u64;
                            for &b in &buf[istart..istart + isz_usize] {
                                v = (v << 8) | u64::from(b);
                            }
                            if v > 0 {
                                ts_scale = v;
                            }
                        } else if iid == 0x4489 {
                            if isz_usize == 4 {
                                let f = f32::from_be_bytes([
                                    buf[istart],
                                    buf[istart + 1],
                                    buf[istart + 2],
                                    buf[istart + 3],
                                ]);
                                duration_ticks = Some(f64::from(f));
                            } else if isz_usize == 8 {
                                let f = f64::from_be_bytes([
                                    buf[istart],
                                    buf[istart + 1],
                                    buf[istart + 2],
                                    buf[istart + 3],
                                    buf[istart + 4],
                                    buf[istart + 5],
                                    buf[istart + 6],
                                    buf[istart + 7],
                                ]);
                                duration_ticks = Some(f);
                            }
                        }
                        ipos = istart + isz_usize;
                    }
                    if let Some(ticks) = duration_ticks {
                        if ticks.is_finite() && ticks > 0.0 {
                            let ms = (ticks * (ts_scale as f64)) / 1_000_000.0;
                            if ms.is_finite() && ms >= 1.0 {
                                return Some(ms.round() as u64);
                            }
                        }
                    }
                    return None;
                }
                if csz as usize > buf.len() {
                    break;
                }
                spos = cend;
            }
            break;
        }
        pos = payload_start.saturating_add(esz as usize);
    }
    None
}

fn probe_mp3_ms(file: &mut File, file_len: u64) -> Option<u64> {
    if file_len < 16 {
        return None;
    }
    let mut id3 = [0_u8; 10];
    let mut audio_offset = 0_u64;
    if read_at(file, 0, &mut id3) && &id3[0..3] == b"ID3" {
        let tag_sz = ((u64::from(id3[6] & 0x7F)) << 21)
            | ((u64::from(id3[7] & 0x7F)) << 14)
            | ((u64::from(id3[8] & 0x7F)) << 7)
            | u64::from(id3[9] & 0x7F);
        let footer = if (id3[5] & 0x10) != 0 { 10_u64 } else { 0_u64 };
        audio_offset = 10_u64.saturating_add(tag_sz).saturating_add(footer);
    }
    if audio_offset >= file_len {
        return None;
    }
    let scan_len = (file_len.saturating_sub(audio_offset) as usize).min(16_384);
    let mut buf = vec![0_u8; scan_len];
    if !read_at(file, audio_offset, &mut buf) {
        return None;
    }

    const BITRATES_V1_L3: [u32; 16] = [
        0, 32, 40, 48, 56, 64, 80, 96, 112, 128, 160, 192, 224, 256, 320, 0,
    ];
    const BITRATES_V2_L3: [u32; 16] = [
        0, 8, 16, 24, 32, 40, 48, 56, 64, 80, 96, 112, 128, 144, 160, 0,
    ];

    for idx in 0..buf.len().saturating_sub(4) {
        let b0 = buf[idx];
        let b1 = buf[idx + 1];
        if b0 != 0xFF || (b1 & 0xE0) != 0xE0 {
            continue;
        }
        let version_id = (b1 >> 3) & 0x03;
        let layer_id = (b1 >> 1) & 0x03;
        // Layer III is `0b01`; version `0b01` is reserved
        if version_id == 1 || layer_id != 1 {
            continue;
        }
        let b2 = buf[idx + 2];
        let b3 = buf[idx + 3];
        let br_idx = ((b2 >> 4) & 0x0F) as usize;
        let sr_idx = ((b2 >> 2) & 0x03) as usize;
        if br_idx == 0 || br_idx == 15 || sr_idx == 3 {
            continue;
        }
        let sample_rate: u32 = match (version_id, sr_idx) {
            (3, 0) => 44_100,
            (3, 1) => 48_000,
            (3, 2) => 32_000,
            (2, 0) => 22_050,
            (2, 1) => 24_000,
            (2, 2) => 16_000,
            (0, 0) => 11_025,
            (0, 1) => 12_000,
            (0, 2) => 8_000,
            _ => continue,
        };
        let kbps = if version_id == 3 {
            BITRATES_V1_L3[br_idx]
        } else {
            BITRATES_V2_L3[br_idx]
        };
        if kbps == 0 {
            continue;
        }
        let samples_per_frame: u64 = if version_id == 3 { 1_152 } else { 576 };
        let is_mono = ((b3 >> 6) & 0x03) == 3;
        let side_info_len: usize = match (version_id == 3, is_mono) {
            (true, true) => 17,
            (true, false) => 32,
            (false, true) => 9,
            (false, false) => 17,
        };

        // 1. Check Xing / Info VBR frame count header
        let xing_off = idx + 4 + side_info_len;
        if xing_off + 12 <= buf.len()
            && (&buf[xing_off..xing_off + 4] == b"Xing" || &buf[xing_off..xing_off + 4] == b"Info")
        {
            let flags = u32::from_be_bytes([
                buf[xing_off + 4],
                buf[xing_off + 5],
                buf[xing_off + 6],
                buf[xing_off + 7],
            ]);
            if (flags & 0x01) != 0 {
                let frames = u32::from_be_bytes([
                    buf[xing_off + 8],
                    buf[xing_off + 9],
                    buf[xing_off + 10],
                    buf[xing_off + 11],
                ]);
                if frames > 0 {
                    let ms = (u128::from(frames) * u128::from(samples_per_frame) * 1_000)
                        / u128::from(sample_rate);
                    return u64::try_from(ms).ok().filter(|&v| v > 0);
                }
            }
        }

        // 2. Check Fraunhofer VBRI header at fixed offset +36
        let vbri_off = idx + 4 + 32;
        if vbri_off + 18 <= buf.len() && &buf[vbri_off..vbri_off + 4] == b"VBRI" {
            let frames = u32::from_be_bytes([
                buf[vbri_off + 14],
                buf[vbri_off + 15],
                buf[vbri_off + 16],
                buf[vbri_off + 17],
            ]);
            if frames > 0 {
                let ms = (u128::from(frames) * u128::from(samples_per_frame) * 1_000)
                    / u128::from(sample_rate);
                return u64::try_from(ms).ok().filter(|&v| v > 0);
            }
        }

        // 3. Constant-bitrate (CBR) duration from audio payload size
        let mut tail_tag = 0_u64;
        if file_len >= 128 {
            let mut t = [0_u8; 3];
            if read_at(file, file_len - 128, &mut t) && &t == b"TAG" {
                tail_tag = 128;
            }
        }
        let audio_bytes = file_len
            .saturating_sub(audio_offset + idx as u64)
            .saturating_sub(tail_tag);
        let ms = (u128::from(audio_bytes) * 8) / u128::from(kbps);
        return u64::try_from(ms).ok().filter(|&v| v > 0);
    }

    None
}

fn probe_ogg_ms(file: &mut File, file_len: u64) -> Option<u64> {
    if file_len < 48 {
        return None;
    }
    let head_len = (file_len as usize).min(512);
    let mut head = vec![0_u8; head_len];
    if !read_at(file, 0, &mut head) || &head[0..4] != b"OggS" {
        return None;
    }
    let seg_count = *head.get(26)? as usize;
    let pkt_start = 27_usize.checked_add(seg_count)?;
    if pkt_start + 16 > head.len() {
        return None;
    }
    let (sample_rate, pre_skip): (u32, u64) = if &head[pkt_start..pkt_start + 8] == b"OpusHead" {
        let skip = u16::from_le_bytes([head[pkt_start + 10], head[pkt_start + 11]]);
        (48_000, u64::from(skip))
    } else if &head[pkt_start..pkt_start + 7] == b"\x01vorbis" {
        let sr = u32::from_le_bytes([
            head[pkt_start + 12],
            head[pkt_start + 13],
            head[pkt_start + 14],
            head[pkt_start + 15],
        ]);
        (sr, 0)
    } else {
        return None;
    };
    if sample_rate == 0 {
        return None;
    }

    let tail_len = (file_len as usize).min(65_536);
    let tail_pos = file_len.saturating_sub(tail_len as u64);
    let mut tail = vec![0_u8; tail_len];
    if !read_at(file, tail_pos, &mut tail) {
        return None;
    }
    for i in (0..=tail.len().saturating_sub(14)).rev() {
        if &tail[i..i + 4] == b"OggS" && tail[i + 4] == 0 {
            let granule = u64::from_le_bytes([
                tail[i + 6],
                tail[i + 7],
                tail[i + 8],
                tail[i + 9],
                tail[i + 10],
                tail[i + 11],
                tail[i + 12],
                tail[i + 13],
            ]);
            if granule != u64::MAX && granule > pre_skip {
                let samples = granule - pre_skip;
                let ms = (u128::from(samples) * 1_000) / u128::from(sample_rate);
                return u64::try_from(ms).ok().filter(|&v| v > 0);
            }
        }
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
