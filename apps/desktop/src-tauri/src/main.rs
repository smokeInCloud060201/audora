//! Audora desktop application entrypoint (`tao` + `wry` native shell with self-update and uninstall cleanup).

#![cfg_attr(
    all(not(debug_assertions), target_os = "windows"),
    windows_subsystem = "windows"
)]

use std::collections::{HashMap, HashSet};
use std::env;
use std::fs::{self, File};
use std::io::{Read, Seek, SeekFrom, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Component, Path, PathBuf};
use std::process::{Command, Stdio};
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
    SetWindowMode {
        minimum: bool,
    },
    DragWindow,
    NativeDragState(bool),
    NativeFilesDropped {
        paths: Vec<PathBuf>,
        auto_play: bool,
    },
    NativeDropDescriptorsReady {
        json: String,
        auto_play: bool,
    },
    WebViewPageLoaded,
}

const MAX_HTTP_BODY_BYTES: usize = 32 * 1024 * 1024;
const MAX_CONCURRENT_HTTP_CONNECTIONS: usize = 64;
const HTTP_REQUEST_DEADLINE_SECS: u64 = 10;
const MAX_NATIVE_DROP_PATHS: usize = 500;
const MAX_CLIPBOARD_BYTES: usize = 65_536;
const DEFAULT_FULL_WIDTH: f64 = 1440.0;
const DEFAULT_FULL_HEIGHT: f64 = 900.0;
const MIN_FULL_WIDTH: f64 = 1040.0;
const MIN_FULL_HEIGHT: f64 = 640.0;
const MINIMUM_WIDGET_WIDTH: f64 = 360.0;
const MINIMUM_WIDGET_HEIGHT: f64 = 168.0;

const WINDOW_ICON_RGBA_32: &[u8; 4096] = include_bytes!("../../../../assets/icons/32x32.rgba");

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
    /// Maps configured/picked lexical directory keys to their last resolved canonical key (`None` while disconnected).
    trusted_dir_sources: HashMap<PathBuf, Option<PathBuf>>,
    session_files: HashSet<PathBuf>,
    session_dirs: HashSet<PathBuf>,
    db_standalone_files: HashSet<PathBuf>,
    db_enabled_dirs: HashSet<PathBuf>,
    db_all_source_dirs: HashSet<PathBuf>,
}

impl MediaAllowlist {
    fn is_unc_slash(b: u8) -> bool {
        b == b'\\' || b == b'/'
    }

    fn is_unc_raw_path(raw: &str) -> bool {
        let trimmed = raw.trim_start().trim_end_matches(['\r', '\n']);
        let bytes = trimmed.as_bytes();
        if bytes.len() < 2 || !Self::is_unc_slash(bytes[0]) || !Self::is_unc_slash(bytes[1]) {
            return false;
        }
        if bytes.len() >= 4
            && (bytes[2] == b'?' || bytes[2] == b'.')
            && Self::is_unc_slash(bytes[3])
        {
            if bytes.len() >= 8
                && bytes[4..7].eq_ignore_ascii_case(b"UNC")
                && Self::is_unc_slash(bytes[7])
            {
                return true;
            }
            let rest = &trimmed[4..];
            let is_drive = rest.len() >= 2
                && rest.as_bytes()[0].is_ascii_alphabetic()
                && rest.as_bytes()[1] == b':';
            return !is_drive;
        }
        true
    }

