//! Audora desktop application entrypoint (`tao` + `wry` native shell with self-update and uninstall cleanup).

#![cfg_attr(
    all(not(debug_assertions), target_os = "windows"),
    windows_subsystem = "windows"
)]

use std::collections::HashSet;
use std::env;
use std::fs::{self, File};
use std::io::{Read, Seek, SeekFrom, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Component, Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

#[cfg(target_os = "windows")]
use std::os::windows::process::CommandExt;

use audora_desktop::{
    adapters::probe_container_duration_ms,
    build_production_dispatcher_with_db,
    commands::CommandDispatcher,
    default_app_data_dir, default_database_path,
    domain::{
        AppSettings, MediaItem, Note, PersistedPlaybackSession, Playlist, ThemeMode, WindowMode,
        DEFAULT_GITHUB_RELEASE_REPO,
    },
    persistence::Database,
    services::cleanup_all_app_artifacts,
};
use serde_json::json;
use sha2::{Digest, Sha256};
use tao::dpi::{LogicalSize, PhysicalPosition};
use tao::event::{Event, WindowEvent};
use tao::event_loop::{ControlFlow, EventLoopBuilder, EventLoopProxy};
use tao::window::{Window, WindowBuilder};
use tracing::{error, info, warn};
use wry::{
    BackgroundThrottlingPolicy, DragDropEvent, NewWindowResponse, PageLoadEvent, WebContext,
    WebViewBuilder,
};

#[derive(Debug, Clone)]
enum DesktopUserEvent {
    RequestGracefulClose,
    QuitWithBatch(Option<String>),
    SetWindowMode { minimum: bool },
    DragWindow,
    NativeDragState(bool),
    NativeFilesDropped(Vec<PathBuf>),
    WebViewPageLoaded,
}

const MAX_HTTP_BODY_BYTES: usize = 32 * 1024 * 1024;
const MAX_CONCURRENT_HTTP_CONNECTIONS: usize = 64;
const HTTP_REQUEST_DEADLINE_SECS: u64 = 10;
const DEFAULT_FULL_WIDTH: f64 = 1440.0;
const DEFAULT_FULL_HEIGHT: f64 = 900.0;
const MIN_FULL_WIDTH: f64 = 1040.0;
const MIN_FULL_HEIGHT: f64 = 640.0;
const MINIMUM_WIDGET_WIDTH: f64 = 360.0;
const MINIMUM_WIDGET_HEIGHT: f64 = 168.0;

const RUNTIME_CSP: &str = "default-src 'self'; script-src 'self'; style-src 'self' 'unsafe-inline'; img-src 'self' data: blob:; media-src 'self' blob:; font-src 'self' data:; connect-src 'self' https://api.github.com https://github.com; object-src 'none'; base-uri 'self'; frame-ancestors 'none'";

fn normalize_allowlist_key(canon: &Path) -> PathBuf {
    #[cfg(target_os = "windows")]
    {
        PathBuf::from(canon.to_string_lossy().to_lowercase())
    }
    #[cfg(not(target_os = "windows"))]
    {
        canon.to_path_buf()
    }
}

const MAX_ALLOWED_RELATIVE_DIR_DEPTH: usize = 4;

#[derive(Default)]
struct MediaAllowlist {
    os_trusted_files: HashSet<PathBuf>,
    os_trusted_dirs: HashSet<PathBuf>,
    session_files: HashSet<PathBuf>,
    session_dirs: HashSet<PathBuf>,
    db_standalone_files: HashSet<PathBuf>,
    db_enabled_dirs: HashSet<PathBuf>,
    db_all_source_dirs: HashSet<PathBuf>,
}

impl MediaAllowlist {
    fn is_unc_raw_path(raw: &str) -> bool {
        let trimmed = raw.trim_start().trim_end_matches(['\r', '\n']);
        let lower = trimmed.to_ascii_lowercase();
        if lower.starts_with(r"\\?\unc\") || lower.starts_with(r"\\.\unc\") {
            return true;
        }
        if trimmed.starts_with(r"\\?\") || trimmed.starts_with(r"\\.\") {
            let rest = &trimmed[4..];
            let is_drive = rest.len() >= 2
                && rest.as_bytes()[0].is_ascii_alphabetic()
                && rest.as_bytes()[1] == b':';
            return !is_drive;
        }
        trimmed.starts_with(r"\\")
    }

    fn lexical_normalize_unc(raw: &str) -> Option<String> {
        let trimmed = raw.trim_start().trim_end_matches(['\r', '\n']);
        if trimmed.chars().any(char::is_control) {
            return None;
        }
        let lower = trimmed.to_ascii_lowercase();
        let tail = if lower.starts_with(r"\\?\unc\") || lower.starts_with(r"\\.\unc\") {
            &trimmed[8..]
        } else if trimmed.starts_with(r"\\?\") || trimmed.starts_with(r"\\.\") {
            let rest = &trimmed[4..];
            let is_drive = rest.len() >= 2
                && rest.as_bytes()[0].is_ascii_alphabetic()
                && rest.as_bytes()[1] == b':';
            if is_drive {
                return None;
            }
            rest
        } else if trimmed.starts_with(r"\\") {
            trimmed.trim_start_matches('\\')
        } else {
            return None;
        };

        let mut clean_segments = Vec::new();
        for seg in tail.split(['\\', '/']) {
            if seg.is_empty() {
                continue;
            }
            if seg == "." || seg == ".." {
                return None;
            }
            clean_segments.push(seg.to_lowercase());
        }
        if clean_segments.len() < 2 {
            return None;
        }
        Some(format!(r"\\{}", clean_segments.join(r"\")))
    }

    fn has_trusted_unc_prefix(&self, raw: &str) -> bool {
        let Some(norm_unc) = Self::lexical_normalize_unc(raw) else {
            return false;
        };
        self.os_trusted_files
            .iter()
            .chain(self.os_trusted_dirs.iter())
            .any(|trusted| {
                let Some(trusted_unc) = Self::lexical_normalize_unc(&trusted.to_string_lossy())
                else {
                    return false;
                };
                norm_unc == trusted_unc || norm_unc.starts_with(&format!(r"{trusted_unc}\"))
            })
    }

    fn relative_dir_depth_if_clean(root_dir: &Path, target_dir: &Path) -> Option<usize> {
        let rel = target_dir.strip_prefix(root_dir).ok()?;
        let mut depth = 0_usize;
        for comp in rel.components() {
            match comp {
                Component::Normal(seg) => {
                    if seg.to_string_lossy().starts_with('.') {
                        return None;
                    }
                    depth = depth.saturating_add(1);
                }
                _ => return None,
            }
        }
        if depth <= MAX_ALLOWED_RELATIVE_DIR_DEPTH {
            Some(depth)
        } else {
            None
        }
    }

    fn is_clean_relative_file_within_depth(root_dir: &Path, target_file: &Path) -> bool {
        let Ok(rel) = target_file.strip_prefix(root_dir) else {
            return false;
        };
        if rel.file_name().is_none() {
            return false;
        }
        let Some(parent_rel) = rel.parent() else {
            return false;
        };
        Self::relative_dir_depth_if_clean(Path::new(""), parent_rel).is_some()
    }

    fn key_for_startup_path(raw: &str) -> Option<PathBuf> {
        let cleaned = raw.trim_start().trim_end_matches(['\r', '\n']);
        if cleaned.trim().is_empty() {
            return None;
        }
        let p = Path::new(cleaned);
        let resolved = if Self::is_unc_raw_path(cleaned) {
            strip_windows_verbatim_prefix(&normalize_folder_path(cleaned))
        } else {
            canonical_existing_path(p)
                .unwrap_or_else(|| strip_windows_verbatim_prefix(&normalize_folder_path(cleaned)))
        };
        Some(normalize_allowlist_key(&resolved))
    }

    fn key_for_trusted_sync(&self, raw: &str) -> Option<PathBuf> {
        let cleaned = raw.trim_start().trim_end_matches(['\r', '\n']);
        if cleaned.trim().is_empty() {
            return None;
        }
        if Self::is_unc_raw_path(cleaned) && !self.has_trusted_unc_prefix(cleaned) {
            return None;
        }
        let p = Path::new(cleaned);
        let resolved = canonical_existing_path(p)
            .unwrap_or_else(|| strip_windows_verbatim_prefix(&normalize_folder_path(cleaned)));
        Some(normalize_allowlist_key(&resolved))
    }

    fn register_file(&mut self, path: &Path) {
        if let Some(canon) = canonical_existing_path(path) {
            let key = normalize_allowlist_key(&canon);
            self.os_trusted_files.insert(key.clone());
            self.session_files.insert(key);
        }
    }

    fn register_dir(&mut self, path: &Path) {
        if let Some(canon) = canonical_existing_path(path) {
            let key = normalize_allowlist_key(&canon);
            self.os_trusted_dirs.insert(key.clone());
            self.session_dirs.insert(key);
        }
    }

    fn revoke_source_folder(&mut self, raw_path: &str) {
        let cleaned = raw_path.trim_start().trim_end_matches(['\r', '\n']);
        if cleaned.trim().is_empty() {
            return;
        }
        let lexical_key = normalize_allowlist_key(&strip_windows_verbatim_prefix(
            &normalize_folder_path(cleaned),
        ));
        self.os_trusted_dirs.remove(&lexical_key);
        self.session_dirs.remove(&lexical_key);
        self.db_enabled_dirs.remove(&lexical_key);
        self.db_all_source_dirs.remove(&lexical_key);

        if (!Self::is_unc_raw_path(cleaned) || self.has_trusted_unc_prefix(cleaned))
            && Path::new(cleaned).exists()
        {
            if let Some(canon) = canonical_existing_path(Path::new(cleaned)) {
                let canon_key = normalize_allowlist_key(&canon);
                self.os_trusted_dirs.remove(&canon_key);
                self.session_dirs.remove(&canon_key);
                self.db_enabled_dirs.remove(&canon_key);
                self.db_all_source_dirs.remove(&canon_key);
            }
        }
    }

    fn clear(&mut self) {
        self.os_trusted_files.clear();
        self.os_trusted_dirs.clear();
        self.session_files.clear();
        self.session_dirs.clear();
        self.db_standalone_files.clear();
        self.db_enabled_dirs.clear();
        self.db_all_source_dirs.clear();
    }

    fn seed_from_startup_state(&mut self, state: &audora_desktop::domain::RelationalAppStateDto) {
        for item in &state.media {
            if item.source_folder_id.is_none() {
                if let Some(key) = Self::key_for_startup_path(&item.path) {
                    self.os_trusted_files.insert(key);
                }
            }
        }
        for folder in &state.settings.source_folders {
            if let Some(key) = Self::key_for_startup_path(&folder.path) {
                self.os_trusted_dirs.insert(key.clone());
                self.db_all_source_dirs.insert(key);
            }
        }
        self.sync_from_relational_state(state);
    }

    fn sync_enabled_dirs(&mut self, folders: &[audora_desktop::domain::TrustedSourceFolder]) {
        let mut next_enabled = HashSet::new();
        let mut next_configured = HashSet::new();
        for folder in folders {
            let Some(key) = self.key_for_trusted_sync(&folder.path) else {
                continue;
            };
            let is_os_trusted = self.os_trusted_dirs.contains(&key)
                || self
                    .os_trusted_dirs
                    .iter()
                    .any(|d| Self::relative_dir_depth_if_clean(d, &key).is_some());
            if !is_os_trusted {
                continue;
            }
            next_configured.insert(key.clone());
            if folder.enabled {
                next_enabled.insert(key);
            } else {
                // Disabling a folder must also revoke any session-picked dir entry for it (`H-1`),
                // while retaining `os_trusted_dirs` so toggling it back on works cleanly.
                self.session_dirs.remove(&key);
            }
        }
        // Any source folder that was deleted from `settings.source_folders` is completely revoked,
        // including removing it from `os_trusted_dirs` (`H-1`).
        for old_configured in &self.db_all_source_dirs {
            if !next_configured.contains(old_configured) {
                self.os_trusted_dirs.remove(old_configured);
                self.session_dirs.remove(old_configured);
            }
        }
        for old_dir in &self.db_enabled_dirs {
            if !next_enabled.contains(old_dir) {
                self.session_dirs.remove(old_dir);
            }
        }
        self.db_all_source_dirs = next_configured;
        self.db_enabled_dirs = next_enabled;
    }

    fn sync_from_sources(
        &mut self,
        standalone_paths: &[String],
        source_folders: &[audora_desktop::domain::TrustedSourceFolder],
    ) {
        let mut next_standalone = HashSet::new();
        for raw_path in standalone_paths {
            // Only standalone media items go into `db_standalone_files`, and ONLY if previously
            // trusted via startup SQLite or an OS-mediated picker/drop (`H-1`).
            if let Some(key) = self.key_for_trusted_sync(raw_path) {
                if self.os_trusted_files.contains(&key)
                    || self
                        .os_trusted_dirs
                        .iter()
                        .any(|d| Self::is_clean_relative_file_within_depth(d, &key))
                {
                    next_standalone.insert(key);
                }
            }
        }
        for old_file in &self.db_standalone_files {
            if !next_standalone.contains(old_file) {
                self.session_files.remove(old_file);
            }
        }
        self.db_standalone_files = next_standalone;
        self.sync_enabled_dirs(source_folders);
    }

    #[cfg(test)]
    fn sync_from_relational_state(
        &mut self,
        state: &audora_desktop::domain::RelationalAppStateDto,
    ) {
        let standalone_paths: Vec<String> = state
            .media
            .iter()
            .filter(|m| m.source_folder_id.is_none())
            .map(|m| m.path.clone())
            .collect();
        self.sync_from_sources(&standalone_paths, &state.settings.source_folders);
    }

    #[cfg(not(test))]
    fn sync_from_relational_state(
        &mut self,
        state: &audora_desktop::domain::RelationalAppStateDto,
    ) {
        let standalone_paths: Vec<String> = state
            .media
            .iter()
            .filter(|m| m.source_folder_id.is_none())
            .map(|m| m.path.clone())
            .collect();
        self.sync_from_sources(&standalone_paths, &state.settings.source_folders);
    }

    fn allowed_scan_remaining_depth(&self, candidate: &Path) -> Option<usize> {
        let raw_lossy = candidate.to_string_lossy();
        if Self::is_unc_raw_path(&raw_lossy) && !self.has_trusted_unc_prefix(&raw_lossy) {
            return None;
        }
        let canon = canonical_existing_path(candidate)?;
        let key = normalize_allowlist_key(&canon);
        let min_depth = self
            .session_dirs
            .iter()
            .chain(self.db_enabled_dirs.iter())
            .filter_map(|d| Self::relative_dir_depth_if_clean(d, &key))
            .min()?;
        Some(MAX_ALLOWED_RELATIVE_DIR_DEPTH.saturating_sub(min_depth))
    }

    fn is_allowed_scan_dir(&self, candidate: &Path) -> bool {
        self.allowed_scan_remaining_depth(candidate).is_some()
    }

    fn resolve_allowed_media_file(&self, raw_path: &str) -> Option<PathBuf> {
        let cleaned = raw_path.trim_start().trim_end_matches(['\r', '\n']);
        if cleaned.trim().is_empty() {
            return None;
        }
        if Self::is_unc_raw_path(cleaned) && !self.has_trusted_unc_prefix(cleaned) {
            return None;
        }
        let candidate = Path::new(cleaned);
        if !candidate.is_absolute() && !cleaned.starts_with(r"\\") {
            return None;
        }
        let canon = canonical_existing_path(candidate)?;
        let meta = fs::metadata(&canon).ok()?;
        if !meta.is_file() {
            return None;
        }
        let ext = canon
            .extension()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .to_ascii_lowercase();
        if !is_supported_media_extension(&ext) {
            return None;
        }
        let key = normalize_allowlist_key(&canon);
        if self.session_files.contains(&key)
            || self.db_standalone_files.contains(&key)
            || self
                .session_dirs
                .iter()
                .chain(self.db_enabled_dirs.iter())
                .any(|d| Self::is_clean_relative_file_within_depth(d, &key))
        {
            Some(canon)
        } else {
            None
        }
    }
}

fn sync_allowlist_from_db(db: &Database, allowlist: &Mutex<MediaAllowlist>) {
    let sources = db.query_allowlist_sources().ok();
    if let Ok(mut guard) = allowlist.lock() {
        if let Some((standalone_paths, folders)) = sources {
            guard.sync_from_sources(&standalone_paths, &folders);
        } else {
            guard.db_standalone_files.clear();
            guard.db_enabled_dirs.clear();
        }
    }
}

fn apply_batch_source_revocations(ops: &[serde_json::Value], allowlist: &Mutex<MediaAllowlist>) {
    let revoked: Vec<&str> = ops
        .iter()
        .filter(|op_val| {
            op_val.get("op").and_then(serde_json::Value::as_str) == Some("revoke_source_folder")
        })
        .filter_map(|op_val| op_val.get("path").and_then(serde_json::Value::as_str))
        .collect();
    if !revoked.is_empty() {
        if let Ok(mut guard) = allowlist.lock() {
            for p in revoked {
                guard.revoke_source_folder(p);
            }
        }
    }
}

fn build_native_drop_descriptors(
    paths: &[PathBuf],
    allowlist: &Mutex<MediaAllowlist>,
) -> Vec<serde_json::Value> {
    let mut descriptors = Vec::new();
    for pb in paths {
        if pb.is_dir() {
            if let Ok(mut guard) = allowlist.lock() {
                guard.register_dir(pb);
            }
            let canon = canonical_path_string(pb);
            let folder_name = Path::new(&canon)
                .file_name()
                .and_then(|s| s.to_str())
                .filter(|s| !s.is_empty())
                .unwrap_or(&canon)
                .to_owned();
            descriptors.push(json!({
                "kind": "dir",
                "path": canon,
                "name": folder_name,
                "size": 0,
                "duration_ms": serde_json::Value::Null,
            }));
        } else if pb.is_file() {
            if let Ok(meta) = fs::metadata(pb) {
                let size = meta.len();
                let ext = pb
                    .extension()
                    .and_then(|s| s.to_str())
                    .unwrap_or("")
                    .to_ascii_lowercase();
                let dur = if size > 0 && is_supported_media_extension(&ext) {
                    if let Ok(mut guard) = allowlist.lock() {
                        guard.register_file(pb);
                    }
                    probe_media_duration_ms(pb)
                } else {
                    None
                };
                let canon = canonical_path_string(pb);
                let name = pb
                    .file_name()
                    .and_then(|s| s.to_str())
                    .unwrap_or("dropped")
                    .to_owned();
                descriptors.push(json!({
                    "kind": "file",
                    "path": canon,
                    "name": name,
                    "size": size,
                    "duration_ms": dur,
                }));
            }
        }
    }
    descriptors
}

struct ConnectionSlotGuard {
    counter: Arc<AtomicUsize>,
}

impl Drop for ConnectionSlotGuard {
    fn drop(&mut self) {
        self.counter.fetch_sub(1, Ordering::AcqRel);
    }
}

struct SingleFlightGuard<'a> {
    flag: &'a AtomicBool,
}

impl<'a> SingleFlightGuard<'a> {
    fn try_acquire(flag: &'a AtomicBool) -> Option<Self> {
        flag.compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .ok()
            .map(|_| Self { flag })
    }
}

impl Drop for SingleFlightGuard<'_> {
    fn drop(&mut self) {
        self.flag.store(false, Ordering::Release);
    }
}

fn generate_session_token() -> String {
    let mut hasher = Sha256::new();
    let mut csprng_bytes = [0_u8; 32];
    if getrandom::getrandom(&mut csprng_bytes).is_err() {
        if let Ok(mut f) = File::open("/dev/urandom") {
            let _ = f.read_exact(&mut csprng_bytes);
        }
    }
    hasher.update(csprng_bytes);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    hasher.update(nanos.to_le_bytes());
    hasher.update(std::process::id().to_le_bytes());
    format!("{:x}", hasher.finalize())
}

fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0_u8;
    for (&x, &y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .with_target(false)
        .init();

    let args: Vec<String> = env::args().collect();

    if args.iter().any(|arg| arg == "--uninstall-cleanup") {
        let data_dir = default_app_data_dir();
        match cleanup_all_app_artifacts(&data_dir) {
            Ok(summary) => {
                info!(
                    removed_count = summary.removed_paths.len(),
                    bytes_reclaimed = summary.bytes_reclaimed,
                    "Completed Audora uninstall cleanup"
                );
                return;
            }
            Err(err) => {
                error!(
                    code = err.code().as_str(),
                    "Audora uninstall cleanup failed"
                );
                std::process::exit(1);
            }
        }
    }

    let db_path = default_database_path();
    let (dispatcher, db) = match build_production_dispatcher_with_db(&db_path) {
        Ok((d, database)) => (Arc::new(d), database),
        Err(err) => {
            error!(
                code = err.code().as_str(),
                "Audora failed to initialize persistence core"
            );
            std::process::exit(1);
        }
    };

    // One-time seamless migration of legacy `ui-state.json` into `audora.sqlite`
    let app_data_dir = default_app_data_dir();
    let legacy_state_file = app_data_dir.join("ui-state.json");
    if legacy_state_file.is_file() {
        if let Ok(None) = db.get_ui_state_json() {
            if let Ok(raw) = fs::read_to_string(&legacy_state_file) {
                if serde_json::from_str::<serde_json::Value>(&raw).is_ok_and(|v| v.is_object())
                    && db.set_ui_state_json(&raw).is_ok()
                {
                    let _ = fs::remove_file(&legacy_state_file);
                }
            }
        } else {
            let _ = fs::remove_file(&legacy_state_file);
        }
    }

    let allowlist = Arc::new(Mutex::new(MediaAllowlist::default()));
    let initial_saved_state = db.export_relational_state().ok().flatten();
    let initial_window_mode = initial_saved_state
        .as_ref()
        .map_or(WindowMode::Full, |s| s.settings.window_mode);
    let initial_theme = initial_saved_state
        .as_ref()
        .map_or(ThemeMode::Dark, |s| s.settings.theme);
    if let Some(ref saved_state) = initial_saved_state {
        if let Ok(mut guard) = allowlist.lock() {
            guard.seed_from_startup_state(saved_state);
        }
    }

    let raw_cli_args: Vec<String> = args
        .iter()
        .skip(1)
        .filter(|arg| !arg.starts_with('-'))
        .cloned()
        .collect();
    let is_headless_check = args.iter().any(|arg| arg == "--headless-check");
    if is_headless_check {
        if !raw_cli_args.is_empty() {
            let canonical_cli_paths: Vec<String> = raw_cli_args
                .iter()
                .map(|raw| canonical_path_string(Path::new(raw)))
                .collect();
            if let Err(err) = dispatcher.open_media_batch(&canonical_cli_paths) {
                warn!(
                    code = err.code.as_str(),
                    "Failed to open CLI media arguments"
                );
            }
        }
        let _ = db.checkpoint_wal();
        info!(
            version = env!("CARGO_PKG_VERSION"),
            verified = true,
            "Audora headless verification succeeded"
        );
        return;
    }

    let mut cli_boot_files: Vec<(String, String, u64, Option<u64>)> = Vec::new();
    if !raw_cli_args.is_empty() {
        for raw in &raw_cli_args {
            if cli_boot_files.len() >= 200 {
                break;
            }
            let pb = PathBuf::from(canonical_path_string(Path::new(raw)));
            if pb.is_dir() {
                collect_media_files_in_dir(&pb, 0, 0, 200, &mut cli_boot_files);
                if let Ok(mut guard) = allowlist.lock() {
                    guard.register_dir(&pb);
                }
            } else if pb.is_file() {
                if let Ok(meta) = fs::metadata(&pb) {
                    if meta.len() > 0 {
                        let ext = pb
                            .extension()
                            .and_then(|s| s.to_str())
                            .unwrap_or("")
                            .to_ascii_lowercase();
                        if is_supported_media_extension(&ext) {
                            if let Ok(mut guard) = allowlist.lock() {
                                guard.register_file(&pb);
                            }
                            let name = pb
                                .file_name()
                                .and_then(|s| s.to_str())
                                .unwrap_or(raw.as_str())
                                .to_owned();
                            let dur = probe_media_duration_ms(&pb);
                            cli_boot_files.push((
                                canonical_path_string(&pb),
                                name,
                                meta.len(),
                                dur,
                            ));
                        }
                    }
                }
            }
        }
    }

    let ui_dist = match resolve_ui_dist_dir() {
        Some(dir) => dir,
        None => {
            error!(
                "Could not locate bundled UI assets (`apps/desktop/ui/dist`). Run `pnpm --dir apps/desktop/ui build` first."
            );
            std::process::exit(1);
        }
    };

    let event_loop = EventLoopBuilder::<DesktopUserEvent>::with_user_event().build();
    let proxy = event_loop.create_proxy();
    let session_token = Arc::new(generate_session_token());
    let exit_db = db.clone();

    let Some((server_url, server_port, active_http_connections)) = start_local_ui_server(
        ui_dist,
        proxy.clone(),
        Arc::clone(&dispatcher),
        db,
        Arc::clone(&session_token),
        Arc::clone(&allowlist),
    ) else {
        error!("Failed to bind local UI loopback server");
        std::process::exit(1);
    };

    info!(url = %server_url, "Launching Audora desktop window");

    let start_in_minimum = initial_window_mode == WindowMode::Minimum;
    let window_builder = if start_in_minimum {
        let min_sz = LogicalSize::new(MINIMUM_WIDGET_WIDTH, MINIMUM_WIDGET_HEIGHT);
        WindowBuilder::new()
            .with_title("Audora")
            .with_decorations(false)
            .with_resizable(false)
            .with_always_on_top(true)
            .with_inner_size(min_sz)
            .with_min_inner_size(min_sz)
    } else {
        WindowBuilder::new()
            .with_title("Audora")
            .with_resizable(true)
            .with_inner_size(LogicalSize::new(DEFAULT_FULL_WIDTH, DEFAULT_FULL_HEIGHT))
            .with_min_inner_size(LogicalSize::new(MIN_FULL_WIDTH, MIN_FULL_HEIGHT))
    };

    let window = match window_builder.build(&event_loop) {
        Ok(win) => win,
        Err(err) => {
            error!(error = %err, "Failed to create native OS window");
            std::process::exit(1);
        }
    };
    if start_in_minimum {
        position_minimum_widget_bottom_right(&window);
    } else {
        apply_full_window_bounds_and_center(
            &window,
            LogicalSize::new(DEFAULT_FULL_WIDTH, DEFAULT_FULL_HEIGHT),
        );
    }

    let cli_files_val: Vec<serde_json::Value> = cli_boot_files
        .iter()
        .map(|(p, name, sz, dur)| {
            json!({
                "path": p,
                "name": name,
                "size": sz,
                "duration_ms": dur,
            })
        })
        .collect();
    let theme_str = match initial_theme {
        ThemeMode::Light => "light",
        ThemeMode::System => "system",
        ThemeMode::Dark => "dark",
    };
    let mode_str = if start_in_minimum { "minimum" } else { "full" };
    let platform_json = serde_json::to_string(&json!({
        "os": env::consts::OS,
        "arch": env::consts::ARCH,
        "app_version": env!("CARGO_PKG_VERSION"),
        "data_dir": app_data_dir.display().to_string(),
        "cli_files": cli_files_val,
        "initial_theme": theme_str,
        "initial_window_mode": mode_str,
    }))
    .unwrap_or_else(|_| "{}".to_owned());

    let expected_origin = format!("http://127.0.0.1:{server_port}");
    let bg_color = if initial_theme == ThemeMode::Light {
        (245, 248, 247, 255)
    } else {
        (16, 21, 19, 255)
    };

    let init_script = format!(
        r#"
      (function() {{
        if (window.top !== window || window.location.origin !== '{expected_origin}') {{
          return;
        }}
        Object.defineProperty(window, '__AUDORA_SESSION_TOKEN__', {{
          value: '{session_token}',
          writable: false,
          configurable: false
        }});
        Object.defineProperty(window, '__AUDORA_PLATFORM__', {{
          value: Object.freeze({platform_json}),
          writable: false,
          configurable: false
        }});
        window.__AUDORA_REQUEST_QUIT__ = function() {{
          try {{
            if (typeof window.__AUDORA_COLLECT_QUIT_BATCH__ === 'function') {{
              var batchJson = window.__AUDORA_COLLECT_QUIT_BATCH__();
              if (window.ipc && typeof window.ipc.postMessage === 'function') {{
                window.ipc.postMessage('quit_with_batch:' + (batchJson || '[]'));
                return;
              }}
            }}
          }} catch (_) {{}}
          if (window.ipc && typeof window.ipc.postMessage === 'function') {{
            window.ipc.postMessage('quit_with_batch:[]');
          }}
        }};
        window.close = function() {{
          window.__AUDORA_REQUEST_QUIT__();
        }};
        window.addEventListener('keydown', function(e) {{
          if ((e.metaKey || e.ctrlKey) && !e.altKey && e.key) {{
            var k = e.key.toLowerCase();
            if (k === 'q') {{
              e.preventDefault();
              window.__AUDORA_REQUEST_QUIT__();
              return;
            }}
            if ('{os_const}' === 'macos' && e.metaKey && !e.ctrlKey) {{
              var el = document.activeElement;
              var isEditable = el && (el.tagName === 'INPUT' || el.tagName === 'TEXTAREA' || el.isContentEditable);
              if (k === 'a' && isEditable) {{
                if (typeof el.select === 'function') {{ e.preventDefault(); el.select(); }}
                else {{ document.execCommand('selectAll'); }}
              }} else if (k === 'c' && (isEditable || String(window.getSelection ? window.getSelection() : '').length > 0)) {{
                document.execCommand('copy');
              }} else if (k === 'x' && isEditable) {{
                document.execCommand('cut');
              }} else if (k === 'v' && isEditable) {{
                document.execCommand('paste');
              }} else if (k === 'z' && isEditable) {{
                document.execCommand(e.shiftKey ? 'redo' : 'undo');
              }}
            }}
          }}
        }});
      }})();
    "#,
        os_const = env::consts::OS
    );

    let webview_data_dir = app_data_dir.join("webview");
    let _ = fs::create_dir_all(&webview_data_dir);
    let mut web_context = WebContext::new(Some(webview_data_dir));

    let allowed_ip_prefix = format!("http://127.0.0.1:{server_port}/");
    let ipc_proxy = proxy.clone();
    let drop_proxy = proxy.clone();
    let load_proxy = proxy.clone();
    let webview = match WebViewBuilder::new_with_web_context(&mut web_context)
        .with_url(&server_url)
        .with_initialization_script_for_main_only(&init_script, true)
        .with_on_page_load_handler(move |event, _| {
            if matches!(event, PageLoadEvent::Finished) {
                let _ = load_proxy.send_event(DesktopUserEvent::WebViewPageLoaded);
            }
        })
        .with_ipc_handler(move |request| {
            let body = request.body();
            if body == "quit" {
                let _ = ipc_proxy.send_event(DesktopUserEvent::RequestGracefulClose);
            } else if let Some(batch) = body.strip_prefix("quit_with_batch:") {
                let opt = if batch.trim().is_empty() || batch.trim() == "[]" {
                    None
                } else {
                    Some(batch.to_owned())
                };
                let _ = ipc_proxy.send_event(DesktopUserEvent::QuitWithBatch(opt));
            } else if body == "window_mode:minimum" {
                let _ = ipc_proxy.send_event(DesktopUserEvent::SetWindowMode { minimum: true });
            } else if body == "window_mode:full" {
                let _ = ipc_proxy.send_event(DesktopUserEvent::SetWindowMode { minimum: false });
            } else if body == "drag_window" {
                let _ = ipc_proxy.send_event(DesktopUserEvent::DragWindow);
            }
        })
        .with_drag_drop_handler(move |event| {
            match event {
                DragDropEvent::Enter { paths, .. } => {
                    if !paths.is_empty() {
                        let _ = drop_proxy.send_event(DesktopUserEvent::NativeDragState(true));
                    }
                }
                DragDropEvent::Leave => {
                    let _ = drop_proxy.send_event(DesktopUserEvent::NativeDragState(false));
                }
                DragDropEvent::Drop { paths, .. } => {
                    let _ = drop_proxy.send_event(DesktopUserEvent::NativeDragState(false));
                    if !paths.is_empty() {
                        let _ = drop_proxy.send_event(DesktopUserEvent::NativeFilesDropped(paths));
                    }
                }
                DragDropEvent::Over { .. } => {}
                #[allow(unreachable_patterns)]
                _ => {}
            }
            true
        })
        .with_new_window_req_handler(|uri: String, _| {
            if uri.starts_with("https://github.com/smokeInCloud060201/audora/")
                && is_safe_external_https_url(&uri)
            {
                open_external_https_url(&uri);
            }
            NewWindowResponse::Deny
        })
        .with_background_color(bg_color)
        .with_background_throttling(BackgroundThrottlingPolicy::Disabled)
        .with_clipboard(true)
        .with_devtools(cfg!(debug_assertions))
        .with_navigation_handler(move |uri: String| {
            uri.starts_with(&allowed_ip_prefix)
                || uri.starts_with("about:blank")
                || uri.starts_with("blob:")
        })
        .build(&window)
    {
        Ok(view) => view,
        Err(err) => {
            error!(error = %err, "Failed to initialize native WebView");
            std::process::exit(1);
        }
    };

    let mut webview = Some(webview);
    let mut web_context = Some(web_context);
    let mut window = Some(window);
    let mut saved_full_size = LogicalSize::new(DEFAULT_FULL_WIDTH, DEFAULT_FULL_HEIGHT);
    let mut saved_full_pos: Option<PhysicalPosition<i32>> = None;
    let mut is_minimum_mode = start_in_minimum;
    let mut close_initiated = false;
    let mut quit_completed = false;
    let mut webview_ready = false;
    let mut pending_opened_paths: Vec<PathBuf> = Vec::new();
    let drop_allowlist = Arc::clone(&allowlist);
    let close_fallback_proxy = proxy.clone();
    let opened_proxy = proxy.clone();

    event_loop.run(move |event, _, control_flow| {
        *control_flow = ControlFlow::Wait;

        match event {
            Event::Opened { urls } => {
                let paths: Vec<PathBuf> = urls
                    .into_iter()
                    .filter(|u| u.scheme() == "file")
                    .filter_map(|u| u.to_file_path().ok())
                    .collect();
                if !paths.is_empty() {
                    if webview_ready {
                        let _ = opened_proxy.send_event(DesktopUserEvent::NativeFilesDropped(paths));
                    } else {
                        pending_opened_paths.extend(paths);
                    }
                }
            }
            Event::UserEvent(DesktopUserEvent::WebViewPageLoaded) => {
                webview_ready = true;
                if !pending_opened_paths.is_empty() {
                    let queued = std::mem::take(&mut pending_opened_paths);
                    let _ = opened_proxy.send_event(DesktopUserEvent::NativeFilesDropped(queued));
                }
            }
            Event::WindowEvent {
                event: WindowEvent::CloseRequested,
                ..
            }
            | Event::UserEvent(DesktopUserEvent::RequestGracefulClose) => {
                if !close_initiated {
                    close_initiated = true;
                    if let Some(wv) = webview.as_ref() {
                        if wv
                            .evaluate_script(
                                "window.__AUDORA_REQUEST_QUIT__ && window.__AUDORA_REQUEST_QUIT__()",
                            )
                            .is_ok()
                        {
                            let fp = close_fallback_proxy.clone();
                            thread::spawn(move || {
                                thread::sleep(Duration::from_millis(1200));
                                let _ = fp.send_event(DesktopUserEvent::QuitWithBatch(None));
                            });
                            return;
                        }
                    }
                }
                let _ = close_fallback_proxy.send_event(DesktopUserEvent::QuitWithBatch(None));
            }
            Event::WindowEvent {
                event: WindowEvent::Destroyed,
                ..
            }
            | Event::LoopDestroyed => {
                drop(webview.take());
                drop(web_context.take());
                drop(window.take());
                let _ = exit_db.checkpoint_wal();
                *control_flow = ControlFlow::Exit;
            }
            Event::UserEvent(DesktopUserEvent::QuitWithBatch(batch_json)) => {
                if quit_completed {
                    return;
                }
                quit_completed = true;
                // Drain active loopback HTTP connections FIRST (up to 500ms) so stale in-flight
                // HTTP requests never overwrite the final shutdown batch (`C-8`).
                let drain_deadline = Instant::now() + Duration::from_millis(500);
                while active_http_connections.load(Ordering::Acquire) > 0
                    && Instant::now() < drain_deadline
                {
                    thread::sleep(Duration::from_millis(10));
                }
                if let Some(raw_batch) = batch_json {
                    if let Ok(ops) = serde_json::from_str::<Vec<serde_json::Value>>(&raw_batch) {
                        if exit_db.rpc_execute_batch(&ops).is_ok() {
                            apply_batch_source_revocations(&ops, &drop_allowlist);
                        }
                    }
                }
                drop(webview.take());
                drop(web_context.take());
                drop(window.take());
                let _ = exit_db.checkpoint_wal();
                *control_flow = ControlFlow::Exit;
            }
            Event::UserEvent(DesktopUserEvent::NativeDragState(active)) => {
                if let Some(wv) = webview.as_ref() {
                    let js = format!(
                        "window.__AUDORA_ON_DRAG_STATE__ && window.__AUDORA_ON_DRAG_STATE__({active});"
                    );
                    let _ = wv.evaluate_script(&js);
                }
            }
            Event::UserEvent(DesktopUserEvent::NativeFilesDropped(paths)) => {
                let descriptors = build_native_drop_descriptors(&paths, &drop_allowlist);
                if !descriptors.is_empty() {
                    if let Some(wv) = webview.as_ref() {
                        if let Ok(serialized) = serde_json::to_string(&descriptors) {
                            let js = format!(
                                "(function dispatchDrop(items, tries) {{ if (typeof window.__AUDORA_ON_NATIVE_DROP__ === 'function') {{ window.__AUDORA_ON_NATIVE_DROP__(items); }} else if (tries > 0) {{ setTimeout(function() {{ dispatchDrop(items, tries - 1); }}, 50); }} }})({serialized}, 40);"
                            );
                            let _ = wv.evaluate_script(&js);
                        }
                    }
                }
            }
            Event::UserEvent(DesktopUserEvent::SetWindowMode { minimum }) => {
                if let Some(win) = window.as_ref() {
                    if minimum && !is_minimum_mode {
                        let curr = win.inner_size().to_logical::<f64>(win.scale_factor());
                        if curr.width >= MIN_FULL_WIDTH && curr.height >= MIN_FULL_HEIGHT {
                            saved_full_size = curr;
                        }
                        saved_full_pos = win.outer_position().ok();
                        is_minimum_mode = true;
                        let min_sz = LogicalSize::new(MINIMUM_WIDGET_WIDTH, MINIMUM_WIDGET_HEIGHT);
                        win.set_min_inner_size(Some(min_sz));
                        win.set_decorations(false);
                        win.set_resizable(false);
                        win.set_always_on_top(true);
                        win.set_inner_size(min_sz);
                        position_minimum_widget_bottom_right(win);
                    } else if !minimum && is_minimum_mode {
                        is_minimum_mode = false;
                        win.set_always_on_top(false);
                        win.set_decorations(true);
                        win.set_resizable(true);
                        if let Some(prev_pos) = saved_full_pos {
                            win.set_min_inner_size(Some(LogicalSize::new(
                                MIN_FULL_WIDTH,
                                MIN_FULL_HEIGHT,
                            )));
                            win.set_inner_size(saved_full_size);
                            win.set_outer_position(prev_pos);
                        } else {
                            apply_full_window_bounds_and_center(win, saved_full_size);
                        }
                    }
                }
            }
            Event::UserEvent(DesktopUserEvent::DragWindow) => {
                if let Some(win) = window.as_ref() {
                    let _ = win.drag_window();
                }
            }
            _ => {}
        }
    });
}

fn is_safe_external_https_url(url: &str) -> bool {
    if !url.starts_with("https://") {
        return false;
    }
    !url.chars()
        .any(|c| c.is_whitespace() || c.is_control() || matches!(c, '"' | '\'' | '`' | ','))
}

fn open_external_https_url(url: &str) {
    if !is_safe_external_https_url(url) {
        return;
    }
    if cfg!(target_os = "macos") {
        let _ = Command::new("/usr/bin/open").arg(url).spawn();
    } else if cfg!(target_os = "windows") {
        let mut cmd = Command::new("rundll32.exe");
        configure_hidden_windows_command(&mut cmd);
        let _ = cmd.arg("url.dll,FileProtocolHandler").arg(url).spawn();
    } else {
        let _ = Command::new("xdg-open").arg(url).spawn();
    }
}

fn apply_full_window_bounds_and_center(win: &Window, desired_logical: LogicalSize<f64>) {
    let Some(monitor) = win.current_monitor().or_else(|| win.primary_monitor()) else {
        win.set_min_inner_size(Some(LogicalSize::new(MIN_FULL_WIDTH, MIN_FULL_HEIGHT)));
        win.set_inner_size(desired_logical);
        return;
    };

    let scale = monitor.scale_factor().max(1.0);
    let mon_pos = monitor.position();
    let mon_phys = monitor.size();
    let mon_logical_w = f64::from(mon_phys.width) / scale;
    let mon_logical_h = f64::from(mon_phys.height) / scale;

    let max_fit_w = (mon_logical_w - 32.0).max(640.0);
    let max_fit_h = (mon_logical_h - 84.0).max(480.0);
    let min_w = MIN_FULL_WIDTH.min(max_fit_w);
    let min_h = MIN_FULL_HEIGHT.min(max_fit_h);

    let target_w = desired_logical.width.clamp(min_w, max_fit_w);
    let target_h = desired_logical.height.clamp(min_h, max_fit_h);

    win.set_min_inner_size(Some(LogicalSize::new(min_w, min_h)));
    win.set_inner_size(LogicalSize::new(target_w, target_h));

    let outer_w_phys = (target_w * scale).round() as i64;
    let outer_h_phys = ((target_h + 28.0) * scale).round() as i64;
    let mon_w_phys = i64::from(mon_phys.width);
    let mon_h_phys = i64::from(mon_phys.height);

    let x = i64::from(mon_pos.x) + ((mon_w_phys - outer_w_phys).max(0) / 2);
    let y = i64::from(mon_pos.y) + (((mon_h_phys - outer_h_phys).max(0) * 45) / 100);
    if let (Ok(px), Ok(py)) = (i32::try_from(x), i32::try_from(y)) {
        win.set_outer_position(PhysicalPosition::new(px, py));
    }
}

fn position_minimum_widget_bottom_right(win: &Window) {
    let Some(monitor) = win.current_monitor().or_else(|| win.primary_monitor()) else {
        return;
    };
    let scale = monitor.scale_factor().max(1.0);
    let mon_pos = monitor.position();
    let mon_phys = monitor.size();

    let widget_w = (MINIMUM_WIDGET_WIDTH * scale).round() as i64;
    let widget_h = (MINIMUM_WIDGET_HEIGHT * scale).round() as i64;
    let margin_right = (24.0 * scale).round() as i64;
    let margin_bottom = (64.0 * scale).round() as i64;

    let right_edge = i64::from(mon_pos.x) + i64::from(mon_phys.width);
    let bottom_edge = i64::from(mon_pos.y) + i64::from(mon_phys.height);

    let x = (right_edge - widget_w - margin_right).max(i64::from(mon_pos.x));
    let y = (bottom_edge - widget_h - margin_bottom).max(i64::from(mon_pos.y));
    if let (Ok(px), Ok(py)) = (i32::try_from(x), i32::try_from(y)) {
        win.set_outer_position(PhysicalPosition::new(px, py));
    }
}

fn resolve_ui_dist_dir() -> Option<PathBuf> {
    if let Ok(exe_path) = env::current_exe() {
        if let Some(exe_dir) = exe_path.parent() {
            let candidates = [
                // macOS `.app` bundle layout: `Audora.app/Contents/MacOS/audora` -> `../Resources/ui`
                exe_dir.join("../Resources/ui"),
                // Windows / Linux portable layout: `<dir>/ui/index.html` or `../share/audora/ui/index.html`
                exe_dir.join("ui"),
                exe_dir.join("../share/audora/ui"),
                PathBuf::from("/usr/share/audora/ui"),
            ];
            for candidate in candidates {
                if candidate.join("index.html").is_file() {
                    return Some(candidate);
                }
            }
        }
    }

    #[cfg(debug_assertions)]
    {
        let manifest_ui = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../ui/dist");
        if manifest_ui.join("index.html").is_file() {
            return Some(manifest_ui);
        }

        if let Ok(cwd) = env::current_dir() {
            let cwd_ui = cwd.join("apps/desktop/ui/dist");
            if cwd_ui.join("index.html").is_file() {
                return Some(cwd_ui);
            }
        }
    }

    None
}

struct ParsedHttpRequest {
    method: String,
    raw_target: String,
    host: Option<String>,
    range_header: Option<String>,
    origin: Option<String>,
    sec_fetch_site: Option<String>,
    auth_token: Option<String>,
    body: Vec<u8>,
}

struct LoopbackServerContext {
    root: PathBuf,
    proxy: EventLoopProxy<DesktopUserEvent>,
    dispatcher: Arc<CommandDispatcher>,
    db: Database,
    local_port: u16,
    session_token: Arc<String>,
    allowlist: Arc<Mutex<MediaAllowlist>>,
    dialog_in_flight: AtomicBool,
    update_in_flight: AtomicBool,
}

fn start_local_ui_server(
    ui_dist: PathBuf,
    proxy: EventLoopProxy<DesktopUserEvent>,
    dispatcher: Arc<CommandDispatcher>,
    db: Database,
    session_token: Arc<String>,
    allowlist: Arc<Mutex<MediaAllowlist>>,
) -> Option<(String, u16, Arc<AtomicUsize>)> {
    // Bind OS-assigned ephemeral IPv4 loopback port (`127.0.0.1:0`) (`C-4` / `M-2`).
    let listener = TcpListener::bind(("127.0.0.1", 0)).ok()?;
    let local_addr = listener.local_addr().ok()?;
    let local_port = local_addr.port();
    let url = format!("http://127.0.0.1:{local_port}/");
    let active_connections = Arc::new(AtomicUsize::new(0));
    let active_out = Arc::clone(&active_connections);
    let ctx = Arc::new(LoopbackServerContext {
        root: ui_dist,
        proxy,
        dispatcher,
        db,
        local_port,
        session_token,
        allowlist,
        dialog_in_flight: AtomicBool::new(false),
        update_in_flight: AtomicBool::new(false),
    });

    thread::spawn(move || {
        for mut stream in listener.incoming().flatten() {
            let prev = active_connections.fetch_add(1, Ordering::AcqRel);
            if prev >= MAX_CONCURRENT_HTTP_CONNECTIONS {
                active_connections.fetch_sub(1, Ordering::AcqRel);
                let _ = stream.write_all(
                    b"HTTP/1.1 503 Service Unavailable\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                );
                continue;
            }
            let slot_guard = ConnectionSlotGuard {
                counter: Arc::clone(&active_connections),
            };
            let conn_ctx = Arc::clone(&ctx);
            thread::spawn(move || {
                let _guard = slot_guard;
                handle_http_connection(stream, &conn_ctx);
            });
        }
    });

    Some((url, local_port, active_out))
}

fn read_http_request(stream: &mut TcpStream) -> Option<ParsedHttpRequest> {
    let deadline = Instant::now() + Duration::from_secs(HTTP_REQUEST_DEADLINE_SECS);
    let _ = stream.set_write_timeout(Some(Duration::from_secs(30)));

    let mut header_buf = Vec::with_capacity(4096);
    let mut chunk = [0_u8; 4096];
    let header_end = loop {
        let now = Instant::now();
        if now >= deadline {
            return None;
        }
        let _ = stream.set_read_timeout(Some(deadline.saturating_duration_since(now)));
        let n = stream.read(&mut chunk).ok()?;
        if n == 0 {
            return None;
        }
        header_buf.extend_from_slice(&chunk[..n]);
        if let Some(pos) = header_buf.windows(4).position(|w| w == b"\r\n\r\n") {
            break pos + 4;
        }
        if header_buf.len() >= 16_384 {
            return None;
        }
    };

    let header_str = String::from_utf8_lossy(&header_buf[..header_end]);
    let mut lines = header_str.lines();
    let request_line = lines.next()?;
    let mut parts = request_line.split_whitespace();
    let method = parts.next()?.to_ascii_uppercase();
    let raw_target = parts.next()?.to_owned();

    let mut content_length = 0_usize;
    let mut seen_content_length = false;
    let mut host: Option<String> = None;
    let mut range_header: Option<String> = None;
    let mut origin: Option<String> = None;
    let mut sec_fetch_site: Option<String> = None;
    let mut auth_token: Option<String> = None;
    for line in lines {
        if let Some((k, v)) = line.split_once(':') {
            let key = k.trim();
            if key.eq_ignore_ascii_case("content-length") {
                if seen_content_length {
                    // Reject duplicate Content-Length smuggling attempts (`H-7`).
                    return None;
                }
                seen_content_length = true;
                content_length = v.trim().parse::<usize>().ok()?;
            } else if key.eq_ignore_ascii_case("host") {
                if host.is_some() {
                    // Reject duplicate Host headers (`H-7`).
                    return None;
                }
                host = Some(v.trim().to_owned());
            } else if key.eq_ignore_ascii_case("range") {
                range_header = Some(v.trim().to_owned());
            } else if key.eq_ignore_ascii_case("origin") {
                origin = Some(v.trim().to_owned());
            } else if key.eq_ignore_ascii_case("sec-fetch-site") {
                sec_fetch_site = Some(v.trim().to_ascii_lowercase());
            } else if key.eq_ignore_ascii_case("transfer-encoding") {
                // Reject Transfer-Encoding headers to block CL.TE / TE.CL HTTP smuggling.
                return None;
            } else if key.eq_ignore_ascii_case("x-audora-token") {
                auth_token = Some(v.trim().to_owned());
            }
        }
    }

    if content_length > MAX_HTTP_BODY_BYTES {
        return None;
    }

    let mut body = Vec::with_capacity(content_length.min(65_536));
    if header_buf.len() > header_end {
        body.extend_from_slice(&header_buf[header_end..]);
    }
    while body.len() < content_length {
        let now = Instant::now();
        if now >= deadline {
            return None;
        }
        let _ = stream.set_read_timeout(Some(deadline.saturating_duration_since(now)));
        let n = stream.read(&mut chunk).ok()?;
        if n == 0 {
            return None;
        }
        body.extend_from_slice(&chunk[..n]);
    }
    body.truncate(content_length);

    Some(ParsedHttpRequest {
        method,
        raw_target,
        host,
        range_header,
        origin,
        sec_fetch_site,
        auth_token,
        body,
    })
}

fn is_valid_host_header(local_port: u16, host: Option<&str>) -> bool {
    let Some(h) = host else {
        return false;
    };
    let expected_ip = format!("127.0.0.1:{local_port}");
    h.eq_ignore_ascii_case(&expected_ip)
}

fn is_trusted_loopback_request(
    local_port: u16,
    origin: Option<&str>,
    sec_fetch_site: Option<&str>,
    provided_token: Option<&str>,
    expected_token: &str,
) -> bool {
    if let Some(site) = sec_fetch_site {
        if site != "same-origin" && site != "none" {
            return false;
        }
    }
    if let Some(orig) = origin {
        let trimmed = orig.trim();
        if trimmed.eq_ignore_ascii_case("null") {
            return false;
        }
        if !trimmed.is_empty() {
            let expected_ip = format!("http://127.0.0.1:{local_port}");
            if !trimmed.eq_ignore_ascii_case(&expected_ip) {
                return false;
            }
        }
    }
    let Some(tok) = provided_token else {
        return false;
    };
    constant_time_eq(tok.as_bytes(), expected_token.as_bytes())
}

fn write_json_response(stream: &mut TcpStream, status: &str, body: &[u8]) {
    let header = format!(
        "HTTP/1.1 {status}\r\nContent-Type: application/json; charset=utf-8\r\nX-Content-Type-Options: nosniff\r\nContent-Length: {}\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n",
        body.len()
    );
    let _ = stream.write_all(header.as_bytes());
    let _ = stream.write_all(body);
    let _ = stream.flush();
}

fn write_json_value(stream: &mut TcpStream, status: &str, val: &serde_json::Value) {
    let bytes = serde_json::to_vec(val).unwrap_or_else(|_| br#"{"ok":false}"#.to_vec());
    write_json_response(stream, status, &bytes);
}

fn is_safe_semver(v: &str) -> bool {
    !v.is_empty()
        && v.len() <= 32
        && v.chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-' || c == '_')
}

fn is_valid_sha256_hex(v: &str) -> bool {
    v.len() == 64 && v.chars().all(|c| c.is_ascii_hexdigit())
}

fn handle_http_connection(mut stream: TcpStream, ctx: &LoopbackServerContext) {
    let Some(req) = read_http_request(&mut stream) else {
        return;
    };

    if !is_valid_host_header(ctx.local_port, req.host.as_deref()) {
        write_json_response(
            &mut stream,
            "403 Forbidden",
            br#"{"ok":false,"message":"Invalid Host header rejected."}"#,
        );
        return;
    }

    let method = req.method;
    let raw_target = req.raw_target;
    let range_header = req.range_header;
    let req_body = req.body;

    let (raw_path, raw_query) = raw_target
        .split_once('?')
        .map_or((raw_target.as_str(), ""), |(p, q)| (p, q));

    let mut query_token: Option<String> = None;
    for pair in raw_query.split('&') {
        if let Some((k, v)) = pair.split_once('=') {
            if k == "token" {
                query_token = Some(percent_decode(v));
            }
        }
    }

    if raw_path.starts_with("/__audora_") {
        // Restrict query-string `?token=` strictly to `/__audora_media` (`<video>`/`<audio>` `src`);
        // all state/RPC/dialog/update endpoints require `X-Audora-Token` header (`M-2`).
        let effective_token = if raw_path == "/__audora_media" {
            req.auth_token.as_deref().or(query_token.as_deref())
        } else {
            req.auth_token.as_deref()
        };
        if !is_trusted_loopback_request(
            ctx.local_port,
            req.origin.as_deref(),
            req.sec_fetch_site.as_deref(),
            effective_token,
            &ctx.session_token,
        ) {
            write_json_response(
                &mut stream,
                "403 Forbidden",
                br#"{"ok":false,"message":"Unauthorized loopback bridge request blocked."}"#,
            );
            return;
        }
    } else if req.sec_fetch_site.as_deref() == Some("cross-site") {
        // Block cross-site static asset fingerprinting (`M-2`).
        let _ = stream
            .write_all(b"HTTP/1.1 403 Forbidden\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
        return;
    }

    if raw_path == "/__audora_media" {
        if method != "GET" && method != "HEAD" {
            write_json_response(&mut stream, "405 Method Not Allowed", br#"{"ok":false}"#);
            return;
        }
        let mut media_path_str = String::new();
        for pair in raw_query.split('&') {
            if let Some((k, v)) = pair.split_once('=') {
                if k == "path" {
                    media_path_str = percent_decode(v);
                }
            }
        }
        let allowed_canon = ctx
            .allowlist
            .lock()
            .ok()
            .and_then(|g| g.resolve_allowed_media_file(&media_path_str));
        let Some(canon_path) = allowed_canon else {
            let _ = stream.write_all(
                b"HTTP/1.1 404 Not Found\r\nX-Content-Type-Options: nosniff\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
            );
            return;
        };
        stream_local_media_file(
            &mut stream,
            &canon_path,
            range_header.as_deref(),
            method == "HEAD",
        );
        return;
    }

    if raw_path == "/__audora_probe_media" {
        if method != "GET" {
            write_json_response(&mut stream, "405 Method Not Allowed", br#"{"ok":false}"#);
            return;
        }
        let mut media_path_str = String::new();
        for pair in raw_query.split('&') {
            if let Some((k, v)) = pair.split_once('=') {
                if k == "path" {
                    media_path_str = percent_decode(v);
                }
            }
        }
        let allowed_canon = ctx
            .allowlist
            .lock()
            .ok()
            .and_then(|g| g.resolve_allowed_media_file(&media_path_str));
        let Some(candidate) = allowed_canon else {
            write_json_response(
                &mut stream,
                "404 Not Found",
                br#"{"ok":false,"exists":false,"duration_ms":null}"#,
            );
            return;
        };
        let dur = probe_media_duration_ms(&candidate);
        write_json_value(
            &mut stream,
            "200 OK",
            &json!({ "ok": true, "exists": true, "duration_ms": dur }),
        );
        return;
    }

    if raw_path == "/__audora_pick_files" {
        if method != "POST" {
            write_json_response(&mut stream, "405 Method Not Allowed", br#"{"ok":false}"#);
            return;
        }
        let Some(_dialog_guard) = SingleFlightGuard::try_acquire(&ctx.dialog_in_flight) else {
            write_json_response(
                &mut stream,
                "409 Conflict",
                br#"{"ok":false,"cancelled":true,"files":[]}"#,
            );
            return;
        };
        let picked = pick_native_media_files(true);
        if picked.is_empty() {
            write_json_response(
                &mut stream,
                "200 OK",
                br#"{"ok":true,"cancelled":true,"files":[]}"#,
            );
        } else {
            if let Ok(mut guard) = ctx.allowlist.lock() {
                for (p, _, _, _) in &picked {
                    guard.register_file(Path::new(p));
                }
            }
            let files: Vec<serde_json::Value> = picked
                .iter()
                .map(|(p, name, sz, dur)| {
                    json!({ "path": p, "name": name, "size": sz, "duration_ms": dur })
                })
                .collect();
            write_json_value(
                &mut stream,
                "200 OK",
                &json!({ "ok": true, "cancelled": false, "files": files }),
            );
        }
        return;
    }

    if raw_path == "/__audora_pick_single_file" {
        if method != "POST" {
            write_json_response(&mut stream, "405 Method Not Allowed", br#"{"ok":false}"#);
            return;
        }
        let Some(_dialog_guard) = SingleFlightGuard::try_acquire(&ctx.dialog_in_flight) else {
            write_json_response(
                &mut stream,
                "409 Conflict",
                br#"{"ok":false,"cancelled":true,"file":null}"#,
            );
            return;
        };
        let picked = pick_native_media_files(false);
        if let Some((p, name, sz, dur)) = picked.first() {
            if let Ok(mut guard) = ctx.allowlist.lock() {
                guard.register_file(Path::new(p));
            }
            write_json_value(
                &mut stream,
                "200 OK",
                &json!({
                    "ok": true,
                    "cancelled": false,
                    "file": { "path": p, "name": name, "size": sz, "duration_ms": dur }
                }),
            );
        } else {
            write_json_response(
                &mut stream,
                "200 OK",
                br#"{"ok":true,"cancelled":true,"file":null}"#,
            );
        }
        return;
    }

    if raw_path == "/__audora_pick_folder" {
        if method != "POST" {
            write_json_response(&mut stream, "405 Method Not Allowed", br#"{"ok":false}"#);
            return;
        }
        let Some(_dialog_guard) = SingleFlightGuard::try_acquire(&ctx.dialog_in_flight) else {
            write_json_response(
                &mut stream,
                "409 Conflict",
                br#"{"ok":false,"cancelled":true,"path":null}"#,
            );
            return;
        };
        match pick_native_folder() {
            Some(selected) => {
                if let Ok(mut guard) = ctx.allowlist.lock() {
                    guard.register_dir(Path::new(&selected));
                }
                write_json_value(
                    &mut stream,
                    "200 OK",
                    &json!({ "ok": true, "cancelled": false, "path": selected }),
                );
            }
            None => {
                write_json_response(
                    &mut stream,
                    "200 OK",
                    br#"{"ok":true,"cancelled":true,"path":null}"#,
                );
            }
        }
        return;
    }

    if raw_path == "/__audora_scan_folder" {
        if method != "GET" && method != "POST" {
            write_json_response(&mut stream, "405 Method Not Allowed", br#"{"ok":false}"#);
            return;
        }
        let mut folder_path_str = String::new();
        for pair in raw_query.split('&') {
            if let Some((k, v)) = pair.split_once('=') {
                if k == "path" {
                    folder_path_str = percent_decode(v);
                }
            }
        }
        handle_scan_folder_request(&mut stream, &folder_path_str, &ctx.allowlist);
        return;
    }

    if raw_path == "/__audora_update" {
        if method != "POST" {
            write_json_response(&mut stream, "405 Method Not Allowed", br#"{"ok":false}"#);
            return;
        }
        let Some(_update_guard) = SingleFlightGuard::try_acquire(&ctx.update_in_flight) else {
            write_json_value(
                &mut stream,
                "409 Conflict",
                &json!({ "ok": false, "status": "busy", "message": "An update operation is already in progress." }),
            );
            return;
        };
        let body_json: Option<serde_json::Value> = std::str::from_utf8(&req_body)
            .ok()
            .and_then(|s| serde_json::from_str(s).ok());
        let raw_ver = body_json
            .as_ref()
            .and_then(|v| v.get("version"))
            .and_then(serde_json::Value::as_str)
            .unwrap_or("");
        let expected_sha = body_json
            .as_ref()
            .and_then(|v| v.get("sha256"))
            .and_then(serde_json::Value::as_str)
            .unwrap_or("")
            .trim()
            .to_ascii_lowercase();

        let clean_ver = raw_ver.trim().trim_start_matches(['v', 'V']).to_owned();
        if !is_safe_semver(&clean_ver) || !is_valid_sha256_hex(&expected_sha) {
            write_json_response(
                &mut stream,
                "400 Bad Request",
                br#"{"ok":false,"status":"invalid_parameters","message":"Verified target version and 64-hex SHA-256 digest are required."}"#,
            );
            return;
        }

        match perform_native_self_update(DEFAULT_GITHUB_RELEASE_REPO, &clean_ver, &expected_sha) {
            Ok((status_code, msg, should_restart)) => {
                write_json_value(
                    &mut stream,
                    "200 OK",
                    &json!({ "ok": true, "status": status_code, "message": msg }),
                );
                drop(stream);
                if should_restart {
                    let close_proxy = ctx.proxy.clone();
                    thread::spawn(move || {
                        thread::sleep(Duration::from_millis(150));
                        let _ = close_proxy.send_event(DesktopUserEvent::RequestGracefulClose);
                    });
                }
            }
            Err((status_code, err_msg)) => {
                write_json_value(
                    &mut stream,
                    "500 Internal Server Error",
                    &json!({ "ok": false, "status": status_code, "message": err_msg }),
                );
            }
        }
        return;
    }

    if raw_path == "/__audora_backup" {
        if method != "POST" {
            write_json_response(&mut stream, "405 Method Not Allowed", br#"{"ok":false}"#);
            return;
        }
        match ctx.db.create_manual_backup() {
            Ok(Some(backup_path)) => {
                write_json_value(
                    &mut stream,
                    "200 OK",
                    &json!({ "ok": true, "backup_path": backup_path.display().to_string() }),
                );
            }
            _ => {
                write_json_response(
                    &mut stream,
                    "500 Internal Server Error",
                    br#"{"ok":false,"message":"Failed to create SQLite manual backup."}"#,
                );
            }
        }
        return;
    }

    if raw_path == "/__audora_rpc" {
        if method != "POST" {
            write_json_response(&mut stream, "405 Method Not Allowed", br#"{"ok":false}"#);
            return;
        }
        handle_relational_rpc_request(&mut stream, &req_body, ctx);
        return;
    }

    if raw_path == "/__audora_state" {
        let legacy_state_file = default_app_data_dir().join("ui-state.json");
        match method.as_str() {
            "GET" => match ctx.db.get_ui_state_json() {
                Ok(Some(json_str)) => {
                    write_json_response(&mut stream, "200 OK", json_str.as_bytes());
                }
                _ => {
                    if let Ok(bytes) = fs::read(&legacy_state_file) {
                        write_json_response(&mut stream, "200 OK", &bytes);
                    } else {
                        write_json_response(&mut stream, "200 OK", b"{}");
                    }
                }
            },
            "POST" => {
                if let Ok(json_str) = std::str::from_utf8(&req_body) {
                    if let Ok(parsed_val) = serde_json::from_str::<serde_json::Value>(json_str) {
                        if parsed_val.is_object() && ctx.db.set_ui_state_json(json_str).is_ok() {
                            sync_allowlist_from_db(&ctx.db, &ctx.allowlist);
                            let _ = fs::remove_file(&legacy_state_file);
                            write_json_response(&mut stream, "200 OK", br#"{"ok":true}"#);
                            return;
                        }
                    }
                }
                write_json_response(&mut stream, "400 Bad Request", br#"{"ok":false}"#);
            }
            "DELETE" => {
                let data_dir = default_app_data_dir();
                let sqlite_file = ctx
                    .db
                    .db_path()
                    .map(Path::to_path_buf)
                    .unwrap_or_else(|| data_dir.join("audora.sqlite"));
                let wal_file = PathBuf::from(format!("{}-wal", sqlite_file.display()));
                let before_db_bytes = fs::metadata(&sqlite_file).map(|m| m.len()).unwrap_or(0)
                    + fs::metadata(&wal_file).map(|m| m.len()).unwrap_or(0);

                let _ = ctx.db.reset_all_user_data();
                let _ = ctx.db.with_conn(|conn| {
                    let _ = conn.execute_batch("VACUUM; PRAGMA wal_checkpoint(TRUNCATE);");
                    Ok(())
                });
                let after_db_bytes = fs::metadata(&sqlite_file).map(|m| m.len()).unwrap_or(0)
                    + fs::metadata(&wal_file).map(|m| m.len()).unwrap_or(0);

                if let Ok(mut guard) = ctx.allowlist.lock() {
                    guard.clear();
                }
                let _ = fs::remove_file(&legacy_state_file);
                let mut removed_paths = vec![sqlite_file.display().to_string()];
                let mut bytes_reclaimed = before_db_bytes.saturating_sub(after_db_bytes);

                for sub in ["updates", "logs", "cache"] {
                    let target = data_dir.join(sub);
                    if target.exists() {
                        if let Ok(meta) = fs::symlink_metadata(&target) {
                            if !meta.file_type().is_symlink() && meta.is_dir() {
                                let sz = measure_dir_bytes(&target);
                                if fs::remove_dir_all(&target).is_ok() {
                                    bytes_reclaimed = bytes_reclaimed.saturating_add(sz);
                                    removed_paths.push(target.display().to_string());
                                }
                            }
                        }
                    }
                }
                let db_file_name = sqlite_file
                    .file_name()
                    .and_then(|s| s.to_str())
                    .unwrap_or("audora.sqlite")
                    .to_owned();
                let mut backup_dirs = vec![data_dir];
                if let Some(db_parent) = sqlite_file.parent().map(Path::to_path_buf) {
                    if !backup_dirs.contains(&db_parent) {
                        backup_dirs.push(db_parent);
                    }
                }
                let db_dot_prefix = format!("{db_file_name}.");
                let db_wal_dot_prefix = format!("{db_file_name}-wal.");
                for dir in backup_dirs {
                    if let Ok(entries) = fs::read_dir(&dir) {
                        for entry in entries.filter_map(Result::ok) {
                            let p = entry.path();
                            if let Some(name) = p.file_name().and_then(|s| s.to_str()) {
                                if (name.starts_with(&db_dot_prefix)
                                    || name.starts_with(&db_wal_dot_prefix))
                                    && (name.ends_with(".bak") || name.ends_with(".bak.tmp"))
                                {
                                    let sz = fs::metadata(&p).map(|m| m.len()).unwrap_or(0);
                                    if fs::remove_file(&p).is_ok() {
                                        bytes_reclaimed = bytes_reclaimed.saturating_add(sz);
                                        removed_paths.push(p.display().to_string());
                                    }
                                }
                            }
                        }
                    }
                }
                write_json_value(
                    &mut stream,
                    "200 OK",
                    &json!({
                        "ok": true,
                        "removed_paths": removed_paths,
                        "bytes_reclaimed": bytes_reclaimed,
                    }),
                );
            }
            _ => {
                write_json_response(&mut stream, "405 Method Not Allowed", br#"{"ok":false}"#);
            }
        }
        return;
    }

    if raw_path.starts_with("/__audora_") {
        write_json_response(&mut stream, "404 Not Found", br#"{"ok":false}"#);
        return;
    }

    serve_static_ui_asset(&mut stream, &ctx.root, &method, raw_path);
}

fn serve_static_ui_asset(stream: &mut TcpStream, root: &Path, method: &str, raw_path: &str) {
    if method != "GET" && method != "HEAD" {
        let _ = stream.write_all(
            b"HTTP/1.1 405 Method Not Allowed\r\nX-Content-Type-Options: nosniff\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
        );
        let _ = stream.flush();
        return;
    }

    let decoded = percent_decode(raw_path);
    if decoded.chars().any(char::is_control) {
        let _ = stream.write_all(
            b"HTTP/1.1 400 Bad Request\r\nX-Content-Type-Options: nosniff\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
        );
        let _ = stream.flush();
        return;
    }
    let trimmed = decoded.trim_start_matches('/');
    let Ok(canon_root) = fs::canonicalize(root) else {
        let _ = stream.write_all(
            b"HTTP/1.1 500 Internal Server Error\r\nX-Content-Type-Options: nosniff\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
        );
        let _ = stream.flush();
        return;
    };

    let mut candidate = root.to_path_buf();
    if trimmed.is_empty() {
        candidate.push("index.html");
    } else {
        for comp in Path::new(trimmed).components() {
            match comp {
                Component::Normal(segment) => candidate.push(segment),
                Component::ParentDir => {
                    let _ = stream.write_all(
                        b"HTTP/1.1 404 Not Found\r\nX-Content-Type-Options: nosniff\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                    );
                    let _ = stream.flush();
                    return;
                }
                _ => {}
            }
        }
    }

    if !candidate.is_file() {
        if Path::new(trimmed).extension().is_some() {
            let _ = stream.write_all(
                b"HTTP/1.1 404 Not Found\r\nX-Content-Type-Options: nosniff\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
            );
            let _ = stream.flush();
            return;
        }
        candidate = root.join("index.html");
    }

    let Ok(canon_candidate) = fs::canonicalize(&candidate) else {
        let _ = stream.write_all(
            b"HTTP/1.1 404 Not Found\r\nX-Content-Type-Options: nosniff\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
        );
        let _ = stream.flush();
        return;
    };
    if !canon_candidate.starts_with(&canon_root) || !canon_candidate.is_file() {
        let _ = stream.write_all(
            b"HTTP/1.1 404 Not Found\r\nX-Content-Type-Options: nosniff\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
        );
        let _ = stream.flush();
        return;
    }

    let content_type = mime_type_for_path(&canon_candidate);
    if method == "HEAD" {
        let Ok(meta) = fs::metadata(&canon_candidate) else {
            let _ = stream.write_all(
                b"HTTP/1.1 404 Not Found\r\nX-Content-Type-Options: nosniff\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
            );
            let _ = stream.flush();
            return;
        };
        let header = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: {content_type}\r\nContent-Security-Policy: {RUNTIME_CSP}\r\nX-Content-Type-Options: nosniff\r\nContent-Length: {}\r\nCache-Control: no-cache\r\nConnection: close\r\n\r\n",
            meta.len()
        );
        let _ = stream.write_all(header.as_bytes());
        let _ = stream.flush();
        return;
    }

    let Ok(body) = fs::read(&canon_candidate) else {
        let _ = stream.write_all(
            b"HTTP/1.1 404 Not Found\r\nX-Content-Type-Options: nosniff\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
        );
        let _ = stream.flush();
        return;
    };

    let header = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: {content_type}\r\nContent-Security-Policy: {RUNTIME_CSP}\r\nX-Content-Type-Options: nosniff\r\nContent-Length: {}\r\nCache-Control: no-cache\r\nConnection: close\r\n\r\n",
        body.len()
    );
    let _ = stream.write_all(header.as_bytes());
    let _ = stream.write_all(&body);
    let _ = stream.flush();
}

fn measure_dir_bytes(path: &Path) -> u64 {
    let Ok(meta) = fs::symlink_metadata(path) else {
        return 0;
    };
    if meta.file_type().is_symlink() {
        return 0;
    }
    if meta.is_file() {
        return meta.len();
    }
    let Ok(entries) = fs::read_dir(path) else {
        return 0;
    };
    entries
        .filter_map(Result::ok)
        .map(|e| measure_dir_bytes(&e.path()))
        .sum()
}

fn compute_file_sha256_hex(path: &Path) -> Option<String> {
    let mut file = File::open(path).ok()?;
    let mut hasher = Sha256::new();
    let mut buf = [0_u8; 65_536];
    loop {
        let n = file.read(&mut buf).ok()?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Some(format!("{:x}", hasher.finalize()))
}

fn resolve_github_release_asset_sha256(repo: &str, tag: &str, asset_name: &str) -> Option<String> {
    let api_url = format!("https://api.github.com/repos/{repo}/releases/tags/{tag}");
    let curl_bin = if cfg!(target_os = "macos") {
        "/usr/bin/curl"
    } else if cfg!(target_os = "windows") {
        "curl.exe"
    } else {
        "curl"
    };
    let mut cmd = Command::new(curl_bin);
    configure_hidden_windows_command(&mut cmd);
    let output = cmd
        .args([
            "--proto",
            "=https",
            "--tlsv1.2",
            "--connect-timeout",
            "10",
            "--max-time",
            "20",
            "--max-redirs",
            "3",
            "-fsSL",
            "-H",
            "Accept: application/vnd.github+json",
            "-H",
            "User-Agent: Audora-Updater",
            &api_url,
        ])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let val: serde_json::Value = serde_json::from_slice(&output.stdout).ok()?;
    let assets = val.get("assets")?.as_array()?;
    for asset in assets {
        let name = asset.get("name").and_then(serde_json::Value::as_str)?;
        if name == asset_name {
            let digest = asset.get("digest").and_then(serde_json::Value::as_str)?;
            let hex = digest
                .trim()
                .strip_prefix("sha256:")
                .unwrap_or(digest.trim())
                .to_ascii_lowercase();
            if is_valid_sha256_hex(&hex) {
                return Some(hex);
            }
        }
    }
    None
}

fn inspect_macos_codesign_identity(bundle: &Path) -> Option<(Option<String>, Option<String>)> {
    let output = Command::new("/usr/bin/codesign")
        .args(["-dv", "--verbose=2"])
        .arg(bundle)
        .output()
        .ok()?;
    let stderr = String::from_utf8_lossy(&output.stderr);
    let mut identifier = None;
    let mut team_id = None;
    for line in stderr.lines() {
        if let Some(rest) = line.strip_prefix("Identifier=") {
            let trimmed = rest.trim();
            if !trimmed.is_empty() {
                identifier = Some(trimmed.to_owned());
            }
        } else if let Some(rest) = line.strip_prefix("TeamIdentifier=") {
            let trimmed = rest.trim();
            if !trimmed.is_empty() && trimmed != "not set" {
                team_id = Some(trimmed.to_owned());
            }
        }
    }
    Some((identifier, team_id))
}

fn verify_macos_staged_bundle_signature(staged_app: &Path) -> bool {
    let verify_ok = Command::new("/usr/bin/codesign")
        .args(["--verify", "--deep", "--strict"])
        .arg(staged_app)
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    if !verify_ok {
        return false;
    }
    let Some((staged_id, staged_team)) = inspect_macos_codesign_identity(staged_app) else {
        return false;
    };
    let current_identity =
        resolve_current_macos_app_bundle().and_then(|cur| inspect_macos_codesign_identity(&cur));
    let expected_id = current_identity
        .as_ref()
        .and_then(|(id, _)| id.as_deref())
        .unwrap_or("com.audora.desktop");
    if staged_id.as_deref() != Some(expected_id) {
        return false;
    }
    if let Some((_, Some(current_team))) = current_identity {
        if staged_team.as_deref() != Some(current_team.as_str()) {
            return false;
        }
    }
    true
}

fn verify_windows_installer_authenticode(setup_path: &Path) -> bool {
    if !cfg!(target_os = "windows") {
        return true;
    }
    let Ok(current_exe) = env::current_exe() else {
        return true;
    };
    let ps_check = r#"
        $cur = Get-AuthenticodeSignature -LiteralPath $env:AUDORA_CUR_EXE
        if ($cur.Status -ne 'Valid') { Write-Output 'UNSIGNED_CURRENT'; exit 0 }
        $nxt = Get-AuthenticodeSignature -LiteralPath $env:AUDORA_SETUP_EXE
        if ($nxt.Status -eq 'Valid' -and $nxt.SignerCertificate.Subject -eq $cur.SignerCertificate.Subject) {
            Write-Output 'VALID_MATCH'
        } else {
            Write-Output 'INVALID'
        }
    "#;
    let mut cmd = Command::new("powershell");
    configure_hidden_windows_command(&mut cmd);
    let Ok(out) = cmd
        .args(["-NoProfile", "-NonInteractive", "-Command", ps_check])
        .env("AUDORA_CUR_EXE", &current_exe)
        .env("AUDORA_SETUP_EXE", setup_path)
        .output()
    else {
        return false;
    };
    let verdict = String::from_utf8_lossy(&out.stdout);
    let trimmed = verdict.trim();
    trimmed == "UNSIGNED_CURRENT" || trimmed == "VALID_MATCH"
}

fn perform_native_self_update(
    repo: &str,
    version: &str,
    expected_sha256: &str,
) -> Result<(&'static str, String, bool), (&'static str, String)> {
    let tag = format!("v{version}");
    let rand_tok = generate_session_token();
    let rand_suffix = &rand_tok[..16];
    let staging_dir = default_app_data_dir()
        .join("updates")
        .join(format!("staging-{version}-{rand_suffix}"));
    fs::create_dir_all(&staging_dir).map_err(|_| {
        (
            "staging_failed",
            "Failed to create secure update staging directory.".to_owned(),
        )
    })?;

    let curl_common_args = [
        "--proto",
        "=https",
        "--tlsv1.2",
        "--connect-timeout",
        "15",
        "--max-time",
        "300",
        "--max-redirs",
        "5",
        "-fSL",
        "--retry",
        "2",
        "-o",
    ];

    if cfg!(target_os = "macos") {
        let mac_arch = if cfg!(target_arch = "x86_64") {
            "x86_64"
        } else {
            "aarch64"
        };
        let archive_name = format!("Audora_{version}_macos-{mac_arch}.app.tar.gz");
        let archive_url =
            format!("https://github.com/{repo}/releases/download/{tag}/{archive_name}");
        let archive_path = staging_dir.join(&archive_name);

        info!(url = %archive_url, "Downloading macOS update bundle");
        let curl_ok = Command::new("/usr/bin/curl")
            .args(curl_common_args)
            .arg(&archive_path)
            .arg(&archive_url)
            .status()
            .map(|s| s.success())
            .unwrap_or(false);

        if curl_ok && archive_path.is_file() {
            let actual_sha = compute_file_sha256_hex(&archive_path).unwrap_or_default();
            let sha_ok = constant_time_eq(actual_sha.as_bytes(), expected_sha256.as_bytes());
            if !sha_ok {
                let _ = fs::remove_dir_all(&staging_dir);
                return Err((
                    "checksum_verification_failed",
                    "Downloaded update bundle failed SHA-256 integrity verification.".to_owned(),
                ));
            }

            let tar_ok = Command::new("/usr/bin/tar")
                .arg("-xzf")
                .arg(&archive_path)
                .arg("-C")
                .arg(&staging_dir)
                .status()
                .map(|s| s.success())
                .unwrap_or(false);

            let staged_app = staging_dir.join("Audora.app");
            if tar_ok && staged_app.is_dir() {
                if !verify_macos_staged_bundle_signature(&staged_app) {
                    let _ = fs::remove_dir_all(&staging_dir);
                    return Err((
                        "signature_verification_failed",
                        "Downloaded macOS bundle failed codesign identity verification.".to_owned(),
                    ));
                }

                let target_app = resolve_current_macos_app_bundle()
                    .unwrap_or_else(|| PathBuf::from("/Applications/Audora.app"));
                let backup_app = target_app.with_extension("app.previous");
                let _ = fs::remove_dir_all(&backup_app);

                if target_app.exists() && fs::rename(&target_app, &backup_app).is_err() {
                    let _ = fs::remove_dir_all(&staging_dir);
                    return Err((
                        "install_permission_denied",
                        format!(
                            "Insufficient permissions to replace {}. Move Audora to a writable folder or install manually.",
                            target_app.display()
                        ),
                    ));
                }

                let moved = fs::rename(&staged_app, &target_app).is_ok()
                    || Command::new("/usr/bin/ditto")
                        .arg(&staged_app)
                        .arg(&target_app)
                        .status()
                        .map(|s| s.success())
                        .unwrap_or(false);

                if moved {
                    let _ = fs::remove_dir_all(&backup_app);
                    let _ = fs::remove_dir_all(&staging_dir);
                    let target_for_relaunch = target_app.clone();
                    thread::spawn(move || {
                        thread::sleep(Duration::from_millis(900));
                        let _ = Command::new("/usr/bin/open")
                            .arg("-n")
                            .arg(&target_for_relaunch)
                            .spawn();
                    });

                    return Ok((
                        "restarting",
                        format!(
                            "Audora {version} SHA-256 & signature verified, installed to {}, and restarting…",
                            target_app.display()
                        ),
                        true,
                    ));
                }

                if backup_app.exists() {
                    let _ = fs::remove_dir_all(&target_app);
                    let _ = fs::rename(&backup_app, &target_app);
                }
                let _ = fs::remove_dir_all(&staging_dir);
                return Err((
                    "install_failed",
                    format!(
                        "Failed to move verified bundle into {}. Existing installation was restored.",
                        target_app.display()
                    ),
                ));
            }
        }

        let dmg_name = format!("Audora_{version}_macos-{mac_arch}.dmg");
        let dmg_url = format!("https://github.com/{repo}/releases/download/{tag}/{dmg_name}");
        let dmg_staged = staging_dir.join(&dmg_name);

        let dmg_ok = Command::new("/usr/bin/curl")
            .args(curl_common_args)
            .arg(&dmg_staged)
            .arg(&dmg_url)
            .status()
            .map(|s| s.success())
            .unwrap_or(false);

        if dmg_ok && dmg_staged.is_file() {
            let actual_sha = compute_file_sha256_hex(&dmg_staged).unwrap_or_default();
            let resolved_dmg_sha = resolve_github_release_asset_sha256(repo, &tag, &dmg_name);
            let dmg_sha_ok = constant_time_eq(actual_sha.as_bytes(), expected_sha256.as_bytes())
                || resolved_dmg_sha
                    .as_deref()
                    .is_some_and(|s| constant_time_eq(actual_sha.as_bytes(), s.as_bytes()));
            if dmg_sha_ok {
                let downloads_dir = env::var_os("HOME")
                    .map(|h| PathBuf::from(h).join("Downloads"))
                    .unwrap_or_else(|| staging_dir.clone());
                let dmg_dest = downloads_dir.join(&dmg_name);
                let _ = fs::copy(&dmg_staged, &dmg_dest);
                let _ = fs::remove_dir_all(&staging_dir);
                let _ = Command::new("/usr/bin/open").arg(&dmg_dest).spawn();
                return Ok((
                    "downloaded_installer",
                    format!("Verified SHA-256 and mounted {dmg_name} in ~/Downloads."),
                    false,
                ));
            }
            let _ = fs::remove_dir_all(&staging_dir);
            return Err((
                "checksum_verification_failed",
                "Downloaded DMG failed SHA-256 integrity verification.".to_owned(),
            ));
        }

        let _ = fs::remove_dir_all(&staging_dir);
        let release_page = format!("https://github.com/{repo}/releases/tag/{tag}");
        open_external_https_url(&release_page);
        return Ok((
            "opened_release",
            format!("Opened GitHub release {tag} in your browser to complete installation."),
            false,
        ));
    }

    if cfg!(target_os = "windows") {
        let setup_name = format!("Audora_{version}_windows-x86_64-setup.exe");
        let setup_url = format!("https://github.com/{repo}/releases/download/{tag}/{setup_name}");
        let setup_path = staging_dir.join(&setup_name);
        let mut curl_cmd = Command::new("curl.exe");
        configure_hidden_windows_command(&mut curl_cmd);
        let curl_ok = curl_cmd
            .args(curl_common_args)
            .arg(&setup_path)
            .arg(&setup_url)
            .status()
            .map(|s| s.success())
            .unwrap_or(false);

        if curl_ok && setup_path.is_file() {
            let actual_sha = compute_file_sha256_hex(&setup_path).unwrap_or_default();
            let sha_ok = constant_time_eq(actual_sha.as_bytes(), expected_sha256.as_bytes());
            if !sha_ok {
                let _ = fs::remove_dir_all(&staging_dir);
                return Err((
                    "checksum_verification_failed",
                    "Downloaded Windows installer failed SHA-256 integrity verification."
                        .to_owned(),
                ));
            }
            if !verify_windows_installer_authenticode(&setup_path) {
                let _ = fs::remove_dir_all(&staging_dir);
                return Err((
                    "signature_verification_failed",
                    "Downloaded Windows installer failed Authenticode verification.".to_owned(),
                ));
            }
            // Spawn detached launcher waiting for `AUDORA_PID` to exit before NSIS overwrites `Audora.exe` (`C-5`).
            let mut launch_cmd = Command::new("powershell");
            configure_hidden_windows_command(&mut launch_cmd);
            let _ = launch_cmd
                .args([
                    "-NoProfile",
                    "-NonInteractive",
                    "-WindowStyle",
                    "Hidden",
                    "-Command",
                    "Wait-Process -Id $env:AUDORA_PID -Timeout 15 -ErrorAction SilentlyContinue; Start-Process -FilePath $env:AUDORA_SETUP_EXE -Wait; if ($env:AUDORA_STAGING_DIR) { Remove-Item -LiteralPath $env:AUDORA_STAGING_DIR -Recurse -Force -ErrorAction SilentlyContinue }",
                ])
                .env("AUDORA_PID", std::process::id().to_string())
                .env("AUDORA_STAGING_DIR", &staging_dir)
                .env("AUDORA_SETUP_EXE", &setup_path)
                .spawn();
            return Ok((
                "restarting",
                format!("Verified and launched {setup_name}."),
                true,
            ));
        }

        let _ = fs::remove_dir_all(&staging_dir);
        let release_page = format!("https://github.com/{repo}/releases/tag/{tag}");
        open_external_https_url(&release_page);
        return Ok(("opened_release", format!("Opened {release_page}."), false));
    }

    let _ = fs::remove_dir_all(&staging_dir);
    let release_page = format!("https://github.com/{repo}/releases/tag/{tag}");
    open_external_https_url(&release_page);
    Ok((
        "opened_release",
        format!("Opened {release_page} in your browser."),
        false,
    ))
}

fn resolve_current_macos_app_bundle() -> Option<PathBuf> {
    let exe = env::current_exe().ok()?;
    for ancestor in exe.ancestors() {
        if ancestor.extension().and_then(|s| s.to_str()) == Some("app")
            && !ancestor.to_string_lossy().starts_with("/Volumes/")
        {
            return Some(ancestor.to_path_buf());
        }
    }
    None
}

fn percent_decode(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut idx = 0;
    while idx < bytes.len() {
        if bytes[idx] == b'%' && idx + 2 < bytes.len() {
            if let Ok(hex_str) = std::str::from_utf8(&bytes[idx + 1..idx + 3]) {
                if let Ok(val) = u8::from_str_radix(hex_str, 16) {
                    out.push(val);
                    idx += 3;
                    continue;
                }
            }
        }
        out.push(bytes[idx]);
        idx += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn is_supported_media_extension(ext: &str) -> bool {
    matches!(
        ext,
        "mp4"
            | "m4v"
            | "mov"
            | "webm"
            | "mkv"
            | "wav"
            | "flac"
            | "ogg"
            | "opus"
            | "mp3"
            | "m4a"
            | "aac"
    )
}

fn parse_http_byte_range(spec: &str, total_len: u64) -> Result<(u64, u64), ()> {
    if total_len == 0 {
        return Err(());
    }
    let range_spec = spec.trim().strip_prefix("bytes=").ok_or(())?;
    // Ignore multi-range requests by falling back to first range or rejecting invalid syntax
    let first_range = range_spec.split(',').next().unwrap_or("").trim();
    let (start_str, end_str) = first_range.split_once('-').ok_or(())?;
    let start_trimmed = start_str.trim();
    let end_trimmed = end_str.trim();

    if start_trimmed.is_empty() {
        // Suffix byte-range-spec (`bytes=-500`)
        let suffix_len: u64 = end_trimmed.parse().map_err(|_| ())?;
        if suffix_len == 0 {
            return Err(());
        }
        let start = total_len.saturating_sub(suffix_len);
        Ok((start, total_len - 1))
    } else {
        let start: u64 = start_trimmed.parse().map_err(|_| ())?;
        if start >= total_len {
            return Err(());
        }
        let end = if end_trimmed.is_empty() {
            total_len - 1
        } else {
            let parsed_end: u64 = end_trimmed.parse().map_err(|_| ())?;
            if parsed_end < start {
                return Err(());
            }
            parsed_end.min(total_len - 1)
        };
        Ok((start, end))
    }
}

fn stream_local_media_file(
    stream: &mut TcpStream,
    candidate: &Path,
    range_header: Option<&str>,
    is_head: bool,
) {
    let Ok(mut file) = File::open(candidate) else {
        let _ = stream.write_all(
            b"HTTP/1.1 404 Not Found\r\nX-Content-Type-Options: nosniff\r\nContent-Security-Policy: default-src 'none'\r\nCache-Control: no-store\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
        );
        return;
    };
    let Ok(meta) = file.metadata() else {
        let _ = stream.write_all(
            b"HTTP/1.1 404 Not Found\r\nX-Content-Type-Options: nosniff\r\nContent-Security-Policy: default-src 'none'\r\nCache-Control: no-store\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
        );
        return;
    };
    let total_len = meta.len();
    let content_type = mime_type_for_path(candidate);

    if total_len == 0 {
        let header = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: {content_type}\r\nX-Content-Type-Options: nosniff\r\nContent-Security-Policy: default-src 'none'\r\nCache-Control: no-store\r\nAccept-Ranges: bytes\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
        );
        let _ = stream.write_all(header.as_bytes());
        let _ = stream.flush();
        return;
    }

    let (start, end, is_partial) = if let Some(range_val) = range_header {
        match parse_http_byte_range(range_val, total_len) {
            Ok((s, e)) => (s, e, true),
            Err(()) => {
                let header = format!(
                    "HTTP/1.1 416 Range Not Satisfiable\r\nX-Content-Type-Options: nosniff\r\nContent-Security-Policy: default-src 'none'\r\nCache-Control: no-store\r\nContent-Range: bytes */{total_len}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                );
                let _ = stream.write_all(header.as_bytes());
                let _ = stream.flush();
                return;
            }
        }
    } else {
        (0_u64, total_len - 1, false)
    };

    if !is_head && file.seek(SeekFrom::Start(start)).is_err() {
        let _ = stream.write_all(
            b"HTTP/1.1 500 Internal Server Error\r\nX-Content-Type-Options: nosniff\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
        );
        return;
    }

    let send_len = end.saturating_sub(start).saturating_add(1);
    let header = if is_partial {
        format!(
            "HTTP/1.1 206 Partial Content\r\nContent-Type: {content_type}\r\nX-Content-Type-Options: nosniff\r\nContent-Security-Policy: default-src 'none'\r\nCache-Control: no-store\r\nAccept-Ranges: bytes\r\nContent-Range: bytes {start}-{end}/{total_len}\r\nContent-Length: {send_len}\r\nConnection: close\r\n\r\n"
        )
    } else {
        format!(
            "HTTP/1.1 200 OK\r\nContent-Type: {content_type}\r\nX-Content-Type-Options: nosniff\r\nContent-Security-Policy: default-src 'none'\r\nCache-Control: no-store\r\nAccept-Ranges: bytes\r\nContent-Length: {total_len}\r\nConnection: close\r\n\r\n"
        )
    };

    let _ = stream.set_write_timeout(Some(Duration::from_secs(60)));
    if stream.write_all(header.as_bytes()).is_err() {
        return;
    }
    if is_head {
        let _ = stream.flush();
        return;
    }

    let mut remaining = send_len;
    let mut buf = [0_u8; 65_536];
    while remaining > 0 {
        let to_read = (remaining as usize).min(buf.len());
        let Ok(n) = file.read(&mut buf[..to_read]) else {
            break;
        };
        if n == 0 {
            break;
        }
        if stream.write_all(&buf[..n]).is_err() {
            break;
        }
        remaining = remaining.saturating_sub(n as u64);
    }
    let _ = stream.flush();
}

fn mime_type_for_path(path: &Path) -> &'static str {
    match path
        .extension()
        .and_then(|ext| ext.to_str())
        .unwrap_or("")
        .to_ascii_lowercase()
        .as_str()
    {
        "html" => "text/html; charset=utf-8",
        "js" | "mjs" => "text/javascript; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "json" => "application/json; charset=utf-8",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "ico" => "image/x-icon",
        "woff2" => "font/woff2",
        "woff" => "font/woff",
        "mp4" | "m4v" => "video/mp4",
        "mov" => "video/quicktime",
        "webm" => "video/webm",
        "mkv" => "video/x-matroska",
        "mp3" => "audio/mpeg",
        "wav" => "audio/wav",
        "flac" => "audio/flac",
        "ogg" | "opus" => "audio/ogg",
        "m4a" => "audio/mp4",
        "aac" => "audio/aac",
        _ => "application/octet-stream",
    }
}

fn normalize_folder_path(raw: &str) -> String {
    let cleaned = raw.trim_start().trim_end_matches(['\r', '\n']);
    if cleaned.trim().is_empty() {
        return String::new();
    }
    if cleaned.len() > 1 {
        let stripped = cleaned.trim_end_matches(['/', '\\']);
        if stripped.is_empty() {
            cleaned[..1].to_owned()
        } else if stripped.len() == 2
            && stripped.as_bytes()[0].is_ascii_alphabetic()
            && stripped.as_bytes()[1] == b':'
        {
            format!(r"{stripped}\")
        } else {
            stripped.to_owned()
        }
    } else {
        cleaned.to_owned()
    }
}

fn strip_windows_verbatim_prefix(raw: &str) -> PathBuf {
    if let Some(unc) = raw.strip_prefix(r"\\?\UNC\") {
        PathBuf::from(format!(r"\\{unc}"))
    } else if let Some(stripped) = raw.strip_prefix(r"\\?\") {
        PathBuf::from(stripped)
    } else {
        PathBuf::from(raw)
    }
}

fn canonical_existing_path(p: &Path) -> Option<PathBuf> {
    let canon = fs::canonicalize(p).ok()?;
    let raw = canon.to_string_lossy();
    Some(strip_windows_verbatim_prefix(&raw))
}

fn canonical_path_string(p: &Path) -> String {
    canonical_existing_path(p)
        .unwrap_or_else(|| strip_windows_verbatim_prefix(&p.to_string_lossy()))
        .to_string_lossy()
        .into_owned()
}

fn handle_relational_rpc_request(
    stream: &mut TcpStream,
    req_body: &[u8],
    ctx: &LoopbackServerContext,
) {
    let Ok(body_str) = std::str::from_utf8(req_body) else {
        write_json_response(stream, "400 Bad Request", br#"{"ok":false}"#);
        return;
    };
    let Ok(val) = serde_json::from_str::<serde_json::Value>(body_str) else {
        write_json_response(stream, "400 Bad Request", br#"{"ok":false}"#);
        return;
    };
    let op = val
        .get("op")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("");

    let result: Result<Option<serde_json::Value>, String> = match op {
        "batch" => match val.get("ops").and_then(serde_json::Value::as_array) {
            Some(ops) => {
                let res = ctx
                    .db
                    .rpc_execute_batch(ops)
                    .map(|()| None)
                    .map_err(|e| e.to_payload().message);
                if res.is_ok() {
                    apply_batch_source_revocations(ops, &ctx.allowlist);
                    sync_allowlist_from_db(&ctx.db, &ctx.allowlist);
                }
                res
            }
            None => Err("Missing 'ops' array".to_owned()),
        },
        "revoke_source_folder" => match val.get("path").and_then(serde_json::Value::as_str) {
            Some(path) => match audora_desktop::domain::validate_local_media_path_str(path) {
                Ok(clean) => {
                    if let Ok(mut guard) = ctx.allowlist.lock() {
                        guard.revoke_source_folder(clean);
                    }
                    Ok(None)
                }
                Err(e) => Err(e.to_payload().message),
            },
            None => Err("Missing 'path'".to_owned()),
        },
        "update_progress" => {
            let Some(media_id) = val.get("media_id").and_then(serde_json::Value::as_str) else {
                write_json_response(stream, "400 Bad Request", br#"{"ok":false}"#);
                return;
            };
            let pos = val
                .get("last_position_ms")
                .and_then(|v| {
                    v.as_u64().map(|u| u.min(i64::MAX as u64)).or_else(|| {
                        v.as_f64()
                            .filter(|f| f.is_finite() && *f >= 0.0)
                            .map(|f| f.round().min(i64::MAX as f64) as u64)
                    })
                })
                .unwrap_or(0);
            let completed = val
                .get("completed")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(false);
            let session: Option<PersistedPlaybackSession> = val
                .get("session")
                .and_then(|v| serde_json::from_value(v.clone()).ok());
            ctx.db
                .rpc_update_progress(media_id, pos, completed, session.as_ref())
                .map(|()| None)
                .map_err(|e| e.to_payload().message)
        }
        "upsert_media_batch" => {
            let items: Vec<MediaItem> = val
                .get("items")
                .and_then(|v| serde_json::from_value(v.clone()).ok())
                .unwrap_or_default();
            let res = ctx
                .db
                .rpc_upsert_media_batch(&items)
                .map(|()| None)
                .map_err(|e| e.to_payload().message);
            if res.is_ok() {
                sync_allowlist_from_db(&ctx.db, &ctx.allowlist);
            }
            res
        }
        "delete_media_ids" => {
            let ids: Vec<String> = val
                .get("ids")
                .and_then(|v| serde_json::from_value(v.clone()).ok())
                .unwrap_or_default();
            let res = ctx
                .db
                .rpc_delete_media_ids(&ids)
                .map(|_| None)
                .map_err(|e| e.to_payload().message);
            if res.is_ok() {
                sync_allowlist_from_db(&ctx.db, &ctx.allowlist);
            }
            res
        }
        "upsert_note" => match val
            .get("note")
            .and_then(|v| serde_json::from_value::<Note>(v.clone()).ok())
        {
            Some(note) => ctx
                .db
                .rpc_upsert_note(&note)
                .map(|()| None)
                .map_err(|e| e.to_payload().message),
            None => Err("Missing valid 'note' payload".to_owned()),
        },
        "delete_note" => match val.get("id").and_then(serde_json::Value::as_str) {
            Some(id) => ctx
                .db
                .rpc_delete_note(id)
                .map(|_| None)
                .map_err(|e| e.to_payload().message),
            None => Err("Missing 'id'".to_owned()),
        },
        "upsert_playlist" => match val
            .get("playlist")
            .and_then(|v| serde_json::from_value::<Playlist>(v.clone()).ok())
        {
            Some(pl) => ctx
                .db
                .rpc_upsert_playlist(&pl)
                .map(|()| None)
                .map_err(|e| e.to_payload().message),
            None => Err("Missing valid 'playlist' payload".to_owned()),
        },
        "delete_playlist" => match val.get("id").and_then(serde_json::Value::as_str) {
            Some(id) => ctx
                .db
                .rpc_delete_playlist(id)
                .map(|_| None)
                .map_err(|e| e.to_payload().message),
            None => Err("Missing 'id'".to_owned()),
        },
        "save_settings" => match val
            .get("settings")
            .and_then(|v| serde_json::from_value::<AppSettings>(v.clone()).ok())
        {
            Some(settings) => {
                let res = ctx
                    .db
                    .rpc_save_settings(&settings)
                    .map(|()| None)
                    .map_err(|e| e.to_payload().message);
                if res.is_ok() {
                    sync_allowlist_from_db(&ctx.db, &ctx.allowlist);
                }
                res
            }
            None => Err("Missing valid 'settings' payload".to_owned()),
        },
        "save_session" => match val
            .get("session")
            .and_then(|v| serde_json::from_value::<PersistedPlaybackSession>(v.clone()).ok())
        {
            Some(session) => ctx
                .db
                .rpc_save_playback_session(&session)
                .map(|()| None)
                .map_err(|e| e.to_payload().message),
            None => Err("Missing valid 'session' payload".to_owned()),
        },
        "diagnostic_report" => ctx
            .dispatcher
            .generate_diagnostic_report()
            .and_then(|rep| {
                serde_json::to_value(rep).map(Some).map_err(|e| {
                    audora_desktop::error::AppError::internal(e.to_string()).to_payload()
                })
            })
            .map_err(|e| e.message),
        _ => Err(format!("Unsupported RPC operation '{op}'")),
    };

    match result {
        Ok(Some(custom_data)) => {
            write_json_value(
                stream,
                "200 OK",
                &json!({ "ok": true, "data": custom_data }),
            );
        }
        Ok(None) => {
            write_json_response(stream, "200 OK", br#"{"ok":true}"#);
        }
        Err(msg) => {
            write_json_value(
                stream,
                "400 Bad Request",
                &json!({ "ok": false, "message": msg }),
            );
        }
    }
}

fn probe_media_duration_ms(path: &Path) -> Option<u64> {
    probe_container_duration_ms(path)
}

fn configure_hidden_windows_command(cmd: &mut Command) -> &mut Command {
    #[cfg(target_os = "windows")]
    {
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    cmd
}

fn pick_native_media_files(multiselect: bool) -> Vec<(String, String, u64, Option<u64>)> {
    let raw_lines: Vec<String> = if cfg!(target_os = "macos") {
        let script = if multiselect {
            r#"
                set chosenFiles to choose file with prompt "Select local audio or video files for Audora" with multiple selections allowed
                set outText to ""
                repeat with f in chosenFiles
                    set outText to outText & (POSIX path of f) & linefeed
                end repeat
                return outText
            "#
        } else {
            r#"
                set chosenFile to choose file with prompt "Locate the moved audio or video file"
                return POSIX path of chosenFile
            "#
        };
        Command::new("/usr/bin/osascript")
            .args(["-e", script])
            .output()
            .ok()
            .filter(|o| o.status.success())
            .map(|o| {
                String::from_utf8_lossy(&o.stdout)
                    .lines()
                    .map(|s| s.trim_end_matches(['\r', '\n']).to_owned())
                    .filter(|s| !s.is_empty())
                    .collect()
            })
            .unwrap_or_default()
    } else if cfg!(target_os = "windows") {
        let multi_flag = if multiselect { "$true" } else { "$false" };
        let ps_script = format!(
            "[Console]::OutputEncoding = [System.Text.Encoding]::UTF8; Add-Type -AssemblyName System.Windows.Forms; $d = New-Object System.Windows.Forms.OpenFileDialog; $d.Multiselect = {multi_flag}; $d.Filter = 'Media Files|*.mp4;*.m4v;*.mov;*.webm;*.mkv;*.mp3;*.wav;*.flac;*.ogg;*.opus;*.m4a;*.aac|All Files|*.*'; if ($d.ShowDialog() -eq 'OK') {{ $d.FileNames | ForEach-Object {{ Write-Output $_ }} }}"
        );
        let mut cmd = Command::new("powershell");
        configure_hidden_windows_command(&mut cmd);
        cmd.args(["-NoProfile", "-Sta", "-Command", &ps_script])
            .output()
            .ok()
            .filter(|o| o.status.success())
            .map(|o| {
                String::from_utf8_lossy(&o.stdout)
                    .lines()
                    .map(|s| s.trim_end_matches(['\r', '\n']).to_owned())
                    .filter(|s| !s.is_empty())
                    .collect()
            })
            .unwrap_or_default()
    } else {
        let mut args = vec![
            "--file-selection",
            "--separator=\n",
            "--title=Select local audio or video files for Audora",
        ];
        if multiselect {
            args.insert(1, "--multiple");
        }
        Command::new("zenity")
            .args(&args)
            .output()
            .ok()
            .filter(|o| o.status.success())
            .map(|o| {
                String::from_utf8_lossy(&o.stdout)
                    .lines()
                    .map(|s| s.trim_end_matches(['\r', '\n']).to_owned())
                    .filter(|s| !s.is_empty())
                    .collect()
            })
            .unwrap_or_default()
    };

    let mut out = Vec::new();
    for line in raw_lines {
        let pb = PathBuf::from(&line);
        let Ok(meta) = fs::metadata(&pb) else {
            continue;
        };
        if !meta.is_file() {
            continue;
        }
        let name = pb
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or(&line)
            .to_owned();
        let canonical = canonical_path_string(&pb);
        let dur = probe_media_duration_ms(&pb);
        out.push((canonical, name, meta.len(), dur));
    }
    out
}

fn pick_native_folder() -> Option<String> {
    if cfg!(target_os = "macos") {
        let output = Command::new("/usr/bin/osascript")
            .args([
                "-e",
                r#"POSIX path of (choose folder with prompt "Select a local media folder for Audora")"#,
            ])
            .output()
            .ok()?;
        if !output.status.success() {
            return None;
        }
        let raw_stdout = String::from_utf8_lossy(&output.stdout);
        let chosen = normalize_folder_path(raw_stdout.trim_end_matches(['\r', '\n']));
        if chosen.is_empty() || !Path::new(&chosen).is_dir() {
            return None;
        }
        return Some(canonical_path_string(Path::new(&chosen)));
    }

    if cfg!(target_os = "windows") {
        let mut cmd = Command::new("powershell");
        configure_hidden_windows_command(&mut cmd);
        let output = cmd
            .args([
                "-NoProfile",
                "-Sta",
                "-Command",
                "[Console]::OutputEncoding = [System.Text.Encoding]::UTF8; Add-Type -AssemblyName System.Windows.Forms; $f = New-Object System.Windows.Forms.FolderBrowserDialog; $f.Description = 'Select a local media folder for Audora'; if ($f.ShowDialog() -eq 'OK') { Write-Output $f.SelectedPath }",
            ])
            .output()
            .ok()?;
        if !output.status.success() {
            return None;
        }
        let raw_stdout = String::from_utf8_lossy(&output.stdout);
        let chosen = normalize_folder_path(raw_stdout.trim_end_matches(['\r', '\n']));
        if chosen.is_empty() || !Path::new(&chosen).is_dir() {
            return None;
        }
        return Some(canonical_path_string(Path::new(&chosen)));
    }

    for (bin, args) in [
        (
            "zenity",
            vec![
                "--file-selection",
                "--directory",
                "--title=Select a local media folder for Audora",
            ],
        ),
        ("kdialog", vec!["--getexistingdirectory"]),
    ] {
        if let Ok(output) = Command::new(bin).args(&args).output() {
            if output.status.success() {
                let raw_stdout = String::from_utf8_lossy(&output.stdout);
                let chosen = normalize_folder_path(raw_stdout.trim_end_matches(['\r', '\n']));
                if !chosen.is_empty() && Path::new(&chosen).is_dir() {
                    return Some(canonical_path_string(Path::new(&chosen)));
                }
            }
        }
    }

    None
}

fn collect_media_files_in_dir(
    dir: &Path,
    depth: usize,
    max_depth: usize,
    max_files: usize,
    out: &mut Vec<(String, String, u64, Option<u64>)>,
) {
    if depth > max_depth || out.len() >= max_files {
        return;
    }
    let Ok(entries_iter) = fs::read_dir(dir) else {
        return;
    };
    let mut entries: Vec<PathBuf> = entries_iter
        .filter_map(Result::ok)
        .map(|e| e.path())
        .collect();
    entries.sort();

    for path in entries {
        if out.len() >= max_files {
            break;
        }
        let Some(name_str) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        if name_str.starts_with('.') {
            continue;
        }
        let Ok(meta) = fs::symlink_metadata(&path) else {
            continue;
        };
        if meta.file_type().is_symlink() {
            continue;
        }
        if meta.is_dir() {
            collect_media_files_in_dir(&path, depth.saturating_add(1), max_depth, max_files, out);
        } else if meta.is_file() && meta.len() > 0 {
            let ext = path
                .extension()
                .and_then(|s| s.to_str())
                .unwrap_or("")
                .to_ascii_lowercase();
            if is_supported_media_extension(&ext) {
                let dur = probe_media_duration_ms(&path);
                out.push((
                    canonical_path_string(&path),
                    name_str.to_owned(),
                    meta.len(),
                    dur,
                ));
            }
        }
    }
}

fn handle_scan_folder_request(
    stream: &mut TcpStream,
    raw_path: &str,
    allowlist: &Mutex<MediaAllowlist>,
) {
    let normalized = normalize_folder_path(raw_path);
    let candidate = Path::new(&normalized);
    if normalized.is_empty() || (!candidate.is_absolute() && !normalized.starts_with(r"\\")) {
        write_json_response(
            stream,
            "400 Bad Request",
            br#"{"ok":false,"message":"Folder not found or not accessible on this device."}"#,
        );
        return;
    }

    // Check `MediaAllowlist` (including pre-canonicalization UNC prefix verification) BEFORE
    // calling `candidate.is_dir()` so untrusted UNC paths never trigger outbound SMB probes!
    let remaining_depth = allowlist.lock().ok().and_then(|guard| {
        if guard.is_allowed_scan_dir(candidate) {
            guard.allowed_scan_remaining_depth(candidate)
        } else {
            None
        }
    });
    let Some(max_scan_depth) = remaining_depth else {
        if !MediaAllowlist::is_unc_raw_path(&normalized) && !candidate.is_dir() {
            write_json_response(
                stream,
                "400 Bad Request",
                br#"{"ok":false,"message":"Folder not found or not accessible on this device."}"#,
            );
        } else {
            write_json_response(
                stream,
                "403 Forbidden",
                br#"{"ok":false,"message":"Folder has not been authorized by the user."}"#,
            );
        }
        return;
    };

    if !candidate.is_dir() {
        write_json_response(
            stream,
            "400 Bad Request",
            br#"{"ok":false,"message":"Folder not found or not accessible on this device."}"#,
        );
        return;
    }

    let canon_folder = canonical_path_string(candidate);
    let folder_name = Path::new(&canon_folder)
        .file_name()
        .and_then(|s| s.to_str())
        .filter(|s| !s.is_empty())
        .unwrap_or(&canon_folder);

    let mut discovered: Vec<(String, String, u64, Option<u64>)> = Vec::new();
    collect_media_files_in_dir(candidate, 0, max_scan_depth, 2_000, &mut discovered);

    // Do NOT register folder-discovered files in `session_files` (`H-1`); their streaming access
    // is dynamically governed by `session_dirs` / `db_enabled_dirs` so disabling the folder revokes them.
    let files_val: Vec<serde_json::Value> = discovered
        .iter()
        .map(|(file_path, file_name, size, dur)| {
            json!({
                "path": file_path,
                "name": file_name,
                "size": size,
                "duration_ms": dur,
            })
        })
        .collect();

    write_json_value(
        stream,
        "200 OK",
        &json!({
            "ok": true,
            "folder_path": canon_folder,
            "folder_name": folder_name,
            "files": files_val,
        }),
    );
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn host_header_and_session_token_block_dns_rebinding_localhost_ipv6_and_null_origin() {
        let tok = generate_session_token();
        assert_eq!(tok.len(), 64);
        assert_ne!(tok, generate_session_token());

        assert!(is_valid_host_header(41957, Some("127.0.0.1:41957")));
        // `localhost` is intentionally rejected so `[::1]:port` hijackers cannot impersonate origin (`C-4`).
        assert!(!is_valid_host_header(41957, Some("localhost:41957")));
        assert!(!is_valid_host_header(41957, Some("[::1]:41957")));
        assert!(!is_valid_host_header(41957, Some("attacker.tld:41957")));
        assert!(!is_valid_host_header(41957, None));

        assert!(!is_trusted_loopback_request(41957, None, None, None, &tok));
        assert!(!is_trusted_loopback_request(
            41957,
            Some("http://127.0.0.1:41957"),
            Some("same-origin"),
            Some("wrong"),
            &tok
        ));
        assert!(!is_trusted_loopback_request(
            41957,
            Some("http://localhost:41957"),
            Some("same-origin"),
            Some(&tok),
            &tok
        ));
        assert!(!is_trusted_loopback_request(
            41957,
            Some("null"),
            Some("cross-site"),
            Some(&tok),
            &tok
        ));
        assert!(!is_trusted_loopback_request(
            41957,
            Some("null"),
            None,
            Some(&tok),
            &tok
        ));

        assert!(is_trusted_loopback_request(
            41957,
            Some("http://127.0.0.1:41957"),
            Some("same-origin"),
            Some(&tok),
            &tok
        ));
    }

    #[test]
    fn windows_verbatim_prefix_stripper_preserves_unc_network_shares() {
        assert_eq!(
            strip_windows_verbatim_prefix(r"\\?\UNC\NAS\Media\lecture.mp4"),
            PathBuf::from(r"\\NAS\Media\lecture.mp4")
        );
        assert_eq!(
            strip_windows_verbatim_prefix(r"\\?\C:\Users\karson\clip.mp4"),
            PathBuf::from(r"C:\Users\karson\clip.mp4")
        );
        assert_eq!(
            strip_windows_verbatim_prefix("/Users/karson/clip.mp4"),
            PathBuf::from("/Users/karson/clip.mp4")
        );
    }

    #[test]
    fn rfc9110_byte_ranges_handle_suffix_and_reject_inverted_or_oob() {
        assert_eq!(parse_http_byte_range("bytes=0-499", 1000), Ok((0, 499)));
        assert_eq!(parse_http_byte_range("bytes=500-", 1000), Ok((500, 999)));
        assert_eq!(parse_http_byte_range("bytes=-200", 1000), Ok((800, 999)));
        assert_eq!(parse_http_byte_range("bytes=-5000", 1000), Ok((0, 999)));
        assert!(parse_http_byte_range("bytes=500-100", 1000).is_err());
        assert!(parse_http_byte_range("bytes=1000-1005", 1000).is_err());
        assert!(parse_http_byte_range("bytes=-0", 1000).is_err());
    }

    #[test]
    fn media_allowlist_blocks_unregistered_files_and_symlinks_to_non_media() {
        let dir = env::temp_dir().join(format!(
            "audora_allowlist_test_{}",
            generate_session_token()
        ));
        fs::create_dir_all(&dir).unwrap();
        let valid_mp4 = dir.join("clip.mp4");
        let secret_txt = dir.join("id_rsa");
        fs::write(&valid_mp4, b"fake-mp4").unwrap();
        fs::write(&secret_txt, b"private-key").unwrap();

        let mut allowlist = MediaAllowlist::default();
        assert!(allowlist
            .resolve_allowed_media_file(&valid_mp4.to_string_lossy())
            .is_none());

        allowlist.register_file(&valid_mp4);
        allowlist.register_file(&secret_txt);
        assert!(allowlist
            .resolve_allowed_media_file(&valid_mp4.to_string_lossy())
            .is_some());
        assert!(allowlist
            .resolve_allowed_media_file(&secret_txt.to_string_lossy())
            .is_none());

        #[cfg(unix)]
        {
            let symlink_mp4 = dir.join("leak.mp4");
            std::os::unix::fs::symlink(&secret_txt, &symlink_mp4).unwrap();
            allowlist.register_dir(&dir);
            assert!(allowlist
                .resolve_allowed_media_file(&symlink_mp4.to_string_lossy())
                .is_none());
        }

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn allowlist_blocks_unpicked_rpc_escalation_and_revokes_deleted_or_disabled_sources() {
        let dir = env::temp_dir().join(format!(
            "audora_rel_allowlist_test_{}",
            generate_session_token()
        ));
        let folder_a = dir.join("folder_a");
        let folder_unpicked = dir.join("folder_unpicked");
        fs::create_dir_all(&folder_a).unwrap();
        fs::create_dir_all(&folder_unpicked).unwrap();
        let track_in_a = folder_a.join("song.flac");
        let track_unpicked = folder_unpicked.join("secret.mp3");
        let standalone_track = dir.join("standalone.mp4");
        fs::write(&track_in_a, b"fLaC-stub").unwrap();
        fs::write(&track_unpicked, b"ID3-secret").unwrap();
        fs::write(&standalone_track, b"mp4-stub").unwrap();

        let mut allowlist = MediaAllowlist::default();

        // Attempting to authorize an unpicked folder via RPC `sync_enabled_dirs` MUST fail (`H-1`).
        let unpicked_dto = audora_desktop::domain::TrustedSourceFolder {
            id: "src_evil".to_owned(),
            path: folder_unpicked.to_string_lossy().into_owned(),
            label: "unpicked".to_owned(),
            enabled: true,
            added_at: "2026-01-01T00:00:00Z".to_owned(),
            last_scanned_at: "2026-01-01T00:00:00Z".to_owned(),
            media_count: 1,
        };
        allowlist.sync_enabled_dirs(&[unpicked_dto]);
        assert!(!allowlist.is_allowed_scan_dir(&folder_unpicked));
        assert!(allowlist
            .resolve_allowed_media_file(&track_unpicked.to_string_lossy())
            .is_none());

        // OS-picked folder + standalone file succeed, and disabling/deleting immediately revokes access!
        allowlist.register_dir(&folder_a);
        allowlist.register_file(&standalone_track);
        let folder_dto = audora_desktop::domain::TrustedSourceFolder {
            id: "src_a".to_owned(),
            path: folder_a.to_string_lossy().into_owned(),
            label: "folder_a".to_owned(),
            enabled: true,
            added_at: "2026-01-01T00:00:00Z".to_owned(),
            last_scanned_at: "2026-01-01T00:00:00Z".to_owned(),
            media_count: 1,
        };
        let standalone_item = audora_desktop::domain::MediaItem {
            id: "m_1".to_owned(),
            path: standalone_track.to_string_lossy().into_owned(),
            display_name: "standalone".to_owned(),
            media_kind: audora_desktop::domain::MediaKind::Video,
            duration_ms: Some(1000),
            duration_probed: true,
            last_position_ms: 0,
            completed: false,
            file_size_bytes: Some(8),
            fingerprint: None,
            availability: audora_desktop::domain::Availability::Available,
            added_at: "2026-01-01T00:00:00Z".to_owned(),
            last_opened_at: None,
            last_seen_at: "2026-01-01T00:00:00Z".to_owned(),
            note_count: 0,
            source_folder_id: None,
        };
        let mut rel_state = audora_desktop::domain::RelationalAppStateDto {
            media: vec![standalone_item.clone()],
            notes: Vec::new(),
            playlists: Vec::new(),
            settings: audora_desktop::domain::AppSettings {
                source_folders: vec![folder_dto.clone()],
                ..audora_desktop::domain::AppSettings::default()
            },
            playback_session: Some(audora_desktop::domain::PersistedPlaybackSession::default()),
        };
        allowlist.sync_from_relational_state(&rel_state);
        assert!(allowlist.is_allowed_scan_dir(&folder_a));
        assert!(allowlist
            .resolve_allowed_media_file(&track_in_a.to_string_lossy())
            .is_some());
        assert!(allowlist
            .resolve_allowed_media_file(&standalone_track.to_string_lossy())
            .is_some());

        // Disabling folder AND deleting standalone item immediately revokes both!
        rel_state.media.clear();
        rel_state.settings.source_folders[0].enabled = false;
        allowlist.sync_from_relational_state(&rel_state);
        assert!(!allowlist.is_allowed_scan_dir(&folder_a));
        assert!(allowlist
            .resolve_allowed_media_file(&track_in_a.to_string_lossy())
            .is_none());
        assert!(allowlist
            .resolve_allowed_media_file(&standalone_track.to_string_lossy())
            .is_none());

        // Re-enabling the previously OS-picked folder or Undo-restoring the standalone item works!
        rel_state.media.push(standalone_item);
        rel_state.settings.source_folders[0].enabled = true;
        allowlist.sync_from_relational_state(&rel_state);
        assert!(allowlist
            .resolve_allowed_media_file(&track_in_a.to_string_lossy())
            .is_some());
        assert!(allowlist
            .resolve_allowed_media_file(&standalone_track.to_string_lossy())
            .is_some());

        // Deleting folder from `settings.source_folders` removes it from `os_trusted_dirs` permanently!
        rel_state.settings.source_folders.clear();
        allowlist.sync_from_relational_state(&rel_state);
        assert!(!allowlist.is_allowed_scan_dir(&folder_a));
        assert!(allowlist
            .resolve_allowed_media_file(&track_in_a.to_string_lossy())
            .is_none());
        // Attempting to re-add via RPC without OS picker fails because `os_trusted_dirs` was revoked!
        rel_state.settings.source_folders.push(folder_dto);
        allowlist.sync_from_relational_state(&rel_state);
        assert!(!allowlist.is_allowed_scan_dir(&folder_a));

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn native_drop_descriptors_keep_dirs_unflattened_and_handle_zero_byte_files() {
        let dir = env::temp_dir().join(format!("audora_drop_test_{}", generate_session_token()));
        let dropped_folder = dir.join("Course Lectures");
        fs::create_dir_all(&dropped_folder).unwrap();
        let child_inside_folder = dropped_folder.join("lesson1.mp4");
        let empty_file = dir.join("empty.mp3");
        let valid_file = dir.join("standalone.wav");
        fs::write(&child_inside_folder, b"child-video").unwrap();
        fs::write(&empty_file, b"").unwrap();
        fs::write(&valid_file, b"RIFF-stub").unwrap();

        let allowlist = Mutex::new(MediaAllowlist::default());
        let descriptors = build_native_drop_descriptors(
            &[
                dropped_folder.clone(),
                empty_file.clone(),
                valid_file.clone(),
            ],
            &allowlist,
        );
        assert_eq!(descriptors.len(), 3);

        // 1. Directory is emitted as `{ kind: "dir", size: 0, duration_ms: null }` without flattening children!
        assert_eq!(descriptors[0]["kind"], "dir");
        assert_eq!(descriptors[0]["name"], "Course Lectures");
        assert_eq!(descriptors[0]["size"], 0);
        assert!(descriptors[0]["duration_ms"].is_null());

        // 2. 0-byte file is included in descriptors (`size: 0`) but NOT registered in allowlist!
        assert_eq!(descriptors[1]["kind"], "file");
        assert_eq!(descriptors[1]["name"], "empty.mp3");
        assert_eq!(descriptors[1]["size"], 0);
        assert!(descriptors[1]["duration_ms"].is_null());

        // 3. Valid non-empty file is included and registered in allowlist.
        assert_eq!(descriptors[2]["kind"], "file");
        assert_eq!(descriptors[2]["name"], "standalone.wav");
        assert_eq!(descriptors[2]["size"], 9);

        let mut guard = allowlist.lock().unwrap();
        // Child inside dropped directory MUST NOT be in `os_trusted_files`!
        let child_canon = canonical_existing_path(&child_inside_folder).unwrap();
        assert!(!guard
            .os_trusted_files
            .contains(&normalize_allowlist_key(&child_canon)));
        // 0-byte file MUST NOT resolve in allowlist!
        assert!(guard
            .resolve_allowed_media_file(&empty_file.to_string_lossy())
            .is_none());
        // Valid file resolves, and revoking `dropped_folder` immediately blocks `child_inside_folder`!
        assert!(guard
            .resolve_allowed_media_file(&valid_file.to_string_lossy())
            .is_some());
        guard.revoke_source_folder(&dropped_folder.to_string_lossy());
        assert!(guard
            .resolve_allowed_media_file(&child_inside_folder.to_string_lossy())
            .is_none());

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn allowlist_enforces_unc_prefix_precheck_dot_dirs_and_max_depth_four() {
        let dir = env::temp_dir().join(format!("audora_depth_test_{}", generate_session_token()));
        let root = dir.join("MediaRoot");
        let hidden_dir = root.join(".secret");
        let depth4_dir = root.join("d1").join("d2").join("d3").join("d4");
        let depth5_dir = depth4_dir.join("d5");
        fs::create_dir_all(&hidden_dir).unwrap();
        fs::create_dir_all(&depth5_dir).unwrap();

        let hidden_track = hidden_dir.join("leak.mp3");
        let depth4_track = depth4_dir.join("ok.mp3");
        let depth5_track = depth5_dir.join("too_deep.mp3");
        fs::write(&hidden_track, b"mp3").unwrap();
        fs::write(&depth4_track, b"mp3").unwrap();
        fs::write(&depth5_track, b"mp3").unwrap();

        let mut allowlist = MediaAllowlist::default();
        allowlist.register_dir(&root);

        // Hidden dot-directories are blocked for both scanning and media resolution.
        assert!(!allowlist.is_allowed_scan_dir(&hidden_dir));
        assert!(allowlist
            .resolve_allowed_media_file(&hidden_track.to_string_lossy())
            .is_none());

        // Depth 4 succeeds; depth 5 is rejected!
        assert!(allowlist.is_allowed_scan_dir(&depth4_dir));
        assert_eq!(allowlist.allowed_scan_remaining_depth(&depth4_dir), Some(0));
        assert!(!allowlist.is_allowed_scan_dir(&depth5_dir));
        assert!(allowlist
            .resolve_allowed_media_file(&depth4_track.to_string_lossy())
            .is_some());
        assert!(allowlist
            .resolve_allowed_media_file(&depth5_track.to_string_lossy())
            .is_none());

        // Untrusted UNC paths are rejected lexically before touching the network/filesystem.
        assert!(!allowlist.has_trusted_unc_prefix(r"\\198.51.100.77\share\clip.mp4"));
        assert!(!allowlist.is_allowed_scan_dir(Path::new(r"\\198.51.100.77\share")));
        assert!(allowlist
            .resolve_allowed_media_file(r"\\198.51.100.77\share\clip.mp4")
            .is_none());

        // Trusted UNC prefix matches descendants without `..`, but blocks `..` traversal and sibling shares.
        allowlist
            .os_trusted_dirs
            .insert(PathBuf::from(r"\\nas\media"));
        assert!(allowlist.has_trusted_unc_prefix(r"\\NAS\Media\Lectures\week1.mp4"));
        assert!(allowlist.has_trusted_unc_prefix(r"\\?\UNC\NAS\Media\Lectures"));
        assert!(!allowlist.has_trusted_unc_prefix(r"\\NAS\Media\..\..\evil\share"));
        assert!(!allowlist.has_trusted_unc_prefix(r"\\NAS\MediaOther\week1.mp4"));

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn url_sanitizer_trailing_space_paths_and_loopback_http_hardening() {
        // External HTTPS URL hardening
        assert!(is_safe_external_https_url(
            "https://github.com/smokeInCloud060201/audora/releases/tag/v1.1.2"
        ));
        assert!(!is_safe_external_https_url(
            "http://github.com/smokeInCloud060201/audora/"
        ));
        for bad in [
            "https://github.com/smokeInCloud060201/audora/ \"--evil",
            "https://github.com/smokeInCloud060201/audora/,calc.exe",
            "https://github.com/smokeInCloud060201/audora/`id`",
            "https://github.com/smokeInCloud060201/audora/'quote",
            "https://github.com/smokeInCloud060201/audora/\r\n",
        ] {
            assert!(!is_safe_external_https_url(bad), "must reject {bad}");
        }

        // Preserve valid trailing space in folder names while stripping `/` and `\r\n`
        assert_eq!(
            normalize_folder_path("/tmp/My Studio /\r\n"),
            "/tmp/My Studio "
        );
        assert_eq!(normalize_folder_path("/\n"), "/");
        assert_eq!(normalize_folder_path("C:\\\r\n"), r"C:\");

        // Static server hardening (`GET`/`HEAD`, missing asset extension `404`, SPA fallback, symlink containment, `Transfer-Encoding` rejection)
        let ui_dir = env::temp_dir().join(format!("audora_http_test_{}", generate_session_token()));
        let outside_dir =
            env::temp_dir().join(format!("audora_outside_{}", generate_session_token()));
        fs::create_dir_all(ui_dir.join("assets")).unwrap();
        fs::create_dir_all(&outside_dir).unwrap();
        fs::write(
            ui_dir.join("index.html"),
            b"<!doctype html><title>Audora</title>",
        )
        .unwrap();
        fs::write(ui_dir.join("assets/app.js"), b"console.log(1);").unwrap();
        let outside_secret = outside_dir.join("secret.js");
        fs::write(&outside_secret, b"top-secret").unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(&outside_secret, ui_dir.join("assets/escape.js")).unwrap();

        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let port = listener.local_addr().unwrap().port();
        let ui_dir_bg = ui_dir.clone();
        let server_thread = thread::spawn(move || {
            for _ in 0..6 {
                let (mut stream, _) = listener.accept().unwrap();
                if let Some(req) = read_http_request(&mut stream) {
                    let (raw_path, _) = req
                        .raw_target
                        .split_once('?')
                        .unwrap_or((&req.raw_target, ""));
                    serve_static_ui_asset(&mut stream, &ui_dir_bg, &req.method, raw_path);
                }
            }
        });

        let send_raw = |raw_req: &str| -> String {
            let mut client = TcpStream::connect(("127.0.0.1", port)).unwrap();
            client.write_all(raw_req.as_bytes()).unwrap();
            client.flush().unwrap();
            let mut resp = String::new();
            let _ = client.read_to_string(&mut resp);
            resp
        };

        // 1. Transfer-Encoding header is rejected immediately
        let te_resp = send_raw(&format!(
            "POST / HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nTransfer-Encoding: chunked\r\n\r\n0\r\n\r\n"
        ));
        assert!(te_resp.is_empty());

        // 2. POST to static asset returns 405 Method Not Allowed
        let post_resp = send_raw(&format!(
            "POST /index.html HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nContent-Length: 0\r\n\r\n"
        ));
        assert!(post_resp.starts_with("HTTP/1.1 405"));

        // 3. HEAD returns Content-Length with empty body
        let head_resp = send_raw(&format!(
            "HEAD /assets/app.js HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\n\r\n"
        ));
        assert!(head_resp.starts_with("HTTP/1.1 200 OK"));
        assert!(head_resp.contains("Content-Length: 15\r\n"));
        assert!(head_resp.ends_with("\r\n\r\n"));

        // 4. Missing file with extension returns 404 (not SPA index.html)
        let missing_js = send_raw(&format!(
            "GET /assets/missing.js HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\n\r\n"
        ));
        assert!(missing_js.starts_with("HTTP/1.1 404"));

        // 5. Extensionless SPA route serves index.html
        let spa_resp = send_raw(&format!(
            "GET /playlists HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\n\r\n"
        ));
        assert!(spa_resp.starts_with("HTTP/1.1 200 OK"));
        assert!(spa_resp.contains("<title>Audora</title>"));

        // 6. Symlink escaping `ctx.root` is blocked with 404
        let escape_resp = send_raw(&format!(
            "GET /assets/escape.js HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\n\r\n"
        ));
        assert!(escape_resp.starts_with("HTTP/1.1 404"));
        assert!(!escape_resp.contains("top-secret"));

        server_thread.join().unwrap();
        let _ = fs::remove_dir_all(&ui_dir);
        let _ = fs::remove_dir_all(&outside_dir);
    }
}
