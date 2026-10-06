fn main() {
    // The Windows manifest — Common Controls v6, which the dialogs and the
    // webview need — is linked here rather than by tauri-build: its resource
    // reaches only the app's binary, and the test binary, without it, does
    // not start (`STATUS_ENTRYPOINT_NOT_FOUND`). A link argument reaches every
    // binary of the package, tests included.
    let windows = tauri_build::WindowsAttributes::new_without_app_manifest();
    // As `tauri_build::build()` ends on a failure.
    if let Err(error) = tauri_build::try_build(tauri_build::Attributes::new().windows_attributes(windows)) {
        println!("{error:#}");
        std::process::exit(1);
    }

    let target = (std::env::var("CARGO_CFG_TARGET_OS"), std::env::var("CARGO_CFG_TARGET_ENV"));
    if matches!(target, (Ok(os), Ok(env)) if os == "windows" && env == "msvc") {
        let manifest = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("windows-app-manifest.xml");
        println!("cargo:rerun-if-changed={}", manifest.display());
        println!("cargo:rustc-link-arg=/MANIFEST:EMBED");
        println!("cargo:rustc-link-arg=/MANIFESTINPUT:{}", manifest.display());
    }
}
