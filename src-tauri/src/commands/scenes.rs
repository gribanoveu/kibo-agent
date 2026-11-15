//! The scenes the model painted (`docs/55-frost-scenes.md`, decision 8), for
//! the Settings group that shows and deletes them. Each is one call to the
//! store with no rule of its own, so there is no service between.

use crate::domain::scene::SceneInfo;
use crate::infra::scene_store;

use super::blocking;

/// Every painted scene, newest first.
#[tauri::command]
pub async fn scenes_list() -> Result<Vec<SceneInfo>, String> {
    blocking(scene_store::list).await
}

/// Scene `id`'s body.
#[tauri::command]
pub async fn scene_read(id: String) -> Result<String, String> {
    blocking(move || scene_store::read(&id)).await
}

/// Scene `id` deleted.
#[tauri::command]
pub async fn scene_delete(id: String) -> Result<(), String> {
    blocking(move || scene_store::delete(&id)).await
}
