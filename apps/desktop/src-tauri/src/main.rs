#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

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

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ShellStatus {
    schema_version: u8,
    connected: bool,
    tray_created: bool,
    platform: &'static str,
}

#[tauri::command]
fn shell_status(state: State<'_, ShellState>) -> ShellStatus {
    ShellStatus {
        schema_version: 1,
        connected: false,
        tray_created: state.tray_created,
        platform: std::env::consts::OS,
    }
}

#[tauri::command]
fn hide_mixer(window: tauri::WebviewWindow) -> Result<(), String> {
    window.hide().map_err(|error| error.to_string())
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
    let status = MenuItem::with_id(app, "status", "Runtime: not connected", false, None::<&str>)?;
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
        .tooltip("Mixyard — runtime not connected")
        .menu(&menu)
        // Linux cannot deliver tray mouse events. The context menu is the baseline
        // on every host; richer anchored panels require separate native proof.
        .show_menu_on_left_click(true)
        .on_menu_event(|app, event| {
            let result = match event.id.as_ref() {
                "open" => open_mixer(app),
                "close" => app
                    .get_webview_window("main")
                    .map_or(Ok(()), |window| window.hide()),
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
                if let Err(error) = window.hide() {
                    eprintln!("Mixyard could not hide window: {error}");
                }
            }
        })
        .invoke_handler(tauri::generate_handler![shell_status, hide_mixer, quit_ui])
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
