//! A picture pasted or dropped into the composer, sanitized before the chat
//! ever holds it.

use std::path::PathBuf;

use tauri::ipc::{InvokeBody, Request};

use crate::domain::image::ImagePart;
use crate::services::image_sanitize;

/// The picture's bytes arrive as the raw request body — a `Uint8Array`, not a
/// JSON array of ten million numbers. Off the IPC loop: decoding and encoding
/// a photo takes a moment.
#[tauri::command]
pub async fn image_prepare(request: Request<'_>) -> Result<ImagePart, String> {
    let InvokeBody::Raw(bytes) = request.body() else {
        return Err("expected the image's bytes as the request body".to_string());
    };
    let bytes = bytes.clone();
    tauri::async_runtime::spawn_blocking(move || image_sanitize::sanitize(&bytes))
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| e.to_string())
}

/// A file dropped on the window, by the path the drop gave. Any path: the
/// answer is a picture encoded from the file's pixels or an error, never the
/// file itself — see `image_sanitize::sanitize_file`.
#[tauri::command]
pub async fn image_prepare_file(path: String) -> Result<ImagePart, String> {
    let path = PathBuf::from(path);
    tauri::async_runtime::spawn_blocking(move || image_sanitize::sanitize_file(&path))
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| e.to_string())
}
