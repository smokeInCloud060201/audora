//! Audora desktop application entrypoint (`tao` + `wry` native shell with self-update and uninstall cleanup).

#![cfg_attr(
    all(not(debug_assertions), target_os = "windows"),
    windows_subsystem = "windows"
)]

use std::env;
use std::fs::{self, File};
use std::io::{Read, Seek, SeekFrom, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Component, Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::thread;
use std::time::{SystemTime, UNIX_EPOCH};

use audora_desktop::{
    build_production_dispatcher_with_db, default_app_data_dir, default_database_path,
    domain::DEFAULT_GITHUB_RELEASE_REPO, persistence::Database,
    services::cleanup_all_app_artifacts,
};
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
const DEFAULT_FULL_WIDTH: f64 = 1440.0;
const DEFAULT_FULL_HEIGHT: f64 = 900.0;
const MIN_FULL_WIDTH: f64 = 1040.0;
const MIN_FULL_HEIGHT: f64 = 640.0;
const MINIMUM_WIDGET_WIDTH: f64 = 360.0;
const MINIMUM_WIDGET_HEIGHT: f64 = 168.0;

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
    let legacy_state_file = default_app_data_dir().join("ui-state.json");
    if legacy_state_file.is_file() {
        if let Ok(None) = db.get_ui_state_json() {
            if let Ok(raw) = fs::read_to_string(&legacy_state_file) {
                if raw.trim().starts_with('{') && db.set_ui_state_json(&raw).is_ok() {
                    let _ = fs::remove_file(&legacy_state_file);
                }
            }
        } else {
            let _ = fs::remove_file(&legacy_state_file);
        }
    }

    let cli_paths: Vec<String> = args
        .iter()
        .skip(1)
        .filter(|arg| !arg.starts_with('-'))
        .cloned()
        .collect();
    if !cli_paths.is_empty() {
        if let Err(err) = dispatcher.open_media_batch(&cli_paths) {
            warn!(
                code = err.code.as_str(),
                "Failed to open CLI media arguments"
            );
        }
    }

    if args.iter().any(|arg| arg == "--headless-check") {
        match dispatcher.check_for_updates() {
            Ok(update_status) => {
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

    let Some(server_url) = start_local_ui_server(ui_dist, proxy.clone(), db) else {
        error!("Failed to bind local UI loopback server");
        std::process::exit(1);
    };

    info!(url = %server_url, "Launching Audora desktop window");

    let window = match WindowBuilder::new()
        .with_title("Audora")
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

    let init_script = r#"
      window.close = function() {
        if (window.ipc && typeof window.ipc.postMessage === 'function') {
          window.ipc.postMessage('quit');
        }
      };
      window.addEventListener('keydown', function(e) {
        if ((e.metaKey || e.ctrlKey) && e.key && e.key.toLowerCase() === 'q') {
          e.preventDefault();
          if (window.ipc && typeof window.ipc.postMessage === 'function') {
            window.ipc.postMessage('quit');
          }
        }
      });
    "#;

    let webview_data_dir = default_app_data_dir().join("webview");
    let _ = fs::create_dir_all(&webview_data_dir);
    let mut web_context = WebContext::new(Some(webview_data_dir));

    let ipc_proxy = proxy.clone();
    let webview = match WebViewBuilder::with_web_context(&mut web_context)
        .with_url(&server_url)
        .with_initialization_script(init_script)
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
        .with_navigation_handler(|uri: String| {
            uri.starts_with("http://127.0.0.1:")
                || uri.starts_with("http://localhost:")
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
                        win.set_max_inner_size(Some(min_sz));
                        win.set_resizable(false);
                        win.set_decorations(false);
                        win.set_always_on_top(true);
                        win.set_inner_size(min_sz);
                        position_minimum_widget_bottom_right(win);
                    } else if !minimum && is_minimum_mode {
                        is_minimum_mode = false;
                        win.set_always_on_top(false);
                        win.set_decorations(true);
                        win.set_resizable(true);
                        win.set_max_inner_size(None::<LogicalSize<f64>>);
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
    range_header: Option<String>,
    origin: Option<String>,
    sec_fetch_site: Option<String>,
    body: Vec<u8>,
}

fn start_local_ui_server(
    ui_dist: PathBuf,
    proxy: EventLoopProxy<DesktopUserEvent>,
    db: Database,
) -> Option<String> {
    let listener = (41957..=41966)
        .find_map(|port| TcpListener::bind(("127.0.0.1", port)).ok())
        .or_else(|| TcpListener::bind(("127.0.0.1", 0)).ok())?;
    let local_addr = listener.local_addr().ok()?;
    let local_port = local_addr.port();
    let url = format!("http://127.0.0.1:{local_port}/");

    thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let root = ui_dist.clone();
            let stream_proxy = proxy.clone();
            let stream_db = db.clone();
            thread::spawn(move || {
                handle_http_connection(stream, &root, &stream_proxy, &stream_db, local_port);
            });
        }
    });

    Some(url)
}

fn read_http_request(stream: &mut TcpStream) -> Option<ParsedHttpRequest> {
    let mut header_buf = Vec::with_capacity(4096);
    let mut chunk = [0_u8; 4096];
    let header_end = loop {
        let n = stream.read(&mut chunk).ok()?;
        if n == 0 {
            if header_buf.is_empty() {
                return None;
            }
            break header_buf.len();
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
    let request_line = lines.next().unwrap_or("GET / HTTP/1.1");
    let mut parts = request_line.split_whitespace();
    let method = parts.next().unwrap_or("GET").to_ascii_uppercase();
    let raw_target = parts.next().unwrap_or("/").to_owned();

    let mut content_length = 0_usize;
    let mut range_header: Option<String> = None;
    let mut origin: Option<String> = None;
    let mut sec_fetch_site: Option<String> = None;
    for line in lines {
        if let Some((k, v)) = line.split_once(':') {
            let key = k.trim();
            if key.eq_ignore_ascii_case("content-length") {
                if let Ok(parsed) = v.trim().parse::<usize>() {
                    content_length = parsed;
                }
            } else if key.eq_ignore_ascii_case("range") {
                range_header = Some(v.trim().to_owned());
            } else if key.eq_ignore_ascii_case("origin") {
                origin = Some(v.trim().to_owned());
            } else if key.eq_ignore_ascii_case("sec-fetch-site") {
                sec_fetch_site = Some(v.trim().to_ascii_lowercase());
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
            break;
        }
        body.extend_from_slice(&chunk[..n]);
    }
    body.truncate(content_length);

    Some(ParsedHttpRequest {
        method,
        raw_target,
        range_header,
        origin,
        sec_fetch_site,
        body,
    })
}

fn is_trusted_loopback_request(
    local_port: u16,
    origin: Option<&str>,
    sec_fetch_site: Option<&str>,
) -> bool {
    if let Some(site) = sec_fetch_site {
        if site != "same-origin" && site != "none" {
            return false;
        }
    }
    if let Some(orig) = origin {
        let trimmed = orig.trim();
        if !trimmed.is_empty() && trimmed != "null" {
            let expected_ip = format!("http://127.0.0.1:{local_port}");
            let expected_local = format!("http://localhost:{local_port}");
            if !trimmed.eq_ignore_ascii_case(&expected_ip)
                && !trimmed.eq_ignore_ascii_case(&expected_local)
            {
                return false;
            }
        }
    }
    true
}

fn write_json_response(stream: &mut TcpStream, status: &str, body: &[u8]) {
    let header = format!(
        "HTTP/1.1 {status}\r\nContent-Type: application/json; charset=utf-8\r\nContent-Length: {}\r\nCache-Control: no-cache\r\nConnection: close\r\n\r\n",
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

fn handle_http_connection(
    mut stream: TcpStream,
    root: &Path,
    proxy: &EventLoopProxy<DesktopUserEvent>,
    db: &Database,
    local_port: u16,
) {
    let Some(req) = read_http_request(&mut stream) else {
        return;
    };
    let method = req.method;
    let raw_target = req.raw_target;
    let range_header = req.range_header;
    let req_body = req.body;

    let (raw_path, raw_query) = raw_target
        .split_once('?')
        .map_or((raw_target.as_str(), ""), |(p, q)| (p, q));

    if raw_path.starts_with("/__audora_")
        && !is_trusted_loopback_request(
            local_port,
            req.origin.as_deref(),
            req.sec_fetch_site.as_deref(),
        )
    {
        write_json_response(
            &mut stream,
            "403 Forbidden",
            br#"{"ok":false,"message":"Cross-origin loopback request blocked."}"#,
        );
        return;
    }

    if raw_path == "/__audora_media" {
        let mut media_path_str = String::new();
        for pair in raw_query.split('&') {
            if let Some((k, v)) = pair.split_once('=') {
                if k == "path" {
                    media_path_str = percent_decode(v);
                }
            }
        }
        stream_local_media_file(&mut stream, &media_path_str, range_header.as_deref());
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
        let candidate = Path::new(media_path_str.trim());
        if !candidate.is_absolute() || !candidate.is_file() {
            write_json_response(
                &mut stream,
                "404 Not Found",
                br#"{"ok":false,"duration_ms":null}"#,
            );
            return;
        }
        let dur_json =
            probe_media_duration_ms(candidate).map_or_else(|| "null".to_owned(), |d| d.to_string());
        let body = format!(r#"{{"ok":true,"duration_ms":{dur_json}}}"#);
        write_json_response(&mut stream, "200 OK", body.as_bytes());
        return;
    }

    if raw_path == "/__audora_pick_files" {
        let picked = pick_native_media_files();
        if picked.is_empty() {
            write_json_response(
                &mut stream,
                "200 OK",
                br#"{"ok":true,"cancelled":true,"files":[]}"#,
            );
        } else {
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

    if raw_path == "/__audora_pick_folder" {
        match pick_native_folder() {
            Some(selected) => {
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
        handle_scan_folder_request(&mut stream, &folder_path_str);
        return;
    }

    if raw_path == "/__audora_update" {
        let mut version = String::new();
        for pair in raw_query.split('&') {
            if let Some((k, v)) = pair.split_once('=') {
                if k == "version" {
                    version = percent_decode(v);
                }
            }
        }

        let clean_ver = version.trim().trim_start_matches(['v', 'V']).to_owned();
        if !is_safe_semver(&clean_ver) {
            write_json_response(
                &mut stream,
                "400 Bad Request",
                br#"{"ok":false,"status":"invalid_parameters","message":"Invalid target version parameter."}"#,
            );
            return;
        }

        match perform_native_self_update(DEFAULT_GITHUB_RELEASE_REPO, &clean_ver, proxy) {
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
        match db.create_manual_backup() {
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

    if raw_path == "/__audora_state" {
        let legacy_state_file = default_app_data_dir().join("ui-state.json");
        match method.as_str() {
            "GET" => match db.get_ui_state_json() {
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
                    if json_str.trim().starts_with('{') && db.set_ui_state_json(json_str).is_ok() {
                        let _ = fs::remove_file(&legacy_state_file);
                        write_json_response(&mut stream, "200 OK", br#"{"ok":true}"#);
                        return;
                    }
                }
                write_json_response(&mut stream, "500 Internal Server Error", br#"{"ok":false}"#);
            }
            "DELETE" => {
                let _ = db.clear_ui_state();
                let _ = fs::remove_file(&legacy_state_file);
                write_json_response(&mut stream, "200 OK", br#"{"ok":true}"#);
            }
            _ => {
                write_json_response(&mut stream, "405 Method Not Allowed", br#"{"ok":false}"#);
            }
        }
        return;
    }

    let decoded = percent_decode(raw_path);
    let trimmed = decoded.trim_start_matches('/');

    let mut candidate = root.to_path_buf();
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
        candidate = root.join("index.html");
    }

    let Ok(body) = fs::read(&candidate) else {
        let _ = stream.write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\n\r\n");
        return;
    };

    let content_type = mime_type_for_path(&candidate);
    let header = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nCache-Control: no-cache\r\nConnection: close\r\n\r\n",
        body.len()
    );
    let _ = stream.write_all(header.as_bytes());
    let _ = stream.write_all(&body);
    let _ = stream.flush();
}

fn perform_native_self_update(
    repo: &str,
    version: &str,
    proxy: &EventLoopProxy<DesktopUserEvent>,
) -> Result<(&'static str, String), (&'static str, String)> {
    let tag = format!("v{version}");
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    let staging_dir = env::temp_dir().join(format!("audora-update-{version}-{stamp}"));
    fs::create_dir_all(&staging_dir).map_err(|e| {
        (
            "staging_failed",
            format!("Failed to create temporary update directory: {e}"),
        )
    })?;

    if cfg!(target_os = "macos") {
        let archive_name = format!("Audora_{version}_macos-aarch64.app.tar.gz");
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
                let _ = Command::new("/usr/bin/xattr")
                    .arg("-cr")
                    .arg(&staged_app)
                    .status();
                let _ = Command::new("/usr/bin/codesign")
                    .args(["--force", "--deep", "--sign", "-"])
                    .arg(&staged_app)
                    .status();
                let verify_ok = Command::new("/usr/bin/codesign")
                    .args(["--verify", "--deep", "--strict"])
                    .arg(&staged_app)
                    .status()
                    .map(|s| s.success())
                    .unwrap_or(false);
                if !verify_ok {
                    return Err((
                        "signature_verification_failed",
                        "Downloaded macOS bundle failed codesign verification.".to_owned(),
                    ));
                }

                let target_app = resolve_current_macos_app_bundle()
                    .unwrap_or_else(|| PathBuf::from("/Applications/Audora.app"));

                if target_app.exists() {
                    let _ = fs::remove_dir_all(&target_app);
                }
                let moved = Command::new("/bin/mv")
                    .arg(&staged_app)
                    .arg(&target_app)
                    .status()
                    .map(|s| s.success())
                    .unwrap_or(false)
                    || Command::new("/bin/cp")
                        .arg("-R")
                        .arg(&staged_app)
                        .arg(&target_app)
                        .status()
                        .map(|s| s.success())
                        .unwrap_or(false);

                if moved {
                    let _ = fs::remove_dir_all(&staging_dir);
                    let target_str = target_app.to_string_lossy().into_owned();
                    let _ = Command::new("/bin/sh")
                        .args([
                            "-c",
                            &format!("sleep 0.6 && /usr/bin/open -n \"{target_str}\""),
                        ])
                        .spawn();
                    let _ = proxy.send_event(DesktopUserEvent::Quit);

                    return Ok((
                        "restarting",
                        format!(
                            "Audora {version} verified, installed to {}, and restarting…",
                            target_app.display()
                        ),
                    ));
                }
            }
        }

        let dmg_name = format!("Audora_{version}_macos-aarch64.dmg");
        let dmg_url = format!("https://github.com/{repo}/releases/download/{tag}/{dmg_name}");
        let downloads_dir = env::var_os("HOME")
            .map(|h| PathBuf::from(h).join("Downloads"))
            .unwrap_or_else(|| staging_dir.clone());
        let dmg_path = downloads_dir.join(&dmg_name);

        let dmg_ok = Command::new("/usr/bin/curl")
            .args(["-fSL", "--retry", "2", "-o"])
            .arg(&dmg_path)
            .arg(&dmg_url)
            .status()
            .map(|s| s.success())
            .unwrap_or(false);

        if dmg_ok && dmg_path.is_file() {
            let _ = Command::new("/usr/bin/open").arg(&dmg_path).spawn();
            return Ok((
                "downloaded_installer",
                format!("Downloaded {dmg_name} to ~/Downloads and mounted installer."),
            ));
        }

        let release_page = format!("https://github.com/{repo}/releases/tag/{tag}");
        let _ = Command::new("/usr/bin/open").arg(&release_page).spawn();
        return Ok((
            "opened_release",
            format!("Opened GitHub release {tag} in your browser to complete installation."),
        ));
    }

    if cfg!(target_os = "windows") {
        let msi_name = format!("Audora_{version}_windows-x86_64.msi");
        let msi_url = format!("https://github.com/{repo}/releases/download/{tag}/{msi_name}");
        let msi_path = staging_dir.join(&msi_name);
        let curl_ok = Command::new("curl.exe")
            .args(["-fSL", "--retry", "2", "-o"])
            .arg(&msi_path)
            .arg(&msi_url)
            .status()
            .map(|s| s.success())
            .unwrap_or(false);

        if curl_ok && msi_path.is_file() {
            let _ = Command::new("msiexec.exe").arg("/i").arg(&msi_path).spawn();
            let _ = proxy.send_event(DesktopUserEvent::Quit);
            return Ok(("restarting", format!("Launched {msi_name} installer.")));
        }

        let release_page = format!("https://github.com/{repo}/releases/tag/{tag}");
        let _ = Command::new("cmd")
            .args(["/C", "start", "", &release_page])
            .spawn();
        return Ok(("opened_release", format!("Opened {release_page}.")));
    }

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

fn stream_local_media_file(stream: &mut TcpStream, raw_path: &str, range_header: Option<&str>) {
    let candidate = Path::new(raw_path.trim());
    let ext = candidate
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();

    if !candidate.is_absolute() || !is_supported_media_extension(&ext) || !candidate.is_file() {
        let _ = stream.write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\n\r\n");
        return;
    }

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

    let mut start = 0_u64;
    let mut end = total_len.saturating_sub(1);
    let mut is_partial = false;

    if let Some(range_val) = range_header {
        if let Some(spec) = range_val.strip_prefix("bytes=") {
            if let Some((start_str, end_str)) = spec.split_once('-') {
                if let Ok(parsed_start) = start_str.trim().parse::<u64>() {
                    if parsed_start < total_len {
                        start = parsed_start;
                        if let Ok(parsed_end) = end_str.trim().parse::<u64>() {
                            end = parsed_end.min(total_len.saturating_sub(1));
                        }
                        is_partial = true;
                    }
                }
            }
        }
    }

    if total_len == 0 {
        let header = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: {content_type}\r\nAccept-Ranges: bytes\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
        );
        let _ = stream.write_all(header.as_bytes());
        let _ = stream.flush();
        return;
    }

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

fn canonical_path_string(p: &Path) -> String {
    let raw = fs::canonicalize(p)
        .unwrap_or_else(|_| p.to_path_buf())
        .to_string_lossy()
        .into_owned();
    if let Some(stripped) = raw.strip_prefix(r"\\?\") {
        stripped.to_owned()
    } else {
        raw
    }
}

fn read_exact_at(file: &mut File, offset: u64, buf: &mut [u8]) -> bool {
    if file.seek(SeekFrom::Start(offset)).is_err() {
        return false;
    }
    file.read_exact(buf).is_ok()
}

fn read_mp4_box_header(file: &mut File, pos: u64, limit: u64) -> Option<([u8; 4], u64, u64)> {
    if pos.checked_add(8)? > limit {
        return None;
    }
    let mut hdr = [0_u8; 16];
    if !read_exact_at(file, pos, &mut hdr[..8]) {
        return None;
    }
    let sz32 = u32::from_be_bytes([hdr[0], hdr[1], hdr[2], hdr[3]]);
    let box_type = [hdr[4], hdr[5], hdr[6], hdr[7]];
    let (box_size, header_len) = if sz32 == 1 {
        if pos.checked_add(16)? > limit
            || !read_exact_at(file, pos.saturating_add(8), &mut hdr[8..16])
        {
            return None;
        }
        let sz64 = u64::from_be_bytes([
            hdr[8], hdr[9], hdr[10], hdr[11], hdr[12], hdr[13], hdr[14], hdr[15],
        ]);
        (sz64, 16_u64)
    } else if sz32 == 0 {
        (limit.saturating_sub(pos), 8_u64)
    } else {
        (u64::from(sz32), 8_u64)
    };
    if box_size < header_len || pos.checked_add(box_size)? > limit {
        return None;
    }
    Some((box_type, box_size, header_len))
}

fn probe_isobmff_duration_ms(file: &mut File, file_len: u64) -> Option<u64> {
    let mut pos = 0_u64;
    let mut sidx_fallback_ms: Option<u64> = None;
    let mut max_boxes = 0_usize;

    while pos < file_len && max_boxes < 256 {
        max_boxes = max_boxes.saturating_add(1);
        let Some((box_type, box_size, hdr_len)) = read_mp4_box_header(file, pos, file_len) else {
            break;
        };
        if &box_type == b"moov" {
            let moov_end = pos.saturating_add(box_size);
            let mut cpos = pos.saturating_add(hdr_len);
            let mut child_guard = 0_usize;
            while cpos < moov_end && child_guard < 128 {
                child_guard = child_guard.saturating_add(1);
                let Some((ctype, csize, chdr)) = read_mp4_box_header(file, cpos, moov_end) else {
                    break;
                };
                if &ctype == b"mvhd" {
                    let payload_pos = cpos.saturating_add(chdr);
                    let payload_len = csize.saturating_sub(chdr);
                    if payload_len >= 20 {
                        let mut mvhd = [0_u8; 32];
                        let want = (payload_len as usize).min(32);
                        if read_exact_at(file, payload_pos, &mut mvhd[..want]) {
                            let version = mvhd[0];
                            let (timescale, duration) = if version == 1 && want >= 32 {
                                let ts =
                                    u32::from_be_bytes([mvhd[20], mvhd[21], mvhd[22], mvhd[23]]);
                                let dur = u64::from_be_bytes([
                                    mvhd[24], mvhd[25], mvhd[26], mvhd[27], mvhd[28], mvhd[29],
                                    mvhd[30], mvhd[31],
                                ]);
                                (ts, if dur == u64::MAX { 0 } else { dur })
                            } else if version == 0 {
                                let ts =
                                    u32::from_be_bytes([mvhd[12], mvhd[13], mvhd[14], mvhd[15]]);
                                let dur32 =
                                    u32::from_be_bytes([mvhd[16], mvhd[17], mvhd[18], mvhd[19]]);
                                (
                                    ts,
                                    if dur32 == u32::MAX {
                                        0
                                    } else {
                                        u64::from(dur32)
                                    },
                                )
                            } else {
                                (0, 0)
                            };
                            if timescale > 0 && duration > 0 {
                                let ms = (u128::from(duration) * 1_000) / u128::from(timescale);
                                if let Ok(ms64) = u64::try_from(ms) {
                                    if ms64 > 0 {
                                        return Some(ms64);
                                    }
                                }
                            }
                        }
                    }
                }
                cpos = cpos.saturating_add(csize);
            }
        } else if &box_type == b"sidx" && sidx_fallback_ms.is_none() {
            let payload_pos = pos.saturating_add(hdr_len);
            let payload_len = box_size.saturating_sub(hdr_len);
            if (24..=2_097_152).contains(&payload_len) {
                let mut buf = vec![0_u8; payload_len as usize];
                if read_exact_at(file, payload_pos, &mut buf) {
                    let version = buf[0];
                    let timescale = u32::from_be_bytes([buf[8], buf[9], buf[10], buf[11]]);
                    let entries_offset = if version == 0 { 20_usize } else { 28_usize };
                    if timescale > 0 && buf.len() >= entries_offset + 4 {
                        let ref_count =
                            u16::from_be_bytes([buf[entries_offset + 2], buf[entries_offset + 3]])
                                as usize;
                        let table_start = entries_offset + 4;
                        if buf.len() >= table_start.saturating_add(ref_count.saturating_mul(12)) {
                            let mut total_dur = 0_u64;
                            for idx in 0..ref_count {
                                let off = table_start + idx * 12 + 4;
                                let sub_dur = u32::from_be_bytes([
                                    buf[off],
                                    buf[off + 1],
                                    buf[off + 2],
                                    buf[off + 3],
                                ]);
                                total_dur = total_dur.saturating_add(u64::from(sub_dur));
                            }
                            if total_dur > 0 {
                                let ms = (u128::from(total_dur) * 1_000) / u128::from(timescale);
                                sidx_fallback_ms = u64::try_from(ms).ok().filter(|&v| v > 0);
                            }
                        }
                    }
                }
            }
        }
        pos = pos.saturating_add(box_size);
    }

    sidx_fallback_ms
}

fn probe_wav_duration_ms(file: &mut File, file_len: u64) -> Option<u64> {
    if file_len < 44 {
        return None;
    }
    let mut riff = [0_u8; 12];
    if !read_exact_at(file, 0, &mut riff) || &riff[0..4] != b"RIFF" || &riff[8..12] != b"WAVE" {
        return None;
    }
    let mut pos = 12_u64;
    let mut byte_rate = 0_u32;
    let mut data_size = 0_u64;
    let mut guard = 0_usize;
    while pos.saturating_add(8) <= file_len && guard < 64 {
        guard = guard.saturating_add(1);
        let mut chdr = [0_u8; 8];
        if !read_exact_at(file, pos, &mut chdr) {
            break;
        }
        let cid = [chdr[0], chdr[1], chdr[2], chdr[3]];
        let csz = u64::from(u32::from_le_bytes([chdr[4], chdr[5], chdr[6], chdr[7]]));
        if &cid == b"fmt " && csz >= 16 {
            let mut fmt_buf = [0_u8; 16];
            if read_exact_at(file, pos.saturating_add(8), &mut fmt_buf) {
                byte_rate = u32::from_le_bytes([fmt_buf[8], fmt_buf[9], fmt_buf[10], fmt_buf[11]]);
            }
        } else if &cid == b"data" {
            data_size = csz.min(file_len.saturating_sub(pos.saturating_add(8)));
            if byte_rate > 0 {
                break;
            }
        }
        let padded = csz.saturating_add(csz & 1);
        pos = pos.saturating_add(8).saturating_add(padded);
    }
    if byte_rate > 0 && data_size > 0 {
        let ms = (u128::from(data_size) * 1_000) / u128::from(byte_rate);
        u64::try_from(ms).ok().filter(|&v| v > 0)
    } else {
        None
    }
}

fn probe_flac_duration_ms(file: &mut File, file_len: u64) -> Option<u64> {
    if file_len < 42 {
        return None;
    }
    let mut hdr = [0_u8; 42];
    if !read_exact_at(file, 0, &mut hdr) || &hdr[0..4] != b"fLaC" || (hdr[4] & 0x7F) != 0 {
        return None;
    }
    let sample_rate =
        (u32::from(hdr[18]) << 12) | (u32::from(hdr[19]) << 4) | (u32::from(hdr[20]) >> 4);
    let total_samples = (u64::from(hdr[21] & 0x0F) << 32)
        | (u64::from(hdr[22]) << 24)
        | (u64::from(hdr[23]) << 16)
        | (u64::from(hdr[24]) << 8)
        | u64::from(hdr[25]);
    if sample_rate > 0 && total_samples > 0 {
        let ms = (u128::from(total_samples) * 1_000) / u128::from(sample_rate);
        u64::try_from(ms).ok().filter(|&v| v > 0)
    } else {
        None
    }
}

fn probe_media_duration_ms(path: &Path) -> Option<u64> {
    let ext = path
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    let mut file = File::open(path).ok()?;
    let file_len = file.metadata().ok()?.len();
    if file_len == 0 {
        return None;
    }
    match ext.as_str() {
        "mp4" | "m4a" | "m4v" | "mov" => probe_isobmff_duration_ms(&mut file, file_len),
        "wav" => probe_wav_duration_ms(&mut file, file_len),
        "flac" => probe_flac_duration_ms(&mut file, file_len),
        _ => None,
    }
}

fn pick_native_media_files() -> Vec<(String, String, u64, Option<u64>)> {
    let raw_lines: Vec<String> = if cfg!(target_os = "macos") {
        let script = r#"
            set chosenFiles to choose file with prompt "Select local audio or video files for Audora" with multiple selections allowed
            set outText to ""
            repeat with f in chosenFiles
                set outText to outText & (POSIX path of f) & linefeed
            end repeat
            return outText
        "#;
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
        Command::new("powershell")
            .args([
                "-NoProfile",
                "-Command",
                "Add-Type -AssemblyName System.Windows.Forms; $d = New-Object System.Windows.Forms.OpenFileDialog; $d.Multiselect = $true; $d.Filter = 'Media Files|*.mp4;*.m4v;*.mov;*.webm;*.mkv;*.mp3;*.wav;*.flac;*.ogg;*.opus;*.m4a;*.aac|All Files|*.*'; if ($d.ShowDialog() -eq 'OK') { $d.FileNames | ForEach-Object { Write-Output $_ } }",
            ])
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
        Command::new("zenity")
            .args([
                "--file-selection",
                "--multiple",
                "--separator=\n",
                "--title=Select local audio or video files for Audora",
            ])
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
        return Some(chosen);
    }

    if cfg!(target_os = "windows") {
        let output = Command::new("powershell")
            .args([
                "-NoProfile",
                "-Command",
                "Add-Type -AssemblyName System.Windows.Forms; $f = New-Object System.Windows.Forms.FolderBrowserDialog; $f.Description = 'Select a local media folder for Audora'; if ($f.ShowDialog() -eq 'OK') { Write-Output $f.SelectedPath }",
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
        return Some(chosen);
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
                    return Some(chosen);
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

fn handle_scan_folder_request(stream: &mut TcpStream, raw_path: &str) {
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

    let folder_name = candidate
        .file_name()
        .and_then(|s| s.to_str())
        .filter(|s| !s.is_empty())
        .unwrap_or(&normalized);

    let mut discovered: Vec<(String, String, u64, Option<u64>)> = Vec::new();
    collect_media_files_in_dir(candidate, 0, 4, 2_000, &mut discovered);

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

    let escaped_folder_path = escape_json_str(&normalized);
    let escaped_folder_name = escape_json_str(folder_name);
    let body = format!(
        r#"{{"ok":true,"folder_path":"{escaped_folder_path}","folder_name":"{escaped_folder_name}","files":{files_json}}}"#
    );
    write_json_response(stream, "200 OK", body.as_bytes());
}
