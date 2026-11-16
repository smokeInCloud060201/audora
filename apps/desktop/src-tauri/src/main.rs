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
    build_production_dispatcher, default_app_data_dir, default_database_path,
    domain::DEFAULT_GITHUB_RELEASE_REPO, services::cleanup_all_app_artifacts,
};
use tao::dpi::LogicalSize;
use tao::event::{Event, WindowEvent};
use tao::event_loop::{ControlFlow, EventLoopBuilder, EventLoopProxy};
use tao::window::WindowBuilder;
use tracing::{error, info, warn};
use wry::{BackgroundThrottlingPolicy, WebViewBuilder};

#[derive(Debug, Clone, Copy)]
enum DesktopUserEvent {
    Quit,
    SetWindowMode { minimum: bool },
    DragWindow,
}

const MAX_HTTP_BODY_BYTES: usize = 32 * 1024 * 1024;

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
    let dispatcher = match build_production_dispatcher(&db_path) {
        Ok(d) => Arc::new(d),
        Err(err) => {
            error!(
                code = err.code().as_str(),
                "Audora failed to initialize persistence core"
            );
            std::process::exit(1);
        }
    };

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

    let Some(server_url) = start_local_ui_server(ui_dist, proxy.clone()) else {
        error!("Failed to bind local UI loopback server");
        std::process::exit(1);
    };

    info!(url = %server_url, "Launching Audora desktop window");

    let window = match WindowBuilder::new()
        .with_title("Audora")
        .with_inner_size(LogicalSize::new(1440.0, 900.0))
        .with_min_inner_size(LogicalSize::new(420.0, 320.0))
        .build(&event_loop)
    {
        Ok(win) => win,
        Err(err) => {
            error!(error = %err, "Failed to create native OS window");
            std::process::exit(1);
        }
    };

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

    let ipc_proxy = proxy.clone();
    let webview = match WebViewBuilder::new()
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
    let mut window = Some(window);
    let mut saved_full_size = LogicalSize::new(1440.0_f64, 900.0_f64);
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
                drop(window.take());
                *control_flow = ControlFlow::Exit;
                std::process::exit(0);
            }
            Event::UserEvent(DesktopUserEvent::SetWindowMode { minimum }) => {
                if let Some(win) = window.as_ref() {
                    if minimum && !is_minimum_mode {
                        let curr = win.inner_size().to_logical::<f64>(win.scale_factor());
                        if curr.width >= 600.0 && curr.height >= 400.0 {
                            saved_full_size = curr;
                        }
                        is_minimum_mode = true;
                        win.set_min_inner_size(Some(LogicalSize::new(360.0, 168.0)));
                        win.set_decorations(false);
                        win.set_always_on_top(true);
                        win.set_inner_size(LogicalSize::new(360.0, 168.0));
                    } else if !minimum && is_minimum_mode {
                        is_minimum_mode = false;
                        win.set_always_on_top(false);
                        win.set_decorations(true);
                        win.set_inner_size(saved_full_size);
                        win.set_min_inner_size(Some(LogicalSize::new(420.0, 320.0)));
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

fn start_local_ui_server(
    ui_dist: PathBuf,
    proxy: EventLoopProxy<DesktopUserEvent>,
) -> Option<String> {
    let listener = (41957..=41966)
        .find_map(|port| TcpListener::bind(("127.0.0.1", port)).ok())
        .or_else(|| TcpListener::bind(("127.0.0.1", 0)).ok())?;
    let local_addr = listener.local_addr().ok()?;
    let url = format!("http://127.0.0.1:{}/", local_addr.port());

    thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let root = ui_dist.clone();
            let stream_proxy = proxy.clone();
            thread::spawn(move || {
                handle_http_connection(stream, &root, &stream_proxy);
            });
        }
    });

    Some(url)
}

fn read_http_request(stream: &mut TcpStream) -> Option<(String, String, Option<String>, Vec<u8>)> {
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
    for line in lines {
        if let Some((k, v)) = line.split_once(':') {
            let key = k.trim();
            if key.eq_ignore_ascii_case("content-length") {
                if let Ok(parsed) = v.trim().parse::<usize>() {
                    content_length = parsed;
                }
            } else if key.eq_ignore_ascii_case("range") {
                range_header = Some(v.trim().to_owned());
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

    Some((method, raw_target, range_header, body))
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
) {
    let Some((method, raw_target, range_header, req_body)) = read_http_request(&mut stream) else {
        return;
    };

    let (raw_path, raw_query) = raw_target
        .split_once('?')
        .map_or((raw_target.as_str(), ""), |(p, q)| (p, q));

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
            for (idx, (p, name, sz)) in picked.iter().enumerate() {
                if idx > 0 {
                    files_json.push(',');
                }
                let ep = escape_json_str(p);
                let en = escape_json_str(name);
                files_json.push_str(&format!(r#"{{"path":"{ep}","name":"{en}","size":{sz}}}"#));
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

    if raw_path == "/__audora_state" {
        let data_dir = default_app_data_dir();
        let state_file = data_dir.join("ui-state.json");
        match method.as_str() {
            "GET" => {
                if let Ok(bytes) = fs::read(&state_file) {
                    write_json_response(&mut stream, "200 OK", &bytes);
                } else {
                    write_json_response(&mut stream, "200 OK", b"{}");
                }
            }
            "POST" => {
                if fs::create_dir_all(&data_dir).is_ok() {
                    let tmp_file = data_dir.join("ui-state.json.tmp");
                    if fs::write(&tmp_file, &req_body).is_ok()
                        && fs::rename(&tmp_file, &state_file).is_ok()
                    {
                        write_json_response(&mut stream, "200 OK", br#"{"ok":true}"#);
                        return;
                    }
                }
                write_json_response(&mut stream, "500 Internal Server Error", br#"{"ok":false}"#);
            }
            "DELETE" => {
                let _ = fs::remove_file(&state_file);
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
    fs::canonicalize(p)
        .unwrap_or_else(|_| p.to_path_buf())
        .to_string_lossy()
        .into_owned()
}

fn pick_native_media_files() -> Vec<(String, String, u64)> {
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
        out.push((canonical, name, meta.len()));
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
    out: &mut Vec<(String, String, u64)>,
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
                out.push((
                    canonical_path_string(&path),
                    name_str.to_owned(),
                    meta.len(),
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

    let mut discovered: Vec<(String, String, u64)> = Vec::new();
    collect_media_files_in_dir(candidate, 0, 4, 2_000, &mut discovered);

    let mut files_json = String::from("[");
    for (idx, (file_path, file_name, size)) in discovered.iter().enumerate() {
        if idx > 0 {
            files_json.push(',');
        }
        let escaped_path = escape_json_str(file_path);
        let escaped_name = escape_json_str(file_name);
        files_json.push_str(&format!(
            r#"{{"path":"{escaped_path}","name":"{escaped_name}","size":{size}}}"#
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
