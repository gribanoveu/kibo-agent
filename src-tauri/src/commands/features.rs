//! The Catalog page's features (`docs/33-features.md`): what there is, and
//! turning one on or off. The window reads the list again on [`FEATURES_EVENT`]
//! — the sidebar shows a feature's page only while it is on.

use std::sync::Arc;

use tauri::{AppHandle, Emitter, Manager, Runtime};

use crate::domain::features::Feature;
use crate::infra::kafka_client::KafkaClients;
use crate::infra::mongo_client::MongoClients;
use crate::services::features::{self, FeatureView};
use crate::services::kafka::KafkaLive;
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
pub async fn features_set<R: Runtime>(app: AppHandle<R>, id: String, on: bool) -> Result<Vec<FeatureView>, String> {
    // The manifest file, its clients' pools and the recognition process are all
    // waits: one thread for the lot, and the window keeps drawing.
    let mongo = app.state::<Arc<MongoClients>>().inner().clone();
    let kafka = app.state::<Arc<KafkaClients>>().inner().clone();
    let live = app.state::<Arc<KafkaLive>>().inner().clone();
    let notes = app.state::<Arc<MeetingNotes>>().inner().clone();
    let dictation = app.state::<Arc<Dictation>>().inner().clone();
    let applied = id.clone();
    let views = super::blocking(move || {
        features::set(&applied, on)?;
        // Off, its clients go: each keeps a pool and a heartbeat to the user's
        // servers, and Kafka's reads keep a consumer on a cluster nobody is
        // watching any more.
        if !on && applied == Feature::Mongodb.manifest().id {
            mongo.forget_all();
        }
        if !on && applied == Feature::Kafka.manifest().id {
            kafka.forget_all();
            live.stop_all();
        }
        // A kill and a wait for the recognition process.
        notes.feature_set(&applied, on);
        dictation.feature_set(&applied, on);
        Ok::<Vec<FeatureView>, crate::domain::features::FeatureError>(features::list())
    })
    .await?;
    let _ = app.emit(FEATURES_EVENT, ());
    Ok(views)
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

#[tauri::command]
pub async fn features_delete_data(
    id: String,
    mongo: tauri::State<'_, Arc<MongoClients>>,
    kafka: tauri::State<'_, Arc<KafkaClients>>,
    live: tauri::State<'_, Arc<KafkaLive>>,
) -> Result<Vec<FeatureView>, String> {
    // A file removed, and the clients made from what was in it: off the main
    // thread, as in `features_set`.
    let mongo = mongo.inner().clone();
    let kafka = kafka.inner().clone();
    let live = live.inner().clone();
    let removed = id.clone();
    let views = super::blocking(move || {
        features::delete_data(&removed)?;
        if removed == Feature::Mongodb.manifest().id {
            mongo.forget_all();
        }
        if removed == Feature::Kafka.manifest().id {
            kafka.forget_all();
            // Its reads hold a consumer and a password for a cluster whose
            // connections were just deleted.
            live.stop_all();
        }
        Ok::<Vec<FeatureView>, String>(features::list())
    })
    .await?;
    Ok(views)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `src/lib/features.ts` listens on this name.
    #[test]
    fn the_channel_name_is_pinned() {
        assert_eq!(FEATURES_EVENT, "features:changed");
    }
}
