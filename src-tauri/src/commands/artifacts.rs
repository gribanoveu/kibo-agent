//! The Artifacts page (`docs/38-artifacts.md`): the meetings and prototypes as
//! folders, read through `services::artifacts`. Nothing here is new state, so
//! there is no channel of its own: the page re-reads on `meeting:event` and
//! `chat:turn-event`, which already say when either changes.

use std::path::Path;

use tauri::AppHandle;

use crate::domain::artifacts::{ArtifactFile, ArtifactKind, Collection};
use crate::infra::app_dir;
use crate::services::{artifacts, features};

use super::blocking;

/// Every collection, newest first; meetings while their feature is on.
#[tauri::command]
pub async fn artifact_list() -> Result<Vec<Collection>, String> {
    blocking(|| artifacts::list(&features::enabled())).await
}

/// Bytes still free on the disk the artifacts are kept on.
#[tauri::command]
pub async fn artifact_free_space() -> Result<u64, String> {
    blocking(app_dir::free_space).await
}

#[tauri::command]
pub async fn artifact_files(kind: ArtifactKind, id: String) -> Result<Vec<ArtifactFile>, String> {
    blocking(move || artifacts::files(kind, &id)).await
}

/// A collection's file as text, for the page to show.
#[tauri::command]
pub async fn artifact_read(kind: ArtifactKind, id: String, path: String) -> Result<String, String> {
    blocking(move || artifacts::read(kind, &id, &path)).await
}

/// A collection's file written at `dest`, from the system's save dialog; a
/// prototype's page as one HTML file.
#[tauri::command]
pub async fn artifact_save_as(app: AppHandle, kind: ArtifactKind, id: String, path: String, dest: String) -> Result<(), String> {
    let roots = super::prototype::roots(&app);
    blocking(move || artifacts::save_as(&roots, kind, &id, &path, Path::new(&dest))).await
}

/// Collection `id` called `title`; a prototype's refused while its chat works.
#[tauri::command]
pub async fn artifact_rename(kind: ArtifactKind, id: String, title: String) -> Result<(), String> {
    blocking(move || artifacts::rename(kind, &id, &title)).await
}

/// Shows the folder a collection is kept in, in Finder or Explorer.
#[tauri::command]
pub async fn artifact_reveal(kind: ArtifactKind, id: String) -> Result<(), String> {
    blocking(move || -> Result<(), String> {
        let folder = artifacts::folder(kind, &id).map_err(|e| e.to_string())?;
        tauri_plugin_opener::reveal_item_in_dir(folder).map_err(|e| e.to_string())
    })
    .await
}
