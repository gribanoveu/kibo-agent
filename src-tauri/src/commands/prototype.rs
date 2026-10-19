//! The preview's protocol, `artifact://`, handed to the builder; what it
//! serves and with what rules is `infra::prototype_protocol`'s.

use tauri::{AppHandle, Builder, Manager, Runtime};

use crate::infra::{app_dir, prototype_export, prototype_protocol, prototype_store};

use super::blocking;

/// Puts a Prototype chat's folder back to version `version`, as a new version
/// (`services::prototype_versions`).
#[tauri::command]
pub async fn prototype_restore(chat_id: String, version: u32) -> Result<crate::domain::prototype::Restored, String> {
    blocking(move || {
        let files = prototype_store::files_dir(&chat_id)?;
        crate::services::prototype_versions::restore(&files, version)
    })
    .await
}

/// Shows a Prototype chat's working folder in Finder or Explorer.
#[tauri::command]
pub async fn prototype_reveal(chat_id: String) -> Result<(), String> {
    blocking(move || -> Result<(), String> {
        let folder = prototype_store::files_dir(&chat_id).map_err(|e| e.to_string())?;
        tauri_plugin_opener::reveal_item_in_dir(folder).map_err(|e| e.to_string())
    })
    .await
}

/// Saves the page the panel shows — of the working folder, or of `version` —
/// as one HTML file at `path`, from the system's save dialog
/// (`infra::prototype_export`).
#[tauri::command]
pub async fn prototype_export(
    app: AppHandle,
    chat_id: String,
    version: Option<u32>,
    page: String,
    path: String,
) -> Result<(), String> {
    let roots = roots(&app);
    blocking(move || prototype_export::export(&roots, &chat_id, version, &page, std::path::Path::new(&path))).await
}

/// Where the protocol and "Save as" read from.
fn roots<R: Runtime>(app: &AppHandle<R>) -> prototype_protocol::Roots {
    prototype_protocol::Roots {
        app_dir: app_dir::dir().ok(),
        libs: prototype_protocol::libs_dir(app.path().resource_dir().ok().as_deref()),
    }
}

pub fn register<R: Runtime>(builder: Builder<R>) -> Builder<R> {
    builder.register_asynchronous_uri_scheme_protocol(prototype_protocol::SCHEME, |ctx, request, responder| {
        let app = ctx.app_handle().clone();
        let path = request.uri().path().to_string();
        // Off the webview's thread: a library is megabytes.
        tauri::async_runtime::spawn_blocking(move || {
            responder.respond(prototype_protocol::respond(&roots(&app), &path));
        });
    })
}
