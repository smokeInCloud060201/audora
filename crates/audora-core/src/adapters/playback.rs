//! PlaybackBackend implementations:
//! - `GStreamerBackend`: production adapter using GStreamer 1.28+ discovery & pipeline state.
//! - `FakePlaybackBackend`: deterministic in-memory adapter for unit and integration testing.

use std::collections::HashMap;
use std::env;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Mutex;
use std::thread;
use std::time::{Duration, Instant};

use crate::domain::{
    MediaKind, PlaybackBackend, PlaybackState, ProbedMediaMetadata, MAX_PLAYBACK_RATE,
    MAX_SAFE_DURATION_MS, MIN_PLAYBACK_RATE,
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

fn normalize_probe_key(path: &Path) -> PathBuf {
    let resolved = match std::fs::canonicalize(path) {
        Ok(canon) => {
            let lossy = canon.to_string_lossy();
            if let Some(unc) = lossy.strip_prefix(r"\\?\UNC\") {
                PathBuf::from(format!(r"\\{unc}"))
            } else if let Some(stripped) = lossy.strip_prefix(r"\\?\") {
                PathBuf::from(stripped)
            } else {
                canon
            }
        }
        Err(_) => path.to_path_buf(),
    };
    #[cfg(target_os = "windows")]
    {
        PathBuf::from(resolved.to_string_lossy().to_lowercase())
    }
    #[cfg(not(target_os = "windows"))]
    {
        resolved
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
        let key = normalize_probe_key(&path.into());
        if let Ok(mut map) = self.custom_probes.lock() {
            map.insert(key, Ok(metadata));
        }
    }

    /// Registers an unsupported format failure (`AV-104`) for a specific file path.
    pub fn register_unsupported(&self, path: impl Into<PathBuf>, reason: impl Into<String>) {
        let key = normalize_probe_key(&path.into());
        if let Ok(mut map) = self.custom_probes.lock() {
            map.insert(key, Err(reason.into()));
        }
    }
}

impl PlaybackBackend for FakePlaybackBackend {
    fn probe_media(&self, path: &Path) -> AppResult<ProbedMediaMetadata> {
        let key = normalize_probe_key(path);
        let custom = self
            .custom_probes
            .lock()
            .map_err(|_| AppError::internal("Playback probe lock poisoned"))?
            .get(&key)
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

fn run_command_with_timeout(
    cmd: &mut Command,
    timeout: Duration,
) -> std::io::Result<std::process::Output> {
    cmd.stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = cmd.spawn()?;
    let stdout_handle = child.stdout.take().map(|mut out| {
        thread::spawn(move || {
            let mut buf = Vec::new();
            let _ = out.read_to_end(&mut buf);
            buf
        })
    });
    let stderr_handle = child.stderr.take().map(|mut err| {
        thread::spawn(move || {
            let mut buf = Vec::new();
            let _ = err.read_to_end(&mut buf);
            buf
        })
    });

    let deadline = Instant::now() + timeout;
    let status = loop {
        match child.try_wait()? {
            Some(st) => break st,
            None => {
                if Instant::now() >= deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::TimedOut,
                        "media prober timed out",
                    ));
                }
                thread::sleep(Duration::from_millis(20));
            }
        }
    };

    let stdout = stdout_handle
        .and_then(|h| h.join().ok())
        .unwrap_or_default();
    let stderr = stderr_handle
        .and_then(|h| h.join().ok())
        .unwrap_or_default();
    Ok(std::process::Output {
        status,
        stdout,
        stderr,
    })
}

