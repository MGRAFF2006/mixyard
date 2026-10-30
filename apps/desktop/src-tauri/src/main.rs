#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod connection;
mod control_http;
#[cfg(target_os = "macos")]
mod macos_termination;
mod paired;

use connection::{Connection, ConnectionStatus};
use serde::{Deserialize, Serialize};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex,
};
use tauri::{
    image::Image,
    menu::{Menu, MenuItem, PredefinedMenuItem},
    tray::TrayIconBuilder,
    Manager, State, WindowEvent,
};

struct ShellState {
    tray_created: bool,
    compact: AtomicBool,
    ui_action: Mutex<Option<&'static str>>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ConsoleRequest {
    path: String,
    method: String,
    token: Option<String>,
    body: Option<String>,
}
#[derive(Serialize)]
struct ConsoleReply {
    status: u16,
    body: String,
}

#[tauri::command]
async fn console_request(
    request: ConsoleRequest,
    console: State<'_, Arc<control_http::LocalConsole>>,
) -> Result<ConsoleReply, String> {
    let console = console.inner().clone();
    let admission = console.acquire()?;
    tauri::async_runtime::spawn_blocking(move || {
        let _admission = admission;
        console
            .request(control_http::Request {
                path: request.path,
                method: request.method,
                token: request.token,
                body: request.body,
            })
            .map(|reply| ConsoleReply {
                status: reply.status,
                body: reply.body,
            })
    })
    .await
    .map_err(|_| "Console worker failed; Refresh and review before another write".to_string())?
}

#[tauri::command]
fn set_compact(window: tauri::WebviewWindow, compact: bool) -> Result<(), String> {
    resize_mixer(window.app_handle(), compact).map_err(|e| e.to_string())
}

