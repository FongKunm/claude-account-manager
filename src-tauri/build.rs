fn main() {
    // Embed one linker manifest for executables and the Rust test harness.
    // Disable the resource manifest below to avoid duplicate MANIFEST resources.
    if std::env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc") {
        println!("cargo:rustc-link-arg=/MANIFEST:EMBED");
        println!("cargo:rustc-link-arg=/MANIFESTDEPENDENCY:type='win32' name='Microsoft.Windows.Common-Controls' version='6.0.0.0' processorArchitecture='*' publicKeyToken='6595b64144ccf1df' language='*'");
    }
    tauri_build::try_build(
        tauri_build::Attributes::new()
            .windows_attributes(tauri_build::WindowsAttributes::new_without_app_manifest())
            .app_manifest(tauri_build::AppManifest::new().commands(&[
                "get_state",
                "edit_card",
                "list_managed_windows",
                "focus_managed_window",
                "begin_normal_login",
                "get_normal_login",
                "cancel_normal_login",
                "add_account",
                "update_account",
                "replace_token",
                "delete_account",
                "switch_to",
                "use_normal",
                "preview_switch",
                "refresh_switch",
                "execute_switch",
                "get_switch_progress",
                "retry_restore",
                "check_usage",
                "check_normal_usage",
                "detect_exit",
                "check_network",
                "export_cpa",
                "open_data_folder",
                "open_help",
            ])),
    )
    .expect("Tauri build configuration failed");
}
