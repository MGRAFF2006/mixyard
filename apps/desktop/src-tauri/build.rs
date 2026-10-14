fn main() {
    tauri_build::try_build(tauri_build::Attributes::new().app_manifest(
        tauri_build::AppManifest::new().commands(&[
            "inspect_snapshot",
            "shell_status",
            "connect_runtime",
            "disconnect_runtime",
            "hide_mixer",
            "quit_ui",
        ]),
    ))
    .expect("failed to build desktop shell resources");
}
