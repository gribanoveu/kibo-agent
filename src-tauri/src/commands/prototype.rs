//! The preview's protocol, `artifact://`, handed to the builder; what it
//! serves and with what rules is `infra::prototype_protocol`'s.

use tauri::{Builder, Manager, Runtime};

use crate::infra::{app_dir, prototype_protocol, prototype_store};

use super::blocking;

/// Shows a Prototype chat's working folder in Finder or Explorer.
#[tauri::command]
pub async fn prototype_reveal(chat_id: String) -> Result<(), String> {
    blocking(move || -> Result<(), String> {
        let folder = prototype_store::files_dir(&chat_id).map_err(|e| e.to_string())?;
        tauri_plugin_opener::reveal_item_in_dir(folder).map_err(|e| e.to_string())
    })
    .await
}

pub fn register<R: Runtime>(builder: Builder<R>) -> Builder<R> {
    builder.register_asynchronous_uri_scheme_protocol(prototype_protocol::SCHEME, |ctx, request, responder| {
        let app = ctx.app_handle().clone();
        let path = request.uri().path().to_string();
        // Off the webview's thread: a library is megabytes.
        tauri::async_runtime::spawn_blocking(move || {
            let roots = prototype_protocol::Roots {
                app_dir: app_dir::dir().ok(),
                libs: prototype_protocol::libs_dir(app.path().resource_dir().ok().as_deref()),
            };
            responder.respond(prototype_protocol::respond(&roots, &path));
        });
    })
}