fn resize_mixer(app: &tauri::AppHandle, compact: bool) -> tauri::Result<()> {
    let (width, height) = if compact {
        (480.0, 720.0)
    } else {
        (1120.0, 850.0)
    };
    if let Some(window) = app.get_webview_window("main") {
        window.unmaximize()?;
        window.set_size(tauri::LogicalSize::new(width, height))?;
        app.state::<ShellState>()
            .compact
            .store(compact, Ordering::SeqCst);
    }
    Ok(())
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SessionSummary {
    device_id: Option<String>,
    authority_id: Option<String>,
    revision: Option<u64>,
    display_name: String,
    connected: bool,
    epoch: u64,
}

#[tauri::command]
fn update_tray_context(app: tauri::AppHandle, summary: SessionSummary) -> Result<(), String> {
    // Display only. No tray action can write a profile, arm automation or mute audio.
    for value in [&summary.device_id, &summary.authority_id] {
        if value
            .as_ref()
            .is_some_and(|v| v.len() > 128 || v.chars().any(char::is_control))
        {
            return Err("Invalid session identity label".into());
        }
    }
    if summary.display_name.len() > 200 || summary.display_name.chars().any(char::is_control) {
        return Err("Invalid target display name".into());
    }
    if let Some(labels) = app.try_state::<TrayLabels>() {
        let target = match (&summary.device_id, &summary.authority_id) {
            (Some(device), Some(_)) => format!("Target: {} ({device})", summary.display_name),
            _ => "Target: local service; identity unavailable".into(),
        };
        labels.target.set_text(target).map_err(|e| e.to_string())?;
        labels
            .status
            .set_text(format!(
                "{} · stored intent · revision {} · session {}",
                if summary.connected {
                    "Connected"
                } else {
                    "Disconnected"
                },
                summary
                    .revision
                    .map(|v| v.to_string())
                    .unwrap_or_else(|| "unknown".into()),
                summary.epoch
            ))
            .map_err(|e| e.to_string())?;
    }
    Ok(())
}
struct TrayLabels {
    target: MenuItem<tauri::Wry>,
    status: MenuItem<tauri::Wry>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ShellStatus {
    schema_version: u8,
    connected: bool,
    runtime: ConnectionStatus,
    tray_created: bool,
    platform: &'static str,
    compact: bool,
}

#[tauri::command]
fn shell_status(state: State<'_, ShellState>, connection: State<'_, Connection>) -> ShellStatus {
    let runtime = connection.status();
    ShellStatus {
        schema_version: 1,
        connected: runtime.phase == "connected",
        runtime,
        tray_created: state.tray_created,
        compact: state.compact.load(Ordering::SeqCst),
        platform: std::env::consts::OS,
    }
}

#[tauri::command]
fn take_ui_action(state: State<'_, ShellState>) -> Option<&'static str> {
    state.ui_action.lock().expect("shell state lock").take()
}

#[tauri::command]
fn connect_runtime(
    instance: String,
    connection: State<'_, Connection>,
) -> Result<ConnectionStatus, String> {
    connection.connect(instance)
}

#[tauri::command]
fn disconnect_runtime(connection: State<'_, Connection>) -> ConnectionStatus {
    connection.disconnect()
}

#[tauri::command]
fn hide_mixer(window: tauri::WebviewWindow) -> Result<(), String> {
    close_mixer(window.app_handle()).map_err(|error| error.to_string())
}

#[tauri::command]
fn quit_ui(app: tauri::AppHandle) {
    // The UI never starts, owns, or stops an audio runtime.
    app.exit(0);
}

fn open_mixer(app: &tauri::AppHandle) -> tauri::Result<()> {
    if let Some(window) = app.get_webview_window("main") {
        window.unminimize()?;
        window.show()?;
        window.set_focus()?;
    }
    Ok(())
}

fn request_ui_quit(app: &tauri::AppHandle) {
    *app.state::<ShellState>()
        .ui_action
        .lock()
        .expect("shell state lock") = Some("quit");
    if let Err(error) = open_mixer(app) {
        eprintln!("Mixyard could not show its close guard: {error}");
    }
}

fn close_mixer(app: &tauri::AppHandle) -> tauri::Result<()> {
    app.state::<Connection>().disconnect();
    // Construction does not prove the host displays the tray. Only macOS has
    // an independent reopen path (Dock); never strand a hidden UI elsewhere.
    #[cfg(target_os = "macos")]
    if let Some(window) = app.get_webview_window("main") {
        window.hide()?;
    }
    #[cfg(not(target_os = "macos"))]
    app.exit(0);
    Ok(())
}

fn tray_image() -> Image<'static> {
    let mut rgba = vec![0; 32 * 32 * 4];
    for y in 0..32 {
        for x in 0..32 {
            let bar = [(6, 12, 20), (12, 5, 27), (18, 9, 23), (24, 14, 18)]
                .iter()
                .any(|&(left, top, bottom)| x >= left && x < left + 3 && y >= top && y < bottom);
            let offset = (y * 32 + x) * 4;
            if bar {
                rgba[offset..offset + 4].copy_from_slice(&[24, 24, 24, 255]);
            } else if !cfg!(target_os = "macos") {
                rgba[offset..offset + 4].copy_from_slice(&[240, 240, 240, 255]);
            }
        }
    }
    Image::new_owned(rgba, 32, 32)
}

fn create_tray(app: &tauri::App) -> tauri::Result<()> {
    let status = MenuItem::with_id(
        app,
        "status",
        "Runtime: see mixer (read-only)",
        false,
        None::<&str>,
    )?;
    let target = MenuItem::with_id(
        app,
        "target",
        "Target: no bound device session",
        false,
        None::<&str>,
    )?;
    let profiles = MenuItem::with_id(
        app,
        "profiles",
        "Profiles — stored intent…",
        true,
        None::<&str>,
    )?;
    let compact = MenuItem::with_id(app, "compact", "Open compact controls", true, None::<&str>)?;
    let mute = MenuItem::with_id(
        app,
        "mute",
        "Audio mute unavailable (no live backend)",
        false,
        None::<&str>,
    )?;
    let open = MenuItem::with_id(app, "open", "Open Mixer", true, None::<&str>)?;
    let close = MenuItem::with_id(app, "close", "Close mixer window", true, None::<&str>)?;
    let stop = MenuItem::with_id(
        app,
        "stop-runtime",
        "Stop runtime (unavailable)",
        false,
        None::<&str>,
    )?;
    let quit = MenuItem::with_id(
        app,
        "quit-ui",
        "Quit UI (leave runtime alone)",
        true,
        None::<&str>,
    )?;
    let separator = PredefinedMenuItem::separator(app)?;
    let menu = Menu::with_items(
        app,
        &[
            &target, &status, &open, &compact, &profiles, &mute, &close, &separator, &stop, &quit,
        ],
    )?;
    TrayIconBuilder::with_id("mixyard-tray")
        .icon(tray_image())
        .icon_as_template(cfg!(target_os = "macos"))
        .tooltip("Mixyard — read-only runtime connection")
        .menu(&menu)
        // Linux cannot deliver tray mouse events. The context menu is the baseline
        // on every host; richer anchored panels require separate native proof.
        .show_menu_on_left_click(true)
        .on_menu_event(|app, event| {
            let result = match event.id.as_ref() {
                "open" => resize_mixer(app, false).and_then(|_| open_mixer(app)),
                "compact" => resize_mixer(app, true).and_then(|_| open_mixer(app)),
                "profiles" => {
                    *app.state::<ShellState>()
                        .ui_action
                        .lock()
                        .expect("shell state lock") = Some("Profiles");
                    open_mixer(app)
                }
                "close" => {
                    *app.state::<ShellState>()
                        .ui_action
                        .lock()
                        .expect("shell state lock") = Some("close");
                    open_mixer(app)
                }
                "quit-ui" => {
                    *app.state::<ShellState>()
                        .ui_action
                        .lock()
                        .expect("shell state lock") = Some("quit");
                    open_mixer(app)
                }
                _ => Ok(()),
            };
            if let Err(error) = result {
                eprintln!("Mixyard window action failed: {error}");
            }
        })
        .build(app)?;
    app.manage(TrayLabels { target, status });
    Ok(())
}

fn main() {
    let app = tauri::Builder::default()
        .setup(|app| {
            app.manage(Connection::new()?);
            app.manage(Arc::new(mixyard_desktop_control::Paired::new(
                mixyard_desktop_control::PlatformVault::open(
                    app.path()
                        .app_local_data_dir()
                        .map_err(|_| "Desktop app data directory unavailable".to_string()),
                ),
            )));
            let port = std::env::var("MIXYARD_WEB_PORT")
                .unwrap_or_else(|_| "8384".into())
                .parse::<u16>()?;
            app.manage(Arc::new(
                control_http::LocalConsole::new(port).map_err(std::io::Error::other)?,
            ));
            let tray_created = if std::env::var_os("MIXYARD_DESKTOP_NO_TRAY").is_some() {
                false
            } else {
                match create_tray(app) {
                    Ok(()) => true,
                    Err(error) => {
                        // Tray absence must not prevent using the app or the headless CLI.
                        eprintln!("Mixyard tray unavailable: {error}");
                        false
                    }
                }
            };
            app.manage(ShellState {
                tray_created,
                ui_action: Mutex::new(None),
                compact: AtomicBool::new(false),
            });
            #[cfg(target_os = "macos")]
            {
                let handle = app.handle().clone();
                macos_termination::install(move || request_ui_quit(&handle))
                    .map_err(std::io::Error::other)?;
            }
            Ok(())
        })
        .on_window_event(|window, event| {
            if let WindowEvent::CloseRequested { api, .. } = event {
                api.prevent_close();
                // The shared console must approve losing unsaved drafts before UI exit.
                *window
                    .app_handle()
                    .state::<ShellState>()
                    .ui_action
                    .lock()
                    .expect("shell state lock") = Some("close");
            }
        })
        .invoke_handler(tauri::generate_handler![
            console_request,
            paired::paired_list,
            paired::paired_pair,
            paired::paired_prepare_open,
            paired::paired_cancel_open,
            paired::paired_open,
            paired::paired_request,
            paired::paired_disconnect,
            paired::paired_forget,
            paired::paired_revoke,
            set_compact,
            update_tray_context,
            shell_status,
            take_ui_action,
            connect_runtime,
            disconnect_runtime,
            hide_mixer,
            quit_ui
        ])
        .build(tauri::generate_context!())
        .expect("failed to initialize Mixyard desktop UI");

    app.run(|app, event| match event {
        // User-requested Tauri exits use this guard. macOS AppKit terminate:
        // uses the native delegate seam installed above (not ExitRequested).
        tauri::RunEvent::ExitRequested {
            code: None, api, ..
        } => {
            api.prevent_exit();
            request_ui_quit(app);
        }
        #[cfg(target_os = "macos")]
        tauri::RunEvent::Reopen { .. } => {
            if let Err(error) = open_mixer(app) {
                eprintln!("Mixyard could not reopen window: {error}");
            }
        }
        _ => {}
    });
}
