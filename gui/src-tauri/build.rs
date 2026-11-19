fn main() {
    // Only these commands get permissions, and the `main` capability lists
    // each one: by default every registered command is open to every window.
    let manifest = tauri_build::AppManifest::new().commands(&[
        "init",
        "domains",
        "create_inbox",
        "select_inbox",
        "use_inbox",
        "delete_inbox",
        "open_message",
        "delete_message",
        "copy_address",
        "copy_from_message",
        "open_link",
        "save_attachment",
        "settings",
        "save_settings",
        "reveal_config",
        "prepare_attachment",
        "drag_attachment",
        "rate_limit",
        "set_window_theme",
        "probe_finish",
    ]);
    tauri_build::try_build(tauri_build::Attributes::new().app_manifest(manifest))
        .expect("tauri build script");
}
