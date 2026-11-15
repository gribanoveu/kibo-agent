//! The Catalog page's features (`docs/33-features.md`): what there is, and
//! turning one on or off. The window reads the list again on [`FEATURES_EVENT`]
//! — the sidebar shows a feature's page only while it is on.

use std::sync::Arc;

use tauri::{AppHandle, Emitter, Manager, Runtime};

use crate::domain::features::Feature;
use crate::infra::mongo_client::MongoClients;
use crate::services::features::{self, FeatureView};
use crate::services::dictation::Dictation;
use crate::services::meeting_notes::MeetingNotes;

/// A feature was turned on or off.
pub const FEATURES_EVENT: &str = "features:changed";

#[tauri::command]
pub fn features_list() -> Vec<FeatureView> {
    features::list()
}

/// Takes effect from the next turn: a running one keeps what it started with.
#[tauri::command]
pub fn features_set<R: Runtime>(app: AppHandle<R>, id: String, on: bool) -> Result<Vec<FeatureView>, String> {
    features::set(&id, on).map_err(|e| e.to_string())?;
    // Off, its clients go: each keeps a pool and a heartbeat to the user's servers. Off the main
    // thread: the lock waits on a client being made, which can be an SRV lookup.
    if !on && id == Feature::Mongodb.manifest().id {
        let mongo = app.state::<Arc<MongoClients>>().inner().clone();
        tauri::async_runtime::spawn_blocking(move || mongo.forget_all());
    }
    // A kill and a wait for the recognition process: off the main thread.
    let notes = app.state::<Arc<MeetingNotes>>().inner().clone();
    let dictation = app.state::<Arc<Dictation>>().inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        notes.feature_set(&id, on);
        dictation.feature_set(&id, on);
    });
    let _ = app.emit(FEATURES_EVENT, ());
    Ok(features::list())
}

/// Whether the model gets the feature's tools, from the next turn. Its page
/// and clients stay: the window still uses them.
#[tauri::command]
pub fn features_set_tools<R: Runtime>(app: AppHandle<R>, id: String, on: bool) -> Result<Vec<FeatureView>, String> {
    features::set_tools(&id, on).map_err(|e| e.to_string())?;
    let _ = app.emit(FEATURES_EVENT, ());
    Ok(features::list())
}

/// Whether Fix also selects the corrected text's dominant system keyboard layout.
#[tauri::command]
pub fn features_set_layout_switch<R: Runtime>(app: AppHandle<R>, on: bool) -> Result<Vec<FeatureView>, String> {
    features::set_layout_switch(on).map_err(|e| e.to_string())?;
    let _ = app.emit(FEATURES_EVENT, ());
    Ok(features::list())
}

/// The feature's data deleted, and said on the channel: what the window
/// drew from it — the painted scenes behind it — is read again.
#[tauri::command]
pub fn features_delete_data<R: Runtime>(app: AppHandle<R>, id: String) -> Result<Vec<FeatureView>, String> {
    features::delete_data(&id)?;
    // Its clients were made from the connections just deleted; off the main thread, as in `features_set`.
    if id == Feature::Mongodb.manifest().id {
        let mongo = app.state::<Arc<MongoClients>>().inner().clone();
        tauri::async_runtime::spawn_blocking(move || mongo.forget_all());
    }
    let _ = app.emit(FEATURES_EVENT, ());
    Ok(features::list())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Deleting the painted scenes says so, or the window keeps drawing them.
    #[test]
    fn deleting_a_features_data_is_said_on_the_channel() {
        crate::testing::with_app_dir("features-delete-said", || {
            use tauri::Listener;
            let app = tauri::test::mock_app();
            let heard = Arc::new(std::sync::atomic::AtomicUsize::new(0));
            let count = Arc::clone(&heard);
            app.handle().listen(FEATURES_EVENT, move |_| {
                count.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            });
            features_delete_data(app.handle().clone(), Feature::Scenes.manifest().id.to_string()).unwrap();
            assert_eq!(heard.load(std::sync::atomic::Ordering::SeqCst), 1);
        });
    }

    /// `src/lib/features.ts` listens on this name.
    #[test]
    fn the_channel_name_is_pinned() {
        assert_eq!(FEATURES_EVENT, "features:changed");
    }
}