    fn lexical_normalize_unc(raw: &str) -> Option<String> {
        let trimmed = raw.trim_start().trim_end_matches(['\r', '\n']);
        if trimmed.chars().any(char::is_control) {
            return None;
        }
        let bytes = trimmed.as_bytes();
        if bytes.len() < 2 || !Self::is_unc_slash(bytes[0]) || !Self::is_unc_slash(bytes[1]) {
            return None;
        }
        let tail = if bytes.len() >= 4
            && (bytes[2] == b'?' || bytes[2] == b'.')
            && Self::is_unc_slash(bytes[3])
        {
            if bytes.len() >= 8
                && bytes[4..7].eq_ignore_ascii_case(b"UNC")
                && Self::is_unc_slash(bytes[7])
            {
                &trimmed[8..]
            } else {
                let rest = &trimmed[4..];
                let is_drive = rest.len() >= 2
                    && rest.as_bytes()[0].is_ascii_alphabetic()
                    && rest.as_bytes()[1] == b':';
                if is_drive {
                    return None;
                }
                rest
            }
        } else {
            trimmed.trim_start_matches(['\\', '/'])
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

    fn lexical_key_for_path(raw: &str) -> Option<PathBuf> {
        let cleaned = raw.trim_start().trim_end_matches(['\r', '\n']);
        if cleaned.trim().is_empty() {
            return None;
        }
        if Self::is_unc_raw_path(cleaned) {
            return Self::lexical_normalize_unc(cleaned).map(PathBuf::from);
        }
        Some(normalize_allowlist_key(&strip_windows_verbatim_prefix(
            &normalize_folder_path(cleaned),
        )))
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
        if Self::is_unc_raw_path(cleaned) {
            return Self::lexical_normalize_unc(cleaned).map(PathBuf::from);
        }
        let p = Path::new(cleaned);
        let resolved = canonical_existing_path(p)
            .unwrap_or_else(|| strip_windows_verbatim_prefix(&normalize_folder_path(cleaned)));
        Some(normalize_allowlist_key(&resolved))
    }

    fn register_resolved_file(&mut self, canon: &Path) {
        let key = normalize_allowlist_key(canon);
        self.os_trusted_files.insert(key.clone());
        self.session_files.insert(key);
    }

    fn register_file(&mut self, path: &Path) {
        if let Some(canon) = canonical_existing_path(path) {
            self.register_resolved_file(&canon);
        }
    }

    fn register_resolved_dir(&mut self, raw_path: &Path, canon: &Path) {
        let canon_key = normalize_allowlist_key(canon);
        self.os_trusted_dirs.insert(canon_key.clone());
        self.session_dirs.insert(canon_key.clone());
        if let Some(lex_key) = Self::lexical_key_for_path(&raw_path.to_string_lossy()) {
            self.trusted_dir_sources
                .insert(lex_key, Some(canon_key.clone()));
        }
        self.trusted_dir_sources
            .insert(canon_key.clone(), Some(canon_key));
    }

    fn register_dir(&mut self, path: &Path) {
        if let Some(canon) = canonical_existing_path(path) {
            self.register_resolved_dir(path, &canon);
        }
    }

    fn revoke_source_folder(&mut self, raw_path: &str) {
        let cleaned = raw_path.trim_start().trim_end_matches(['\r', '\n']);
        if cleaned.trim().is_empty() {
            return;
        }
        let is_unc = Self::is_unc_raw_path(cleaned);
        // Evaluate UNC trust BEFORE mutating `os_trusted_dirs`.
        let unc_was_trusted = is_unc && self.has_trusted_unc_prefix(cleaned);

        let mut keys_to_remove: HashSet<PathBuf> = HashSet::new();
        let stripped_path = strip_windows_verbatim_prefix(&normalize_folder_path(cleaned));
        let lexical_key = normalize_allowlist_key(&stripped_path);
        keys_to_remove.insert(lexical_key.clone());

        if is_unc {
            if let Some(unc_norm) = Self::lexical_normalize_unc(cleaned) {
                keys_to_remove.insert(PathBuf::from(unc_norm));
            }
        }

        if let Some(Some(mapped_canon)) = self.trusted_dir_sources.get(&lexical_key) {
            keys_to_remove.insert(mapped_canon.clone());
        }

        if !is_unc || unc_was_trusted {
            let resolved = if Path::new(cleaned).exists() {
                canonical_existing_path(Path::new(cleaned))
            } else if !is_unc {
                canonicalize_with_existing_ancestor(&stripped_path)
            } else {
                None
            };
            if let Some(canon) = resolved {
                keys_to_remove.insert(normalize_allowlist_key(&canon));
            }
        }

        for key in &keys_to_remove {
            self.os_trusted_dirs.remove(key);
            self.session_dirs.remove(key);
            self.db_enabled_dirs.remove(key);
            self.db_all_source_dirs.remove(key);
        }
        self.trusted_dir_sources.retain(|k, v| {
            !keys_to_remove.contains(k) && v.as_ref().is_none_or(|c| !keys_to_remove.contains(c))
        });
    }

    fn clear(&mut self) {
        self.os_trusted_files.clear();
        self.os_trusted_dirs.clear();
        self.trusted_dir_sources.clear();
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
            let cleaned = folder.path.trim_start().trim_end_matches(['\r', '\n']);
            if cleaned.trim().is_empty() {
                continue;
            }
            let Some(lex_key) = Self::lexical_key_for_path(cleaned) else {
                continue;
            };
            let canon_opt = if Self::is_unc_raw_path(cleaned) {
                None
            } else {
                canonical_existing_path(Path::new(cleaned)).map(|c| normalize_allowlist_key(&c))
            };
            self.trusted_dir_sources
                .insert(lex_key.clone(), canon_opt.clone());
            self.os_trusted_dirs.insert(lex_key.clone());
            self.db_all_source_dirs.insert(lex_key);
            if let Some(canon_key) = canon_opt {
                self.trusted_dir_sources
                    .insert(canon_key.clone(), Some(canon_key.clone()));
                self.os_trusted_dirs.insert(canon_key.clone());
                self.db_all_source_dirs.insert(canon_key);
            }
        }
        self.sync_from_relational_state(state);
    }

    fn promote_files_in_trusted_dirs(&mut self, resolved_paths: &[(String, Option<PathBuf>)]) {
        for (raw_path, pre_canon) in resolved_paths {
            let cleaned = raw_path.trim_start().trim_end_matches(['\r', '\n']);
            if cleaned.trim().is_empty() {
                continue;
            }
            if Self::is_unc_raw_path(cleaned) && !self.has_trusted_unc_prefix(cleaned) {
                continue;
            }
            let Some(key) = pre_canon
                .clone()
                .or_else(|| Self::lexical_key_for_path(cleaned))
            else {
                continue;
            };
            if self
                .os_trusted_dirs
                .iter()
                .any(|d| Self::is_clean_relative_file_within_depth(d, &key))
            {
                self.os_trusted_files.insert(key);
            }
        }
    }

    fn sync_enabled_dirs_resolved(
        &mut self,
        resolved_folders: &[(audora_desktop::domain::TrustedSourceFolder, Option<PathBuf>)],
    ) {
        let mut next_enabled = HashSet::new();
        let mut next_configured = HashSet::new();
        let mut next_bindings: Vec<(PathBuf, Option<PathBuf>)> = Vec::new();

        for (folder, pre_canon) in resolved_folders {
            let cleaned = folder.path.trim_start().trim_end_matches(['\r', '\n']);
            if cleaned.trim().is_empty() {
                continue;
            }
            if Self::is_unc_raw_path(cleaned) && !self.has_trusted_unc_prefix(cleaned) {
                continue;
            }
            let Some(lex_key) = Self::lexical_key_for_path(cleaned) else {
                continue;
            };
            let known_binding = self.trusted_dir_sources.get(&lex_key).cloned();
            let is_direct_trusted = known_binding.is_some()
                || self.os_trusted_dirs.contains(&lex_key)
                || pre_canon
                    .as_ref()
                    .is_some_and(|c| self.os_trusted_dirs.contains(c));
            let is_clean_subfolder = !is_direct_trusted
                && pre_canon.as_ref().is_some_and(|c| {
                    self.os_trusted_dirs
                        .iter()
                        .any(|d| Self::relative_dir_depth_if_clean(d, c).is_some())
                });

            if !is_direct_trusted && !is_clean_subfolder {
                continue;
            }

            // Preserve configured binding & canonical target across temporary drive disconnects;
            // dynamically resolve canonical key once the drive reconnects.
            let effective_canon = pre_canon.clone().or_else(|| known_binding.flatten());

            next_configured.insert(lex_key.clone());
            if let Some(ref canon_key) = effective_canon {
                self.os_trusted_dirs.insert(canon_key.clone());
                next_configured.insert(canon_key.clone());
            }
            if is_direct_trusted {
                self.os_trusted_dirs.insert(lex_key.clone());
                next_bindings.push((lex_key.clone(), effective_canon.clone()));
            }

            if folder.enabled {
                next_enabled.insert(lex_key);
                if let Some(canon_key) = effective_canon {
                    next_enabled.insert(canon_key);
                }
            } else {
                // Disabling a folder revokes any session-picked dir entry for it (`H-1`),
                // while retaining `os_trusted_dirs` so toggling it back on works cleanly.
                self.session_dirs.remove(&lex_key);
                if let Some(ref canon_key) = effective_canon {
                    self.session_dirs.remove(canon_key);
                }
            }
        }

        // Any source folder deleted from `settings.source_folders` is permanently revoked.
        let mut removed_keys = HashSet::new();
        for old_configured in &self.db_all_source_dirs {
            if !next_configured.contains(old_configured) {
                self.os_trusted_dirs.remove(old_configured);
                self.session_dirs.remove(old_configured);
                removed_keys.insert(old_configured.clone());
            }
        }
        for (k, v) in next_bindings {
            self.trusted_dir_sources.insert(k, v);
        }
        if !removed_keys.is_empty() {
            self.trusted_dir_sources.retain(|k, v| {
                !removed_keys.contains(k) && v.as_ref().is_none_or(|c| !removed_keys.contains(c))
            });
        }
        for old_dir in &self.db_enabled_dirs {
            if !next_enabled.contains(old_dir) {
                self.session_dirs.remove(old_dir);
            }
        }
        self.db_all_source_dirs = next_configured;
        self.db_enabled_dirs = next_enabled;
    }

    #[cfg_attr(not(test), allow(dead_code))]
    fn sync_enabled_dirs(&mut self, folders: &[audora_desktop::domain::TrustedSourceFolder]) {
        let resolved: Vec<(audora_desktop::domain::TrustedSourceFolder, Option<PathBuf>)> = folders
            .iter()
            .map(|folder| {
                let cleaned = folder.path.trim_start().trim_end_matches(['\r', '\n']);
                let canon = if !cleaned.trim().is_empty()
                    && (!Self::is_unc_raw_path(cleaned) || self.has_trusted_unc_prefix(cleaned))
                {
                    canonical_existing_path(Path::new(cleaned)).map(|c| normalize_allowlist_key(&c))
                } else {
                    None
                };
                (folder.clone(), canon)
            })
            .collect();
        self.sync_enabled_dirs_resolved(&resolved);
    }

    fn sync_from_resolved(
        &mut self,
        standalone_resolved: &[(String, Option<PathBuf>)],
        resolved_folders: &[(audora_desktop::domain::TrustedSourceFolder, Option<PathBuf>)],
    ) {
        let mut next_standalone = HashSet::new();
        for (raw_path, pre_canon) in standalone_resolved {
            let cleaned = raw_path.trim_start().trim_end_matches(['\r', '\n']);
            if cleaned.trim().is_empty() {
                continue;
            }
            if Self::is_unc_raw_path(cleaned) && !self.has_trusted_unc_prefix(cleaned) {
                continue;
            }
            let Some(key) = pre_canon
                .clone()
                .or_else(|| Self::lexical_key_for_path(cleaned))
            else {
                continue;
            };
            // Promote detached standalone tracks residing cleanly inside an active
            // `os_trusted_dirs` entry into `os_trusted_files` BEFORE folder pruning.
            let inside_trusted_dir = self
                .os_trusted_dirs
                .iter()
                .any(|d| Self::is_clean_relative_file_within_depth(d, &key));
            if self.os_trusted_files.contains(&key) || inside_trusted_dir {
                if inside_trusted_dir {
                    self.os_trusted_files.insert(key.clone());
                }
                next_standalone.insert(key);
            }
        }
        for old_file in &self.db_standalone_files {
            if !next_standalone.contains(old_file) {
                self.session_files.remove(old_file);
            }
        }
        self.db_standalone_files = next_standalone;
        self.sync_enabled_dirs_resolved(resolved_folders);
    }

    fn sync_from_sources(
        &mut self,
        standalone_paths: &[String],
        source_folders: &[audora_desktop::domain::TrustedSourceFolder],
    ) {
        let resolved_standalone: Vec<(String, Option<PathBuf>)> = standalone_paths
            .iter()
            .map(|raw_path| {
                let cleaned = raw_path.trim_start().trim_end_matches(['\r', '\n']);
                let canon = if !cleaned.trim().is_empty()
                    && (!Self::is_unc_raw_path(cleaned) || self.has_trusted_unc_prefix(cleaned))
                {
                    canonical_existing_path(Path::new(cleaned)).map(|c| normalize_allowlist_key(&c))
                } else {
                    None
                };
                (raw_path.clone(), canon)
            })
            .collect();
        let resolved_folders: Vec<(audora_desktop::domain::TrustedSourceFolder, Option<PathBuf>)> =
            source_folders
                .iter()
                .map(|folder| {
                    let cleaned = folder.path.trim_start().trim_end_matches(['\r', '\n']);
                    let canon = if !cleaned.trim().is_empty()
                        && (!Self::is_unc_raw_path(cleaned) || self.has_trusted_unc_prefix(cleaned))
                    {
                        canonical_existing_path(Path::new(cleaned))
                            .map(|c| normalize_allowlist_key(&c))
                    } else {
                        None
                    };
                    (folder.clone(), canon)
                })
                .collect();
        self.sync_from_resolved(&resolved_standalone, &resolved_folders);
    }

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
        if !candidate.is_absolute() && !Self::is_unc_raw_path(cleaned) {
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

fn resolve_paths_outside_allowlist_lock(
    paths: &[String],
    allowlist: &Mutex<MediaAllowlist>,
) -> Vec<(String, Option<PathBuf>)> {
    let any_unc = paths.iter().any(|p| MediaAllowlist::is_unc_raw_path(p));
    let allow_fs_flags: Option<Vec<bool>> = if any_unc {
        allowlist.lock().ok().map(|guard| {
            paths
                .iter()
                .map(|p| !MediaAllowlist::is_unc_raw_path(p) || guard.has_trusted_unc_prefix(p))
                .collect()
        })
    } else {
        None
    };

    paths
        .iter()
        .enumerate()
        .map(|(idx, raw)| {
            let allow_fs = allow_fs_flags
                .as_ref()
                .and_then(|flags| flags.get(idx).copied())
                .unwrap_or_else(|| !MediaAllowlist::is_unc_raw_path(raw));
            let canon = if allow_fs {
                let cleaned = raw.trim_start().trim_end_matches(['\r', '\n']);
                if cleaned.trim().is_empty() {
                    None
                } else {
                    canonical_existing_path(Path::new(cleaned)).map(|c| normalize_allowlist_key(&c))
                }
            } else {
                None
            };
            (raw.clone(), canon)
        })
        .collect()
}

fn promote_noted_db_media_in_trusted_dirs(db: &Database, allowlist: &Mutex<MediaAllowlist>) {
    let Ok(noted_paths) = db.with_conn(|conn| {
        let mut stmt = conn.prepare(
            "SELECT DISTINCT m.path FROM media m INNER JOIN notes n ON n.media_id = m.id",
        )?;
        let rows = stmt.query_map([], |r| r.get::<_, String>(0))?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        Ok(out)
    }) else {
        return;
    };
    if noted_paths.is_empty() {
        return;
    }
    let resolved = resolve_paths_outside_allowlist_lock(&noted_paths, allowlist);
    if let Ok(mut guard) = allowlist.lock() {
        guard.promote_files_in_trusted_dirs(&resolved);
    }
}

fn sync_allowlist_from_db(db: &Database, allowlist: &Mutex<MediaAllowlist>) {
    let Ok((standalone_paths, folders)) = db.query_allowlist_sources() else {
        if let Ok(mut guard) = allowlist.lock() {
            guard.db_standalone_files.clear();
            guard.db_enabled_dirs.clear();
        }
        return;
    };

    let resolved_standalone = resolve_paths_outside_allowlist_lock(&standalone_paths, allowlist);
    let folder_paths: Vec<String> = folders.iter().map(|f| f.path.clone()).collect();
    let resolved_folder_paths = resolve_paths_outside_allowlist_lock(&folder_paths, allowlist);
    let resolved_folders: Vec<(audora_desktop::domain::TrustedSourceFolder, Option<PathBuf>)> =
        folders
            .into_iter()
            .zip(resolved_folder_paths)
            .map(|(folder, (_, canon))| (folder, canon))
            .collect();

    if let Ok(mut guard) = allowlist.lock() {
        guard.sync_from_resolved(&resolved_standalone, &resolved_folders);
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
    for pb in paths.iter().take(MAX_NATIVE_DROP_PATHS) {
        if pb.is_dir() {
            if let Some(canon_pb) = canonical_existing_path(pb) {
                if let Ok(mut guard) = allowlist.lock() {
                    guard.register_resolved_dir(pb, &canon_pb);
                }
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
                    if let Some(canon_pb) = canonical_existing_path(pb) {
                        if let Ok(mut guard) = allowlist.lock() {
                            guard.register_resolved_file(&canon_pb);
                        }
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

struct ActiveMutatingRpcGuard {
    counter: Arc<AtomicUsize>,
}

impl ActiveMutatingRpcGuard {
    fn enter(counter: &Arc<AtomicUsize>) -> Self {
        counter.fetch_add(1, Ordering::SeqCst);
        Self {
            counter: Arc::clone(counter),
        }
    }
}

impl Drop for ActiveMutatingRpcGuard {
    fn drop(&mut self) {
        self.counter.fetch_sub(1, Ordering::SeqCst);
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
        match db.get_ui_state_json() {
            Ok(None) => {
                if let Ok(raw) = fs::read_to_string(&legacy_state_file) {
                    if serde_json::from_str::<serde_json::Value>(&raw).is_ok_and(|v| v.is_object())
                        && db.set_ui_state_json(&raw).is_ok()
                    {
                        let _ = fs::remove_file(&legacy_state_file);
                    }
                }
            }
            Ok(Some(_)) => {
                let _ = fs::remove_file(&legacy_state_file);
            }
            Err(err) => {
                warn!(
                    code = err.code().as_str(),
                    "Preserving legacy ui-state.json after transient DB read error"
                );
            }
        }
    }

    sweep_stale_update_staging_dirs(&app_data_dir);

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
                let start_idx = cli_boot_files.len();
                collect_media_files_in_dir(&pb, 0, 0, 200, &mut cli_boot_files);
                if let Ok(mut guard) = allowlist.lock() {
                    for (discovered_path, _, _, _) in &cli_boot_files[start_idx..] {
                        guard.register_file(Path::new(discovered_path));
                    }
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

    let Some((server_url, server_port, shutting_down, active_mutating_rpcs)) =
        start_local_ui_server(
            ui_dist,
            proxy.clone(),
            Arc::clone(&dispatcher),
            db,
            Arc::clone(&session_token),
            Arc::clone(&allowlist),
        )
    else {
        error!("Failed to bind local UI loopback server");
        std::process::exit(1);
    };

    info!(url = %server_url, "Launching Audora desktop window");

    let start_in_minimum = initial_window_mode == WindowMode::Minimum;
    let window_icon = tao::window::Icon::from_rgba(WINDOW_ICON_RGBA_32.to_vec(), 32, 32).ok();
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
    }
    .with_window_icon(window_icon);

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
    let audora_initial_theme = match initial_theme {
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
        "initial_theme": audora_initial_theme,
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
        var initialTheme = '{audora_initial_theme}';
        var resolvedInitialTheme = initialTheme === 'light'
          ? 'light'
          : (initialTheme === 'dark'
            ? 'dark'
            : (window.matchMedia && window.matchMedia('(prefers-color-scheme: light)').matches ? 'light' : 'dark'));
        if (document.documentElement) {{
          document.documentElement.dataset.theme = resolvedInitialTheme;
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
        function setNativeControlValue(target, nextValue, caretPos) {{
          var proto = target.tagName === 'TEXTAREA' ? HTMLTextAreaElement.prototype : HTMLInputElement.prototype;
          var desc = Object.getOwnPropertyDescriptor(proto, 'value');
          if (desc && typeof desc.set === 'function') {{
            desc.set.call(target, nextValue);
          }} else {{
            target.value = nextValue;
          }}
          if (typeof target.setSelectionRange === 'function') {{
            try {{
              target.setSelectionRange(caretPos, caretPos);
            }} catch (_) {{}}
          }}
          target.dispatchEvent(new Event('input', {{ bubbles: true }}));
        }}
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
              }} else if (k === 'c') {{
                var selText = '';
                if (isEditable && (el.tagName === 'INPUT' || el.tagName === 'TEXTAREA')) {{
                  if (typeof el.selectionStart === 'number' && typeof el.selectionEnd === 'number' && el.selectionEnd > el.selectionStart) {{
                    selText = el.value.slice(el.selectionStart, el.selectionEnd);
                  }}
                }}
                if (!selText && window.getSelection) {{
                  selText = String(window.getSelection());
                }}
                if (isEditable || selText.length > 0) {{
                  var copied = false;
                  try {{ copied = document.execCommand('copy'); }} catch (_) {{ copied = false; }}
                  if (!copied && selText.length > 0) {{
                    e.preventDefault();
                    fetch('/__audora_clipboard', {{
                      method: 'POST',
                      headers: {{
                        'Content-Type': 'application/json',
                        'X-Audora-Token': '{session_token}'
                      }},
                      body: JSON.stringify({{ text: selText }})
                    }}).catch(function() {{}});
                  }}
                }}
              }} else if (k === 'x' && isEditable) {{
                var cutText = '';
                if (el.tagName === 'INPUT' || el.tagName === 'TEXTAREA') {{
                  if (typeof el.selectionStart === 'number' && typeof el.selectionEnd === 'number' && el.selectionEnd > el.selectionStart) {{
                    cutText = el.value.slice(el.selectionStart, el.selectionEnd);
                  }}
                }} else if (window.getSelection) {{
                  cutText = String(window.getSelection());
                }}
                var cutOk = false;
                try {{ cutOk = document.execCommand('cut'); }} catch (_) {{ cutOk = false; }}
                if (!cutOk && cutText.length > 0) {{
                  e.preventDefault();
                  if ((el.tagName === 'INPUT' || el.tagName === 'TEXTAREA') && !el.readOnly && !el.disabled) {{
                    var cStart = el.selectionStart;
                    var cEnd = el.selectionEnd;
                    var cVal = typeof el.value === 'string' ? el.value : '';
                    setNativeControlValue(el, cVal.slice(0, cStart) + cVal.slice(cEnd), cStart);
                  }} else if (el.isContentEditable) {{
                    document.execCommand('delete');
                  }}
                  fetch('/__audora_clipboard', {{
                    method: 'POST',
                    headers: {{
                      'Content-Type': 'application/json',
                      'X-Audora-Token': '{session_token}'
                    }},
                    body: JSON.stringify({{ text: cutText }})
                  }}).catch(function() {{}});
                }}
              }} else if (k === 'v' && isEditable) {{
                e.preventDefault();
                var pasted = false;
                try {{ pasted = document.execCommand('paste'); }} catch (_) {{ pasted = false; }}
                if (!pasted) {{
                  var target = el;
                  fetch('/__audora_clipboard', {{
                    method: 'GET',
                    headers: {{ 'X-Audora-Token': '{session_token}' }}
                  }}).then(function(res) {{
                    return res.ok ? res.json() : null;
                  }}).then(function(data) {{
                    if (!data || !data.ok || typeof data.text !== 'string' || !data.text.length) return;
                    if (target.isContentEditable) {{
                      target.focus();
                      document.execCommand('insertText', false, data.text);
                    }} else if ((target.tagName === 'INPUT' || target.tagName === 'TEXTAREA') && !target.readOnly && !target.disabled) {{
                      var curVal = typeof target.value === 'string' ? target.value : '';
                      var start = typeof target.selectionStart === 'number' ? target.selectionStart : curVal.length;
                      var end = typeof target.selectionEnd === 'number' ? target.selectionEnd : start;
                      var nextVal = curVal.slice(0, start) + data.text + curVal.slice(end);
                      setNativeControlValue(target, nextVal, start + data.text.length);
                    }}
                  }}).catch(function() {{}});
                }}
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
    let builder = WebViewBuilder::new_with_web_context(&mut web_context)
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
                        let _ = drop_proxy.send_event(DesktopUserEvent::NativeFilesDropped {
                            paths,
                            auto_play: false,
                        });
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
        });

    #[cfg(target_os = "linux")]
    let webview_res = {
        use tao::platform::unix::WindowExtUnix;
        use wry::WebViewBuilderExtUnix;
        if let Some(vbox) = window.default_vbox() {
            builder.build_gtk(vbox)
        } else {
            builder.build_gtk(window.gtk_window())
        }
    };
    #[cfg(not(target_os = "linux"))]
    let webview_res = builder.build(&window);

    let webview = match webview_res {
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
    let drop_worker_proxy = proxy.clone();

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
                        let _ = opened_proxy.send_event(DesktopUserEvent::NativeFilesDropped {
                            paths,
                            auto_play: true,
                        });
                    } else {
                        pending_opened_paths.extend(paths);
                    }
                }
            }
            Event::UserEvent(DesktopUserEvent::WebViewPageLoaded) => {
                webview_ready = true;
                if !pending_opened_paths.is_empty() {
                    let queued = std::mem::take(&mut pending_opened_paths);
                    let _ = opened_proxy.send_event(DesktopUserEvent::NativeFilesDropped {
                        paths: queued,
                        auto_play: true,
                    });
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
                shutting_down.store(true, Ordering::SeqCst);
                drop(webview.take());
                drop(web_context.take());
                drop(window.take());
                if let Err(err) = exit_db.checkpoint_wal() {
                    warn!(
                        code = err.code().as_str(),
                        "Failed to checkpoint SQLite WAL on window destruction"
                    );
                }
                *control_flow = ControlFlow::Exit;
            }
            Event::UserEvent(DesktopUserEvent::QuitWithBatch(batch_json)) => {
                if quit_completed {
                    return;
                }
                quit_completed = true;
                shutting_down.store(true, Ordering::SeqCst);
                // Drain active mutating RPCs FIRST (up to 500ms) so stale in-flight
                // mutations never overwrite the final shutdown batch (`R2-M3`).
                let drain_deadline = Instant::now() + Duration::from_millis(500);
                while active_mutating_rpcs.load(Ordering::SeqCst) > 0
                    && Instant::now() < drain_deadline
                {
                    thread::sleep(Duration::from_millis(10));
                }
                if let Some(raw_batch) = batch_json {
                    match serde_json::from_str::<Vec<serde_json::Value>>(&raw_batch) {
                        Ok(ops) => {
                            if let Err(err) = exit_db.rpc_execute_batch(&ops) {
                                warn!(
                                    code = err.code().as_str(),
                                    "Failed to execute shutdown RPC batch"
                                );
                            } else {
                                apply_batch_source_revocations(&ops, &drop_allowlist);
                            }
                        }
                        Err(err) => {
                            warn!(error = %err, "Failed to parse shutdown RPC batch");
                        }
                    }
                }
                drop(webview.take());
                drop(web_context.take());
                drop(window.take());
                if let Err(err) = exit_db.checkpoint_wal() {
                    warn!(
                        code = err.code().as_str(),
                        "Failed to checkpoint SQLite WAL on quit"
                    );
                }
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
            Event::UserEvent(DesktopUserEvent::NativeFilesDropped { paths, auto_play }) => {
                let worker_allowlist = Arc::clone(&drop_allowlist);
                let worker_proxy = drop_worker_proxy.clone();
                thread::spawn(move || {
                    let descriptors = build_native_drop_descriptors(&paths, &worker_allowlist);
                    if !descriptors.is_empty() {
                        if let Ok(json) = serde_json::to_string(&descriptors) {
                            let _ = worker_proxy.send_event(
                                DesktopUserEvent::NativeDropDescriptorsReady { json, auto_play },
                            );
                        }
                    }
                });
            }
            Event::UserEvent(DesktopUserEvent::NativeDropDescriptorsReady { json, auto_play }) => {
                if let Some(wv) = webview.as_ref() {
                    let js = format!(
                        "(function dispatchDrop(items, autoPlay, tries) {{ if (typeof window.__AUDORA_ON_NATIVE_DROP__ === 'function') {{ window.__AUDORA_ON_NATIVE_DROP__(items, autoPlay); }} else if (tries > 0) {{ setTimeout(function() {{ dispatchDrop(items, autoPlay, tries - 1); }}, 50); }} }})({json}, {auto_play}, 40);"
                    );
                    let _ = wv.evaluate_script(&js);
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

fn sweep_stale_update_staging_dirs(app_data_dir: &Path) {
    let updates_dir = app_data_dir.join("updates");
    let Ok(updates_meta) = fs::symlink_metadata(&updates_dir) else {
        return;
    };
    if updates_meta.file_type().is_symlink() || !updates_meta.is_dir() {
        return;
    }
    let Ok(entries) = fs::read_dir(&updates_dir) else {
        return;
    };
    for entry in entries.filter_map(Result::ok) {
        let path = entry.path();
        let Some(name) = path.file_name().and_then(|s| s.to_str()) else {
            continue;
        };
        if !name.starts_with("staging-") {
            continue;
        }
        let Ok(meta) = fs::symlink_metadata(&path) else {
            continue;
        };
        if meta.file_type().is_symlink() {
            let _ = fs::remove_file(&path);
        } else if meta.is_dir() {
            let _ = fs::remove_dir_all(&path);
        }
    }
}

#[cfg(target_os = "windows")]
fn windows_system32_dir() -> PathBuf {
    let sys_root = env::var_os("SystemRoot")
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| std::ffi::OsString::from(r"C:\Windows"));
    PathBuf::from(sys_root).join("System32")
}

#[cfg(target_os = "windows")]
fn windows_curl_path() -> PathBuf {
    windows_system32_dir().join("curl.exe")
}

#[cfg(target_os = "windows")]
fn windows_rundll32_path() -> PathBuf {
    windows_system32_dir().join("rundll32.exe")
}

#[cfg(target_os = "windows")]
fn windows_powershell_path() -> PathBuf {
    windows_system32_dir()
        .join("WindowsPowerShell")
        .join("v1.0")
        .join("powershell.exe")
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
    #[cfg(target_os = "macos")]
    {
        let _ = Command::new("/usr/bin/open").arg(url).spawn();
    }
    #[cfg(target_os = "windows")]
    {
        let mut cmd = Command::new(windows_rundll32_path());
        configure_hidden_windows_command(&mut cmd);
        let _ = cmd.arg("url.dll,FileProtocolHandler").arg(url).spawn();
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
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
    shutting_down: Arc<AtomicBool>,
    active_mutating_rpcs: Arc<AtomicUsize>,
}

fn start_local_ui_server(
    ui_dist: PathBuf,
    proxy: EventLoopProxy<DesktopUserEvent>,
    dispatcher: Arc<CommandDispatcher>,
    db: Database,
    session_token: Arc<String>,
    allowlist: Arc<Mutex<MediaAllowlist>>,
) -> Option<(String, u16, Arc<AtomicBool>, Arc<AtomicUsize>)> {
    // Bind OS-assigned ephemeral IPv4 loopback port (`127.0.0.1:0`) (`C-4` / `M-2`).
    let listener = TcpListener::bind(("127.0.0.1", 0)).ok()?;
    let local_addr = listener.local_addr().ok()?;
    let local_port = local_addr.port();
    let url = format!("http://127.0.0.1:{local_port}/");
    let active_connections = Arc::new(AtomicUsize::new(0));
    let shutting_down = Arc::new(AtomicBool::new(false));
    let active_mutating_rpcs = Arc::new(AtomicUsize::new(0));
    let shutting_down_out = Arc::clone(&shutting_down);
    let active_mutating_out = Arc::clone(&active_mutating_rpcs);
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
        shutting_down,
        active_mutating_rpcs,
    });

    thread::spawn(move || {
        for incoming in listener.incoming() {
            let mut stream = match incoming {
                Ok(s) => s,
                Err(err) => {
                    warn!(error = %err, "Loopback accept failed; backing off briefly");
                    thread::sleep(Duration::from_millis(25));
                    continue;
                }
            };
            let prev = active_connections.fetch_add(1, Ordering::AcqRel);
            if prev >= MAX_CONCURRENT_HTTP_CONNECTIONS {
                active_connections.fetch_sub(1, Ordering::AcqRel);
                let _ = stream.set_write_timeout(Some(Duration::from_millis(200)));
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

    Some((url, local_port, shutting_down_out, active_mutating_out))
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
        .unwrap_or((raw_target.as_str(), ""));

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

    if raw_path == "/__audora_clipboard" {
        handle_clipboard_request(&mut stream, &method, &req_body);
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
        match pick_native_media_files(true) {
            NativePickOutcome::Selected(picked) => {
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
            NativePickOutcome::Cancelled => {
                write_json_response(
                    &mut stream,
                    "200 OK",
                    br#"{"ok":true,"cancelled":true,"files":[]}"#,
                );
            }
            NativePickOutcome::Unavailable => {
                write_json_response(
                    &mut stream,
                    "501 Not Implemented",
                    br#"{"ok":false,"unavailable":true}"#,
                );
            }
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
        match pick_native_media_files(false) {
            NativePickOutcome::Selected(picked) => {
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
            }
            NativePickOutcome::Cancelled => {
                write_json_response(
                    &mut stream,
                    "200 OK",
                    br#"{"ok":true,"cancelled":true,"file":null}"#,
                );
            }
            NativePickOutcome::Unavailable => {
                write_json_response(
                    &mut stream,
                    "501 Not Implemented",
                    br#"{"ok":false,"unavailable":true}"#,
                );
            }
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
            NativePickOutcome::Selected(selected) => {
                if let Ok(mut guard) = ctx.allowlist.lock() {
                    guard.register_dir(Path::new(&selected));
                }
                write_json_value(
                    &mut stream,
                    "200 OK",
                    &json!({ "ok": true, "cancelled": false, "path": selected }),
                );
            }
            NativePickOutcome::Cancelled => {
                write_json_response(
                    &mut stream,
                    "200 OK",
                    br#"{"ok":true,"cancelled":true,"path":null}"#,
                );
            }
            NativePickOutcome::Unavailable => {
                write_json_response(
                    &mut stream,
                    "501 Not Implemented",
                    br#"{"ok":false,"unavailable":true}"#,
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
            Ok(outcome) => {
                write_json_value(
                    &mut stream,
                    "200 OK",
                    &json!({
                        "ok": true,
                        "status": outcome.status,
                        "message": outcome.message,
                        "deferred_install": outcome.deferred_install,
                    }),
                );
                drop(stream);
                if outcome.should_restart {
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
        let _mut_guard = ActiveMutatingRpcGuard::enter(&ctx.active_mutating_rpcs);
        if ctx.shutting_down.load(Ordering::SeqCst) {
            write_json_response(
                &mut stream,
                "503 Service Unavailable",
                br#"{"ok":false,"message":"Application is shutting down."}"#,
            );
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
                Ok(None) => {
                    if let Ok(bytes) = fs::read(&legacy_state_file) {
                        write_json_response(&mut stream, "200 OK", &bytes);
                    } else {
                        write_json_response(&mut stream, "200 OK", b"{}");
                    }
                }
                Err(err) => {
                    warn!(
                        code = err.code().as_str(),
                        "Failed to read persisted UI state"
                    );
                    write_json_response(
                        &mut stream,
                        "500 Internal Server Error",
                        br#"{"ok":false,"message":"Failed to read persisted application state."}"#,
                    );
                }
            },
            "POST" => {
                let _mut_guard = ActiveMutatingRpcGuard::enter(&ctx.active_mutating_rpcs);
                if ctx.shutting_down.load(Ordering::SeqCst) {
                    write_json_response(
                        &mut stream,
                        "503 Service Unavailable",
                        br#"{"ok":false,"message":"Application is shutting down."}"#,
                    );
                    return;
                }
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
                let _mut_guard = ActiveMutatingRpcGuard::enter(&ctx.active_mutating_rpcs);
                if ctx.shutting_down.load(Ordering::SeqCst) {
                    write_json_response(
                        &mut stream,
                        "503 Service Unavailable",
                        br#"{"ok":false,"message":"Application is shutting down."}"#,
                    );
                    return;
                }
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

fn handle_clipboard_request(stream: &mut TcpStream, method: &str, req_body: &[u8]) {
    match method {
        "GET" => {
            #[cfg(target_os = "macos")]
            {
                let mut child = match Command::new("/usr/bin/pbpaste")
                    .env("LANG", "en_US.UTF-8")
                    .stdin(Stdio::null())
                    .stdout(Stdio::piped())
                    .stderr(Stdio::null())
                    .spawn()
                {
                    Ok(c) => c,
                    Err(_) => {
                        write_json_response(
                            stream,
                            "500 Internal Server Error",
                            br#"{"ok":false}"#,
                        );
                        return;
                    }
                };
                let mut buf = Vec::new();
                if let Some(stdout) = child.stdout.take() {
                    let _ = stdout
                        .take(MAX_CLIPBOARD_BYTES as u64)
                        .read_to_end(&mut buf);
                }
                let status = child.wait();
                if status.is_ok_and(|s| s.success()) || buf.len() == MAX_CLIPBOARD_BYTES {
                    let text = String::from_utf8_lossy(&buf).into_owned();
                    write_json_value(stream, "200 OK", &json!({ "ok": true, "text": text }));
                } else {
                    write_json_response(stream, "500 Internal Server Error", br#"{"ok":false}"#);
                }
            }
            #[cfg(not(target_os = "macos"))]
            {
                write_json_response(stream, "200 OK", br#"{"ok":false}"#);
            }
        }
        "POST" => {
            if req_body.len() > MAX_CLIPBOARD_BYTES {
                write_json_response(stream, "413 Payload Too Large", br#"{"ok":false}"#);
                return;
            }
            let parsed_text = std::str::from_utf8(req_body)
                .ok()
                .and_then(|s| serde_json::from_str::<serde_json::Value>(s).ok())
                .and_then(|v| {
                    v.get("text")
                        .and_then(serde_json::Value::as_str)
                        .map(str::to_owned)
                });
            let Some(text) = parsed_text else {
                write_json_response(stream, "400 Bad Request", br#"{"ok":false}"#);
                return;
            };
            #[cfg(target_os = "macos")]
            {
                let mut child = match Command::new("/usr/bin/pbcopy")
                    .env("LANG", "en_US.UTF-8")
                    .stdin(Stdio::piped())
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .spawn()
                {
                    Ok(c) => c,
                    Err(_) => {
                        write_json_response(
                            stream,
                            "500 Internal Server Error",
                            br#"{"ok":false}"#,
                        );
                        return;
                    }
                };
                let write_ok = if let Some(mut stdin) = child.stdin.take() {
                    stdin.write_all(text.as_bytes()).is_ok()
                } else {
                    false
                };
                if child.wait().is_ok_and(|s| s.success()) && write_ok {
                    write_json_response(stream, "200 OK", br#"{"ok":true}"#);
                } else {
                    write_json_response(stream, "500 Internal Server Error", br#"{"ok":false}"#);
                }
            }
            #[cfg(not(target_os = "macos"))]
            {
                let _ = text;
                write_json_response(stream, "200 OK", br#"{"ok":false}"#);
            }
        }
        _ => {
            write_json_response(stream, "405 Method Not Allowed", br#"{"ok":false}"#);
        }
    }
}

#[cfg(unix)]
fn unix_tool_path(
    usr_bin: &'static str,
    bin: &'static str,
    fallback: &'static str,
) -> &'static str {
    if Path::new(usr_bin).is_file() {
        usr_bin
    } else if Path::new(bin).is_file() {
        bin
    } else {
        fallback
    }
}

#[cfg(unix)]
fn unix_curl_path() -> &'static str {
    unix_tool_path("/usr/bin/curl", "/bin/curl", "curl")
}

#[cfg(unix)]
fn unix_tar_path() -> &'static str {
    unix_tool_path("/usr/bin/tar", "/bin/tar", "tar")
}

fn resolve_github_release_asset_sha256(repo: &str, tag: &str, asset_name: &str) -> Option<String> {
    let api_url = format!("https://api.github.com/repos/{repo}/releases/tags/{tag}");
    #[cfg(unix)]
    let mut cmd = Command::new(unix_curl_path());
    #[cfg(target_os = "windows")]
    let mut cmd = {
        let mut c = Command::new(windows_curl_path());
        configure_hidden_windows_command(&mut c);
        c
    };
    #[cfg(not(any(unix, target_os = "windows")))]
    let mut cmd = Command::new("curl");

    let mut child = cmd
        .args([
            "--proto",
            "=https",
            "--proto-redir",
            "=https",
            "--tlsv1.2",
            "--connect-timeout",
            "10",
            "--max-time",
            "20",
            "--max-redirs",
            "3",
            "--max-filesize",
            "2097152",
            "-fsSL",
            "-H",
            "Accept: application/vnd.github+json",
            "-H",
            "User-Agent: Audora-Updater",
            &api_url,
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let mut stdout_buf = Vec::new();
    if let Some(stdout) = child.stdout.take() {
        let _ = stdout.take(2_097_152).read_to_end(&mut stdout_buf);
    }
    let status = child.wait().ok()?;
    if !status.success() {
        return None;
    }
    let val: serde_json::Value = serde_json::from_slice(&stdout_buf).ok()?;
    let assets = val.get("assets")?.as_array()?;
    for asset in assets {
        let Some(name) = asset.get("name").and_then(serde_json::Value::as_str) else {
            continue;
        };
        if name == asset_name {
            let Some(digest) = asset.get("digest").and_then(serde_json::Value::as_str) else {
                continue;
            };
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

#[cfg(unix)]
fn verify_dir_no_escaping_symlinks(canon_root: &Path, dir: &Path, depth: usize) -> bool {
    if depth > 64 {
        return false;
    }
    let Ok(entries) = fs::read_dir(dir) else {
        return false;
    };
    for entry in entries {
        let Ok(entry) = entry else {
            return false;
        };
        let path = entry.path();
        let Ok(meta) = fs::symlink_metadata(&path) else {
            return false;
        };
        if meta.file_type().is_symlink() {
            let Ok(target_canon) = fs::canonicalize(&path) else {
                return false;
            };
            if !target_canon.starts_with(canon_root) {
                return false;
            }
        } else if meta.is_dir() && !verify_dir_no_escaping_symlinks(canon_root, &path, depth + 1) {
            return false;
        }
    }
    true
}

#[cfg(unix)]
fn verify_macos_staged_bundle_no_escaping_symlinks(staged_app: &Path) -> bool {
    let Ok(root_meta) = fs::symlink_metadata(staged_app) else {
        return false;
    };
    if root_meta.file_type().is_symlink() || !root_meta.is_dir() {
        return false;
    }
    let Ok(canon_root) = fs::canonicalize(staged_app) else {
        return false;
    };
    verify_dir_no_escaping_symlinks(&canon_root, &canon_root, 0)
}

#[cfg(unix)]
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

#[cfg(unix)]
fn verify_macos_staged_bundle_signature(staged_app: &Path, target_app: &Path) -> bool {
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
    let target_identity = if target_app.exists() {
        inspect_macos_codesign_identity(target_app)
    } else {
        None
    };
    let expected_id = current_identity
        .as_ref()
        .and_then(|(id, _)| id.as_deref())
        .or_else(|| target_identity.as_ref().and_then(|(id, _)| id.as_deref()))
        .unwrap_or("com.audora.desktop");
    if staged_id.as_deref() != Some(expected_id) {
        return false;
    }
    let expected_team = current_identity
        .as_ref()
        .and_then(|(_, team)| team.as_deref())
        .or_else(|| {
            target_identity
                .as_ref()
                .and_then(|(_, team)| team.as_deref())
        });
    if let Some(expected_team) = expected_team {
        if staged_team.as_deref() != Some(expected_team) {
            return false;
        }
    }
    true
}

#[cfg(target_os = "windows")]
fn verify_windows_installer_authenticode(setup_path: &Path) -> bool {
    let Ok(current_exe) = env::current_exe() else {
        return false;
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
    let mut cmd = Command::new(windows_powershell_path());
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

struct NativeSelfUpdateOutcome {
    status: &'static str,
    message: String,
    should_restart: bool,
    deferred_install: bool,
}

#[cfg(unix)]
fn probe_dir_writable(dir: &Path) -> bool {
    if !dir.is_dir() {
        return false;
    }
    let tok = generate_session_token();
    let probe_path = dir.join(format!(".audora-write-test-{}", &tok[..12]));
    match File::create(&probe_path) {
        Ok(f) => {
            drop(f);
            let _ = fs::remove_file(&probe_path);
            true
        }
        Err(_) => false,
    }
}

#[cfg(unix)]
fn copy_dir_recursive_no_symlinks(src: &Path, dst: &Path, depth: usize) -> std::io::Result<()> {
    if depth > 32 {
        return Err(std::io::Error::other("directory recursion depth exceeded"));
    }
    fs::create_dir_all(dst)?;
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let path = entry.path();
        let meta = fs::symlink_metadata(&path)?;
        if meta.file_type().is_symlink() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "unexpected symlink in staged UI directory",
            ));
        }
        let target = dst.join(entry.file_name());
        if meta.is_dir() {
            copy_dir_recursive_no_symlinks(&path, &target, depth + 1)?;
        } else if meta.is_file() {
            fs::copy(&path, &target)?;
        }
    }
    Ok(())
}

#[cfg(unix)]
fn apply_unix_portable_tar_update(
    staged_extract_dir: &Path,
    current_exe: &Path,
) -> Result<(), (&'static str, String)> {
    use std::os::unix::fs::PermissionsExt;

    if !verify_macos_staged_bundle_no_escaping_symlinks(staged_extract_dir) {
        return Err((
            "signature_verification_failed",
            "Downloaded Linux update archive contained an unsafe escaping symlink.".to_owned(),
        ));
    }

    let new_bin = if staged_extract_dir.join("bin/audora").is_file() {
        staged_extract_dir.join("bin/audora")
    } else if staged_extract_dir.join("audora").is_file() {
        staged_extract_dir.join("audora")
    } else {
        return Err((
            "install_failed",
            "Downloaded Linux archive did not contain an audora executable.".to_owned(),
        ));
    };

    let new_ui = if staged_extract_dir
        .join("share/audora/ui/index.html")
        .is_file()
    {
        staged_extract_dir.join("share/audora/ui")
    } else if staged_extract_dir.join("ui/index.html").is_file() {
        staged_extract_dir.join("ui")
    } else {
        return Err((
            "install_failed",
            "Downloaded Linux archive did not contain compiled UI assets.".to_owned(),
        ));
    };

    let Some(exe_dir) = current_exe.parent() else {
        return Err((
            "install_failed",
            "Unable to determine current executable directory.".to_owned(),
        ));
    };

    let is_share_layout = exe_dir.join("../share/audora/ui").is_dir()
        || (exe_dir.file_name().and_then(|s| s.to_str()) == Some("bin")
            && exe_dir.parent().is_some_and(|p| {
                p.join("share/audora").is_dir() || probe_dir_writable(&p.join("share"))
            }));
    let target_ui = if is_share_layout {
        exe_dir.join("../share/audora/ui")
    } else {
        exe_dir.join("ui")
    };
    let Some(ui_parent) = target_ui.parent() else {
        return Err((
            "install_failed",
            "Unable to resolve target UI asset directory.".to_owned(),
        ));
    };
    if fs::create_dir_all(ui_parent).is_err() {
        return Err((
            "install_permission_denied",
            format!(
                "Insufficient permissions to update {}.",
                ui_parent.display()
            ),
        ));
    }

    let ui_stage = ui_parent.join(".audora-ui.new");
    let ui_backup = ui_parent.join(".audora-ui.prev");
    let _ = fs::remove_dir_all(&ui_stage);
    let _ = fs::remove_dir_all(&ui_backup);

    if copy_dir_recursive_no_symlinks(&new_ui, &ui_stage, 0).is_err() {
        let _ = fs::remove_dir_all(&ui_stage);
        return Err((
            "install_failed",
            "Failed to stage updated UI assets in target directory.".to_owned(),
        ));
    }

    let had_existing_ui = target_ui.exists();
    if had_existing_ui && fs::rename(&target_ui, &ui_backup).is_err() {
        let _ = fs::remove_dir_all(&ui_stage);
        return Err((
            "install_permission_denied",
            format!(
                "Insufficient permissions to replace {}.",
                target_ui.display()
            ),
        ));
    }

    if fs::rename(&ui_stage, &target_ui).is_err() {
        if had_existing_ui && ui_backup.exists() {
            let _ = fs::rename(&ui_backup, &target_ui);
        }
        let _ = fs::remove_dir_all(&ui_stage);
        return Err((
            "install_failed",
            format!("Failed to swap UI assets at {}.", target_ui.display()),
        ));
    }

    let bin_stage = exe_dir.join(".audora.bin.new");
    let _ = fs::remove_file(&bin_stage);
    let bin_swapped = fs::copy(&new_bin, &bin_stage).is_ok()
        && fs::set_permissions(&bin_stage, fs::Permissions::from_mode(0o755)).is_ok()
        && fs::rename(&bin_stage, current_exe).is_ok();

    if !bin_swapped {
        let _ = fs::remove_file(&bin_stage);
        let _ = fs::remove_dir_all(&target_ui);
        if had_existing_ui && ui_backup.exists() {
            let _ = fs::rename(&ui_backup, &target_ui);
        }
        return Err((
            "install_permission_denied",
            format!(
                "Insufficient permissions to replace executable {}.",
                current_exe.display()
            ),
        ));
    }

    let _ = fs::remove_dir_all(&ui_backup);
    Ok(())
}

#[cfg(unix)]
fn spawn_detached_unix_binary_relauncher(wait_pid: u32, target_exe: &Path) -> std::io::Result<()> {
    use std::os::unix::process::CommandExt;
    let mut cmd = Command::new("/bin/sh");
    cmd.current_dir("/")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .process_group(0)
        .env("AUDORA_PID", wait_pid.to_string())
        .env("AUDORA_RELAUNCH_EXE", target_exe)
        .args([
            "-c",
            r#"trap '' HUP INT TERM; i=0; while kill -0 "$AUDORA_PID" 2>/dev/null && [ "$i" -lt 150 ]; do sleep 0.1; i=$((i + 1)); done; sleep 0.25; "$AUDORA_RELAUNCH_EXE" </dev/null >/dev/null 2>&1 &"#,
        ]);
    cmd.spawn().map(|_| ())
}

#[cfg(unix)]
fn spawn_detached_linux_deb_updater(
    wait_pid: u32,
    deb_path: &Path,
    staging_dir: &Path,
    pkexec_bin: &Path,
    dpkg_bin: &Path,
    relaunch_exe: &Path,
) -> std::io::Result<()> {
    use std::os::unix::process::CommandExt;
    let mut cmd = Command::new("/bin/sh");
    cmd.current_dir("/")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .process_group(0)
        .env("AUDORA_PID", wait_pid.to_string())
        .env("AUDORA_DEB_PATH", deb_path)
        .env("AUDORA_STAGING_DIR", staging_dir)
        .env("AUDORA_PKEXEC", pkexec_bin)
        .env("AUDORA_DPKG", dpkg_bin)
        .env("AUDORA_RELAUNCH_EXE", relaunch_exe)
        .args([
            "-c",
            r#"trap '' HUP INT TERM; i=0; while kill -0 "$AUDORA_PID" 2>/dev/null && [ "$i" -lt 150 ]; do sleep 0.1; i=$((i + 1)); done; "$AUDORA_PKEXEC" "$AUDORA_DPKG" -i "$AUDORA_DEB_PATH" >/dev/null 2>&1; rc=$?; rm -rf "$AUDORA_STAGING_DIR" 2>/dev/null || true; if [ "$rc" -eq 0 ] && [ -x "$AUDORA_RELAUNCH_EXE" ]; then "$AUDORA_RELAUNCH_EXE" </dev/null >/dev/null 2>&1 & fi"#,
        ]);
    cmd.spawn().map(|_| ())
}

#[cfg(unix)]
fn spawn_detached_macos_relauncher(
    wait_pid: u32,
    target_app: &Path,
    open_bin: &Path,
) -> std::io::Result<()> {
    use std::os::unix::process::CommandExt;
    let mut cmd = Command::new("/bin/sh");
    cmd.current_dir("/")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .process_group(0)
        .env("AUDORA_PID", wait_pid.to_string())
        .env("AUDORA_TARGET_APP", target_app)
        .env("AUDORA_OPEN_BIN", open_bin)
        .args([
            "-c",
            r#"trap '' HUP INT TERM; i=0; while kill -0 "$AUDORA_PID" 2>/dev/null && [ "$i" -lt 150 ]; do sleep 0.1; i=$((i + 1)); done; sleep 0.3; "$AUDORA_OPEN_BIN" -n "$AUDORA_TARGET_APP" >/dev/null 2>&1 || "$AUDORA_OPEN_BIN" "$AUDORA_TARGET_APP" >/dev/null 2>&1 || "$AUDORA_TARGET_APP/Contents/MacOS/audora" </dev/null >/dev/null 2>&1 &"#,
        ]);
    cmd.spawn().map(|_| ())
}

fn perform_native_self_update(
    repo: &str,
    version: &str,
    expected_sha256: &str,
) -> Result<NativeSelfUpdateOutcome, (&'static str, String)> {
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
        "--proto-redir",
        "=https",
        "--tlsv1.2",
        "--connect-timeout",
        "15",
        "--max-time",
        "300",
        "--max-redirs",
        "5",
        "--max-filesize",
        "536870912",
        "-fSL",
        "--retry",
        "2",
        "-o",
    ];

    #[cfg(unix)]
    {
        if cfg!(target_os = "macos") {
            let mac_arch = if cfg!(target_arch = "x86_64") {
                "x86_64"
            } else {
                "aarch64"
            };
            let archive_candidates = [
                format!("Audora_{version}_macos-{mac_arch}.app.tar.gz"),
                format!("Audora_{version}_macos-universal.app.tar.gz"),
            ];

            for archive_name in &archive_candidates {
                let archive_url =
                    format!("https://github.com/{repo}/releases/download/{tag}/{archive_name}");
                let archive_path = staging_dir.join(archive_name);

                info!(url = %archive_url, "Downloading macOS update bundle");
                let curl_ok = Command::new(unix_curl_path())
                    .args(curl_common_args)
                    .arg(&archive_path)
                    .arg(&archive_url)
                    .status()
                    .map(|s| s.success())
                    .unwrap_or(false);

                if !curl_ok || !archive_path.is_file() {
                    let _ = fs::remove_file(&archive_path);
                    continue;
                }

                let actual_sha = compute_file_sha256_hex(&archive_path).unwrap_or_default();
                let resolved_archive_sha =
                    resolve_github_release_asset_sha256(repo, &tag, archive_name);
                let sha_ok = constant_time_eq(actual_sha.as_bytes(), expected_sha256.as_bytes())
                    || resolved_archive_sha
                        .as_deref()
                        .is_some_and(|s| constant_time_eq(actual_sha.as_bytes(), s.as_bytes()));
                if !sha_ok {
                    let _ = fs::remove_dir_all(&staging_dir);
                    return Err((
                        "checksum_verification_failed",
                        "Downloaded update bundle failed SHA-256 integrity verification."
                            .to_owned(),
                    ));
                }

                let tar_ok = Command::new(unix_tar_path())
                    .args(["--no-same-owner", "-xzf"])
                    .arg(&archive_path)
                    .arg("-C")
                    .arg(&staging_dir)
                    .status()
                    .map(|s| s.success())
                    .unwrap_or(false);

                let staged_app = staging_dir.join("Audora.app");
                let staged_is_real_dir = fs::symlink_metadata(&staged_app)
                    .is_ok_and(|m| !m.file_type().is_symlink() && m.is_dir());
                if tar_ok && staged_is_real_dir {
                    if !verify_macos_staged_bundle_no_escaping_symlinks(&staged_app) {
                        let _ = fs::remove_dir_all(&staging_dir);
                        return Err((
                            "signature_verification_failed",
                            "Downloaded macOS bundle contained an unsafe escaping symlink."
                                .to_owned(),
                        ));
                    }

                    let target_app = resolve_current_macos_app_bundle()
                        .unwrap_or_else(|| PathBuf::from("/Applications/Audora.app"));
                    if !verify_macos_staged_bundle_signature(&staged_app, &target_app) {
                        let _ = fs::remove_dir_all(&staging_dir);
                        return Err((
                            "signature_verification_failed",
                            "Downloaded macOS bundle failed codesign identity verification."
                                .to_owned(),
                        ));
                    }

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
                        if let Err(err) = spawn_detached_macos_relauncher(
                            std::process::id(),
                            &target_app,
                            Path::new("/usr/bin/open"),
                        ) {
                            warn!(
                                error = %err,
                                "Failed to spawn macOS relaunch helper after update"
                            );
                            return Ok(NativeSelfUpdateOutcome {
                                status: "installed_manual_reopen",
                                message: format!(
                                    "Audora {version} was installed to {}. Please reopen Audora.",
                                    target_app.display()
                                ),
                                should_restart: false,
                                deferred_install: false,
                            });
                        }

                        return Ok(NativeSelfUpdateOutcome {
                            status: "restarting",
                            message: format!(
                                "Audora {version} SHA-256 & signature verified, installed to {}, and restarting…",
                                target_app.display()
                            ),
                            should_restart: true,
                            deferred_install: false,
                        });
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

            let dmg_candidates = [
                format!("Audora_{version}_macos-{mac_arch}.dmg"),
                format!("Audora_{version}_macos-universal.dmg"),
            ];

            for dmg_name in &dmg_candidates {
                let dmg_url =
                    format!("https://github.com/{repo}/releases/download/{tag}/{dmg_name}");
                let dmg_staged = staging_dir.join(dmg_name);

                let dmg_ok = Command::new(unix_curl_path())
                    .args(curl_common_args)
                    .arg(&dmg_staged)
                    .arg(&dmg_url)
                    .status()
                    .map(|s| s.success())
                    .unwrap_or(false);

                if !dmg_ok || !dmg_staged.is_file() {
                    let _ = fs::remove_file(&dmg_staged);
                    continue;
                }

                let actual_sha = compute_file_sha256_hex(&dmg_staged).unwrap_or_default();
                let resolved_dmg_sha = resolve_github_release_asset_sha256(repo, &tag, dmg_name);
                let dmg_sha_ok =
                    constant_time_eq(actual_sha.as_bytes(), expected_sha256.as_bytes())
                        || resolved_dmg_sha
                            .as_deref()
                            .is_some_and(|s| constant_time_eq(actual_sha.as_bytes(), s.as_bytes()));
                if dmg_sha_ok {
                    let downloads_dir = env::var_os("HOME")
                        .map(|h| PathBuf::from(h).join("Downloads"))
                        .unwrap_or_else(|| staging_dir.clone());
                    let dmg_dest = downloads_dir.join(dmg_name);
                    if fs::symlink_metadata(&dmg_dest).is_ok_and(|m| m.file_type().is_symlink()) {
                        let _ = fs::remove_file(&dmg_dest);
                    }
                    if fs::copy(&dmg_staged, &dmg_dest).is_err() {
                        let _ = fs::remove_dir_all(&staging_dir);
                        return Err((
                            "install_failed",
                            format!(
                                "Failed to copy verified installer to {}.",
                                dmg_dest.display()
                            ),
                        ));
                    }
                    let _ = fs::remove_dir_all(&staging_dir);
                    let _ = Command::new("/usr/bin/open").arg(&dmg_dest).spawn();
                    return Ok(NativeSelfUpdateOutcome {
                        status: "downloaded_installer",
                        message: format!("Verified SHA-256 and mounted {dmg_name} in ~/Downloads."),
                        should_restart: false,
                        deferred_install: false,
                    });
                }
                let _ = fs::remove_dir_all(&staging_dir);
                return Err((
                    "checksum_verification_failed",
                    "Downloaded DMG failed SHA-256 integrity verification.".to_owned(),
                ));
            }
        } else if cfg!(target_os = "linux") {
            let linux_arch = if cfg!(target_arch = "aarch64") {
                "aarch64"
            } else {
                "x86_64"
            };
            let current_exe = env::current_exe().ok();
            let writable_portable = current_exe.as_ref().is_some_and(|exe| {
                !exe.starts_with("/usr/") && exe.parent().is_some_and(probe_dir_writable)
            });

            if writable_portable {
                if let Some(ref exe_path) = current_exe {
                    let tar_name = format!("Audora_{version}_linux-{linux_arch}.tar.gz");
                    let tar_url =
                        format!("https://github.com/{repo}/releases/download/{tag}/{tar_name}");
                    let tar_path = staging_dir.join(&tar_name);
                    let curl_ok = Command::new(unix_curl_path())
                        .args(curl_common_args)
                        .arg(&tar_path)
                        .arg(&tar_url)
                        .status()
                        .map(|s| s.success())
                        .unwrap_or(false);

                    if curl_ok && tar_path.is_file() {
                        let actual_sha = compute_file_sha256_hex(&tar_path).unwrap_or_default();
                        let resolved_tar_sha =
                            resolve_github_release_asset_sha256(repo, &tag, &tar_name);
                        let sha_ok =
                            constant_time_eq(actual_sha.as_bytes(), expected_sha256.as_bytes())
                                || resolved_tar_sha.as_deref().is_some_and(|s| {
                                    constant_time_eq(actual_sha.as_bytes(), s.as_bytes())
                                });
                        if !sha_ok {
                            let _ = fs::remove_dir_all(&staging_dir);
                            return Err((
                                "checksum_verification_failed",
                                "Downloaded Linux update archive failed SHA-256 integrity verification."
                                    .to_owned(),
                            ));
                        }

                        let extract_dir = staging_dir.join("extracted");
                        if fs::create_dir_all(&extract_dir).is_ok() {
                            let tar_ok = Command::new(unix_tar_path())
                                .args(["--no-same-owner", "-xzf"])
                                .arg(&tar_path)
                                .arg("-C")
                                .arg(&extract_dir)
                                .status()
                                .map(|s| s.success())
                                .unwrap_or(false);
                            if tar_ok {
                                if let Err(err) =
                                    apply_unix_portable_tar_update(&extract_dir, exe_path)
                                {
                                    let _ = fs::remove_dir_all(&staging_dir);
                                    return Err(err);
                                }
                                let _ = fs::remove_dir_all(&staging_dir);
                                if let Err(err) = spawn_detached_unix_binary_relauncher(
                                    std::process::id(),
                                    exe_path,
                                ) {
                                    warn!(
                                        error = %err,
                                        "Failed to spawn Linux portable relaunch helper"
                                    );
                                    return Ok(NativeSelfUpdateOutcome {
                                        status: "installed_manual_reopen",
                                        message: format!(
                                            "Audora {version} was installed to {}. Please reopen Audora.",
                                            exe_path.display()
                                        ),
                                        should_restart: false,
                                        deferred_install: false,
                                    });
                                }
                                return Ok(NativeSelfUpdateOutcome {
                                    status: "restarting",
                                    message: format!(
                                        "Audora {version} SHA-256 verified, installed to {}, and restarting…",
                                        exe_path.display()
                                    ),
                                    should_restart: true,
                                    deferred_install: false,
                                });
                            }
                        }
                    }
                }
            }

            let has_gui = env::var_os("DISPLAY").is_some_and(|v| !v.is_empty())
                || env::var_os("WAYLAND_DISPLAY").is_some_and(|v| !v.is_empty());
            let pkexec_bin = Path::new("/usr/bin/pkexec");
            let dpkg_bin = if Path::new("/usr/bin/dpkg").is_file() {
                Some(Path::new("/usr/bin/dpkg"))
            } else if Path::new("/bin/dpkg").is_file() {
                Some(Path::new("/bin/dpkg"))
            } else {
                None
            };

            if has_gui && pkexec_bin.is_file() {
                if let (Some(dpkg_path), Some(relaunch_exe)) = (dpkg_bin, current_exe.as_ref()) {
                    let deb_name = format!("Audora_{version}_linux-{linux_arch}.deb");
                    let deb_url =
                        format!("https://github.com/{repo}/releases/download/{tag}/{deb_name}");
                    let deb_path = staging_dir.join(&deb_name);
                    let curl_ok = Command::new(unix_curl_path())
                        .args(curl_common_args)
                        .arg(&deb_path)
                        .arg(&deb_url)
                        .status()
                        .map(|s| s.success())
                        .unwrap_or(false);

                    if curl_ok && deb_path.is_file() {
                        let actual_sha = compute_file_sha256_hex(&deb_path).unwrap_or_default();
                        let resolved_deb_sha =
                            resolve_github_release_asset_sha256(repo, &tag, &deb_name);
                        let sha_ok =
                            constant_time_eq(actual_sha.as_bytes(), expected_sha256.as_bytes())
                                || resolved_deb_sha.as_deref().is_some_and(|s| {
                                    constant_time_eq(actual_sha.as_bytes(), s.as_bytes())
                                });
                        if !sha_ok {
                            let _ = fs::remove_dir_all(&staging_dir);
                            return Err((
                                "checksum_verification_failed",
                                "Downloaded Linux .deb package failed SHA-256 integrity verification."
                                    .to_owned(),
                            ));
                        }
                        if let Ok(()) = spawn_detached_linux_deb_updater(
                            std::process::id(),
                            &deb_path,
                            &staging_dir,
                            pkexec_bin,
                            dpkg_path,
                            relaunch_exe,
                        ) {
                            return Ok(NativeSelfUpdateOutcome {
                                status: "restarting",
                                message: format!(
                                    "Audora {version} verified. Closing to install {deb_name} and reopen…"
                                ),
                                should_restart: true,
                                deferred_install: true,
                            });
                        }
                    }
                }
            }
        }

        let _ = fs::remove_dir_all(&staging_dir);
        let release_page = format!("https://github.com/{repo}/releases/tag/{tag}");
        open_external_https_url(&release_page);
        Ok(NativeSelfUpdateOutcome {
            status: "opened_release",
            message: format!(
                "Opened GitHub release {tag} in your browser to complete installation."
            ),
            should_restart: false,
            deferred_install: false,
        })
    }

    #[cfg(target_os = "windows")]
    {
        let win_arch = if cfg!(target_arch = "aarch64") {
            "aarch64"
        } else {
            "x86_64"
        };
        let current_exe = env::current_exe().ok();
        let is_standalone_portable = current_exe.as_ref().is_some_and(|exe| {
            exe.parent().is_some_and(|dir| {
                dir.join("ui").join("index.html").is_file() && !dir.join("Uninstall.exe").exists()
            })
        });

        if !is_standalone_portable {
            let setup_name = format!("Audora_{version}_windows-{win_arch}-setup.exe");
            let setup_url =
                format!("https://github.com/{repo}/releases/download/{tag}/{setup_name}");
            let setup_path = staging_dir.join(&setup_name);
            let mut curl_cmd = Command::new(windows_curl_path());
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
                let resolved_setup_sha =
                    resolve_github_release_asset_sha256(repo, &tag, &setup_name);
                let sha_ok = constant_time_eq(actual_sha.as_bytes(), expected_sha256.as_bytes())
                    || resolved_setup_sha
                        .as_deref()
                        .is_some_and(|s| constant_time_eq(actual_sha.as_bytes(), s.as_bytes()));
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
                let relaunch_exe = current_exe.unwrap_or_default();
                let mut launch_cmd = Command::new(windows_powershell_path());
                configure_detached_windows_command(&mut launch_cmd);
                let _ = launch_cmd
                    .current_dir(env::temp_dir())
                    .stdin(Stdio::null())
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .args([
                        "-NoProfile",
                        "-NonInteractive",
                        "-WindowStyle",
                        "Hidden",
                        "-Command",
                        r#"Wait-Process -Id $env:AUDORA_PID -Timeout 15 -ErrorAction SilentlyContinue; $p = Start-Process -FilePath $env:AUDORA_SETUP_EXE -ArgumentList '/S' -Wait -PassThru; if (-not $p -or $p.ExitCode -ne 0) { $p = Start-Process -FilePath $env:AUDORA_SETUP_EXE -Wait -PassThru }; if ($env:AUDORA_STAGING_DIR) { Remove-Item -LiteralPath $env:AUDORA_STAGING_DIR -Recurse -Force -ErrorAction SilentlyContinue }; if ($p -and $p.ExitCode -eq 0) { $targetExe = $null; $reg = Get-ItemProperty -Path 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall\Audora' -ErrorAction SilentlyContinue; if ($reg -and $reg.InstallLocation) { $cand = Join-Path $reg.InstallLocation 'Audora.exe'; if (Test-Path -LiteralPath $cand) { $targetExe = $cand } }; if (-not $targetExe -and $reg -and $reg.UninstallString) { $uDir = Split-Path -Parent ($reg.UninstallString.Trim('"')); if ($uDir) { $cand = Join-Path $uDir 'Audora.exe'; if (Test-Path -LiteralPath $cand) { $targetExe = $cand } } }; if (-not $targetExe -and $env:LOCALAPPDATA) { $cand = Join-Path $env:LOCALAPPDATA 'Programs\Audora\Audora.exe'; if (Test-Path -LiteralPath $cand) { $targetExe = $cand } }; if (-not $targetExe -and $env:AUDORA_RELAUNCH_EXE -and (Test-Path -LiteralPath $env:AUDORA_RELAUNCH_EXE)) { $targetExe = $env:AUDORA_RELAUNCH_EXE }; if ($targetExe) { Start-Process -FilePath $targetExe } }"#,
                    ])
                    .env("AUDORA_PID", std::process::id().to_string())
                    .env("AUDORA_STAGING_DIR", &staging_dir)
                    .env("AUDORA_SETUP_EXE", &setup_path)
                    .env("AUDORA_RELAUNCH_EXE", &relaunch_exe)
                    .spawn();
                return Ok(NativeSelfUpdateOutcome {
                    status: "restarting",
                    message: format!(
                        "Verified and installing {setup_name}. Reopening automatically…"
                    ),
                    should_restart: true,
                    deferred_install: true,
                });
            }
        }

        if let Some(ref exe_path) = current_exe {
            if let Some(exe_dir) = exe_path.parent() {
                let zip_name = format!("Audora_{version}_windows-{win_arch}-portable.zip");
                let zip_url =
                    format!("https://github.com/{repo}/releases/download/{tag}/{zip_name}");
                let zip_path = staging_dir.join(&zip_name);
                let mut curl_cmd = Command::new(windows_curl_path());
                configure_hidden_windows_command(&mut curl_cmd);
                let curl_ok = curl_cmd
                    .args(curl_common_args)
                    .arg(&zip_path)
                    .arg(&zip_url)
                    .status()
                    .map(|s| s.success())
                    .unwrap_or(false);

                if curl_ok && zip_path.is_file() {
                    let actual_sha = compute_file_sha256_hex(&zip_path).unwrap_or_default();
                    let resolved_zip_sha =
                        resolve_github_release_asset_sha256(repo, &tag, &zip_name);
                    let sha_ok =
                        constant_time_eq(actual_sha.as_bytes(), expected_sha256.as_bytes())
                            || resolved_zip_sha.as_deref().is_some_and(|s| {
                                constant_time_eq(actual_sha.as_bytes(), s.as_bytes())
                            });
                    if !sha_ok {
                        let _ = fs::remove_dir_all(&staging_dir);
                        return Err((
                            "checksum_verification_failed",
                            "Downloaded Windows portable archive failed SHA-256 integrity verification."
                                .to_owned(),
                        ));
                    }

                    let portable_out = staging_dir.join("portable");
                    let _ = fs::create_dir_all(&portable_out);
                    let mut expand_cmd = Command::new(windows_powershell_path());
                    configure_hidden_windows_command(&mut expand_cmd);
                    let expand_ok = expand_cmd
                        .current_dir(env::temp_dir())
                        .stdin(Stdio::null())
                        .args([
                            "-NoProfile",
                            "-NonInteractive",
                            "-WindowStyle",
                            "Hidden",
                            "-Command",
                            "Expand-Archive -LiteralPath $env:AUDORA_ZIP -DestinationPath $env:AUDORA_PORTABLE_OUT -Force",
                        ])
                        .env("AUDORA_ZIP", &zip_path)
                        .env("AUDORA_PORTABLE_OUT", &portable_out)
                        .status()
                        .map(|s| s.success())
                        .unwrap_or(false);

                    let staged_exe = if portable_out.join("Audora.exe").is_file() {
                        Some(portable_out.join("Audora.exe"))
                    } else if portable_out.join("Audora").join("Audora.exe").is_file() {
                        Some(portable_out.join("Audora").join("Audora.exe"))
                    } else {
                        None
                    };

                    if expand_ok {
                        if let Some(staged_exe_path) = staged_exe {
                            if !verify_windows_installer_authenticode(&staged_exe_path) {
                                let _ = fs::remove_dir_all(&staging_dir);
                                return Err((
                                    "signature_verification_failed",
                                    "Downloaded Windows portable binary failed Authenticode verification."
                                        .to_owned(),
                                ));
                            }
                            let src_dir = staged_exe_path
                                .parent()
                                .unwrap_or(portable_out.as_path())
                                .to_path_buf();
                            let mut launch_cmd = Command::new(windows_powershell_path());
                            configure_detached_windows_command(&mut launch_cmd);
                            let _ = launch_cmd
                                .current_dir(env::temp_dir())
                                .stdin(Stdio::null())
                                .stdout(Stdio::null())
                                .stderr(Stdio::null())
                                .args([
                                    "-NoProfile",
                                    "-NonInteractive",
                                    "-WindowStyle",
                                    "Hidden",
                                    "-Command",
                                    "Wait-Process -Id $env:AUDORA_PID -Timeout 15 -ErrorAction SilentlyContinue; Start-Sleep -Milliseconds 300; Copy-Item -Path (Join-Path $env:AUDORA_SRC_DIR '*') -Destination $env:AUDORA_DEST_DIR -Recurse -Force; if ($env:AUDORA_STAGING_DIR) { Remove-Item -LiteralPath $env:AUDORA_STAGING_DIR -Recurse -Force -ErrorAction SilentlyContinue }; if ($env:AUDORA_RELAUNCH_EXE -and (Test-Path -LiteralPath $env:AUDORA_RELAUNCH_EXE)) { Start-Process -FilePath $env:AUDORA_RELAUNCH_EXE }",
                                ])
                                .env("AUDORA_PID", std::process::id().to_string())
                                .env("AUDORA_SRC_DIR", &src_dir)
                                .env("AUDORA_DEST_DIR", exe_dir)
                                .env("AUDORA_STAGING_DIR", &staging_dir)
                                .env("AUDORA_RELAUNCH_EXE", exe_path)
                                .spawn();
                            return Ok(NativeSelfUpdateOutcome {
                                status: "restarting",
                                message: format!(
                                    "Verified {zip_name}. Updating portable installation and reopening…"
                                ),
                                should_restart: true,
                                deferred_install: true,
                            });
                        }
                    }
                }
            }
        }

        let _ = fs::remove_dir_all(&staging_dir);
        let release_page = format!("https://github.com/{repo}/releases/tag/{tag}");
        open_external_https_url(&release_page);
        Ok(NativeSelfUpdateOutcome {
            status: "opened_release",
            message: format!("Opened {release_page}."),
            should_restart: false,
            deferred_install: false,
        })
    }

    #[cfg(not(any(unix, target_os = "windows")))]
    {
        let _ = fs::remove_dir_all(&staging_dir);
        let release_page = format!("https://github.com/{repo}/releases/tag/{tag}");
        open_external_https_url(&release_page);
        Ok(NativeSelfUpdateOutcome {
            status: "opened_release",
            message: format!("Opened {release_page} in your browser."),
            should_restart: false,
            deferred_install: false,
        })
    }
}

#[cfg(unix)]
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
    let trimmed = spec.trim();
    let prefix_bytes = trimmed.as_bytes();
    if prefix_bytes.len() < 6 || !prefix_bytes[..6].eq_ignore_ascii_case(b"bytes=") {
        return Err(());
    }
    let range_spec = &trimmed[6..];
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
        if range_header.is_some() {
            let header = "HTTP/1.1 416 Range Not Satisfiable\r\nX-Content-Type-Options: nosniff\r\nContent-Security-Policy: default-src 'none'\r\nCache-Control: no-store\r\nContent-Range: bytes */0\r\nContent-Length: 0\r\nConnection: close\r\n\r\n";
            let _ = stream.write_all(header.as_bytes());
            let _ = stream.flush();
            return;
        }
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
    let bytes = raw.as_bytes();
    if bytes.len() >= 8
        && (bytes[..8].eq_ignore_ascii_case(br"\\?\UNC\")
            || bytes[..8].eq_ignore_ascii_case(br"\\.\UNC\"))
    {
        PathBuf::from(format!(r"\\{}", &raw[8..]))
    } else if bytes.len() >= 4
        && (bytes[..4].eq_ignore_ascii_case(br"\\?\") || bytes[..4].eq_ignore_ascii_case(br"\\.\"))
    {
        PathBuf::from(&raw[4..])
    } else {
        PathBuf::from(raw)
    }
}

fn canonical_existing_path(p: &Path) -> Option<PathBuf> {
    let canon = fs::canonicalize(p).ok()?;
    let raw = canon.to_string_lossy();
    Some(strip_windows_verbatim_prefix(&raw))
}

fn canonicalize_with_existing_ancestor(path: &Path) -> Option<PathBuf> {
    if let Some(canon) = canonical_existing_path(path) {
        return Some(canon);
    }
    let mut remaining: Vec<std::ffi::OsString> = Vec::new();
    let mut cursor = path;
    while !cursor.exists() {
        let name = cursor.file_name()?;
        remaining.push(name.to_os_string());
        cursor = cursor.parent()?;
    }
    let mut base = canonical_existing_path(cursor)?;
    for comp in remaining.into_iter().rev() {
        base.push(comp);
    }
    Some(base)
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
                    promote_noted_db_media_in_trusted_dirs(&ctx.db, &ctx.allowlist);
                    sync_allowlist_from_db(&ctx.db, &ctx.allowlist);
                    apply_batch_source_revocations(ops, &ctx.allowlist);
                }
                res
            }
            None => Err("Missing 'ops' array".to_owned()),
        },
        "revoke_source_folder" => match val.get("path").and_then(serde_json::Value::as_str) {
            Some(path) => match audora_desktop::domain::validate_local_media_path_str(path) {
                Ok(clean) => {
                    promote_noted_db_media_in_trusted_dirs(&ctx.db, &ctx.allowlist);
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
                    promote_noted_db_media_in_trusted_dirs(&ctx.db, &ctx.allowlist);
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

#[cfg(target_os = "windows")]
fn configure_hidden_windows_command(cmd: &mut Command) -> &mut Command {
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    cmd.creation_flags(CREATE_NO_WINDOW);
    cmd
}

#[cfg(target_os = "windows")]
fn configure_detached_windows_command(cmd: &mut Command) -> &mut Command {
    const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    cmd.creation_flags(CREATE_NO_WINDOW | CREATE_NEW_PROCESS_GROUP);
    cmd
}

type PickedMediaFile = (String, String, u64, Option<u64>);

enum NativePickOutcome<T> {
    Selected(T),
    Cancelled,
    Unavailable,
}

fn parse_picked_media_lines(raw_lines: Vec<String>) -> Vec<PickedMediaFile> {
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

fn extract_stdout_lines(stdout: &[u8]) -> Vec<String> {
    String::from_utf8_lossy(stdout)
        .lines()
        .map(|s| s.trim_end_matches(['\r', '\n']).to_owned())
        .filter(|s| !s.is_empty())
        .collect()
}

fn pick_native_media_files(multiselect: bool) -> NativePickOutcome<Vec<PickedMediaFile>> {
    #[cfg(target_os = "macos")]
    {
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
        let Ok(output) = Command::new("/usr/bin/osascript")
            .args(["-e", script])
            .output()
        else {
            return NativePickOutcome::Unavailable;
        };
        if !output.status.success() {
            return NativePickOutcome::Cancelled;
        }
        let parsed = parse_picked_media_lines(extract_stdout_lines(&output.stdout));
        if parsed.is_empty() {
            NativePickOutcome::Cancelled
        } else {
            NativePickOutcome::Selected(parsed)
        }
    }

    #[cfg(target_os = "windows")]
    {
        let multi_flag = if multiselect { "$true" } else { "$false" };
        let ps_script = format!(
            "[Console]::OutputEncoding = [System.Text.Encoding]::UTF8; Add-Type -AssemblyName System.Windows.Forms; $d = New-Object System.Windows.Forms.OpenFileDialog; $d.Multiselect = {multi_flag}; $d.Filter = 'Media Files|*.mp4;*.m4v;*.mov;*.webm;*.mkv;*.mp3;*.wav;*.flac;*.ogg;*.opus;*.m4a;*.aac|All Files|*.*'; if ($d.ShowDialog() -eq 'OK') {{ $d.FileNames | ForEach-Object {{ Write-Output $_ }} }}"
        );
        let mut cmd = Command::new(windows_powershell_path());
        configure_hidden_windows_command(&mut cmd);
        let Ok(output) = cmd
            .args(["-NoProfile", "-Sta", "-Command", &ps_script])
            .output()
        else {
            return NativePickOutcome::Unavailable;
        };
        if !output.status.success() {
            return NativePickOutcome::Cancelled;
        }
        let parsed = parse_picked_media_lines(extract_stdout_lines(&output.stdout));
        if parsed.is_empty() {
            NativePickOutcome::Cancelled
        } else {
            NativePickOutcome::Selected(parsed)
        }
    }

    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        let mut args = vec![
            "--file-selection",
            "--separator=\n",
            "--title=Select local audio or video files for Audora",
        ];
        if multiselect {
            args.insert(1, "--multiple");
        }
        let output = match Command::new("zenity").args(&args).output() {
            Ok(out) => {
                if !out.status.success() {
                    return NativePickOutcome::Cancelled;
                }
                out
            }
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                let kdialog_args: Vec<&str> = if multiselect {
                    vec!["--getopenfilename", "--multiple", "--separate-output"]
                } else {
                    vec!["--getopenfilename"]
                };
                match Command::new("kdialog").args(&kdialog_args).output() {
                    Ok(out) => {
                        if !out.status.success() {
                            return NativePickOutcome::Cancelled;
                        }
                        out
                    }
                    Err(_) => return NativePickOutcome::Unavailable,
                }
            }
            Err(_) => return NativePickOutcome::Unavailable,
        };

        let parsed = parse_picked_media_lines(extract_stdout_lines(&output.stdout));
        if parsed.is_empty() {
            NativePickOutcome::Cancelled
        } else {
            NativePickOutcome::Selected(parsed)
        }
    }
}

fn pick_native_folder() -> NativePickOutcome<String> {
    #[cfg(target_os = "macos")]
    {
        let Ok(output) = Command::new("/usr/bin/osascript")
            .args([
                "-e",
                r#"POSIX path of (choose folder with prompt "Select a local media folder for Audora")"#,
            ])
            .output()
        else {
            return NativePickOutcome::Unavailable;
        };
        if !output.status.success() {
            return NativePickOutcome::Cancelled;
        }
        let raw_stdout = String::from_utf8_lossy(&output.stdout);
        let chosen = normalize_folder_path(raw_stdout.trim_end_matches(['\r', '\n']));
        if chosen.is_empty() || !Path::new(&chosen).is_dir() {
            return NativePickOutcome::Cancelled;
        }
        NativePickOutcome::Selected(canonical_path_string(Path::new(&chosen)))
    }

    #[cfg(target_os = "windows")]
    {
        let mut cmd = Command::new(windows_powershell_path());
        configure_hidden_windows_command(&mut cmd);
        let Ok(output) = cmd
            .args([
                "-NoProfile",
                "-Sta",
                "-Command",
                "[Console]::OutputEncoding = [System.Text.Encoding]::UTF8; Add-Type -AssemblyName System.Windows.Forms; $f = New-Object System.Windows.Forms.FolderBrowserDialog; $f.Description = 'Select a local media folder for Audora'; if ($f.ShowDialog() -eq 'OK') { Write-Output $f.SelectedPath }",
            ])
            .output()
        else {
            return NativePickOutcome::Unavailable;
        };
        if !output.status.success() {
            return NativePickOutcome::Cancelled;
        }
        let raw_stdout = String::from_utf8_lossy(&output.stdout);
        let chosen = normalize_folder_path(raw_stdout.trim_end_matches(['\r', '\n']));
        if chosen.is_empty() || !Path::new(&chosen).is_dir() {
            return NativePickOutcome::Cancelled;
        }
        NativePickOutcome::Selected(canonical_path_string(Path::new(&chosen)))
    }

    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        let output = match Command::new("zenity")
            .args([
                "--file-selection",
                "--directory",
                "--title=Select a local media folder for Audora",
            ])
            .output()
        {
            Ok(out) => {
                if !out.status.success() {
                    return NativePickOutcome::Cancelled;
                }
                out
            }
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                match Command::new("kdialog")
                    .args(["--getexistingdirectory"])
                    .output()
                {
                    Ok(out) => {
                        if !out.status.success() {
                            return NativePickOutcome::Cancelled;
                        }
                        out
                    }
                    Err(_) => return NativePickOutcome::Unavailable,
                }
            }
            Err(_) => return NativePickOutcome::Unavailable,
        };

        let raw_stdout = String::from_utf8_lossy(&output.stdout);
        let chosen = normalize_folder_path(raw_stdout.trim_end_matches(['\r', '\n']));
        if chosen.is_empty() || !Path::new(&chosen).is_dir() {
            return NativePickOutcome::Cancelled;
        }
        NativePickOutcome::Selected(canonical_path_string(Path::new(&chosen)))
    }
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
    if normalized.is_empty()
        || (!candidate.is_absolute() && !MediaAllowlist::is_unc_raw_path(&normalized))
    {
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
            strip_windows_verbatim_prefix(r"\\?\unc\NAS\Media\lecture.mp4"),
            PathBuf::from(r"\\NAS\Media\lecture.mp4")
        );
        assert_eq!(
            strip_windows_verbatim_prefix(r"\\.\UNC\NAS\Media\lecture.mp4"),
            PathBuf::from(r"\\NAS\Media\lecture.mp4")
        );
        assert_eq!(
            strip_windows_verbatim_prefix(r"\\.\unc\NAS\Media\lecture.mp4"),
            PathBuf::from(r"\\NAS\Media\lecture.mp4")
        );
        assert_eq!(
            strip_windows_verbatim_prefix(r"\\?\C:\Users\karson\clip.mp4"),
            PathBuf::from(r"C:\Users\karson\clip.mp4")
        );
        assert_eq!(
            strip_windows_verbatim_prefix(r"\\.\C:\Users\karson\clip.mp4"),
            PathBuf::from(r"C:\Users\karson\clip.mp4")
        );
        assert_eq!(
            strip_windows_verbatim_prefix("/Users/karson/clip.mp4"),
            PathBuf::from("/Users/karson/clip.mp4")
        );
    }

    #[test]
    fn rfc9110_byte_ranges_handle_suffix_case_insensitive_and_zero_byte_416() {
        assert_eq!(parse_http_byte_range("bytes=0-499", 1000), Ok((0, 499)));
        assert_eq!(parse_http_byte_range("Bytes=0-499", 1000), Ok((0, 499)));
        assert_eq!(parse_http_byte_range("BYTES=-200", 1000), Ok((800, 999)));
        assert_eq!(parse_http_byte_range("bytes=500-", 1000), Ok((500, 999)));
        assert_eq!(parse_http_byte_range("bytes=-5000", 1000), Ok((0, 999)));
        assert!(parse_http_byte_range("bytes=500-100", 1000).is_err());
        assert!(parse_http_byte_range("bytes=1000-1005", 1000).is_err());
        assert!(parse_http_byte_range("bytes=-0", 1000).is_err());
        assert!(parse_http_byte_range("bytes=0-0", 0).is_err());

        let dir = env::temp_dir().join(format!(
            "audora_range_zero_test_{}",
            generate_session_token()
        ));
        fs::create_dir_all(&dir).unwrap();
        let empty_mp3 = dir.join("empty.mp3");
        fs::write(&empty_mp3, b"").unwrap();

        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let port = listener.local_addr().unwrap().port();
        let file_for_server = empty_mp3.clone();
        let server_thread = thread::spawn(move || {
            for _ in 0..2 {
                let (mut stream, _) = listener.accept().unwrap();
                if let Some(req) = read_http_request(&mut stream) {
                    stream_local_media_file(
                        &mut stream,
                        &file_for_server,
                        req.range_header.as_deref(),
                        req.method == "HEAD",
                    );
                }
            }
        });

        let mut ranged = TcpStream::connect(("127.0.0.1", port)).unwrap();
        ranged
            .write_all(
                format!("GET /__audora_media HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nRange: bytes=0-0\r\n\r\n")
                    .as_bytes(),
            )
            .unwrap();
        ranged.flush().unwrap();
        let mut ranged_resp = String::new();
        let _ = ranged.read_to_string(&mut ranged_resp);
        assert!(ranged_resp.starts_with("HTTP/1.1 416 Range Not Satisfiable"));
        assert!(ranged_resp.contains("Content-Range: bytes */0\r\n"));

        let mut plain = TcpStream::connect(("127.0.0.1", port)).unwrap();
        plain
            .write_all(
                format!("GET /__audora_media HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\n\r\n")
                    .as_bytes(),
            )
            .unwrap();
        plain.flush().unwrap();
        let mut plain_resp = String::new();
        let _ = plain.read_to_string(&mut plain_resp);
        assert!(plain_resp.starts_with("HTTP/1.1 200 OK"));
        assert!(plain_resp.contains("Content-Length: 0\r\n"));

        server_thread.join().unwrap();
        let _ = fs::remove_dir_all(&dir);
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

    #[test]
    fn unc_forward_and_mixed_slash_prechecks_and_revocation_work() {
        for raw in [
            r"\\198.51.100.77\share\clip.mp4",
            "//198.51.100.77/share/clip.mp4",
            r"/\198.51.100.77\share/clip.mp4",
            r"\/198.51.100.77/share\clip.mp4",
            "//?/UNC/198.51.100.77/share/clip.mp4",
            r"\\.\unc\198.51.100.77\share\clip.mp4",
        ] {
            assert!(
                MediaAllowlist::is_unc_raw_path(raw),
                "expected UNC detection for {raw}"
            );
            assert_eq!(
                MediaAllowlist::lexical_normalize_unc(raw).as_deref(),
                Some(r"\\198.51.100.77\share\clip.mp4")
            );
        }

        assert!(!MediaAllowlist::is_unc_raw_path(r"\\?\C:\Users\clip.mp4"));
        assert!(!MediaAllowlist::is_unc_raw_path(r"\\.\C:\Users\clip.mp4"));
        assert!(!MediaAllowlist::is_unc_raw_path("/Users/karson/clip.mp4"));

        let mut allowlist = MediaAllowlist::default();
        allowlist
            .os_trusted_dirs
            .insert(PathBuf::from(r"\\nas\media"));

        for allowed in [
            "//NAS/Media/Lectures/week1.mp4",
            r"/\NAS\Media\Lectures\week1.mp4",
            r"\/NAS/Media/Lectures/week1.mp4",
            r"\\?\unc\NAS\Media\Lectures\week1.mp4",
        ] {
            assert!(
                allowlist.has_trusted_unc_prefix(allowed),
                "expected trusted UNC match for {allowed}"
            );
        }

        // Revoking the UNC share evaluates trust BEFORE removing `lexical_key`, then clears it.
        allowlist.revoke_source_folder("//NAS/Media");
        assert!(!allowlist.has_trusted_unc_prefix(r"\\NAS\Media\Lectures\week1.mp4"));
        assert!(!allowlist.has_trusted_unc_prefix("//NAS/Media/Lectures/week1.mp4"));
    }

    #[test]
    fn detached_note_track_remains_playable_across_revoke_source_folder() {
        let dir = env::temp_dir().join(format!(
            "audora_detached_note_test_{}",
            generate_session_token()
        ));
        let folder = dir.join("Course");
        fs::create_dir_all(&folder).unwrap();
        let noted_track = folder.join("noted.mp3");
        let unnoted_track = folder.join("unnoted.mp3");
        fs::write(&noted_track, b"ID3-noted").unwrap();
        fs::write(&unnoted_track, b"ID3-plain").unwrap();

        let db = Database::open_in_memory().unwrap();
        let allowlist = Mutex::new(MediaAllowlist::default());
        {
            let mut guard = allowlist.lock().unwrap();
            guard.register_dir(&folder);
        }

        let folder_dto = audora_desktop::domain::TrustedSourceFolder {
            id: "src_course".to_owned(),
            path: canonical_path_string(&folder),
            label: "Course".to_owned(),
            enabled: true,
            added_at: "2026-01-01T00:00:00Z".to_owned(),
            last_scanned_at: "2026-01-01T00:00:00Z".to_owned(),
            media_count: 2,
        };
        let noted_item = audora_desktop::domain::MediaItem {
            id: "m_noted".to_owned(),
            path: canonical_path_string(&noted_track),
            display_name: "noted".to_owned(),
            media_kind: audora_desktop::domain::MediaKind::Audio,
            duration_ms: Some(1000),
            duration_probed: true,
            last_position_ms: 0,
            completed: false,
            file_size_bytes: Some(9),
            fingerprint: None,
            availability: audora_desktop::domain::Availability::Available,
            added_at: "2026-01-01T00:00:00Z".to_owned(),
            last_opened_at: None,
            last_seen_at: "2026-01-01T00:00:00Z".to_owned(),
            note_count: 1,
            source_folder_id: Some("src_course".to_owned()),
        };
        let unnoted_item = audora_desktop::domain::MediaItem {
            id: "m_unnoted".to_owned(),
            path: canonical_path_string(&unnoted_track),
            display_name: "unnoted".to_owned(),
            media_kind: audora_desktop::domain::MediaKind::Audio,
            duration_ms: Some(1000),
            duration_probed: true,
            last_position_ms: 0,
            completed: false,
            file_size_bytes: Some(9),
            fingerprint: None,
            availability: audora_desktop::domain::Availability::Available,
            added_at: "2026-01-01T00:00:00Z".to_owned(),
            last_opened_at: None,
            last_seen_at: "2026-01-01T00:00:00Z".to_owned(),
            note_count: 0,
            source_folder_id: Some("src_course".to_owned()),
        };
        let note = audora_desktop::domain::Note {
            id: "n_1".to_owned(),
            media_id: "m_noted".to_owned(),
            position_ms: 250,
            is_quick_marker: false,
            body: "Remember this".to_owned(),
            created_at: "2026-01-01T00:00:00Z".to_owned(),
            updated_at: "2026-01-01T00:00:00Z".to_owned(),
        };

        let mut settings = audora_desktop::domain::AppSettings::default();
        settings.source_folders.push(folder_dto);
        db.rpc_save_settings(&settings).unwrap();
        db.rpc_upsert_media_batch(&[noted_item.clone(), unnoted_item])
            .unwrap();
        db.rpc_upsert_note(&note).unwrap();
        sync_allowlist_from_db(&db, &allowlist);

        // Simulate `save_settings` + `revoke_source_folder` BEFORE `upsert_media_batch` clears `source_folder_id`.
        settings.source_folders.clear();
        db.rpc_save_settings(&settings).unwrap();
        promote_noted_db_media_in_trusted_dirs(&db, &allowlist);
        sync_allowlist_from_db(&db, &allowlist);
        {
            let mut guard = allowlist.lock().unwrap();
            guard.revoke_source_folder(&folder.to_string_lossy());
        }
        db.rpc_delete_media_ids(&["m_unnoted".to_owned()]).unwrap();
        let mut detached_item = noted_item;
        detached_item.source_folder_id = None;
        db.rpc_upsert_media_batch(&[detached_item]).unwrap();
        sync_allowlist_from_db(&db, &allowlist);

        let guard = allowlist.lock().unwrap();
        assert!(!guard.is_allowed_scan_dir(&folder));
        assert!(guard
            .resolve_allowed_media_file(&noted_track.to_string_lossy())
            .is_some());
        assert!(guard
            .resolve_allowed_media_file(&unnoted_track.to_string_lossy())
            .is_none());

        drop(guard);
        let _ = fs::remove_dir_all(&dir);
    }

    #[cfg(unix)]
    #[test]
    fn deleted_symlinked_folder_revocation_and_disconnect_reconnect_preservation() {
        let dir = env::temp_dir().join(format!(
            "audora_symlink_revoke_test_{}",
            generate_session_token()
        ));
        let real_parent = dir.join("real_parent");
        let alias_parent = dir.join("alias_parent");
        fs::create_dir_all(&real_parent).unwrap();
        std::os::unix::fs::symlink(&real_parent, &alias_parent).unwrap();

        let real_sub = real_parent.join("Music");
        let alias_sub = alias_parent.join("Music");
        fs::create_dir_all(&real_sub).unwrap();
        let track = real_sub.join("track.mp3");
        fs::write(&track, b"ID3-track").unwrap();

        // 1. Deleted folder beneath symlinked ancestor (`alias_parent/Music` -> `real_parent/Music`) is revoked.
        let mut allowlist = MediaAllowlist::default();
        allowlist.register_dir(&alias_sub);
        assert!(allowlist.is_allowed_scan_dir(&real_sub));
        fs::remove_dir_all(&real_sub).unwrap();
        assert!(!alias_sub.exists());
        allowlist.revoke_source_folder(&alias_sub.to_string_lossy());
        // Recreating the folder must NOT grant access because the canonical path was evicted!
        fs::create_dir_all(&real_sub).unwrap();
        fs::write(&track, b"ID3-track").unwrap();
        assert!(!allowlist.is_allowed_scan_dir(&real_sub));
        assert!(allowlist
            .resolve_allowed_media_file(&track.to_string_lossy())
            .is_none());

        // 2. Temporary drive disconnect does NOT wipe `os_trusted_dirs`; reconnecting restores access!
        let drive_target = dir.join("DriveMount");
        let drive_offline = dir.join("DriveMount_offline");
        let drive_link = dir.join("DriveLink");
        fs::create_dir_all(&drive_target).unwrap();
        let ext_track = drive_target.join("ext.flac");
        fs::write(&ext_track, b"fLaC-ext").unwrap();
        std::os::unix::fs::symlink(&drive_target, &drive_link).unwrap();

        let folder_dto = audora_desktop::domain::TrustedSourceFolder {
            id: "src_ext".to_owned(),
            path: drive_link.to_string_lossy().into_owned(),
            label: "DriveLink".to_owned(),
            enabled: true,
            added_at: "2026-01-01T00:00:00Z".to_owned(),
            last_scanned_at: "2026-01-01T00:00:00Z".to_owned(),
            media_count: 1,
        };
        let state = audora_desktop::domain::RelationalAppStateDto {
            media: Vec::new(),
            notes: Vec::new(),
            playlists: Vec::new(),
            settings: audora_desktop::domain::AppSettings {
                source_folders: vec![folder_dto],
                ..audora_desktop::domain::AppSettings::default()
            },
            playback_session: None,
        };

        let mut ext_allowlist = MediaAllowlist::default();
        ext_allowlist.seed_from_startup_state(&state);
        assert!(ext_allowlist.is_allowed_scan_dir(&drive_link));

        // Simulate temporary disconnect (`DriveMount` unmounted) while saving an unrelated setting.
        fs::rename(&drive_target, &drive_offline).unwrap();
        ext_allowlist.sync_from_relational_state(&state);
        assert!(!ext_allowlist.os_trusted_dirs.is_empty());

        // Simulate reconnect (`DriveMount` remounted): scan & stream access is restored!
        fs::rename(&drive_offline, &drive_target).unwrap();
        ext_allowlist.sync_from_relational_state(&state);
        assert!(ext_allowlist.is_allowed_scan_dir(&drive_link));
        assert!(ext_allowlist
            .resolve_allowed_media_file(&ext_track.to_string_lossy())
            .is_some());

        let _ = fs::remove_dir_all(&dir);
    }

    #[cfg(unix)]
    #[test]
    fn staged_macos_bundle_symlink_validator_blocks_escaping_and_dangling_symlinks() {
        let dir = env::temp_dir().join(format!(
            "audora_bundle_symlink_test_{}",
            generate_session_token()
        ));
        let staged = dir.join("Audora.app");
        let contents = staged.join("Contents");
        let macos_dir = contents.join("MacOS");
        let resources = contents.join("Resources");
        let outside_secret = dir.join("outside.txt");
        fs::create_dir_all(&macos_dir).unwrap();
        fs::create_dir_all(&resources).unwrap();
        fs::write(macos_dir.join("audora"), b"stub").unwrap();
        fs::write(&outside_secret, b"secret").unwrap();

        // Internal relative symlink is valid.
        std::os::unix::fs::symlink("../MacOS/audora", resources.join("audora_link")).unwrap();
        assert!(verify_macos_staged_bundle_no_escaping_symlinks(&staged));

        // Escaping symlink outside `Audora.app` is rejected.
        std::os::unix::fs::symlink(&outside_secret, resources.join("escape")).unwrap();
        assert!(!verify_macos_staged_bundle_no_escaping_symlinks(&staged));

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn sweep_stale_update_staging_dirs_and_native_drop_cap() {
        let dir = env::temp_dir().join(format!(
            "audora_staging_sweep_test_{}",
            generate_session_token()
        ));
        let updates = dir.join("updates");
        let stale = updates.join("staging-1.0.0-abc");
        let keep = updates.join("other-file.txt");
        fs::create_dir_all(&stale).unwrap();
        fs::write(stale.join("part.tmp"), b"123").unwrap();
        fs::write(&keep, b"keep").unwrap();

        sweep_stale_update_staging_dirs(&dir);
        assert!(!stale.exists());
        assert!(keep.is_file());

        // Verify `build_native_drop_descriptors` caps input at 500 paths.
        let mut many_paths = Vec::with_capacity(550);
        for idx in 0..550 {
            let p = dir.join(format!("track_{idx:03}.mp3"));
            fs::write(&p, b"mp3").unwrap();
            many_paths.push(p);
        }
        let allowlist = Mutex::new(MediaAllowlist::default());
        let descriptors = build_native_drop_descriptors(&many_paths, &allowlist);
        assert_eq!(descriptors.len(), MAX_NATIVE_DROP_PATHS);

        let _ = fs::remove_dir_all(&dir);
    }

    #[cfg(unix)]
    #[test]
    fn detached_macos_relauncher_waits_for_pid_exit_and_launches_updated_bundle() {
        use std::os::unix::fs::PermissionsExt;

        let dir =
            env::temp_dir().join(format!("audora_relaunch_test_{}", generate_session_token()));
        let mock_app = dir.join("Audora.app");
        let macos_dir = mock_app.join("Contents").join("MacOS");
        fs::create_dir_all(&macos_dir).unwrap();

        let open_marker = dir.join("open_marker.txt");
        let fallback_marker = dir.join("binary_fallback_marker.txt");

        let mock_open_ok = dir.join("mock_open_ok.sh");
        fs::write(
            &mock_open_ok,
            format!(
                "#!/bin/sh\nprintf '%s' \"$*\" > '{}'\n",
                open_marker.display()
            ),
        )
        .unwrap();
        fs::set_permissions(&mock_open_ok, fs::Permissions::from_mode(0o755)).unwrap();

        let mock_app_bin = macos_dir.join("audora");
        fs::write(
            &mock_app_bin,
            format!(
                "#!/bin/sh\nprintf 'launched' > '{}'\n",
                fallback_marker.display()
            ),
        )
        .unwrap();
        fs::set_permissions(&mock_app_bin, fs::Permissions::from_mode(0o755)).unwrap();

        // 1. Primary path: wait until target PID exits, then run `open -n <Audora.app>`.
        let mut child = Command::new("/bin/sleep").arg("0.4").spawn().unwrap();
        let wait_pid = child.id();
        let reaper = thread::spawn(move || {
            let _ = child.wait();
        });

        spawn_detached_macos_relauncher(wait_pid, &mock_app, &mock_open_ok).unwrap();

        // While the target PID is still alive, relaunch must NOT trigger early.
        thread::sleep(Duration::from_millis(120));
        assert!(!open_marker.exists());

        let deadline = Instant::now() + Duration::from_millis(4000);
        while !open_marker.exists() && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(40));
        }
        let _ = reaper.join();
        assert!(open_marker.exists(), "expected open marker to be written");
        let recorded = fs::read_to_string(&open_marker).unwrap();
        assert_eq!(recorded, format!("-n {}", mock_app.display()));

        // 2. Fallback path: when `open` fails, launches `Audora.app/Contents/MacOS/audora`.
        let mock_open_fail = dir.join("mock_open_fail.sh");
        fs::write(&mock_open_fail, "#!/bin/sh\nexit 1\n").unwrap();
        fs::set_permissions(&mock_open_fail, fs::Permissions::from_mode(0o755)).unwrap();

        let mut child2 = Command::new("/bin/sleep").arg("0.1").spawn().unwrap();
        let wait_pid2 = child2.id();
        let reaper2 = thread::spawn(move || {
            let _ = child2.wait();
        });
        spawn_detached_macos_relauncher(wait_pid2, &mock_app, &mock_open_fail).unwrap();

        let deadline2 = Instant::now() + Duration::from_millis(4000);
        while !fallback_marker.exists() && Instant::now() < deadline2 {
            thread::sleep(Duration::from_millis(40));
        }
        let _ = reaper2.join();
        assert!(
            fallback_marker.exists(),
            "expected fallback binary marker when open exits non-zero"
        );
        assert_eq!(fs::read_to_string(&fallback_marker).unwrap(), "launched");

        let _ = fs::remove_dir_all(&dir);
    }

    #[cfg(unix)]
    #[test]
    fn unix_portable_tar_update_and_detached_relauncher_replace_and_reopen() {
        use std::os::unix::fs::PermissionsExt;

        let dir = env::temp_dir().join(format!(
            "audora_unix_portable_update_test_{}",
            generate_session_token()
        ));
        let root = dir.join("install");
        let bin_dir = root.join("bin");
        let ui_dir = root.join("share").join("audora").join("ui");
        fs::create_dir_all(&bin_dir).unwrap();
        fs::create_dir_all(&ui_dir).unwrap();

        let exe_path = bin_dir.join("audora");
        fs::write(&exe_path, "#!/bin/sh\necho old\n").unwrap();
        fs::set_permissions(&exe_path, fs::Permissions::from_mode(0o755)).unwrap();
        fs::write(ui_dir.join("index.html"), "<html>old</html>").unwrap();

        let extract_dir = dir.join("extract");
        let staged_bin_dir = extract_dir.join("bin");
        let staged_ui_dir = extract_dir.join("share").join("audora").join("ui");
        fs::create_dir_all(&staged_bin_dir).unwrap();
        fs::create_dir_all(&staged_ui_dir).unwrap();

        let relaunch_marker = dir.join("relaunched.txt");
        fs::write(
            staged_bin_dir.join("audora"),
            format!(
                "#!/bin/sh\nprintf 'new-ok' > '{}'\n",
                relaunch_marker.display()
            ),
        )
        .unwrap();
        fs::write(staged_ui_dir.join("index.html"), "<html>new</html>").unwrap();

        apply_unix_portable_tar_update(&extract_dir, &exe_path).unwrap();
        assert_eq!(
            fs::read_to_string(ui_dir.join("index.html")).unwrap(),
            "<html>new</html>"
        );
        assert!(!ui_dir.with_file_name(".audora-ui.prev").exists());
        assert!(!ui_dir.with_file_name(".audora-ui.new").exists());
        assert!(!bin_dir.join(".audora.bin.new").exists());

        let mut child = Command::new("/bin/sleep").arg("0.25").spawn().unwrap();
        let wait_pid = child.id();
        let reaper = thread::spawn(move || {
            let _ = child.wait();
        });
        spawn_detached_unix_binary_relauncher(wait_pid, &exe_path).unwrap();

        thread::sleep(Duration::from_millis(90));
        assert!(!relaunch_marker.exists());

        let deadline = Instant::now() + Duration::from_millis(4000);
        while !relaunch_marker.exists() && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(40));
        }
        let _ = reaper.join();
        assert_eq!(fs::read_to_string(&relaunch_marker).unwrap(), "new-ok");

        // Also verify `spawn_detached_linux_deb_updater` cleans staging and relaunches only on exit 0.
        let deb_staging_ok = dir.join("deb_stage_ok");
        fs::create_dir_all(&deb_staging_ok).unwrap();
        let deb_file_ok = deb_staging_ok.join("audora.deb");
        fs::write(&deb_file_ok, b"deb").unwrap();

        let deb_relaunch_marker = dir.join("deb_relaunch.txt");
        let deb_exe = dir.join("deb_audora.sh");
        fs::write(
            &deb_exe,
            format!(
                "#!/bin/sh\nprintf 'deb-ok' > '{}'\n",
                deb_relaunch_marker.display()
            ),
        )
        .unwrap();
        fs::set_permissions(&deb_exe, fs::Permissions::from_mode(0o755)).unwrap();

        let mock_dpkg = dir.join("mock_dpkg.sh");
        fs::write(&mock_dpkg, "#!/bin/sh\nexit 0\n").unwrap();
        fs::set_permissions(&mock_dpkg, fs::Permissions::from_mode(0o755)).unwrap();

        let mock_pkexec_ok = dir.join("mock_pkexec_ok.sh");
        fs::write(&mock_pkexec_ok, "#!/bin/sh\n\"$@\"\n").unwrap();
        fs::set_permissions(&mock_pkexec_ok, fs::Permissions::from_mode(0o755)).unwrap();

        let mut child2 = Command::new("/bin/sleep").arg("0.1").spawn().unwrap();
        let wait_pid2 = child2.id();
        let reaper2 = thread::spawn(move || {
            let _ = child2.wait();
        });
        spawn_detached_linux_deb_updater(
            wait_pid2,
            &deb_file_ok,
            &deb_staging_ok,
            &mock_pkexec_ok,
            &mock_dpkg,
            &deb_exe,
        )
        .unwrap();
        let deadline2 = Instant::now() + Duration::from_millis(4000);
        while !deb_relaunch_marker.exists() && Instant::now() < deadline2 {
            thread::sleep(Duration::from_millis(40));
        }
        let _ = reaper2.join();
        assert!(!deb_staging_ok.exists());
        assert_eq!(fs::read_to_string(&deb_relaunch_marker).unwrap(), "deb-ok");

        let _ = fs::remove_file(&deb_relaunch_marker);
        let deb_staging_fail = dir.join("deb_stage_fail");
        fs::create_dir_all(&deb_staging_fail).unwrap();
        let deb_file_fail = deb_staging_fail.join("audora.deb");
        fs::write(&deb_file_fail, b"deb").unwrap();
        let mock_pkexec_fail = dir.join("mock_pkexec_fail.sh");
        fs::write(&mock_pkexec_fail, "#!/bin/sh\nexit 1\n").unwrap();
        fs::set_permissions(&mock_pkexec_fail, fs::Permissions::from_mode(0o755)).unwrap();

        let mut child3 = Command::new("/bin/sleep").arg("0.05").spawn().unwrap();
        let wait_pid3 = child3.id();
        let reaper3 = thread::spawn(move || {
            let _ = child3.wait();
        });
        spawn_detached_linux_deb_updater(
            wait_pid3,
            &deb_file_fail,
            &deb_staging_fail,
            &mock_pkexec_fail,
            &mock_dpkg,
            &deb_exe,
        )
        .unwrap();
        let deadline3 = Instant::now() + Duration::from_millis(4000);
        while deb_staging_fail.exists() && Instant::now() < deadline3 {
            thread::sleep(Duration::from_millis(40));
        }
        let _ = reaper3.join();
        thread::sleep(Duration::from_millis(120));
        assert!(!deb_staging_fail.exists());
        assert!(!deb_relaunch_marker.exists());

        let _ = fs::remove_dir_all(&dir);
    }
}
