fn main() {
    tauri_build::try_build(tauri_build::Attributes::new().app_manifest(
        tauri_build::AppManifest::new().commands(&["shell_status", "hide_mixer", "quit_ui"]),
    ))
    .expect("failed to build desktop shell resources");
}
