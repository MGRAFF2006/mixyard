fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("macos") {
        println!("cargo:rerun-if-changed=src/macos_termination.m");
        cc::Build::new()
            .file("src/macos_termination.m")
            .flag("-fobjc-arc")
            .compile("mixyard_macos_termination");
        println!("cargo:rustc-link-lib=framework=AppKit");
    }
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
