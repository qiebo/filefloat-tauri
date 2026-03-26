#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use serde::{Deserialize, Serialize};
#[cfg(windows)]
use std::os::windows::process::CommandExt;
#[allow(unused_imports)]
use std::{
    collections::HashMap,
    env, fs,
    io::{Read, Write},
    net::{SocketAddr, TcpStream},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{Arc, Mutex},
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use tauri::{
    menu::{CheckMenuItem, MenuBuilder, MenuItemBuilder, Submenu},
    tray::TrayIconBuilder,
    AppHandle, Emitter, Manager, PhysicalPosition, PhysicalSize, Position, Size, WebviewWindow,
    Window,
};
use walkdir::{DirEntry, WalkDir};

const COLLAPSED_SIZE_LOGICAL: u32 = 64;
#[allow(dead_code)]
const EVERYTHING_PORT: u16 = 6789;
const WINDOW_MARGIN: i32 = 5;
const SNAP_THRESHOLD: i32 = 40;
const SNAP_REVEAL_SIZE: i32 = 6;
const SNAP_SHOW_THRESHOLD: i32 = 8;
const SNAP_HIDE_THRESHOLD: i32 = 120;
const EVERYTHING_INSTANCE_NAME: &str = "FileFloat";
const EVERYTHING_RUNTIME_DIR_NAME: &str = "FileFloat-Everything";
const EVERYTHING_HTTP_PORT: u16 = 18999;
const AUTOSTART_REG_PATH: &str = r"HKCU\Software\Microsoft\Windows\CurrentVersion\Run";
const AUTOSTART_REG_VALUE: &str = "FileFloat";
#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

fn check_everything() -> bool {
    let addr: SocketAddr = ([127, 0, 0, 1], EVERYTHING_PORT).into();
    TcpStream::connect_timeout(&addr, Duration::from_millis(600)).is_ok()
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
enum SnapState {
    #[default]
    None,
    Left,
    Right,
    Top,
    Bottom,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
struct WindowBounds {
    x: i32,
    y: i32,
    width: u32,
    height: u32,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct SnapPayload {
    snap_state: SnapState,
    is_snapped: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct SearchResult {
    name: String,
    path: String,
    kind: String,
}

fn normalize_search_result(mut result: SearchResult) -> SearchResult {
    let path = Path::new(result.path.trim());
    let kind = result.kind.trim().to_ascii_lowercase();
    let is_folder = path.is_dir()
        || kind.contains("folder")
        || kind.contains("directory")
        || result.path.ends_with('\\')
        || result.path.ends_with('/');

    result.kind = if is_folder {
        "folder".to_string()
    } else {
        "file".to_string()
    };
    result
}

fn normalize_search_results(results: Vec<SearchResult>) -> Vec<SearchResult> {
    results.into_iter().map(normalize_search_result).collect()
}

#[derive(Clone, Copy, Debug)]
struct DragTracking {
    start_mouse_x: f64,
    start_mouse_y: f64,
    start_win_x: i32,
    start_win_y: i32,
}

#[derive(Default)]
struct RuntimeState {
    collapse_restore_bounds: Option<WindowBounds>,
    collapse_restore_snap_state: SnapState,
    expanded_anchor_position: Option<(i32, i32)>,
    current_snap_state: SnapState,
    is_snapped_hidden: bool,
    snapped_x: i32,
    snapped_y: i32,
    snap_reveal_armed: bool,
    drag_tracking: Option<DragTracking>,
    drag_snap_state: Option<SnapState>,
    drag_has_moved: bool,
    drag_poller_started: bool,
    snap_poller_started: bool,
    everything_runtime_started: bool,
}

#[derive(Default)]
struct SharedState {
    inner: Mutex<RuntimeState>,
}

fn toggle_window(app: &tauri::AppHandle) -> tauri::Result<()> {
    if let Some(window) = app.get_webview_window("main") {
        if window.is_visible()? {
            window.hide()?;
        } else {
            window.show()?;
            window.set_focus()?;
        }
    }
    Ok(())
}

fn handle_global_shortcut(app: &tauri::AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.show();
        let _ = window.set_focus();
        let _ = window.eval("window.dispatchEvent(new CustomEvent('filefloat-shortcut-toggle'))");
    }
}

fn get_main_window(app: &AppHandle) -> Result<WebviewWindow, String> {
    app.get_webview_window("main")
        .ok_or_else(|| "main window not found".to_string())
}

fn read_window_bounds(window: &Window) -> Result<WindowBounds, String> {
    let position = window
        .outer_position()
        .map_err(|err| format!("failed to read window position: {err}"))?;
    let size = window
        .inner_size()
        .map_err(|err| format!("failed to read window size: {err}"))?;

    Ok(WindowBounds {
        x: position.x,
        y: position.y,
        width: size.width,
        height: size.height,
    })
}

fn read_window_bounds_webview(window: &WebviewWindow) -> Result<WindowBounds, String> {
    let position = window
        .outer_position()
        .map_err(|err| format!("failed to read window position: {err}"))?;
    let size = window
        .inner_size()
        .map_err(|err| format!("failed to read window size: {err}"))?;

    Ok(WindowBounds {
        x: position.x,
        y: position.y,
        width: size.width,
        height: size.height,
    })
}

fn emit_snap_state<R: tauri::Runtime, E: Emitter<R>>(emitter: &E, snap_state: SnapState) {
    let _ = emitter.emit(
        "snap-state",
        SnapPayload {
            snap_state,
            is_snapped: snap_state != SnapState::None,
        },
    );
}

fn get_work_area(window: &Window) -> Result<(i32, i32, u32, u32), String> {
    let monitor = window
        .current_monitor()
        .map_err(|err| format!("failed to query current monitor: {err}"))?
        .or_else(|| window.primary_monitor().ok().flatten())
        .ok_or_else(|| "monitor unavailable".to_string())?;
    let work_area = monitor.work_area();
    Ok((
        work_area.position.x,
        work_area.position.y,
        work_area.size.width,
        work_area.size.height,
    ))
}

fn get_work_area_webview(window: &WebviewWindow) -> Result<(i32, i32, u32, u32), String> {
    let monitor = window
        .current_monitor()
        .map_err(|err| format!("failed to query current monitor: {err}"))?
        .or_else(|| window.primary_monitor().ok().flatten())
        .ok_or_else(|| "monitor unavailable".to_string())?;
    let work_area = monitor.work_area();
    Ok((
        work_area.position.x,
        work_area.position.y,
        work_area.size.width,
        work_area.size.height,
    ))
}

fn get_scale_factor(window: &Window) -> Result<f64, String> {
    window
        .current_monitor()
        .map_err(|err| format!("failed to query current monitor scale factor: {err}"))?
        .or_else(|| window.primary_monitor().ok().flatten())
        .map(|monitor| monitor.scale_factor())
        .ok_or_else(|| "monitor unavailable".to_string())
}

fn get_scale_factor_webview(window: &WebviewWindow) -> Result<f64, String> {
    window
        .current_monitor()
        .map_err(|err| format!("failed to query current monitor scale factor: {err}"))?
        .or_else(|| window.primary_monitor().ok().flatten())
        .map(|monitor| monitor.scale_factor())
        .ok_or_else(|| "monitor unavailable".to_string())
}

fn logical_to_physical_size(value: u32, scale_factor: f64) -> u32 {
    ((value as f64) * scale_factor).round().max(1.0) as u32
}

fn collapsed_size_for_window(window: &Window) -> Result<u32, String> {
    Ok(logical_to_physical_size(
        COLLAPSED_SIZE_LOGICAL,
        get_scale_factor(window)?,
    ))
}

fn collapsed_size_for_webview(window: &WebviewWindow) -> Result<u32, String> {
    Ok(logical_to_physical_size(
        COLLAPSED_SIZE_LOGICAL,
        get_scale_factor_webview(window)?,
    ))
}

fn set_window_bounds(window: &Window, bounds: WindowBounds) -> Result<(), String> {
    window
        .set_resizable(true)
        .map_err(|err| format!("failed to set resizable=true: {err}"))?;
    window
        .set_position(Position::Physical(PhysicalPosition::new(
            bounds.x, bounds.y,
        )))
        .map_err(|err| format!("failed to set position: {err}"))?;
    window
        .set_size(Size::Physical(PhysicalSize::new(
            bounds.width,
            bounds.height,
        )))
        .map_err(|err| format!("failed to set size: {err}"))?;
    Ok(())
}

fn set_window_bounds_webview(window: &WebviewWindow, bounds: WindowBounds) -> Result<(), String> {
    window
        .set_resizable(true)
        .map_err(|err| format!("failed to set resizable=true: {err}"))?;
    window
        .set_position(Position::Physical(PhysicalPosition::new(
            bounds.x, bounds.y,
        )))
        .map_err(|err| format!("failed to set position: {err}"))?;
    window
        .set_size(Size::Physical(PhysicalSize::new(
            bounds.width,
            bounds.height,
        )))
        .map_err(|err| format!("failed to set size: {err}"))?;
    Ok(())
}

fn snap_window_to_edge(
    window: &WebviewWindow,
    shared_state: &Arc<SharedState>,
) -> Result<(), String> {
    let bounds = read_window_bounds_webview(window)?;
    let (work_x, work_y, work_w, work_h) = get_work_area_webview(window)?;

    let mut target = bounds;
    let snap_state = if bounds.x <= work_x + SNAP_THRESHOLD {
        target.x = work_x;
        SnapState::Left
    } else if bounds.x + bounds.width as i32 >= work_x + work_w as i32 - SNAP_THRESHOLD {
        target.x = work_x + work_w as i32 - bounds.width as i32;
        SnapState::Right
    } else if bounds.y <= work_y + SNAP_THRESHOLD {
        target.y = work_y;
        SnapState::Top
    } else if bounds.y + bounds.height as i32 >= work_y + work_h as i32 - SNAP_THRESHOLD {
        target.y = work_y + work_h as i32 - bounds.height as i32;
        SnapState::Bottom
    } else {
        SnapState::None
    };

    if snap_state == SnapState::None {
        let mut runtime = shared_state.inner.lock().expect("state poisoned");
        runtime.current_snap_state = SnapState::None;
        runtime.is_snapped_hidden = false;
        runtime.collapse_restore_snap_state = SnapState::None;
        drop(runtime);
        emit_snap_state(window, SnapState::None);
        return Ok(());
    }

    let hidden = match snap_state {
        SnapState::Left => WindowBounds {
            x: work_x - target.width as i32 + SNAP_REVEAL_SIZE,
            ..target
        },
        SnapState::Right => WindowBounds {
            x: work_x + work_w as i32 - SNAP_REVEAL_SIZE,
            ..target
        },
        SnapState::Top => WindowBounds {
            y: work_y - target.height as i32 + SNAP_REVEAL_SIZE,
            ..target
        },
        SnapState::Bottom => WindowBounds {
            y: work_y + work_h as i32 - SNAP_REVEAL_SIZE,
            ..target
        },
        SnapState::None => target,
    };

    set_window_bounds_webview(window, hidden)?;

    {
        let mut runtime = shared_state.inner.lock().expect("state poisoned");
        runtime.current_snap_state = snap_state;
        runtime.is_snapped_hidden = true;
        runtime.snap_reveal_armed = false;
        match snap_state {
            SnapState::Left | SnapState::Right => runtime.snapped_y = target.y,
            SnapState::Top | SnapState::Bottom => runtime.snapped_x = target.x,
            SnapState::None => {}
        }
    }

    emit_snap_state(window, snap_state);
    Ok(())
}

fn visible_bounds_for_snap(
    bounds: WindowBounds,
    snap_state: SnapState,
    panel_w: u32,
    panel_h: u32,
    work_x: i32,
    work_y: i32,
    work_w: u32,
    work_h: u32,
) -> Option<WindowBounds> {
    if snap_state == SnapState::None {
        return None;
    }

    let max_x = work_x + work_w as i32 - panel_w as i32 - WINDOW_MARGIN;
    let max_y = work_y + work_h as i32 - panel_h as i32 - WINDOW_MARGIN;
    let min_x = work_x + WINDOW_MARGIN;
    let min_y = work_y + WINDOW_MARGIN;

    let target = match snap_state {
        SnapState::Left => WindowBounds {
            x: work_x,
            y: bounds.y.clamp(min_y, max_y),
            width: panel_w,
            height: panel_h,
        },
        SnapState::Right => WindowBounds {
            x: work_x + work_w as i32 - panel_w as i32,
            y: bounds.y.clamp(min_y, max_y),
            width: panel_w,
            height: panel_h,
        },
        SnapState::Top => WindowBounds {
            x: bounds.x.clamp(min_x, max_x),
            y: work_y,
            width: panel_w,
            height: panel_h,
        },
        SnapState::Bottom => WindowBounds {
            x: bounds.x.clamp(min_x, max_x),
            y: work_y + work_h as i32 - panel_h as i32,
            width: panel_w,
            height: panel_h,
        },
        SnapState::None => unreachable!(),
    };

    Some(target)
}

fn restore_hidden_snap_state(
    window: &Window,
    shared_state: &Arc<SharedState>,
    target: WindowBounds,
    snap_state: SnapState,
) -> Result<(), String> {
    if snap_state == SnapState::None {
        set_window_bounds(window, target)?;
        emit_snap_state(window, SnapState::None);
        return Ok(());
    }

    let (work_x, work_y, work_w, work_h) = get_work_area(window)?;
    let hidden = match snap_state {
        SnapState::Left => WindowBounds {
            x: work_x - target.width as i32 + SNAP_REVEAL_SIZE,
            ..target
        },
        SnapState::Right => WindowBounds {
            x: work_x + work_w as i32 - SNAP_REVEAL_SIZE,
            ..target
        },
        SnapState::Top => WindowBounds {
            y: work_y - target.height as i32 + SNAP_REVEAL_SIZE,
            ..target
        },
        SnapState::Bottom => WindowBounds {
            y: work_y + work_h as i32 - SNAP_REVEAL_SIZE,
            ..target
        },
        SnapState::None => target,
    };

    set_window_bounds(window, hidden)?;

    {
        let mut runtime = shared_state.inner.lock().expect("state poisoned");
        runtime.current_snap_state = snap_state;
        runtime.is_snapped_hidden = true;
        runtime.snap_reveal_armed = false;
        match snap_state {
            SnapState::Left | SnapState::Right => runtime.snapped_y = target.y,
            SnapState::Top | SnapState::Bottom => runtime.snapped_x = target.x,
            SnapState::None => {}
        }
    }

    emit_snap_state(window, snap_state);
    Ok(())
}

#[allow(dead_code)]
fn bundled_everything_resource_dir(app: &AppHandle) -> Result<PathBuf, String> {
    app.path()
        .resource_dir()
        .map(|path| path.join("everything"))
        .map_err(|err| format!("failed to resolve resource dir: {err}"))
}

#[allow(dead_code)]
fn bundled_everything_runtime_dir(app: &AppHandle) -> Result<PathBuf, String> {
    app.path()
        .app_local_data_dir()
        .map(|path| path.join(EVERYTHING_RUNTIME_DIR_NAME))
        .map_err(|err| format!("failed to resolve app local data dir: {err}"))
}

#[allow(dead_code)]
fn sync_bundled_everything_runtime(app: &AppHandle) -> Result<PathBuf, String> {
    let resource_dir = bundled_everything_resource_dir(app)?;
    let runtime_dir = bundled_everything_runtime_dir(app)?;
    fs::create_dir_all(&runtime_dir)
        .map_err(|err| format!("failed to create Everything runtime dir: {err}"))?;

    for file_name in ["everything.exe", "Everything.lng", "es.exe"] {
        let source = resource_dir.join(file_name);
        if !source.exists() {
            return Err(format!(
                "missing bundled Everything resource: {}",
                source.display()
            ));
        }

        let target = runtime_dir.join(file_name);
        let should_copy = match (fs::metadata(&source), fs::metadata(&target)) {
            (Ok(source_meta), Ok(target_meta)) => {
                source_meta.len() != target_meta.len()
                    || source_meta.modified().ok() != target_meta.modified().ok()
            }
            (Ok(_), Err(_)) => true,
            _ => true,
        };

        if should_copy {
            fs::copy(&source, &target).map_err(|err| {
                format!(
                    "failed to copy Everything runtime file {} -> {}: {err}",
                    source.display(),
                    target.display()
                )
            })?;
        }
    }

    let ini_path = runtime_dir.join("Everything.ini");
    let ini = format!(
        "[Everything]\r\napp_data=0\r\ninstance_name={EVERYTHING_INSTANCE_NAME}\r\nrun_in_background=1\r\nshow_tray_icon=0\r\nrun_as_admin=1\r\ncheck_for_updates_on_startup=0\r\nhttp_server_enabled=1\r\nhttp_server_port={EVERYTHING_PORT}\r\n"
    );
    fs::write(&ini_path, ini).map_err(|err| format!("failed to write Everything.ini: {err}"))?;

    Ok(runtime_dir)
}

#[allow(dead_code)]
fn start_bundled_everything(app: &AppHandle) -> Result<(), String> {
    if check_everything() {
        return Ok(());
    }

    let runtime_dir = sync_bundled_everything_runtime(app)?;
    let exe_path = runtime_dir.join("everything.exe");

    let mut command = Command::new(&exe_path);
    command
        .current_dir(&runtime_dir)
        .args(["-instance", EVERYTHING_INSTANCE_NAME, "-startup"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    #[cfg(windows)]
    command.creation_flags(CREATE_NO_WINDOW);
    command
        .spawn()
        .map_err(|err| format!("failed to start bundled Everything: {err}"))?;

    let started = Instant::now();
    while started.elapsed() < Duration::from_secs(5) {
        if check_everything() {
            return Ok(());
        }
        thread::sleep(Duration::from_millis(200));
    }

    Err("bundled Everything did not start its HTTP server in time".to_string())
}

#[derive(Clone, Debug)]
struct EverythingRuntimePaths {
    root: PathBuf,
    everything_exe: PathBuf,
    lang: PathBuf,
    ini: PathBuf,
}

fn everything_runtime_root() -> Result<PathBuf, String> {
    let local_app_data =
        env::var("LOCALAPPDATA").map_err(|err| format!("LOCALAPPDATA unavailable: {err}"))?;
    Ok(PathBuf::from(local_app_data)
        .join("FileFloat")
        .join(EVERYTHING_RUNTIME_DIR_NAME))
}

fn everything_runtime_paths() -> Result<EverythingRuntimePaths, String> {
    let root = everything_runtime_root()?;
    Ok(EverythingRuntimePaths {
        everything_exe: root.join("everything.exe"),
        lang: root.join("Everything.lng"),
        ini: root.join(format!(
            "Everything-{}.ini",
            EVERYTHING_INSTANCE_NAME.to_ascii_uppercase()
        )),
        root,
    })
}

fn write_file_if_changed(path: &Path, bytes: &[u8]) -> Result<(), String> {
    if path.exists() {
        let existing =
            fs::read(path).map_err(|err| format!("failed to read {}: {err}", path.display()))?;
        if existing == bytes {
            return Ok(());
        }
    }

    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|err| format!("failed to create {}: {err}", parent.display()))?;
    }

    fs::write(path, bytes).map_err(|err| format!("failed to write {}: {err}", path.display()))
}

fn ensure_everything_runtime_files() -> Result<EverythingRuntimePaths, String> {
    let paths = everything_runtime_paths()?;
    fs::create_dir_all(&paths.root).map_err(|err| {
        format!(
            "failed to create runtime dir {}: {err}",
            paths.root.display()
        )
    })?;

    write_file_if_changed(
        &paths.everything_exe,
        include_bytes!("../runtime/everything.exe"),
    )?;
    write_file_if_changed(&paths.lang, include_bytes!("../runtime/Everything.lng"))?;

    let ini = format!(
        "[Everything]\r\napp_data=0\r\ninstance_name={EVERYTHING_INSTANCE_NAME}\r\nshow_tray_icon=0\r\nrun_in_background=1\r\nminimize_to_tray=1\r\nrun_as_admin=1\r\ncheck_for_updates_on_startup=0\r\nallow_http_server=1\r\nhttp_server_enabled=1\r\nhttp_server_port={EVERYTHING_HTTP_PORT}\r\nhttp_server_bindings=127.0.0.1\r\n"
    );
    fs::write(&paths.ini, ini)
        .map_err(|err| format!("failed to write {}: {err}", paths.ini.display()))?;

    Ok(paths)
}

fn launch_everything_runtime(paths: &EverythingRuntimePaths) -> Result<(), String> {
    let mut cmd = Command::new(&paths.everything_exe);
    cmd.current_dir(&paths.root);
    cmd.args([
        "-instance",
        EVERYTHING_INSTANCE_NAME,
        "-config",
        paths.ini.to_string_lossy().as_ref(),
        "-startup",
        "-minimized",
    ]);
    #[cfg(windows)]
    {
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    cmd.stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());

    let _child = cmd
        .spawn()
        .map_err(|err| format!("failed to launch Everything runtime: {err}"))?;
    Ok(())
}

fn parse_everything_http_results(json_text: &str) -> Result<Vec<SearchResult>, String> {
    let value: serde_json::Value = serde_json::from_str(json_text)
        .map_err(|err| format!("invalid Everything HTTP JSON: {err}"))?;

    let records: Vec<serde_json::Value> = match value {
        serde_json::Value::Array(items) => items,
        serde_json::Value::Object(map) => map
            .get("results")
            .and_then(|items| items.as_array())
            .cloned()
            .or_else(|| map.get("items").and_then(|items| items.as_array()).cloned())
            .unwrap_or_default(),
        _ => Vec::new(),
    };

    let mut results = Vec::new();
    for record in records {
        let Some(obj) = record.as_object() else {
            continue;
        };

        let explicit_full_path = obj
            .get("full_path")
            .or_else(|| obj.get("fullname"))
            .and_then(|value| value.as_str())
            .unwrap_or("")
            .trim()
            .to_string();
        let container_path = obj
            .get("path")
            .or_else(|| obj.get("folder"))
            .and_then(|value| value.as_str())
            .unwrap_or("")
            .trim()
            .to_string();
        let name_fallback_source = if !explicit_full_path.is_empty() {
            explicit_full_path.as_str()
        } else {
            container_path.as_str()
        };
        let name = obj
            .get("name")
            .or_else(|| obj.get("filename"))
            .or_else(|| obj.get("title"))
            .and_then(|value| value.as_str())
            .map(|value| value.trim().to_string())
            .unwrap_or_else(|| {
                Path::new(name_fallback_source)
                    .file_name()
                    .and_then(|value| value.to_str())
                    .unwrap_or_default()
                    .to_string()
            });
        let path = if !explicit_full_path.is_empty() {
            explicit_full_path
        } else if !container_path.is_empty() && !name.is_empty() {
            Path::new(&container_path)
                .join(&name)
                .to_string_lossy()
                .to_string()
        } else {
            container_path
        };

        if path.is_empty() && name.is_empty() {
            continue;
        }

        let kind = obj
            .get("type")
            .or_else(|| obj.get("kind"))
            .and_then(|value| value.as_str())
            .map(|value| {
                let lower = value.to_ascii_lowercase();
                if lower.contains("folder") || lower.contains("directory") {
                    "folder"
                } else {
                    "file"
                }
            })
            .unwrap_or_else(|| {
                if path.ends_with('\\') || path.ends_with('/') {
                    "folder"
                } else {
                    "file"
                }
            });

        results.push(SearchResult {
            name,
            path,
            kind: kind.to_string(),
        });
    }

    Ok(normalize_search_results(results))
}

fn http_get_text(host: &str, port: u16, path: &str) -> Result<String, String> {
    let mut stream = TcpStream::connect_timeout(
        &SocketAddr::from(([127, 0, 0, 1], port)),
        Duration::from_millis(800),
    )
    .map_err(|err| format!("failed to connect to Everything HTTP server: {err}"))?;
    stream
        .set_read_timeout(Some(Duration::from_secs(3)))
        .map_err(|err| format!("failed to set read timeout: {err}"))?;
    stream
        .set_write_timeout(Some(Duration::from_secs(3)))
        .map_err(|err| format!("failed to set write timeout: {err}"))?;

    let request =
        format!("GET {path} HTTP/1.1\r\nHost: {host}:{port}\r\nConnection: close\r\n\r\n");
    stream
        .write_all(request.as_bytes())
        .map_err(|err| format!("failed to send HTTP request: {err}"))?;

    let mut response = String::new();
    stream
        .read_to_string(&mut response)
        .map_err(|err| format!("failed to read HTTP response: {err}"))?;

    let body = response
        .split_once("\r\n\r\n")
        .map(|(_, body)| body)
        .unwrap_or(&response);

    Ok(body.to_string())
}

fn wait_for_everything_http(port: u16) -> Result<(), String> {
    let started = Instant::now();
    while started.elapsed() < Duration::from_secs(6) {
        if http_get_text("127.0.0.1", port, "/?search=&count=0&json=1")
            .map(|body| body.contains('{') || body.contains('['))
            .unwrap_or(false)
        {
            return Ok(());
        }
        thread::sleep(Duration::from_millis(200));
    }
    Err("bundled Everything HTTP server did not become ready in time".to_string())
}

fn search_by_everything(
    query: &str,
    state: &tauri::State<Arc<SharedState>>,
) -> Result<Vec<SearchResult>, String> {
    let mut runtime = state.inner.lock().expect("state poisoned");
    if !runtime.everything_runtime_started {
        let paths = ensure_everything_runtime_files()?;
        launch_everything_runtime(&paths)?;
        wait_for_everything_http(EVERYTHING_HTTP_PORT)?;
        runtime.everything_runtime_started = true;
    }
    drop(runtime);

    let encoded = urlencoding::encode(query);
    let path =
        format!("/?search={encoded}&offset=0&count=30&json=1&path_column=1&sort=name&ascending=1");
    let body = http_get_text("127.0.0.1", EVERYTHING_HTTP_PORT, &path)?;
    parse_everything_http_results(&body)
}

fn escape_ps_single_quote(value: &str) -> String {
    value.replace('\'', "''").replace(['\r', '\n'], "")
}

fn parse_search_results(text: &str) -> Result<Vec<SearchResult>, String> {
    let trimmed = text.trim();
    if trimmed.is_empty() || trimmed == "null" {
        return Ok(Vec::new());
    }

    if trimmed.starts_with('[') {
        serde_json::from_str::<Vec<SearchResult>>(trimmed)
            .map(normalize_search_results)
            .map_err(|err| format!("invalid Windows Search JSON: {err}"))
    } else {
        Ok(vec![serde_json::from_str::<SearchResult>(trimmed)
            .map(normalize_search_result)
            .map_err(|err| {
                format!("invalid Windows Search result: {err}")
            })?])
    }
}

fn run_hidden_powershell(script: &str) -> Result<String, String> {
    let mut cmd = Command::new("powershell.exe");
    cmd.args([
        "-NoLogo",
        "-NoProfile",
        "-NonInteractive",
        "-ExecutionPolicy",
        "Bypass",
        "-WindowStyle",
        "Hidden",
        "-Command",
        script,
    ]);
    #[cfg(windows)]
    {
        cmd.creation_flags(CREATE_NO_WINDOW);
    }

    let output = cmd
        .output()
        .map_err(|err| format!("failed to launch PowerShell: {err}"))?;

    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).trim().to_string());
    }

    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

fn search_by_windows_search(query: &str) -> Result<Vec<SearchResult>, String> {
    let safe = escape_ps_single_quote(query);
    let script = format!(
        r#"$ErrorActionPreference='Stop'
[Console]::OutputEncoding = [System.Text.Encoding]::UTF8
$q = '{safe}'
$conn = New-Object -ComObject ADODB.Connection
$conn.Open(""Provider=Search.CollatorDSO;Extended Properties='Application=Windows'"")
$sql = ""SELECT TOP 30 System.ItemName, System.ItemPathDisplay, System.Kind FROM SystemIndex WHERE SCOPE='file:' AND System.ItemName LIKE '%"" + $q.Replace(""'"", ""''"") + ""%'"" 
$rs = $conn.Execute($sql)
$items = @()
while (-not $rs.EOF) {{
  $name = [string]$rs.Fields.Item('System.ItemName').Value
  $path = [string]$rs.Fields.Item('System.ItemPathDisplay').Value
  $kind = [string]$rs.Fields.Item('System.Kind').Value
  if ($name -and $path) {{
    $items += [pscustomobject]@{{ name = $name; path = $path; kind = $kind }}
  }}
  $rs.MoveNext()
}}
$conn.Close()
$items | ConvertTo-Json -Compress"#,
    );
    let text = run_hidden_powershell(&script)?;
    let parsed = parse_search_results(&text)?;
    if !parsed.is_empty() {
        return Ok(parsed);
    }
    Ok(normalize_search_results(search_by_filesystem(query)))
}

fn should_descend(entry: &DirEntry) -> bool {
    if entry.depth() == 0 || !entry.file_type().is_dir() {
        return true;
    }

    !matches!(
        entry.file_name().to_string_lossy().as_ref(),
        "$Recycle.Bin"
            | "Windows"
            | "Program Files"
            | "Program Files (x86)"
            | "ProgramData"
            | "node_modules"
            | "target"
            | ".git"
    )
}

fn filesystem_search_roots() -> Vec<PathBuf> {
    let mut roots = Vec::new();
    let mut push_unique = |path: PathBuf| {
        if path.exists() && !roots.iter().any(|existing| existing == &path) {
            roots.push(path);
        }
    };

    if let Ok(current_dir) = env::current_dir() {
        push_unique(current_dir);
    }

    if let Ok(user_profile) = env::var("USERPROFILE") {
        let user_root = PathBuf::from(&user_profile);
        push_unique(user_root.clone());
        for child in [
            "Desktop",
            "Documents",
            "Downloads",
            "Pictures",
            "Music",
            "Videos",
            "OneDrive",
        ] {
            push_unique(user_root.join(child));
        }
    }

    for drive in 'C'..='Z' {
        push_unique(PathBuf::from(format!("{drive}:\\")).to_path_buf());
    }

    roots
}

fn file_kind(path: &Path) -> String {
    if path.is_dir() {
        return "folder".to_string();
    }

    match path
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase()
        .as_str()
    {
        "png" | "jpg" | "jpeg" | "gif" | "bmp" | "svg" | "webp" => "image".to_string(),
        "mp4" | "mkv" | "avi" | "mov" | "wmv" => "video".to_string(),
        "mp3" | "wav" | "flac" | "aac" | "m4a" => "audio".to_string(),
        "exe" | "msi" | "bat" | "cmd" => "application".to_string(),
        _ => "document".to_string(),
    }
}

fn search_by_filesystem(query: &str) -> Vec<SearchResult> {
    let needle = query.trim().to_ascii_lowercase();
    if needle.is_empty() {
        return Vec::new();
    }

    let started = Instant::now();
    let timeout = Duration::from_millis(1800);
    let max_results = 40;
    let mut results = Vec::new();

    for root in filesystem_search_roots() {
        if started.elapsed() >= timeout || results.len() >= max_results {
            break;
        }

        let walker = WalkDir::new(root)
            .follow_links(false)
            .into_iter()
            .filter_entry(should_descend);

        for entry in walker {
            if started.elapsed() >= timeout || results.len() >= max_results {
                break;
            }

            let Ok(entry) = entry else {
                continue;
            };

            let path = entry.path();
            let Some(name) = path.file_name().and_then(|value| value.to_str()) else {
                continue;
            };

            if !name.to_ascii_lowercase().contains(&needle) {
                continue;
            }

            results.push(SearchResult {
                name: name.to_string(),
                path: path.display().to_string(),
                kind: file_kind(path),
            });
        }
    }

    normalize_search_results(results)
}

fn open_with_default_app(target: &str) -> Result<(), String> {
    let mut cmd = Command::new("cmd");
    cmd.args(["/C", "start", "", target]);
    #[cfg(windows)]
    {
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    cmd.spawn()
        .map_err(|err| format!("failed to open target: {err}"))?;
    Ok(())
}

#[cfg(windows)]
fn run_hidden_registry_command(args: &[&str]) -> Result<std::process::Output, String> {
    let mut cmd = Command::new("reg.exe");
    cmd.args(args);
    cmd.stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(windows)]
    {
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    cmd.output()
        .map_err(|err| format!("failed to run registry command: {err}"))
}

#[cfg(not(windows))]
fn run_hidden_registry_command(_args: &[&str]) -> Result<std::process::Output, String> {
    Err("registry commands are only supported on Windows".to_string())
}

#[tauri::command]
fn get_auto_start_enabled() -> Result<bool, String> {
    #[cfg(windows)]
    {
        let output =
            run_hidden_registry_command(&["query", AUTOSTART_REG_PATH, "/v", AUTOSTART_REG_VALUE])?;
        return Ok(output.status.success());
    }

    #[cfg(not(windows))]
    {
        Ok(false)
    }
}

#[tauri::command]
fn set_auto_start_enabled(enabled: bool) -> Result<(), String> {
    #[cfg(windows)]
    {
        if enabled {
            let exe = env::current_exe()
                .map_err(|err| format!("failed to resolve current executable: {err}"))?;
            let value = format!("\"{}\"", exe.display());
            let output = run_hidden_registry_command(&[
                "add",
                AUTOSTART_REG_PATH,
                "/v",
                AUTOSTART_REG_VALUE,
                "/t",
                "REG_SZ",
                "/d",
                &value,
                "/f",
            ])?;
            if output.status.success() {
                return Ok(());
            }
            let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
            return Err(if stderr.is_empty() {
                "failed to enable auto start".to_string()
            } else {
                stderr
            });
        }

        let output = run_hidden_registry_command(&[
            "delete",
            AUTOSTART_REG_PATH,
            "/v",
            AUTOSTART_REG_VALUE,
            "/f",
        ])?;
        if output.status.success() {
            return Ok(());
        }
        if !get_auto_start_enabled().unwrap_or(true) {
            return Ok(());
        }
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        return Err(if stderr.is_empty() {
            "failed to disable auto start".to_string()
        } else {
            stderr
        });
    }

    #[cfg(not(windows))]
    {
        let _ = enabled;
        Ok(())
    }
}

fn reveal_in_explorer(target: &str) -> Result<(), String> {
    let mut cmd = Command::new("explorer");
    cmd.args(["/select,", target]);
    #[cfg(windows)]
    {
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    cmd.spawn()
        .map_err(|err| format!("failed to reveal file: {err}"))?;
    Ok(())
}

fn reveal_path(path: &str) -> Result<(), String> {
    reveal_in_explorer(path)
}

fn run_hidden_powershell_script(
    script: &str,
    env_vars: &[(&str, &str)],
) -> Result<std::process::Output, String> {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|err| format!("failed to create script timestamp: {err}"))?
        .as_nanos();
    let script_path = env::temp_dir().join(format!("filefloat-{stamp}-{}.ps1", std::process::id()));
    fs::write(&script_path, script).map_err(|err| {
        format!(
            "failed to write PowerShell script {}: {err}",
            script_path.display()
        )
    })?;

    let mut cmd = Command::new("powershell.exe");
    cmd.args([
        "-NoProfile",
        "-NonInteractive",
        "-ExecutionPolicy",
        "Bypass",
        "-STA",
        "-WindowStyle",
        "Hidden",
        "-File",
    ])
    .arg(&script_path);

    for (key, value) in env_vars {
        cmd.env(key, value);
    }

    cmd.stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(windows)]
    {
        cmd.creation_flags(CREATE_NO_WINDOW);
    }

    let output = cmd
        .output()
        .map_err(|err| format!("failed to run PowerShell script: {err}"))?;
    let _ = fs::remove_file(&script_path);
    Ok(output)
}

fn run_hidden_powershell_script_ok(script: &str, env_vars: &[(&str, &str)]) -> Result<(), String> {
    let output = run_hidden_powershell_script(script, env_vars)?;
    if output.status.success() {
        return Ok(());
    }

    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    Err(if stderr.is_empty() {
        "PowerShell command failed".to_string()
    } else {
        stderr
    })
}

fn canonical_existing_path(path: &str) -> Result<PathBuf, String> {
    let target = Path::new(path);
    if !target.exists() {
        return Err(format!("path not found: {path}"));
    }

    target
        .canonicalize()
        .map_err(|err| format!("failed to resolve path {path}: {err}"))
}

fn copy_text_to_clipboard(text: &str) -> Result<(), String> {
    const SCRIPT: &str = r#"
$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName System.Windows.Forms
$text = [Environment]::GetEnvironmentVariable('FILEFLOAT_TEXT')
[System.Windows.Forms.Clipboard]::SetText($text)
"#;

    run_hidden_powershell_script_ok(SCRIPT, &[("FILEFLOAT_TEXT", text)])
}

fn copy_file_paths_to_clipboard(paths: &[String], cut: bool) -> Result<(), String> {
    const SCRIPT: &str = r#"
$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName System.Windows.Forms
$raw = [Environment]::GetEnvironmentVariable('FILEFLOAT_PATHS')
if ([string]::IsNullOrWhiteSpace($raw)) {
  throw 'No file paths provided'
}
$paths = $raw -split "`n" | ForEach-Object { $_.Trim() } | Where-Object { $_ }
if (-not $paths -or $paths.Count -eq 0) {
  throw 'No file paths provided'
}
$files = New-Object System.Collections.Specialized.StringCollection
foreach ($path in $paths) {
  [void]$files.Add($path)
}
$data = New-Object System.Windows.Forms.DataObject
$data.SetFileDropList($files)
$effect = if ([Environment]::GetEnvironmentVariable('FILEFLOAT_CUT') -eq '1') { 2 } else { 1 }
$bytes = [BitConverter]::GetBytes([int32]$effect)
$stream = New-Object System.IO.MemoryStream
$stream.Write($bytes, 0, $bytes.Length)
$stream.Position = 0
$data.SetData('Preferred DropEffect', $stream)
[System.Windows.Forms.Clipboard]::SetDataObject($data, $true)
"#;

    let joined = paths.join("\n");
    let cut_flag = if cut { "1" } else { "0" };
    run_hidden_powershell_script_ok(
        SCRIPT,
        &[("FILEFLOAT_PATHS", &joined), ("FILEFLOAT_CUT", cut_flag)],
    )
}

fn delete_path_to_recycle_bin(path: &str) -> Result<(), String> {
    const SCRIPT: &str = r#"
$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName Microsoft.VisualBasic
$path = [Environment]::GetEnvironmentVariable('FILEFLOAT_TARGET')
if ([string]::IsNullOrWhiteSpace($path)) {
  throw 'Missing target path'
}
if (Test-Path -LiteralPath $path -PathType Container) {
  [Microsoft.VisualBasic.FileIO.FileSystem]::DeleteDirectory($path, 'OnlyErrorDialogs', 'SendToRecycleBin')
} elseif (Test-Path -LiteralPath $path -PathType Leaf) {
  [Microsoft.VisualBasic.FileIO.FileSystem]::DeleteFile($path, 'OnlyErrorDialogs', 'SendToRecycleBin')
} else {
  throw "Path not found: $path"
}
"#;

    run_hidden_powershell_script_ok(SCRIPT, &[("FILEFLOAT_TARGET", path)])
}

fn start_drag_poller(app: AppHandle, shared_state: Arc<SharedState>) {
    thread::spawn(move || loop {
        thread::sleep(Duration::from_millis(16));

        let tracking = {
            let state = shared_state.inner.lock().expect("state poisoned");
            state.drag_tracking
        };

        let Some(tracking) = tracking else {
            continue;
        };

        let Some(window) = app.get_webview_window("main") else {
            continue;
        };

        let mouse = match app.cursor_position() {
            Ok(pos) => pos,
            Err(_) => continue,
        };

        let dx = mouse.x - tracking.start_mouse_x;
        let dy = mouse.y - tracking.start_mouse_y;
        if dx.abs() <= 8.0 && dy.abs() <= 8.0 {
            continue;
        }

        {
            let mut state = shared_state.inner.lock().expect("state poisoned");
            state.drag_has_moved = true;
        }

        let _ = window.set_position(Position::Physical(PhysicalPosition::new(
            tracking.start_win_x + dx.round() as i32,
            tracking.start_win_y + dy.round() as i32,
        )));
    });
}

fn start_snap_poller(app: AppHandle, shared_state: Arc<SharedState>) {
    thread::spawn(move || loop {
        thread::sleep(Duration::from_millis(80));

        let (snap_state, is_hidden, snapped_x, snapped_y, is_dragging, snap_reveal_armed) = {
            let state = shared_state.inner.lock().expect("state poisoned");
            (
                state.current_snap_state,
                state.is_snapped_hidden,
                state.snapped_x,
                state.snapped_y,
                state.drag_tracking.is_some(),
                state.snap_reveal_armed,
            )
        };

        if snap_state == SnapState::None || is_dragging {
            continue;
        }

        let Some(window) = app.get_webview_window("main") else {
            continue;
        };

        let mouse = match app.cursor_position() {
            Ok(pos) => pos,
            Err(_) => continue,
        };

        let (work_x, work_y, work_w, work_h) = match get_work_area_webview(&window) {
            Ok(bounds) => bounds,
            Err(_) => continue,
        };

        let bounds = match read_window_bounds_webview(&window) {
            Ok(bounds) => bounds,
            Err(_) => continue,
        };

        let should_show = match snap_state {
            SnapState::Left => mouse.x <= (work_x + SNAP_SHOW_THRESHOLD) as f64,
            SnapState::Right => mouse.x >= (work_x + work_w as i32 - SNAP_SHOW_THRESHOLD) as f64,
            SnapState::Top => mouse.y <= (work_y + SNAP_SHOW_THRESHOLD) as f64,
            SnapState::Bottom => mouse.y >= (work_y + work_h as i32 - SNAP_SHOW_THRESHOLD) as f64,
            SnapState::None => false,
        };

        let should_hide = match snap_state {
            SnapState::Left => mouse.x > (work_x + SNAP_HIDE_THRESHOLD) as f64,
            SnapState::Right => mouse.x < (work_x + work_w as i32 - SNAP_HIDE_THRESHOLD) as f64,
            SnapState::Top => mouse.y > (work_y + SNAP_HIDE_THRESHOLD) as f64,
            SnapState::Bottom => mouse.y < (work_y + work_h as i32 - SNAP_HIDE_THRESHOLD) as f64,
            SnapState::None => false,
        };

        if is_hidden && !snap_reveal_armed && should_hide {
            let mut state = shared_state.inner.lock().expect("state poisoned");
            if state.current_snap_state == snap_state && state.is_snapped_hidden {
                state.snap_reveal_armed = true;
            }
        } else if should_show && is_hidden && snap_reveal_armed {
            let visible = match snap_state {
                SnapState::Left => WindowBounds {
                    x: work_x,
                    y: snapped_y,
                    ..bounds
                },
                SnapState::Right => WindowBounds {
                    x: work_x + work_w as i32 - bounds.width as i32,
                    y: snapped_y,
                    ..bounds
                },
                SnapState::Top => WindowBounds {
                    x: snapped_x,
                    y: work_y,
                    ..bounds
                },
                SnapState::Bottom => WindowBounds {
                    x: snapped_x,
                    y: work_y + work_h as i32 - bounds.height as i32,
                    ..bounds
                },
                SnapState::None => bounds,
            };

            if set_window_bounds_webview(&window, visible).is_ok() {
                let mut state = shared_state.inner.lock().expect("state poisoned");
                if state.current_snap_state == snap_state {
                    state.is_snapped_hidden = false;
                    state.snap_reveal_armed = true;
                }
            }
        } else if should_hide && !is_hidden {
            let hidden = match snap_state {
                SnapState::Left => WindowBounds {
                    x: work_x - bounds.width as i32 + SNAP_REVEAL_SIZE,
                    y: snapped_y,
                    ..bounds
                },
                SnapState::Right => WindowBounds {
                    x: work_x + work_w as i32 - SNAP_REVEAL_SIZE,
                    y: snapped_y,
                    ..bounds
                },
                SnapState::Top => WindowBounds {
                    x: snapped_x,
                    y: work_y - bounds.height as i32 + SNAP_REVEAL_SIZE,
                    ..bounds
                },
                SnapState::Bottom => WindowBounds {
                    x: snapped_x,
                    y: work_y + work_h as i32 - SNAP_REVEAL_SIZE,
                    ..bounds
                },
                SnapState::None => bounds,
            };

            if set_window_bounds_webview(&window, hidden).is_ok() {
                let mut state = shared_state.inner.lock().expect("state poisoned");
                if state.current_snap_state == snap_state {
                    state.is_snapped_hidden = true;
                    state.snap_reveal_armed = false;
                }
            }
        }
    });
}

#[tauri::command]
fn search_files(
    query: String,
    state: tauri::State<Arc<SharedState>>,
) -> Result<Vec<SearchResult>, String> {
    if query.trim().is_empty() {
        return Ok(Vec::new());
    }

    if let Ok(results) = search_by_everything(&query, &state) {
        if !results.is_empty() {
            return Ok(results);
        }
    }

    search_by_windows_search(&query).or_else(|_| Ok(search_by_filesystem(&query)))
}

#[tauri::command]
fn open_file(path: String) -> Result<(), String> {
    open_with_default_app(&path)
}

#[tauri::command]
fn show_in_folder(path: String) -> Result<(), String> {
    reveal_path(&path)
}

#[tauri::command]
fn copy_item(path: String, cut: bool) -> Result<(), String> {
    let resolved = canonical_existing_path(&path)?;
    let resolved = resolved
        .to_str()
        .ok_or_else(|| format!("path is not valid UTF-8: {}", resolved.display()))?
        .to_string();

    copy_file_paths_to_clipboard(&[resolved], cut)
}

#[tauri::command]
fn delete_item(path: String) -> Result<(), String> {
    let resolved = canonical_existing_path(&path)?;
    delete_path_to_recycle_bin(
        resolved
            .to_str()
            .ok_or_else(|| format!("path is not valid UTF-8: {}", resolved.display()))?,
    )
}

#[tauri::command]
fn copy_text(text: String) -> Result<(), String> {
    copy_text_to_clipboard(&text)
}

#[tauri::command]
fn set_window_size(window: tauri::Window, width: u32, height: u32) -> Result<(), String> {
    let bounds = read_window_bounds(&window)?;
    let scale_factor = get_scale_factor(&window)?;
    let physical_width = logical_to_physical_size(width, scale_factor);
    let physical_height = logical_to_physical_size(height, scale_factor);
    set_window_bounds(
        &window,
        WindowBounds {
            x: bounds.x,
            y: bounds.y,
            width: physical_width,
            height: physical_height,
        },
    )
}

#[tauri::command]
fn set_window_position(window: tauri::Window, x: i32, y: i32) -> Result<(), String> {
    window
        .set_position(Position::Physical(PhysicalPosition::new(x, y)))
        .map_err(|err| format!("failed to set window position: {err}"))
}

#[tauri::command]
fn get_screen_bounds(window: tauri::Window) -> Result<serde_json::Value, String> {
    let (x, y, width, height) = get_work_area(&window)?;
    Ok(serde_json::json!({
        "x": x,
        "y": y,
        "width": width,
        "height": height
    }))
}

#[tauri::command]
fn get_window_bounds(window: tauri::Window) -> Result<WindowBounds, String> {
    read_window_bounds(&window)
}

#[tauri::command]
fn ensure_safe_position(
    window: tauri::Window,
    state: tauri::State<Arc<SharedState>>,
    panel_w: u32,
    panel_h: u32,
) -> Result<(), String> {
    let bounds = read_window_bounds(&window)?;
    let (work_x, work_y, work_w, work_h) = get_work_area(&window)?;
    let scale_factor = get_scale_factor(&window)?;
    let panel_w = logical_to_physical_size(panel_w, scale_factor);
    let panel_h = logical_to_physical_size(panel_h, scale_factor);
    let collapsed_size = collapsed_size_for_window(&window)?;
    let current_snap_state = {
        let runtime = state.inner.lock().expect("state poisoned");
        runtime.current_snap_state
    };

    let restore_bounds = visible_bounds_for_snap(
        bounds,
        current_snap_state,
        collapsed_size,
        collapsed_size,
        work_x,
        work_y,
        work_w,
        work_h,
    )
    .unwrap_or(bounds);

    {
        let mut runtime = state.inner.lock().expect("state poisoned");
        runtime.collapse_restore_bounds = Some(restore_bounds);
        runtime.collapse_restore_snap_state = current_snap_state;
        runtime.current_snap_state = SnapState::None;
        runtime.is_snapped_hidden = false;
        runtime.snap_reveal_armed = true;
    }

    let mut new_x = restore_bounds.x;
    let mut new_y = restore_bounds.y;

    if new_x + panel_w as i32 > work_x + work_w as i32 - WINDOW_MARGIN {
        new_x = work_x + work_w as i32 - panel_w as i32 - WINDOW_MARGIN;
    }
    if new_x < work_x + WINDOW_MARGIN {
        new_x = work_x + WINDOW_MARGIN;
    }
    if new_y + panel_h as i32 > work_y + work_h as i32 - WINDOW_MARGIN {
        new_y = work_y + work_h as i32 - panel_h as i32 - WINDOW_MARGIN;
    }
    if new_y < work_y + WINDOW_MARGIN {
        new_y = work_y + WINDOW_MARGIN;
    }

    {
        let mut runtime = state.inner.lock().expect("state poisoned");
        runtime.expanded_anchor_position = Some((new_x, new_y));
    }

    set_window_bounds(
        &window,
        WindowBounds {
            x: new_x,
            y: new_y,
            width: panel_w,
            height: panel_h.max(bounds.height),
        },
    )?;

    emit_snap_state(&window, SnapState::None);

    Ok(())
}

#[tauri::command]
fn restore_collapsed_window(
    window: tauri::Window,
    state: tauri::State<Arc<SharedState>>,
) -> Result<(), String> {
    let current = read_window_bounds(&window)?;
    let collapsed_size = collapsed_size_for_window(&window)?;
    let (expanded_anchor, restore_bounds, restore_snap_state) = {
        let mut runtime = state.inner.lock().expect("state poisoned");
        let anchor = runtime.expanded_anchor_position.take();
        let restore = runtime.collapse_restore_bounds.take();
        let restore_snap_state = runtime.collapse_restore_snap_state;
        runtime.collapse_restore_snap_state = SnapState::None;
        runtime.current_snap_state = SnapState::None;
        runtime.is_snapped_hidden = false;
        runtime.snap_reveal_armed = true;
        (anchor, restore, restore_snap_state)
    };

    let moved_while_expanded = expanded_anchor
        .map(|(x, y)| current.x != x || current.y != y)
        .unwrap_or(false);

    let target = if moved_while_expanded {
        WindowBounds {
            x: current.x,
            y: current.y,
            width: collapsed_size,
            height: collapsed_size,
        }
    } else {
        let restore = restore_bounds.unwrap_or(current);
        WindowBounds {
            x: restore.x,
            y: restore.y,
            width: collapsed_size,
            height: collapsed_size,
        }
    };

    if !moved_while_expanded && restore_snap_state != SnapState::None {
        restore_hidden_snap_state(&window, state.inner(), target, restore_snap_state)?;
    } else {
        set_window_bounds(&window, target)?;
        emit_snap_state(&window, SnapState::None);
    }

    Ok(())
}

#[tauri::command]
fn drag_start(app: AppHandle, state: tauri::State<Arc<SharedState>>) -> Result<(), String> {
    let window = get_main_window(&app)?;
    let bounds = read_window_bounds_webview(&window)?;
    let (work_x, work_y, work_w, work_h) = get_work_area_webview(&window)?;
    let collapsed_size = collapsed_size_for_webview(&window)?;
    let (current_snap_state, need_start_snap_poller) = {
        let mut runtime = state.inner.lock().expect("state poisoned");
        let snap_state = runtime.current_snap_state;
        let start_snap_poller = !runtime.snap_poller_started;
        if start_snap_poller {
            runtime.snap_poller_started = true;
        }
        (snap_state, start_snap_poller)
    };

    let visible_bounds = visible_bounds_for_snap(
        bounds,
        current_snap_state,
        collapsed_size,
        collapsed_size,
        work_x,
        work_y,
        work_w,
        work_h,
    )
    .unwrap_or(bounds);

    if current_snap_state != SnapState::None {
        set_window_bounds_webview(&window, visible_bounds)?;
    }

    let mouse = app
        .cursor_position()
        .map_err(|err| format!("failed to query cursor: {err}"))?;

    let mut runtime = state.inner.lock().expect("state poisoned");
    runtime.is_snapped_hidden = false;
    runtime.snap_reveal_armed = true;
    runtime.drag_snap_state = Some(current_snap_state);
    runtime.drag_tracking = Some(DragTracking {
        start_mouse_x: mouse.x,
        start_mouse_y: mouse.y,
        start_win_x: visible_bounds.x,
        start_win_y: visible_bounds.y,
    });
    runtime.drag_has_moved = false;

    if need_start_snap_poller {
        start_snap_poller(app.clone(), state.inner().clone());
    }

    if !runtime.drag_poller_started {
        runtime.drag_poller_started = true;
        drop(runtime);
        start_drag_poller(app, state.inner().clone());
    } else {
        drop(runtime);
    }

    emit_snap_state(&window, SnapState::None);

    Ok(())
}

#[tauri::command]
fn drag_end(
    app: AppHandle,
    state: tauri::State<Arc<SharedState>>,
) -> Result<serde_json::Value, String> {
    let (moved, drag_snap_state) = {
        let mut runtime = state.inner.lock().expect("state poisoned");
        runtime.drag_tracking = None;
        let moved = runtime.drag_has_moved;
        runtime.drag_has_moved = false;
        let snap_state = runtime.drag_snap_state.take();
        if !moved {
            runtime.current_snap_state = snap_state.unwrap_or(SnapState::None);
            runtime.is_snapped_hidden = runtime.current_snap_state != SnapState::None;
            runtime.snap_reveal_armed = runtime.current_snap_state == SnapState::None;
        }
        (moved, snap_state)
    };

    if moved {
        if let Ok(window) = get_main_window(&app) {
            let _ = snap_window_to_edge(&window, state.inner());
        } else if drag_snap_state.is_some() {
            // Preserve the snap state for the click-to-expand path.
        }
    }

    Ok(serde_json::json!({ "moved": moved }))
}

fn main() {
    let shared_state = Arc::new(SharedState::default());

    tauri::Builder::default()
        .manage(shared_state)
        .plugin(
            tauri_plugin_global_shortcut::Builder::new()
                .with_handler(|app, _shortcut, event| {
                    if event.state() == tauri_plugin_global_shortcut::ShortcutState::Pressed {
                        handle_global_shortcut(app);
                    }
                })
                .build(),
        )
        .invoke_handler(tauri::generate_handler![
            search_files,
            open_file,
            show_in_folder,
            copy_item,
            delete_item,
            copy_text,
            get_auto_start_enabled,
            set_auto_start_enabled,
            set_window_size,
            set_window_position,
            get_window_bounds,
            get_screen_bounds,
            ensure_safe_position,
            restore_collapsed_window,
            drag_start,
            drag_end,
        ])
        .setup(|app| {
            use tauri_plugin_global_shortcut::{GlobalShortcutExt, Shortcut};
            use std::str::FromStr;
            if let Ok(shortcut) = Shortcut::from_str("Alt+Space") {
                let _ = app.global_shortcut().register(shortcut);
            }

            let autostart_enabled = get_auto_start_enabled().unwrap_or(false);
            let toggle_item = MenuItemBuilder::with_id("toggle", "显示 / 隐藏").build(app)?;
            let autostart_item = CheckMenuItem::with_id(
                app,
                "tray_autostart",
                "开机自启",
                true,
                autostart_enabled,
                None::<&str>,
            )?;
            let settings_menu = Submenu::with_id(app, "settings", "设置", true)?;
            settings_menu.append(&autostart_item)?;
            let quit_item = MenuItemBuilder::with_id("quit", "退出").build(app)?;
            let menu = MenuBuilder::new(app)
                .items(&[&toggle_item, &settings_menu, &quit_item])
                .build()?;

            let default_icon = app
                .default_window_icon()
                .cloned()
                .expect("default window icon should exist");

            TrayIconBuilder::with_id("filefloat-tray")
                .icon(default_icon)
                .menu(&menu)
                .show_menu_on_left_click(false)
                .on_menu_event(move |app, event| match event.id().as_ref() {
                    "toggle" => {
                        let _ = toggle_window(app);
                    }
                    "tray_autostart" => {
                        let current_enabled = get_auto_start_enabled().unwrap_or(false);
                        let next_enabled = !current_enabled;
                        if set_auto_start_enabled(next_enabled).is_ok() {
                            let _ = autostart_item.set_checked(next_enabled);
                        } else {
                            let _ = autostart_item.set_checked(current_enabled);
                        }
                    }
                    "quit" => {
                        app.exit(0);
                    }
                    _ => {}
                })
                .build(app)?;

            if let Some(window) = app.get_webview_window("main") {
                if let (Ok(bounds), Ok(collapsed_size)) = (
                    read_window_bounds_webview(&window),
                    collapsed_size_for_webview(&window),
                ) {
                    let _ = set_window_bounds_webview(
                        &window,
                        WindowBounds {
                            x: bounds.x,
                            y: bounds.y,
                            width: collapsed_size,
                            height: collapsed_size,
                        },
                    );
                }
                emit_snap_state(&window, SnapState::None);
            }

            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running FileFloat");
}