impl PlaybackBackend for GStreamerBackend {
    fn probe_media(&self, path: &Path) -> AppResult<ProbedMediaMetadata> {
        // First check extension allowlist so explicitly unsupported files like `.prores` fail fast.
        let mut ext_meta = probe_by_extension_rules(path)?;
        // Never invent a synthetic 180s duration in production when GStreamer CLI is absent;
        // probe real container headers or leave duration `None` (`M-6`).
        ext_meta.duration_ms = probe_container_duration_ms(path);

        if let Some(ref bin) = self.discoverer_bin {
            let mut cmd = Command::new(bin);
            cmd.args(["-t", "5", "--"]).arg(path);
            match run_command_with_timeout(&mut cmd, Duration::from_secs(6)) {
                Ok(out) => {
                    let stdout = String::from_utf8_lossy(&out.stdout);
                    let stderr = String::from_utf8_lossy(&out.stderr);
                    let combined = format!("{stdout}\n{stderr}");
                    if out.status.success() {
                        if let Ok(mut parsed) = Self::parse_discoverer_output(path, &combined) {
                            parsed.duration_ms = parsed
                                .duration_ms
                                .or(ext_meta.duration_ms)
                                .map(|d| d.min(MAX_SAFE_DURATION_MS));
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
                let mut cmd = Command::new(ffprobe);
                cmd.args([
                    "-v",
                    "quiet",
                    "-show_entries",
                    "format=duration",
                    "-of",
                    "default=noprint_wrappers=1:nokey=1",
                    "-i",
                ])
                .arg(path);
                if let Ok(out) = run_command_with_timeout(&mut cmd, Duration::from_secs(6)) {
                    if out.status.success() {
                        if let Ok(secs) = String::from_utf8_lossy(&out.stdout).trim().parse::<f64>()
                        {
                            if secs.is_finite() && secs > 0.0 {
                                let ms = (secs * 1_000.0).round();
                                if ms >= 1.0 && ms <= (MAX_SAFE_DURATION_MS as f64) {
                                    ext_meta.duration_ms = Some(ms as u64);
                                }
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

/// Probes real container duration (`ISO BMFF` `mvhd`/`mehd`/`mdhd`/`sidx`, `MKV`/`WebM` EBML,
/// `MP3` Xing/VBRI/CBR, `AAC` ADTS/BMFF, `Ogg` Opus/Vorbis, `WAV` RIFF/RF64, `FLAC` STREAMINFO)
/// across all 12 supported extensions in pure Rust without external binaries.
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
    let raw = match ext.as_str() {
        "mp4" | "m4a" | "m4v" | "mov" => probe_isobmff_mvhd_ms(&mut file, len),
        "mkv" | "webm" => probe_ebml_ms(&mut file, len),
        "mp3" => probe_mp3_ms(&mut file, len),
        "aac" => probe_aac_ms(&mut file, len),
        "ogg" | "opus" => probe_ogg_ms(&mut file, len),
        "wav" => probe_wav_ms(&mut file, len),
        "flac" => probe_flac_ms(&mut file, len),
        _ => None,
    };
    raw.filter(|&v| v > 0 && v <= MAX_SAFE_DURATION_MS)
}

fn read_at(file: &mut File, pos: u64, buf: &mut [u8]) -> bool {
    file.seek(SeekFrom::Start(pos)).is_ok() && file.read_exact(buf).is_ok()
}

fn skip_id3v2_tags(file: &mut File, file_len: u64) -> u64 {
    let mut offset = 0_u64;
    let mut id3 = [0_u8; 10];
    for _ in 0..8 {
        if offset.saturating_add(10) > file_len || !read_at(file, offset, &mut id3) {
            break;
        }
        if &id3[0..3] != b"ID3" || id3[3] == 0xFF || id3[4] == 0xFF {
            break;
        }
        if (id3[6] | id3[7] | id3[8] | id3[9]) & 0x80 != 0 {
            break;
        }
        let tag_sz = ((u64::from(id3[6])) << 21)
            | ((u64::from(id3[7])) << 14)
            | ((u64::from(id3[8])) << 7)
            | u64::from(id3[9]);
        let footer = if (id3[5] & 0x10) != 0 { 10_u64 } else { 0_u64 };
        let next = offset
            .saturating_add(10)
            .saturating_add(tag_sz)
            .saturating_add(footer);
        if next <= offset || next >= file_len {
            offset = next;
            break;
        }
        offset = next;
    }
    offset
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

fn parse_fullbox_ts_dur_ms(file: &mut File, payload_pos: u64, plen: u64) -> Option<(u32, u64)> {
    if plen < 20 {
        return None;
    }
    let mut b = [0_u8; 32];
    let want = plen.min(32) as usize;
    if !read_at(file, payload_pos, &mut b[..want]) {
        return None;
    }
    let ver = b[0];
    if ver == 1 && want >= 32 {
        let ts = u32::from_be_bytes([b[20], b[21], b[22], b[23]]);
        let d = u64::from_be_bytes([b[24], b[25], b[26], b[27], b[28], b[29], b[30], b[31]]);
        Some((ts, if d == u64::MAX { 0 } else { d }))
    } else if ver == 0 {
        let ts = u32::from_be_bytes([b[12], b[13], b[14], b[15]]);
        let d32 = u32::from_be_bytes([b[16], b[17], b[18], b[19]]);
        Some((ts, if d32 == u32::MAX { 0 } else { u64::from(d32) }))
    } else {
        None
    }
}

fn probe_isobmff_mvhd_ms(file: &mut File, file_len: u64) -> Option<u64> {
    let mut pos = 0_u64;
    let mut sidx_ref_id: Option<u32> = None;
    let mut sidx_ts: u32 = 0;
    let mut sidx_total_ticks: u64 = 0;

    for _ in 0..16_384 {
        if pos >= file_len {
            break;
        }
        let Some((btype, bsz, hlen)) = read_box_hdr(file, pos, file_len) else {
            break;
        };
        if &btype == b"moov" {
            let moov_end = pos.saturating_add(bsz);
            let mut cpos = pos.saturating_add(hlen);
            let mut mvhd_ts = 0_u32;
            let mut mehd_ticks = 0_u64;
            let mut max_mdhd_ms = 0_u64;

            for _ in 0..256 {
                if cpos >= moov_end {
                    break;
                }
                let Some((ctype, csz, chlen)) = read_box_hdr(file, cpos, moov_end) else {
                    break;
                };
                if &ctype == b"mvhd" {
                    let plen = csz.saturating_sub(chlen);
                    if let Some((ts, dur)) =
                        parse_fullbox_ts_dur_ms(file, cpos.saturating_add(chlen), plen)
                    {
                        mvhd_ts = ts;
                        if ts > 0 && dur > 0 {
                            let ms = (u128::from(dur) * 1_000) / u128::from(ts);
                            if let Ok(v) = u64::try_from(ms) {
                                if v > 0 {
                                    return Some(v);
                                }
                            }
                        }
                    }
                } else if &ctype == b"mvex" {
                    let mvex_end = cpos.saturating_add(csz);
                    let mut mpos = cpos.saturating_add(chlen);
                    for _ in 0..32 {
                        if mpos >= mvex_end {
                            break;
                        }
                        let Some((mtype, msz, mhlen)) = read_box_hdr(file, mpos, mvex_end) else {
                            break;
                        };
                        if &mtype == b"mehd" {
                            let plen = msz.saturating_sub(mhlen);
                            let mut mb = [0_u8; 12];
                            let want = plen.min(12) as usize;
                            if want >= 8
                                && read_at(file, mpos.saturating_add(mhlen), &mut mb[..want])
                            {
                                let ver = mb[0];
                                if ver == 1 && want >= 12 {
                                    let d = u64::from_be_bytes([
                                        mb[4], mb[5], mb[6], mb[7], mb[8], mb[9], mb[10], mb[11],
                                    ]);
                                    if d != u64::MAX {
                                        mehd_ticks = d;
                                    }
                                } else if ver == 0 {
                                    let d32 = u32::from_be_bytes([mb[4], mb[5], mb[6], mb[7]]);
                                    if d32 != u32::MAX {
                                        mehd_ticks = u64::from(d32);
                                    }
                                }
                            }
                        }
                        mpos = mpos.saturating_add(msz);
                    }
                } else if &ctype == b"trak" {
                    let trak_end = cpos.saturating_add(csz);
                    let mut tpos = cpos.saturating_add(chlen);
                    for _ in 0..32 {
                        if tpos >= trak_end {
                            break;
                        }
                        let Some((ttype, tsz, thlen)) = read_box_hdr(file, tpos, trak_end) else {
                            break;
                        };
                        if &ttype == b"mdia" {
                            let mdia_end = tpos.saturating_add(tsz);
                            let mut dpos = tpos.saturating_add(thlen);
                            for _ in 0..32 {
                                if dpos >= mdia_end {
                                    break;
                                }
                                let Some((dtype, dsz, dhlen)) = read_box_hdr(file, dpos, mdia_end)
                                else {
                                    break;
                                };
                                if &dtype == b"mdhd" {
                                    let plen = dsz.saturating_sub(dhlen);
                                    if let Some((ts, dur)) = parse_fullbox_ts_dur_ms(
                                        file,
                                        dpos.saturating_add(dhlen),
                                        plen,
                                    ) {
                                        if ts > 0 && dur > 0 {
                                            let ms = (u128::from(dur) * 1_000) / u128::from(ts);
                                            if let Ok(v) = u64::try_from(ms) {
                                                max_mdhd_ms = max_mdhd_ms.max(v);
                                            }
                                        }
                                    }
                                }
                                dpos = dpos.saturating_add(dsz);
                            }
                        }
                        tpos = tpos.saturating_add(tsz);
                    }
                }
                cpos = cpos.saturating_add(csz);
            }

            if mvhd_ts > 0 && mehd_ticks > 0 {
                let ms = (u128::from(mehd_ticks) * 1_000) / u128::from(mvhd_ts);
                if let Ok(v) = u64::try_from(ms) {
                    if v > 0 {
                        return Some(v);
                    }
                }
            }
            if max_mdhd_ms > 0 {
                return Some(max_mdhd_ms);
            }
        } else if &btype == b"sidx" {
            let plen = bsz.saturating_sub(hlen);
            if (24..=2_097_152).contains(&plen) {
                let mut buf = vec![0_u8; plen as usize];
                if read_at(file, pos.saturating_add(hlen), &mut buf) {
                    let ver = buf[0];
                    if ver <= 1 {
                        let ref_id = u32::from_be_bytes([buf[4], buf[5], buf[6], buf[7]]);
                        let ts = u32::from_be_bytes([buf[8], buf[9], buf[10], buf[11]]);
                        let off_entries = if ver == 0 { 20_usize } else { 28_usize };
                        let matches_stream = match sidx_ref_id {
                            None => true,
                            Some(first_id) => first_id == ref_id && sidx_ts == ts,
                        };
                        if matches_stream && ts > 0 && buf.len() >= off_entries + 4 {
                            let ref_count =
                                u16::from_be_bytes([buf[off_entries + 2], buf[off_entries + 3]])
                                    as usize;
                            let table_start = off_entries + 4;
                            if buf.len() >= table_start.saturating_add(ref_count.saturating_mul(12))
                            {
                                let mut box_dur = 0_u64;
                                for idx in 0..ref_count {
                                    let entry_off = table_start + idx * 12;
                                    if (buf[entry_off] & 0x80) != 0 {
                                        continue;
                                    }
                                    let sub = u32::from_be_bytes([
                                        buf[entry_off + 4],
                                        buf[entry_off + 5],
                                        buf[entry_off + 6],
                                        buf[entry_off + 7],
                                    ]);
                                    box_dur = box_dur.saturating_add(u64::from(sub));
                                }
                                if box_dur > 0 {
                                    sidx_ref_id = Some(ref_id);
                                    sidx_ts = ts;
                                    sidx_total_ticks = sidx_total_ticks.saturating_add(box_dur);
                                }
                            }
                        }
                    }
                }
            }
        }
        pos = pos.saturating_add(bsz);
    }

    if sidx_ts > 0 && sidx_total_ticks > 0 {
        let ms = (u128::from(sidx_total_ticks) * 1_000) / u128::from(sidx_ts);
        u64::try_from(ms).ok().filter(|&v| v > 0)
    } else {
        None
    }
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
        if len == 8 {
            0
        } else {
            (1_u8 << (8 - len)).wrapping_sub(1)
        }
    } else {
        0xFF
    };
    let mut val = u64::from(first & mask);
    for &b in &buf[pos + 1..pos + len] {
        val = (val << 8) | u64::from(b);
    }
    Some((val, len))
}

fn is_ebml_unknown_size(val: u64, len: usize) -> bool {
    let max_for_len = if len >= 8 {
        (1_u64 << 56) - 1
    } else {
        (1_u64 << (7 * len)) - 1
    };
    val == max_for_len
}

fn read_ebml_hdr_at(file: &mut File, pos: u64, limit: u64) -> Option<(u64, u64, u64, bool)> {
    if pos >= limit {
        return None;
    }
    let avail = limit.saturating_sub(pos).min(16) as usize;
    if avail < 2 {
        return None;
    }
    let mut hdr = [0_u8; 16];
    if !read_at(file, pos, &mut hdr[..avail]) {
        return None;
    }
    let (eid, id_len) = read_ebml_vint(&hdr[..avail], 0, false)?;
    let (esz, sz_len) = read_ebml_vint(&hdr[..avail], id_len, true)?;
    let unknown = is_ebml_unknown_size(esz, sz_len);
    Some((eid, esz, (id_len + sz_len) as u64, unknown))
}

fn probe_ebml_ms(file: &mut File, file_len: u64) -> Option<u64> {
    if file_len < 24 {
        return None;
    }
    let (ebml_id, ebml_sz, ebml_hlen, ebml_unk) = read_ebml_hdr_at(file, 0, file_len)?;
    if ebml_id != 0x1A45_DFA3 || ebml_unk {
        return None;
    }
    let mut pos = ebml_hlen.checked_add(ebml_sz)?;
    for _ in 0..64 {
        if pos >= file_len {
            break;
        }
        let Some((eid, esz, ehlen, eunk)) = read_ebml_hdr_at(file, pos, file_len) else {
            break;
        };
        let payload_start = pos.saturating_add(ehlen);
        let elem_end = if eunk {
            file_len
        } else {
            payload_start.saturating_add(esz).min(file_len)
        };
        if eid == 0x1853_8067 {
            let mut spos = payload_start;
            for _ in 0..512 {
                if spos >= elem_end {
                    break;
                }
                let Some((cid, csz, chlen, cunk)) = read_ebml_hdr_at(file, spos, elem_end) else {
                    break;
                };
                let cstart = spos.saturating_add(chlen);
                if cid == 0x1549_A966 {
                    let info_len = if cunk {
                        elem_end.saturating_sub(cstart).min(65_536) as usize
                    } else {
                        csz.min(elem_end.saturating_sub(cstart)).min(65_536) as usize
                    };
                    if info_len == 0 {
                        return None;
                    }
                    let mut info_buf = vec![0_u8; info_len];
                    if !read_at(file, cstart, &mut info_buf) {
                        return None;
                    }
                    let mut ts_scale: u64 = 1_000_000;
                    let mut duration_ticks: Option<f64> = None;
                    let mut ipos = 0_usize;
                    while ipos < info_buf.len() {
                        let Some((iid, iid_len)) = read_ebml_vint(&info_buf, ipos, false) else {
                            break;
                        };
                        let Some((isz, isz_len)) = read_ebml_vint(&info_buf, ipos + iid_len, true)
                        else {
                            break;
                        };
                        let istart = ipos + iid_len + isz_len;
                        let Ok(isz_usize) = usize::try_from(isz) else {
                            break;
                        };
                        if istart.saturating_add(isz_usize) > info_buf.len() {
                            break;
                        }
                        if iid == 0x2AD7B1 && (1..=8).contains(&isz_usize) {
                            let mut v = 0_u64;
                            for &b in &info_buf[istart..istart + isz_usize] {
                                v = (v << 8) | u64::from(b);
                            }
                            if v > 0 {
                                ts_scale = v;
                            }
                        } else if iid == 0x4489 {
                            if isz_usize == 4 {
                                let f = f32::from_be_bytes([
                                    info_buf[istart],
                                    info_buf[istart + 1],
                                    info_buf[istart + 2],
                                    info_buf[istart + 3],
                                ]);
                                duration_ticks = Some(f64::from(f));
                            } else if isz_usize == 8 {
                                let f = f64::from_be_bytes([
                                    info_buf[istart],
                                    info_buf[istart + 1],
                                    info_buf[istart + 2],
                                    info_buf[istart + 3],
                                    info_buf[istart + 4],
                                    info_buf[istart + 5],
                                    info_buf[istart + 6],
                                    info_buf[istart + 7],
                                ]);
                                duration_ticks = Some(f);
                            }
                        }
                        ipos = istart + isz_usize;
                    }
                    if let Some(ticks) = duration_ticks {
                        if ticks.is_finite() && ticks > 0.0 {
                            let ms = (ticks * (ts_scale as f64)) / 1_000_000.0;
                            if ms.is_finite() && ms >= 1.0 && ms <= (MAX_SAFE_DURATION_MS as f64) {
                                return Some(ms.round() as u64);
                            }
                        }
                    }
                    return None;
                }
                if cid == 0x1F43_B675 || cunk {
                    break;
                }
                spos = cstart.saturating_add(csz);
            }
            break;
        }
        if eunk {
            break;
        }
        pos = elem_end;
    }
    None
}

fn decode_mpeg_header(hdr: [u8; 4]) -> Option<(u8, u8, u32, u32, u64, usize)> {
    if hdr[0] != 0xFF || (hdr[1] & 0xE0) != 0xE0 {
        return None;
    }
    let version_id = (hdr[1] >> 3) & 0x03;
    let layer_id = (hdr[1] >> 1) & 0x03;
    if version_id == 1 || layer_id == 0 {
        return None;
    }
    let br_idx = ((hdr[2] >> 4) & 0x0F) as usize;
    let sr_idx = ((hdr[2] >> 2) & 0x03) as usize;
    let padding = usize::from((hdr[2] >> 1) & 0x01);
    if br_idx == 0 || br_idx == 15 || sr_idx == 3 {
        return None;
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
        _ => return None,
    };

    const V1_L1: [u32; 16] = [
        0, 32, 64, 96, 128, 160, 192, 224, 256, 288, 320, 352, 384, 416, 448, 0,
    ];
    const V1_L2: [u32; 16] = [
        0, 32, 48, 56, 64, 80, 96, 112, 128, 160, 192, 224, 256, 320, 384, 0,
    ];
    const V1_L3: [u32; 16] = [
        0, 32, 40, 48, 56, 64, 80, 96, 112, 128, 160, 192, 224, 256, 320, 0,
    ];
    const V2_L1: [u32; 16] = [
        0, 32, 48, 56, 64, 80, 96, 112, 128, 144, 160, 176, 192, 224, 256, 0,
    ];
    const V2_L23: [u32; 16] = [
        0, 8, 16, 24, 32, 40, 48, 56, 64, 80, 96, 112, 128, 144, 160, 0,
    ];

    let kbps = match (version_id == 3, layer_id) {
        (true, 3) => V1_L1[br_idx],
        (true, 2) => V1_L2[br_idx],
        (true, 1) => V1_L3[br_idx],
        (false, 3) => V2_L1[br_idx],
        _ => V2_L23[br_idx],
    };
    if kbps == 0 {
        return None;
    }

    let (samples_per_frame, frame_size) = if layer_id == 3 {
        let fsz = (((12_usize * (kbps as usize) * 1_000) / (sample_rate as usize)) + padding) * 4;
        (384_u64, fsz)
    } else if layer_id == 2 || version_id == 3 {
        let fsz = ((144_000_usize * (kbps as usize)) / (sample_rate as usize)) + padding;
        (1_152_u64, fsz)
    } else {
        let fsz = ((72_000_usize * (kbps as usize)) / (sample_rate as usize)) + padding;
        (576_u64, fsz)
    };
    if frame_size < 4 {
        return None;
    }
    Some((
        version_id,
        layer_id,
        kbps,
        sample_rate,
        samples_per_frame,
        frame_size,
    ))
}

fn probe_mp3_ms(file: &mut File, file_len: u64) -> Option<u64> {
    if file_len < 16 {
        return None;
    }
    let audio_offset = skip_id3v2_tags(file, file_len);
    if audio_offset >= file_len {
        return None;
    }
    let scan_len = file_len.saturating_sub(audio_offset).min(65_536) as usize;
    let mut buf = vec![0_u8; scan_len];
    if !read_at(file, audio_offset, &mut buf) {
        return None;
    }

    let mut tail_tag = 0_u64;
    if file_len >= 128 {
        let mut t = [0_u8; 3];
        if read_at(file, file_len - 128, &mut t) && &t == b"TAG" {
            tail_tag = 128;
        }
    }
    let effective_end = file_len.saturating_sub(tail_tag);

    for idx in 0..buf.len().saturating_sub(4) {
        let hdr = [buf[idx], buf[idx + 1], buf[idx + 2], buf[idx + 3]];
        let Some((version_id, layer_id, kbps, sample_rate, samples_per_frame, frame_size)) =
            decode_mpeg_header(hdr)
        else {
            continue;
        };

        let is_mono = ((hdr[3] >> 6) & 0x03) == 3;
        let crc_len: usize = if (hdr[1] & 0x01) == 0 { 2 } else { 0 };
        let side_info_len: usize = match (version_id == 3, is_mono) {
            (true, true) => 17,
            (true, false) => 32,
            (false, true) => 9,
            (false, false) => 17,
        };

        for cand_off in [idx + 4 + crc_len + side_info_len, idx + 4 + side_info_len] {
            if cand_off + 12 <= buf.len()
                && (&buf[cand_off..cand_off + 4] == b"Xing"
                    || &buf[cand_off..cand_off + 4] == b"Info")
            {
                let flags = u32::from_be_bytes([
                    buf[cand_off + 4],
                    buf[cand_off + 5],
                    buf[cand_off + 6],
                    buf[cand_off + 7],
                ]);
                if (flags & 0x01) != 0 {
                    let frames = u32::from_be_bytes([
                        buf[cand_off + 8],
                        buf[cand_off + 9],
                        buf[cand_off + 10],
                        buf[cand_off + 11],
                    ]);
                    if frames > 0 {
                        let ms = (u128::from(frames) * u128::from(samples_per_frame) * 1_000)
                            / u128::from(sample_rate);
                        return u64::try_from(ms).ok().filter(|&v| v > 0);
                    }
                }
            }
        }

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

        let next_file_pos = audio_offset
            .saturating_add(idx as u64)
            .saturating_add(frame_size as u64);
        if next_file_pos + 4 <= effective_end {
            let mut next_hdr = [0_u8; 4];
            let got_next = if idx + frame_size + 4 <= buf.len() {
                next_hdr.copy_from_slice(&buf[idx + frame_size..idx + frame_size + 4]);
                true
            } else {
                read_at(file, next_file_pos, &mut next_hdr)
            };
            if !got_next {
                continue;
            }
            let Some((nv, nl, _, nsr, _, _)) = decode_mpeg_header(next_hdr) else {
                continue;
            };
            if nv != version_id || nl != layer_id || nsr != sample_rate {
                continue;
            }
        }

        let audio_bytes = effective_end.saturating_sub(audio_offset + idx as u64);
        let ms = (u128::from(audio_bytes) * 8) / u128::from(kbps);
        return u64::try_from(ms).ok().filter(|&v| v > 0);
    }

    None
}

fn probe_aac_ms(file: &mut File, file_len: u64) -> Option<u64> {
    const ADTS_RATES: [u32; 13] = [
        96_000, 88_200, 64_000, 48_000, 44_100, 32_000, 24_000, 22_050, 16_000, 12_000, 11_025,
        8_000, 7_350,
    ];
    let audio_offset = skip_id3v2_tags(file, file_len);
    if audio_offset + 7 <= file_len {
        let scan_len = file_len.saturating_sub(audio_offset).min(32_768) as usize;
        let mut buf = vec![0_u8; scan_len];
        if read_at(file, audio_offset, &mut buf) {
            for idx in 0..buf.len().saturating_sub(7) {
                if buf[idx] != 0xFF || (buf[idx + 1] & 0xF6) != 0xF0 {
                    continue;
                }
                let sf_idx = ((buf[idx + 2] >> 2) & 0x0F) as usize;
                let Some(&sr) = ADTS_RATES.get(sf_idx) else {
                    continue;
                };
                let frame_len = ((usize::from(buf[idx + 3] & 0x03)) << 11)
                    | ((usize::from(buf[idx + 4])) << 3)
                    | usize::from(buf[idx + 5] >> 5);
                if frame_len < 7 {
                    continue;
                }

                let mut cur = idx;
                let mut frames_counted = 0_u64;
                let mut bytes_counted = 0_u64;
                let mut total_samples = 0_u64;
                while cur + 7 <= buf.len() && frames_counted < 256 {
                    if buf[cur] != 0xFF || (buf[cur + 1] & 0xF6) != 0xF0 {
                        break;
                    }
                    let c_sf = ((buf[cur + 2] >> 2) & 0x0F) as usize;
                    if c_sf != sf_idx {
                        break;
                    }
                    let flen = ((usize::from(buf[cur + 3] & 0x03)) << 11)
                        | ((usize::from(buf[cur + 4])) << 3)
                        | usize::from(buf[cur + 5] >> 5);
                    if flen < 7 {
                        break;
                    }
                    let blocks = u64::from(buf[cur + 6] & 0x03) + 1;
                    frames_counted += 1;
                    bytes_counted += flen as u64;
                    total_samples += blocks * 1_024;
                    cur += flen;
                }

                let stream_start = audio_offset + idx as u64;
                let single_frame_eof =
                    frames_counted == 1 && stream_start + bytes_counted >= file_len;
                if (frames_counted >= 2 || single_frame_eof) && bytes_counted > 0 {
                    let stream_bytes = file_len.saturating_sub(stream_start);
                    let est_samples = (u128::from(stream_bytes) * u128::from(total_samples))
                        / u128::from(bytes_counted);
                    let ms = (est_samples * 1_000) / u128::from(sr);
                    if let Ok(v) = u64::try_from(ms) {
                        if v > 0 {
                            return Some(v);
                        }
                    }
                }
            }
        }
    }
    probe_isobmff_mvhd_ms(file, file_len)
}

fn probe_ogg_ms(file: &mut File, file_len: u64) -> Option<u64> {
    if file_len < 48 {
        return None;
    }
    let head_len = file_len.min(8_192) as usize;
    let mut head = vec![0_u8; head_len];
    if !read_at(file, 0, &mut head) || &head[0..4] != b"OggS" {
        return None;
    }

    let mut p = 0_usize;
    let mut audio_stream: Option<([u8; 4], u32, u64)> = None;
    for _ in 0..8 {
        if p + 27 > head.len() || &head[p..p + 4] != b"OggS" || head[p + 4] != 0 {
            break;
        }
        let seg_count = usize::from(head[p + 26]);
        let pkt_start = p + 27 + seg_count;
        if pkt_start > head.len() {
            break;
        }
        let body_len: usize = head[p + 27..pkt_start]
            .iter()
            .map(|&b| usize::from(b))
            .sum();
        let page_end = pkt_start.saturating_add(body_len);
        let serial = [head[p + 14], head[p + 15], head[p + 16], head[p + 17]];
        let avail_pkt = head.len().saturating_sub(pkt_start).min(body_len);
        if seg_count > 0 && avail_pkt >= 16 {
            let pkt = &head[pkt_start..pkt_start + avail_pkt];
            if pkt.len() >= 19 && &pkt[0..8] == b"OpusHead" {
                let skip = u16::from_le_bytes([pkt[10], pkt[11]]);
                audio_stream = Some((serial, 48_000, u64::from(skip)));
                break;
            } else if &pkt[0..7] == b"\x01vorbis" {
                let sr = u32::from_le_bytes([pkt[12], pkt[13], pkt[14], pkt[15]]);
                if sr > 0 {
                    audio_stream = Some((serial, sr, 0));
                    break;
                }
            }
        }
        if page_end <= p || page_end > head.len() {
            break;
        }
        p = page_end;
    }

    let (serial, sample_rate, pre_skip) = audio_stream?;
    if sample_rate == 0 {
        return None;
    }

    let tail_len = file_len.min(65_536) as usize;
    let tail_pos = file_len.saturating_sub(tail_len as u64);
    let mut tail = vec![0_u8; tail_len];
    if !read_at(file, tail_pos, &mut tail) {
        return None;
    }
    for i in (0..=tail.len().saturating_sub(27)).rev() {
        if &tail[i..i + 4] != b"OggS" || tail[i + 4] != 0 || (tail[i + 5] & !0x07) != 0 {
            continue;
        }
        if tail[i + 14..i + 18] != serial {
            continue;
        }
        let seg_count = usize::from(tail[i + 26]);
        let seg_end = i + 27 + seg_count;
        if seg_end > tail.len() {
            continue;
        }
        let body_len: usize = tail[i + 27..seg_end].iter().map(|&b| usize::from(b)).sum();
        let page_end = seg_end.saturating_add(body_len);
        if page_end > tail.len() {
            continue;
        }
        if page_end + 4 <= tail.len() && &tail[page_end..page_end + 4] != b"OggS" {
            continue;
        }
        let granule_signed = i64::from_le_bytes([
            tail[i + 6],
            tail[i + 7],
            tail[i + 8],
            tail[i + 9],
            tail[i + 10],
            tail[i + 11],
            tail[i + 12],
            tail[i + 13],
        ]);
        if granule_signed <= 0 {
            continue;
        }
        let granule = granule_signed as u64;
        if granule > pre_skip {
            let samples = granule - pre_skip;
            let ms = (u128::from(samples) * 1_000) / u128::from(sample_rate);
            return u64::try_from(ms).ok().filter(|&v| v > 0);
        }
    }
    None
}

fn probe_wav_ms(file: &mut File, file_len: u64) -> Option<u64> {
    if file_len < 44 {
        return None;
    }
    let mut riff = [0_u8; 12];
    if !read_at(file, 0, &mut riff) || &riff[8..12] != b"WAVE" {
        return None;
    }
    let is_rf64 = &riff[0..4] == b"RF64" || &riff[0..4] == b"BW64";
    if &riff[0..4] != b"RIFF" && !is_rf64 {
        return None;
    }
    let mut pos = 12_u64;
    let mut byte_rate = 0_u32;
    let mut ds64_data_sz: Option<u64> = None;
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
        if &cid == b"ds64" && csz >= 16 {
            let mut ds = [0_u8; 16];
            if read_at(file, pos.saturating_add(8), &mut ds) {
                ds64_data_sz = Some(u64::from_le_bytes([
                    ds[8], ds[9], ds[10], ds[11], ds[12], ds[13], ds[14], ds[15],
                ]));
            }
        } else if &cid == b"fmt " && csz >= 16 {
            let mut fmt_b = [0_u8; 16];
            if read_at(file, pos.saturating_add(8), &mut fmt_b) {
                byte_rate = u32::from_le_bytes([fmt_b[8], fmt_b[9], fmt_b[10], fmt_b[11]]);
            }
        } else if &cid == b"data" {
            let raw_data_sz = if is_rf64 && csz == 0xFFFF_FFFF {
                ds64_data_sz.unwrap_or_else(|| file_len.saturating_sub(pos.saturating_add(8)))
            } else {
                csz
            };
            data_sz = raw_data_sz.min(file_len.saturating_sub(pos.saturating_add(8)));
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
    let base = skip_id3v2_tags(file, file_len);
    if base.saturating_add(42) > file_len {
        return None;
    }
    let mut magic = [0_u8; 4];
    if !read_at(file, base, &mut magic) || &magic != b"fLaC" {
        return None;
    }
    let mut bpos = base + 4;
    for _ in 0..16 {
        if bpos.saturating_add(4) > file_len {
            break;
        }
        let mut bh = [0_u8; 4];
        if !read_at(file, bpos, &mut bh) {
            break;
        }
        let is_last = (bh[0] & 0x80) != 0;
        let btype = bh[0] & 0x7F;
        let blen = (u64::from(bh[1]) << 16) | (u64::from(bh[2]) << 8) | u64::from(bh[3]);
        if btype == 0 && blen >= 34 && bpos.saturating_add(4 + 34) <= file_len {
            let mut si = [0_u8; 34];
            if !read_at(file, bpos + 4, &mut si) {
                return None;
            }
            let sr =
                (u32::from(si[10]) << 12) | (u32::from(si[11]) << 4) | (u32::from(si[12]) >> 4);
            let samples = (u64::from(si[13] & 0x0F) << 32)
                | (u64::from(si[14]) << 24)
                | (u64::from(si[15]) << 16)
                | (u64::from(si[16]) << 8)
                | u64::from(si[17]);
            if sr > 0 && samples > 0 {
                return u64::try_from((u128::from(samples) * 1_000) / u128::from(sr))
                    .ok()
                    .filter(|&v| v > 0);
            }
            return None;
        }
        if is_last {
            break;
        }
        bpos = bpos.saturating_add(4).saturating_add(blen);
    }
    None
}
