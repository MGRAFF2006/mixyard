#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod connection;
mod inspection;

use connection::{Connection, ConnectionStatus};
use serde::Serialize;
use tauri::{
    image::Image,
    menu::{Menu, MenuItem, PredefinedMenuItem},
    tray::TrayIconBuilder,
    Manager, State, WindowEvent,
};

struct ShellState {
    tray_created: bool,
}

#[tauri::command]
async fn inspect_snapshot(request: inspection::SnapshotRequest) -> Result<String, String> {
    let admission = inspection::acquire()?;
    tauri::async_runtime::spawn_blocking(move || {
        let _admission = admission;
        inspection::inspect(request)
    })
    .await
    .map_err(|error| format!("Inspection worker failed: {error}"))?
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ShellStatus {
    schema_version: u8,
    connected: bool,
    runtime: ConnectionStatus,
    tray_created: bool,
    platform: &'static str,
}

#[tauri::command]
fn shell_status(state: State<'_, ShellState>, connection: State<'_, Connection>) -> ShellStatus {
    let runtime = connection.status();
    ShellStatus {
        schema_version: 1,
        connected: runtime.phase == "connected",
        runtime,
        tray_created: state.tray_created,
        platform: std::env::consts::OS,
    }
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
    let menu = Menu::with_items(app, &[&status, &open, &close, &separator, &stop, &quit])?;
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
                "open" => open_mixer(app),
                "close" => close_mixer(app),
                "quit-ui" => {
                    app.exit(0);
                    Ok(())
                }
                _ => Ok(()),
            };
            if let Err(error) = result {
                eprintln!("Mixyard window action failed: {error}");
            }
        })
        .build(app)?;
    Ok(())
}

fn main() {
    let app = tauri::Builder::default()
        .setup(|app| {
            app.manage(Connection::new()?);
            let tray_created = match create_tray(app) {
                Ok(()) => true,
                Err(error) => {
                    // Tray absence must not prevent using the app or the headless CLI.
                    eprintln!("Mixyard tray unavailable: {error}");
                    false
                }
            };
            app.manage(ShellState { tray_created });
            Ok(())
        })
        .on_window_event(|window, event| {
            if let WindowEvent::CloseRequested { api, .. } = event {
                api.prevent_close();
                if let Err(error) = close_mixer(window.app_handle()) {
                    eprintln!("Mixyard could not close mixer: {error}");
                }
            }
        })
        .invoke_handler(tauri::generate_handler![
            inspect_snapshot,
            shell_status,
            connect_runtime,
            disconnect_runtime,
            hide_mixer,
            quit_ui
        ])
        .build(tauri::generate_context!())
        .expect("failed to initialize Mixyard desktop UI");

    app.run(|app, event| {
        // The macOS dock can restore the hidden window without any tray host.
        #[cfg(target_os = "macos")]
        if let tauri::RunEvent::Reopen { .. } = event {
            if let Err(error) = open_mixer(app) {
                eprintln!("Mixyard could not reopen window: {error}");
            }
        }
        #[cfg(not(target_os = "macos"))]
        let _ = (app, event);
    });
}
