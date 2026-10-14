//! The Catalog page's features (`docs/33-features.md`): what there is, and
//! turning one on or off. The window reads the list again on [`FEATURES_EVENT`]
//! — the sidebar shows a feature's page only while it is on.

use tauri::{AppHandle, Emitter, Runtime};

use crate::services::features::{self, FeatureView};

/// A feature was turned on or off.
pub const FEATURES_EVENT: &str = "features:changed";

#[tauri::command]
pub fn features_list() -> Vec<FeatureView> {
    features::list()
}

/// Takes effect from the next turn: a running one keeps what it started with.
#[tauri::command]
pub fn features_set<R: Runtime>(app: AppHandle<R>, id: String, on: bool) -> Result<Vec<FeatureView>, String> {
    features::set(&id, on)?;
    let _ = app.emit(FEATURES_EVENT, ());
    Ok(features::list())
}

#[tauri::command]
pub fn features_delete_data(id: String) -> Result<Vec<FeatureView>, String> {
    features::delete_data(&id)?;
    Ok(features::list())
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
