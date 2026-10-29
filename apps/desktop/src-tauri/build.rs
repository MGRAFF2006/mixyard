fn main() {
    tauri_build::try_build(tauri_build::Attributes::new().app_manifest(
        tauri_build::AppManifest::new().commands(&[
            "console_request",
            "set_compact",
            "update_tray_context",
            "shell_status",
            "take_ui_action",
            "connect_runtime",
            "disconnect_runtime",
            "hide_mixer",
            "quit_ui",
        ]),
    ))
    .expect("failed to build desktop shell resources");
}
