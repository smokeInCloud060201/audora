//! Audora desktop application entrypoint (`tao` + `wry` native shell with self-update and uninstall cleanup).

#![cfg_attr(
    all(not(debug_assertions), target_os = "windows"),
    windows_subsystem = "windows"
)]

use std::collections::hash_map::RandomState;
use std::collections::HashSet;
use std::env;
use std::fs::{self, File};
use std::hash::{BuildHasher, Hasher};
use std::io::{Read, Seek, SeekFrom, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Component, Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

#[cfg(target_os = "windows")]
use std::os::windows::process::CommandExt;

use audora_desktop::{
    adapters::probe_container_duration_ms,
    build_production_dispatcher_with_db,
    commands::CommandDispatcher,
    default_app_data_dir, default_database_path,
    domain::{
        AppSettings, MediaItem, Note, PersistedPlaybackSession, Playlist,
        DEFAULT_GITHUB_RELEASE_REPO,
    },
    persistence::Database,
    services::cleanup_all_app_artifacts,
};
use sha2::{Digest, Sha256};
use tao::dpi::{LogicalSize, PhysicalPosition};
use tao::event::{Event, WindowEvent};
use tao::event_loop::{ControlFlow, EventLoopBuilder, EventLoopProxy};
use tao::window::{Window, WindowBuilder};
use tracing::{error, info, warn};
use wry::{BackgroundThrottlingPolicy, WebContext, WebViewBuilder};

#[derive(Debug, Clone, Copy)]
enum DesktopUserEvent {
    Quit,
    SetWindowMode { minimum: bool },
    DragWindow,
}

const MAX_HTTP_BODY_BYTES: usize = 32 * 1024 * 1024;
const MAX_CONCURRENT_HTTP_CONNECTIONS: usize = 64;
const DEFAULT_FULL_WIDTH: f64 = 1440.0;
const DEFAULT_FULL_HEIGHT: f64 = 900.0;
const MIN_FULL_WIDTH: f64 = 1040.0;
const MIN_FULL_HEIGHT: f64 = 640.0;
const MINIMUM_WIDGET_WIDTH: f64 = 360.0;
const MINIMUM_WIDGET_HEIGHT: f64 = 168.0;

const RUNTIME_CSP: &str = "default-src 'self'; script-src 'self'; style-src 'self' 'unsafe-inline'; img-src 'self' data: blob:; media-src 'self' blob:; font-src 'self' data:; connect-src 'self' https://api.github.com https://github.com; object-src 'none'; base-uri 'self'; frame-ancestors 'none'";

#[derive(Default)]
struct MediaAllowlist {
    files: HashSet<PathBuf>,
    dirs: HashSet<PathBuf>,
}

impl MediaAllowlist {
    fn register_file(&mut self, path: &Path) {
        if let Some(canon) = canonical_existing_path(path) {
            self.files.insert(canon);
        }
    }

    fn register_dir(&mut self, path: &Path) {
        if let Some(canon) = canonical_existing_path(path) {
            self.dirs.insert(canon);
        }
    }

    fn clear(&mut self) {
        self.files.clear();
        self.dirs.clear();
    }

    fn sync_enabled_dirs(&mut self, folders: &[audora_desktop::domain::TrustedSourceFolder]) {
        self.dirs.clear();
        for folder in folders {
            if folder.enabled {
                let trimmed = folder.path.trim();
                if !trimmed.is_empty() {
                    self.register_dir(Path::new(trimmed));
                }
            }
        }
    }

    fn register_from_relational_state(
        &mut self,
        state: &audora_desktop::domain::RelationalAppStateDto,
    ) {
        for item in &state.media {
            let trimmed = item.path.trim();
            if !trimmed.is_empty() {
                self.register_file(Path::new(trimmed));
            }
        }
        self.sync_enabled_dirs(&state.settings.source_folders);
    }

    fn register_from_ui_state_value(&mut self, val: &serde_json::Value) {
        if let Some(media_arr) = val.get("media").and_then(serde_json::Value::as_array) {
            for item in media_arr {
                if let Some(p) = item.get("path").and_then(serde_json::Value::as_str) {
                    let trimmed = p.trim();
                    if !trimmed.is_empty() {
                        self.register_file(Path::new(trimmed));
                    }
                }
            }
        }
        if let Some(folders) = val
            .get("settings")
            .and_then(|s| s.get("source_folders"))
            .and_then(serde_json::Value::as_array)
        {
            for folder in folders {
                if let Some(p) = folder.get("path").and_then(serde_json::Value::as_str) {
                    let trimmed = p.trim();
                    if !trimmed.is_empty() {
                        self.register_dir(Path::new(trimmed));
                    }
                }
            }
        }
    }

    fn resolve_allowed_media_file(&self, raw_path: &str) -> Option<PathBuf> {
        let candidate = Path::new(raw_path.trim());
        if !candidate.is_absolute() {
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
        if self.files.contains(&canon) || self.dirs.iter().any(|d| canon.starts_with(d)) {
            Some(canon)
        } else {
            None
        }
    }
}

struct ConnectionSlotGuard {
    counter: Arc<AtomicUsize>,
}

impl Drop for ConnectionSlotGuard {
    fn drop(&mut self) {
        self.counter.fetch_sub(1, Ordering::AcqRel);
    }
}

fn generate_session_token() -> String {
    let mut hasher = Sha256::new();
    let mut urandom_bytes = [0_u8; 32];
    if let Ok(mut f) = File::open("/dev/urandom") {
        let _ = f.read_exact(&mut urandom_bytes);
    }
    hasher.update(urandom_bytes);
    for _ in 0..4 {
        let seed = RandomState::new().build_hasher().finish();
        hasher.update(seed.to_le_bytes());
    }
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
    if let Ok(Some(saved_state)) = db.export_relational_state() {
        if let Ok(mut guard) = allowlist.lock() {
            guard.register_from_relational_state(&saved_state);
        }
    }

    let cli_paths: Vec<String> = args
        .iter()
        .skip(1)
        .filter(|arg| !arg.starts_with('-'))
        .cloned()
        .collect();
    let mut cli_boot_files: Vec<(String, String, u64, Option<u64>)> = Vec::new();
    if !cli_paths.is_empty() {
        if let Err(err) = dispatcher.open_media_batch(&cli_paths) {
            warn!(
                code = err.code.as_str(),
                "Failed to open CLI media arguments"
            );
        }
        for raw_cli in &cli_paths {
            let pb = PathBuf::from(raw_cli);
            if pb.is_dir() {
                let start_len = cli_boot_files.len();
                collect_media_files_in_dir(&pb, 0, 1, 200, &mut cli_boot_files);
                if let Ok(mut guard) = allowlist.lock() {
                    guard.register_dir(&pb);
                    for (fp, _, _, _) in &cli_boot_files[start_len..] {
                        guard.register_file(Path::new(fp));
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
                                .unwrap_or(raw_cli)
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

    if args.iter().any(|arg| arg == "--headless-check") {
        match dispatcher.check_for_updates() {
            Ok(update_status) => {
                let _ = db.checkpoint_wal();
                info!(
                    version = %update_status.current_version,
                    verified = update_status.verified,
                    "Audora headless verification succeeded"
                );
                return;
            }
            Err(err) => {
                error!(code = err.code.as_str(), "Headless update check failed");
                std::process::exit(1);
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

    let Some((server_url, server_port)) = start_local_ui_server(
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

    let window = match WindowBuilder::new()
        .with_title("Audora")
        .with_resizable(true)
        .with_inner_size(LogicalSize::new(DEFAULT_FULL_WIDTH, DEFAULT_FULL_HEIGHT))
        .with_min_inner_size(LogicalSize::new(MIN_FULL_WIDTH, MIN_FULL_HEIGHT))
        .build(&event_loop)
    {
        Ok(win) => win,
        Err(err) => {
            error!(error = %err, "Failed to create native OS window");
            std::process::exit(1);
        }
    };
    apply_full_window_bounds_and_center(
        &window,
        LogicalSize::new(DEFAULT_FULL_WIDTH, DEFAULT_FULL_HEIGHT),
    );

    let mut cli_json = String::from("[");
    for (idx, (p, name, sz, dur)) in cli_boot_files.iter().enumerate() {
        if idx > 0 {
            cli_json.push(',');
        }
        let ep = escape_json_str(p);
        let en = escape_json_str(name);
        let dur_json = dur.map_or_else(|| "null".to_owned(), |d| d.to_string());
        cli_json.push_str(&format!(
            r#"{{"path":"{ep}","name":"{en}","size":{sz},"duration_ms":{dur_json}}}"#
        ));
    }
    cli_json.push(']');
    let escaped_data_dir = escape_json_str(&app_data_dir.display().to_string());
    let os_name = env::consts::OS;
    let arch_name = env::consts::ARCH;

    let init_script = format!(
        r#"
      Object.defineProperty(window, '__AUDORA_SESSION_TOKEN__', {{
        value: '{session_token}',
        writable: false,
        configurable: false
      }});
      Object.defineProperty(window, '__AUDORA_PLATFORM__', {{
        value: Object.freeze({{
          os: '{os_name}',
          arch: '{arch_name}',
          data_dir: "{escaped_data_dir}",
          cli_files: {cli_json}
        }}),
        writable: false,
        configurable: false
      }});
      window.close = function() {{
        if (window.ipc && typeof window.ipc.postMessage === 'function') {{
          window.ipc.postMessage('quit');
        }}
      }};
      window.addEventListener('keydown', function(e) {{
        if ((e.metaKey || e.ctrlKey) && e.key && e.key.toLowerCase() === 'q') {{
          e.preventDefault();
          if (window.ipc && typeof window.ipc.postMessage === 'function') {{
            window.ipc.postMessage('quit');
          }}
        }}
      }});
    "#
    );

    let webview_data_dir = app_data_dir.join("webview");
    let _ = fs::create_dir_all(&webview_data_dir);
    let mut web_context = WebContext::new(Some(webview_data_dir));

    let allowed_ip_prefix = format!("http://127.0.0.1:{server_port}/");
    let allowed_local_prefix = format!("http://localhost:{server_port}/");
    let ipc_proxy = proxy.clone();
    let webview = match WebViewBuilder::with_web_context(&mut web_context)
        .with_url(&server_url)
        .with_initialization_script(&init_script)
        .with_ipc_handler(move |request| match request.body().as_str() {
            "quit" => {
                let _ = ipc_proxy.send_event(DesktopUserEvent::Quit);
            }
            "window_mode:minimum" => {
                let _ = ipc_proxy.send_event(DesktopUserEvent::SetWindowMode { minimum: true });
            }
            "window_mode:full" => {
                let _ = ipc_proxy.send_event(DesktopUserEvent::SetWindowMode { minimum: false });
            }
            "drag_window" => {
                let _ = ipc_proxy.send_event(DesktopUserEvent::DragWindow);
            }
            _ => {}
        })
        .with_background_color((16, 21, 19, 255))
        .with_background_throttling(BackgroundThrottlingPolicy::Disabled)
        .with_clipboard(true)
        .with_devtools(cfg!(debug_assertions))
        .with_navigation_handler(move |uri: String| {
            uri.starts_with(&allowed_ip_prefix)
                || uri.starts_with(&allowed_local_prefix)
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
    let mut is_minimum_mode = false;

    event_loop.run(move |event, _, control_flow| {
        *control_flow = ControlFlow::Wait;

        match event {
            Event::WindowEvent {
                event: WindowEvent::CloseRequested | WindowEvent::Destroyed,
                ..
            }
            | Event::UserEvent(DesktopUserEvent::Quit)
            | Event::LoopDestroyed => {
                drop(webview.take());
                drop(web_context.take());
                drop(window.take());
                let _ = exit_db.checkpoint_wal();
                *control_flow = ControlFlow::Exit;
                std::process::exit(0);
            }
            Event::UserEvent(DesktopUserEvent::SetWindowMode { minimum }) => {
                if let Some(win) = window.as_ref() {
                    if minimum && !is_minimum_mode {
                        let curr = win.inner_size().to_logical::<f64>(win.scale_factor());
                        if curr.width >= MIN_FULL_WIDTH && curr.height >= MIN_FULL_HEIGHT {
                            saved_full_size = curr;
                        }
                        is_minimum_mode = true;
                        let min_sz = LogicalSize::new(MINIMUM_WIDGET_WIDTH, MINIMUM_WIDGET_HEIGHT);
                        win.set_min_inner_size(Some(min_sz));
                        win.set_decorations(false);
                        win.set_always_on_top(true);
                        win.set_inner_size(min_sz);
                        position_minimum_widget_bottom_right(win);
                    } else if !minimum && is_minimum_mode {
                        is_minimum_mode = false;
                        win.set_always_on_top(false);
                        win.set_decorations(true);
                        win.set_resizable(true);
                        apply_full_window_bounds_and_center(win, saved_full_size);
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
}

fn start_local_ui_server(
    ui_dist: PathBuf,
    proxy: EventLoopProxy<DesktopUserEvent>,
    dispatcher: Arc<CommandDispatcher>,
    db: Database,
    session_token: Arc<String>,
    allowlist: Arc<Mutex<MediaAllowlist>>,
) -> Option<(String, u16)> {
    let listener = (41957..=41966)
        .find_map(|port| TcpListener::bind(("127.0.0.1", port)).ok())
        .or_else(|| TcpListener::bind(("127.0.0.1", 0)).ok())?;
    let local_addr = listener.local_addr().ok()?;
    let local_port = local_addr.port();
    let url = format!("http://127.0.0.1:{local_port}/");
    let active_connections = Arc::new(AtomicUsize::new(0));
    let ctx = Arc::new(LoopbackServerContext {
        root: ui_dist,
        proxy,
        dispatcher,
        db,
        local_port,
        session_token,
        allowlist,
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

    Some((url, local_port))
}

fn read_http_request(stream: &mut TcpStream) -> Option<ParsedHttpRequest> {
    let _ = stream.set_read_timeout(Some(Duration::from_secs(15)));
    let _ = stream.set_write_timeout(Some(Duration::from_secs(30)));

    let mut header_buf = Vec::with_capacity(4096);
    let mut chunk = [0_u8; 4096];
    let header_end = loop {
        let n = stream.read(&mut chunk).ok()?;
        if n == 0 {
            // Never parse incomplete HTTP headers on premature EOF (`C-5`).
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
    let mut host: Option<String> = None;
    let mut range_header: Option<String> = None;
    let mut origin: Option<String> = None;
    let mut sec_fetch_site: Option<String> = None;
    let mut auth_token: Option<String> = None;
    for line in lines {
        if let Some((k, v)) = line.split_once(':') {
            let key = k.trim();
            if key.eq_ignore_ascii_case("content-length") {
                content_length = v.trim().parse::<usize>().ok()?;
            } else if key.eq_ignore_ascii_case("host") {
                host = Some(v.trim().to_owned());
            } else if key.eq_ignore_ascii_case("range") {
                range_header = Some(v.trim().to_owned());
            } else if key.eq_ignore_ascii_case("origin") {
                origin = Some(v.trim().to_owned());
            } else if key.eq_ignore_ascii_case("sec-fetch-site") {
                sec_fetch_site = Some(v.trim().to_ascii_lowercase());
            } else if key.eq_ignore_ascii_case("x-audora-token") {
                auth_token = Some(v.trim().to_owned());
            }
        }
    }

    if content_length > MAX_HTTP_BODY_BYTES {
        return None;
    }

    let mut body = Vec::with_capacity(content_length);
    if header_buf.len() > header_end {
        body.extend_from_slice(&header_buf[header_end..]);
    }
    while body.len() < content_length {
        let n = stream.read(&mut chunk).ok()?;
        if n == 0 {
            // Premature EOF before full Content-Length must abort (`C-5`).
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
    let expected_local = format!("localhost:{local_port}");
    h.eq_ignore_ascii_case(&expected_ip) || h.eq_ignore_ascii_case(&expected_local)
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
            let expected_local = format!("http://localhost:{local_port}");
            if !trimmed.eq_ignore_ascii_case(&expected_ip)
                && !trimmed.eq_ignore_ascii_case(&expected_local)
            {
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

fn is_safe_semver(v: &str) -> bool {
    !v.is_empty()
        && v.len() <= 32
        && v.chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-' || c == '_')
}

fn is_valid_sha256_hex(v: &str) -> bool {
    v.len() == 64 && v.chars().all(|c| c.is_ascii_hexdigit())
}

fn escape_json_str(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len() + 8);
    for ch in raw.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c.is_control() => {}
            c => out.push(c),
        }
    }
    out
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
        let effective_token = req.auth_token.as_deref().or(query_token.as_deref());
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
            let _ = stream.write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\n\r\n");
            return;
        };
        stream_local_media_file(&mut stream, &canon_path, range_header.as_deref());
        return;
    }

    if raw_path == "/__audora_probe_media" {
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
                br#"{"ok":false,"duration_ms":null}"#,
            );
            return;
        };
        let dur_json = probe_media_duration_ms(&candidate)
            .map_or_else(|| "null".to_owned(), |d| d.to_string());
        let body = format!(r#"{{"ok":true,"duration_ms":{dur_json}}}"#);
        write_json_response(&mut stream, "200 OK", body.as_bytes());
        return;
    }

    if raw_path == "/__audora_pick_files" {
        if method != "POST" {
            write_json_response(&mut stream, "405 Method Not Allowed", br#"{"ok":false}"#);
            return;
        }
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
            let mut files_json = String::from("[");
            for (idx, (p, name, sz, dur)) in picked.iter().enumerate() {
                if idx > 0 {
                    files_json.push(',');
                }
                let ep = escape_json_str(p);
                let en = escape_json_str(name);
                let dur_json = dur.map_or_else(|| "null".to_owned(), |d| d.to_string());
                files_json.push_str(&format!(
                    r#"{{"path":"{ep}","name":"{en}","size":{sz},"duration_ms":{dur_json}}}"#
                ));
            }
            files_json.push(']');
            let body = format!(r#"{{"ok":true,"cancelled":false,"files":{files_json}}}"#);
            write_json_response(&mut stream, "200 OK", body.as_bytes());
        }
        return;
    }

    if raw_path == "/__audora_pick_single_file" {
        if method != "POST" {
            write_json_response(&mut stream, "405 Method Not Allowed", br#"{"ok":false}"#);
            return;
        }
        let picked = pick_native_media_files(false);
        if let Some((p, name, sz, dur)) = picked.first() {
            if let Ok(mut guard) = ctx.allowlist.lock() {
                guard.register_file(Path::new(p));
            }
            let ep = escape_json_str(p);
            let en = escape_json_str(name);
            let dur_json = dur.map_or_else(|| "null".to_owned(), |d| d.to_string());
            let body = format!(
                r#"{{"ok":true,"cancelled":false,"file":{{"path":"{ep}","name":"{en}","size":{sz},"duration_ms":{dur_json}}}}}"#
            );
            write_json_response(&mut stream, "200 OK", body.as_bytes());
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
        match pick_native_folder() {
            Some(selected) => {
                if let Ok(mut guard) = ctx.allowlist.lock() {
                    guard.register_dir(Path::new(&selected));
                }
                let escaped = escape_json_str(&selected);
                let body = format!(r#"{{"ok":true,"cancelled":false,"path":"{escaped}"}}"#);
                write_json_response(&mut stream, "200 OK", body.as_bytes());
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

        match perform_native_self_update(
            DEFAULT_GITHUB_RELEASE_REPO,
            &clean_ver,
            &expected_sha,
            &ctx.proxy,
        ) {
            Ok((status_code, msg)) => {
                let escaped = escape_json_str(&msg);
                let body =
                    format!(r#"{{"ok":true,"status":"{status_code}","message":"{escaped}"}}"#);
                write_json_response(&mut stream, "200 OK", body.as_bytes());
            }
            Err((status_code, err_msg)) => {
                let escaped = escape_json_str(&err_msg);
                let body =
                    format!(r#"{{"ok":false,"status":"{status_code}","message":"{escaped}"}}"#);
                write_json_response(&mut stream, "500 Internal Server Error", body.as_bytes());
            }
        }
        return;
    }

    if raw_path == "/__audora_backup" && method == "POST" {
        match ctx.db.create_manual_backup() {
            Ok(Some(backup_path)) => {
                let escaped = escape_json_str(&backup_path.display().to_string());
                let body = format!(r#"{{"ok":true,"backup_path":"{escaped}"}}"#);
                write_json_response(&mut stream, "200 OK", body.as_bytes());
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
                Ok(Some(json)) => {
                    write_json_response(&mut stream, "200 OK", json.as_bytes());
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
                            if let Ok(mut guard) = ctx.allowlist.lock() {
                                guard.register_from_ui_state_value(&parsed_val);
                            }
                            let _ = fs::remove_file(&legacy_state_file);
                            write_json_response(&mut stream, "200 OK", br#"{"ok":true}"#);
                            return;
                        }
                    }
                }
                write_json_response(&mut stream, "400 Bad Request", br#"{"ok":false}"#);
            }
            "DELETE" => {
                let _ = ctx.db.reset_all_user_data();
                if let Ok(mut guard) = ctx.allowlist.lock() {
                    guard.clear();
                }
                let _ = fs::remove_file(&legacy_state_file);
                let data_dir = default_app_data_dir();
                let mut removed_paths = vec![data_dir.join("audora.sqlite").display().to_string()];
                let mut bytes_reclaimed: u64 = 0;
                for sub in ["updates", "logs", "cache"] {
                    let target = data_dir.join(sub);
                    if target.exists() {
                        if let Ok(meta) = fs::symlink_metadata(&target) {
                            if !meta.file_type().is_symlink() && meta.is_dir() {
                                let _ = fs::remove_dir_all(&target);
                                removed_paths.push(target.display().to_string());
                            }
                        }
                    }
                }
                if let Ok(entries) = fs::read_dir(&data_dir) {
                    for entry in entries.filter_map(Result::ok) {
                        let p = entry.path();
                        if let Some(name) = p.file_name().and_then(|s| s.to_str()) {
                            if name.starts_with("audora.sqlite.") && name.ends_with(".bak") {
                                let sz = fs::metadata(&p).map(|m| m.len()).unwrap_or(0);
                                if fs::remove_file(&p).is_ok() {
                                    bytes_reclaimed = bytes_reclaimed.saturating_add(sz);
                                    removed_paths.push(p.display().to_string());
                                }
                            }
                        }
                    }
                }
                let paths_json: Vec<String> = removed_paths
                    .iter()
                    .map(|p| format!("\"{}\"", escape_json_str(p)))
                    .collect();
                let body = format!(
                    r#"{{"ok":true,"removed_paths":[{}],"bytes_reclaimed":{bytes_reclaimed}}}"#,
                    paths_json.join(",")
                );
                write_json_response(&mut stream, "200 OK", body.as_bytes());
            }
            _ => {
                write_json_response(&mut stream, "405 Method Not Allowed", br#"{"ok":false}"#);
            }
        }
        return;
    }

    let decoded = percent_decode(raw_path);
    let trimmed = decoded.trim_start_matches('/');

    let mut candidate = ctx.root.clone();
    if trimmed.is_empty() {
        candidate.push("index.html");
    } else {
        for comp in Path::new(trimmed).components() {
            if let Component::Normal(segment) = comp {
                candidate.push(segment);
            }
        }
    }

    if !candidate.is_file() {
        candidate = ctx.root.join("index.html");
    }

    let Ok(body) = fs::read(&candidate) else {
        let _ = stream.write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\n\r\n");
        return;
    };

    let content_type = mime_type_for_path(&candidate);
    let header = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: {content_type}\r\nContent-Security-Policy: {RUNTIME_CSP}\r\nX-Content-Type-Options: nosniff\r\nContent-Length: {}\r\nCache-Control: no-cache\r\nConnection: close\r\n\r\n",
        body.len()
    );
    let _ = stream.write_all(header.as_bytes());
    let _ = stream.write_all(&body);
    let _ = stream.flush();
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

fn perform_native_self_update(
    repo: &str,
    version: &str,
    expected_sha256: &str,
    proxy: &EventLoopProxy<DesktopUserEvent>,
) -> Result<(&'static str, String), (&'static str, String)> {
    let tag = format!("v{version}");
    let rand_suffix = RandomState::new().build_hasher().finish();
    let staging_dir = default_app_data_dir()
        .join("updates")
        .join(format!("staging-{version}-{rand_suffix:016x}"));
    fs::create_dir_all(&staging_dir).map_err(|_| {
        (
            "staging_failed",
            "Failed to create secure update staging directory.".to_owned(),
        )
    })?;

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
            .args(["-fSL", "--retry", "2", "-o"])
            .arg(&archive_path)
            .arg(&archive_url)
            .status()
            .map(|s| s.success())
            .unwrap_or(false);

        if curl_ok && archive_path.is_file() {
            let actual_sha = compute_file_sha256_hex(&archive_path).unwrap_or_default();
            if !constant_time_eq(actual_sha.as_bytes(), expected_sha256.as_bytes()) {
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
                // Verify bundle code signature directly WITHOUT re-signing (`C-2`).
                let verify_ok = Command::new("/usr/bin/codesign")
                    .args(["--verify", "--deep", "--strict"])
                    .arg(&staged_app)
                    .status()
                    .map(|s| s.success())
                    .unwrap_or(false);
                if !verify_ok {
                    let _ = fs::remove_dir_all(&staging_dir);
                    return Err((
                        "signature_verification_failed",
                        "Downloaded macOS bundle failed codesign verification.".to_owned(),
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
                    || Command::new("/bin/cp")
                        .arg("-R")
                        .arg(&staged_app)
                        .arg(&target_app)
                        .status()
                        .map(|s| s.success())
                        .unwrap_or(false);

                if moved {
                    let _ = fs::remove_dir_all(&backup_app);
                    let _ = fs::remove_dir_all(&staging_dir);
                    // Spawn `/usr/bin/open` directly without `/bin/sh -c` interpolation (`C-2`).
                    let target_for_relaunch = target_app.clone();
                    thread::spawn(move || {
                        thread::sleep(Duration::from_millis(600));
                        let _ = Command::new("/usr/bin/open")
                            .arg("-n")
                            .arg(&target_for_relaunch)
                            .spawn();
                    });
                    let _ = proxy.send_event(DesktopUserEvent::Quit);

                    return Ok((
                        "restarting",
                        format!(
                            "Audora {version} SHA-256 & signature verified, installed to {}, and restarting…",
                            target_app.display()
                        ),
                    ));
                }

                if backup_app.exists() {
                    let _ = fs::rename(&backup_app, &target_app);
                }
            }
        }

        let dmg_name = format!("Audora_{version}_macos-{mac_arch}.dmg");
        let dmg_url = format!("https://github.com/{repo}/releases/download/{tag}/{dmg_name}");
        let dmg_staged = staging_dir.join(&dmg_name);

        let dmg_ok = Command::new("/usr/bin/curl")
            .args(["-fSL", "--retry", "2", "-o"])
            .arg(&dmg_staged)
            .arg(&dmg_url)
            .status()
            .map(|s| s.success())
            .unwrap_or(false);

        if dmg_ok && dmg_staged.is_file() {
            let actual_sha = compute_file_sha256_hex(&dmg_staged).unwrap_or_default();
            if constant_time_eq(actual_sha.as_bytes(), expected_sha256.as_bytes()) {
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
        let _ = Command::new("/usr/bin/open").arg(&release_page).spawn();
        return Ok((
            "opened_release",
            format!("Opened GitHub release {tag} in your browser to complete installation."),
        ));
    }

    if cfg!(target_os = "windows") {
        let setup_name = format!("Audora_{version}_windows-x86_64-setup.exe");
        let setup_url = format!("https://github.com/{repo}/releases/download/{tag}/{setup_name}");
        let setup_path = staging_dir.join(&setup_name);
        let curl_ok = Command::new("curl.exe")
            .args(["-fSL", "--retry", "2", "-o"])
            .arg(&setup_path)
            .arg(&setup_url)
            .status()
            .map(|s| s.success())
            .unwrap_or(false);

        if curl_ok && setup_path.is_file() {
            let actual_sha = compute_file_sha256_hex(&setup_path).unwrap_or_default();
            if !constant_time_eq(actual_sha.as_bytes(), expected_sha256.as_bytes()) {
                let _ = fs::remove_dir_all(&staging_dir);
                return Err((
                    "checksum_verification_failed",
                    "Downloaded Windows installer failed SHA-256 integrity verification."
                        .to_owned(),
                ));
            }
            let _ = Command::new(&setup_path).spawn();
            let _ = proxy.send_event(DesktopUserEvent::Quit);
            return Ok(("restarting", format!("Verified and launched {setup_name}.")));
        }

        let _ = fs::remove_dir_all(&staging_dir);
        let release_page = format!("https://github.com/{repo}/releases/tag/{tag}");
        let _ = Command::new("explorer.exe").arg(&release_page).spawn();
        return Ok(("opened_release", format!("Opened {release_page}.")));
    }

    let _ = fs::remove_dir_all(&staging_dir);
    let release_page = format!("https://github.com/{repo}/releases/tag/{tag}");
    let _ = Command::new("xdg-open").arg(&release_page).spawn();
    Ok((
        "opened_release",
        format!("Opened {release_page} in your browser."),
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

fn stream_local_media_file(stream: &mut TcpStream, candidate: &Path, range_header: Option<&str>) {
    let Ok(mut file) = File::open(candidate) else {
        let _ = stream.write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\n\r\n");
        return;
    };
    let Ok(meta) = file.metadata() else {
        let _ = stream.write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\n\r\n");
        return;
    };
    let total_len = meta.len();
    let content_type = mime_type_for_path(candidate);

    if total_len == 0 {
        let header = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: {content_type}\r\nAccept-Ranges: bytes\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
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
                    "HTTP/1.1 416 Range Not Satisfiable\r\nContent-Range: bytes */{total_len}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                );
                let _ = stream.write_all(header.as_bytes());
                let _ = stream.flush();
                return;
            }
        }
    } else {
        (0_u64, total_len - 1, false)
    };

    if file.seek(SeekFrom::Start(start)).is_err() {
        let _ =
            stream.write_all(b"HTTP/1.1 500 Internal Server Error\r\nContent-Length: 0\r\n\r\n");
        return;
    }

    let send_len = end.saturating_sub(start).saturating_add(1);
    let header = if is_partial {
        format!(
            "HTTP/1.1 206 Partial Content\r\nContent-Type: {content_type}\r\nAccept-Ranges: bytes\r\nContent-Range: bytes {start}-{end}/{total_len}\r\nContent-Length: {send_len}\r\nConnection: close\r\n\r\n"
        )
    } else {
        format!(
            "HTTP/1.1 200 OK\r\nContent-Type: {content_type}\r\nAccept-Ranges: bytes\r\nContent-Length: {total_len}\r\nConnection: close\r\n\r\n"
        )
    };

    if stream.write_all(header.as_bytes()).is_err() {
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
    let trimmed = raw.trim();
    if trimmed.len() > 1 {
        let stripped = trimmed.trim_end_matches(['/', '\\']);
        if stripped.is_empty() {
            trimmed.to_owned()
        } else {
            stripped.to_owned()
        }
    } else {
        trimmed.to_owned()
    }
}

fn canonical_existing_path(p: &Path) -> Option<PathBuf> {
    let canon = fs::canonicalize(p).ok()?;
    let raw = canon.to_string_lossy();
    if let Some(stripped) = raw.strip_prefix(r"\\?\") {
        Some(PathBuf::from(stripped))
    } else {
        Some(canon)
    }
}

fn canonical_path_string(p: &Path) -> String {
    canonical_existing_path(p)
        .unwrap_or_else(|| p.to_path_buf())
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

    let result: Result<Option<String>, String> = match op {
        "update_progress" => {
            let Some(media_id) = val.get("media_id").and_then(serde_json::Value::as_str) else {
                write_json_response(stream, "400 Bad Request", br#"{"ok":false}"#);
                return;
            };
            let pos = val
                .get("last_position_ms")
                .and_then(serde_json::Value::as_u64)
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
            if let Ok(mut guard) = ctx.allowlist.lock() {
                for item in &items {
                    guard.register_file(Path::new(&item.path));
                }
            }
            ctx.db
                .rpc_upsert_media_batch(&items)
                .map(|()| None)
                .map_err(|e| e.to_payload().message)
        }
        "delete_media_ids" => {
            let ids: Vec<String> = val
                .get("ids")
                .and_then(|v| serde_json::from_value(v.clone()).ok())
                .unwrap_or_default();
            ctx.db
                .rpc_delete_media_ids(&ids)
                .map(|_| None)
                .map_err(|e| e.to_payload().message)
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
                if let Ok(mut guard) = ctx.allowlist.lock() {
                    guard.sync_enabled_dirs(&settings.source_folders);
                }
                ctx.db
                    .rpc_save_settings(&settings)
                    .map(|()| None)
                    .map_err(|e| e.to_payload().message)
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
                serde_json::to_string(&rep).map(Some).map_err(|e| {
                    audora_desktop::error::AppError::internal(e.to_string()).to_payload()
                })
            })
            .map_err(|e| e.message),
        _ => Err(format!("Unsupported RPC operation '{op}'")),
    };

    match result {
        Ok(Some(custom_data)) => {
            let body = format!(r#"{{"ok":true,"data":{custom_data}}}"#);
            write_json_response(stream, "200 OK", body.as_bytes());
        }
        Ok(None) => {
            write_json_response(stream, "200 OK", br#"{"ok":true}"#);
        }
        Err(msg) => {
            let escaped = escape_json_str(&msg);
            let body = format!(r#"{{"ok":false,"message":"{escaped}"}}"#);
            write_json_response(stream, "400 Bad Request", body.as_bytes());
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
                    .map(|s| s.trim().to_owned())
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
                    .map(|s| s.trim().to_owned())
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
                    .map(|s| s.trim().to_owned())
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
        let chosen = normalize_folder_path(&String::from_utf8_lossy(&output.stdout));
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
        let chosen = normalize_folder_path(&String::from_utf8_lossy(&output.stdout));
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
                let chosen = normalize_folder_path(&String::from_utf8_lossy(&output.stdout));
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
    if normalized.is_empty() || !candidate.is_absolute() || !candidate.is_dir() {
        write_json_response(
            stream,
            "400 Bad Request",
            br#"{"ok":false,"message":"Folder not found or not accessible on this device."}"#,
        );
        return;
    }

    if let Ok(mut guard) = allowlist.lock() {
        guard.register_dir(candidate);
    }

    let canon_folder = canonical_path_string(candidate);
    let folder_name = Path::new(&canon_folder)
        .file_name()
        .and_then(|s| s.to_str())
        .filter(|s| !s.is_empty())
        .unwrap_or(&canon_folder);

    let mut discovered: Vec<(String, String, u64, Option<u64>)> = Vec::new();
    collect_media_files_in_dir(candidate, 0, 4, 2_000, &mut discovered);

    if let Ok(mut guard) = allowlist.lock() {
        for (fp, _, _, _) in &discovered {
            guard.register_file(Path::new(fp));
        }
    }

    let mut files_json = String::from("[");
    for (idx, (file_path, file_name, size, dur)) in discovered.iter().enumerate() {
        if idx > 0 {
            files_json.push(',');
        }
        let escaped_path = escape_json_str(file_path);
        let escaped_name = escape_json_str(file_name);
        let dur_json = dur.map_or_else(|| "null".to_owned(), |d| d.to_string());
        files_json.push_str(&format!(
            r#"{{"path":"{escaped_path}","name":"{escaped_name}","size":{size},"duration_ms":{dur_json}}}"#
        ));
    }
    files_json.push(']');

    let escaped_folder_path = escape_json_str(&canon_folder);
    let escaped_folder_name = escape_json_str(folder_name);
    let body = format!(
        r#"{{"ok":true,"folder_path":"{escaped_folder_path}","folder_name":"{escaped_folder_name}","files":{files_json}}}"#
    );
    write_json_response(stream, "200 OK", body.as_bytes());
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn host_header_and_session_token_block_dns_rebinding_and_null_origin() {
        let tok = generate_session_token();
        assert_eq!(tok.len(), 64);

        assert!(is_valid_host_header(41957, Some("127.0.0.1:41957")));
        assert!(is_valid_host_header(41957, Some("localhost:41957")));
        assert!(!is_valid_host_header(41957, Some("attacker.tld:41957")));
        assert!(!is_valid_host_header(41957, None));

        // Missing token or wrong token rejected
        assert!(!is_trusted_loopback_request(41957, None, None, None, &tok));
        assert!(!is_trusted_loopback_request(
            41957,
            Some("http://127.0.0.1:41957"),
            Some("same-origin"),
            Some("wrong"),
            &tok
        ));

        // Origin: null rejected even with valid token
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

        // Valid same-origin + token accepted
        assert!(is_trusted_loopback_request(
            41957,
            Some("http://127.0.0.1:41957"),
            Some("same-origin"),
            Some(&tok),
            &tok
        ));
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
        // Unregistered media file blocked
        assert!(allowlist
            .resolve_allowed_media_file(&valid_mp4.to_string_lossy())
            .is_none());

        allowlist.register_file(&valid_mp4);
        assert!(allowlist
            .resolve_allowed_media_file(&valid_mp4.to_string_lossy())
            .is_some());

        #[cfg(unix)]
        {
            let symlink_mp4 = dir.join("leak.mp4");
            std::os::unix::fs::symlink(&secret_txt, &symlink_mp4).unwrap();
            allowlist.register_dir(&dir);
            // Symlink named .mp4 pointing to `id_rsa` MUST be blocked because canonical extension is empty!
            assert!(allowlist
                .resolve_allowed_media_file(&symlink_mp4.to_string_lossy())
                .is_none());
        }

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn allowlist_sync_from_relational_state_revokes_disabled_or_removed_source_folders() {
        let dir = env::temp_dir().join(format!(
            "audora_rel_allowlist_test_{}",
            generate_session_token()
        ));
        let folder_a = dir.join("folder_a");
        fs::create_dir_all(&folder_a).unwrap();
        let track_in_a = folder_a.join("song.flac");
        fs::write(&track_in_a, b"fLaC-stub").unwrap();

        let mut allowlist = MediaAllowlist::default();
        let folder_dto = audora_desktop::domain::TrustedSourceFolder {
            id: "src_a".to_owned(),
            path: folder_a.to_string_lossy().into_owned(),
            label: "folder_a".to_owned(),
            enabled: true,
            added_at: "1".to_owned(),
            last_scanned_at: "1".to_owned(),
            media_count: 1,
        };
        allowlist.sync_enabled_dirs(std::slice::from_ref(&folder_dto));
        assert!(allowlist
            .resolve_allowed_media_file(&track_in_a.to_string_lossy())
            .is_some());

        // Disabling folder immediately revokes directory-wide streaming access
        let mut disabled = folder_dto;
        disabled.enabled = false;
        allowlist.sync_enabled_dirs(&[disabled]);
        assert!(allowlist
            .resolve_allowed_media_file(&track_in_a.to_string_lossy())
            .is_none());

        let _ = fs::remove_dir_all(&dir);
    }
}
