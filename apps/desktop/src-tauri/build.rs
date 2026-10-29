fn main() {
    tauri_build::try_build(tauri_build::Attributes::new().app_manifest(
        tauri_build::AppManifest::new().commands(&[
            "console_request",
            "paired_list",
            "paired_pair",
            "paired_open",
            "paired_request",
            "paired_disconnect",
            "paired_forget",
            "paired_revoke",
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
