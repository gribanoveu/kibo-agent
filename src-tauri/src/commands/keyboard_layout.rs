//! The optional system-layout selection after an explicit wrong-layout Fix.

use crate::domain::keyboard_layout::KeyboardLayout;
use crate::infra::keyboard_layout;

/// Synchronous on purpose: Windows applies the layout to the calling (main) thread.
#[tauri::command]
pub fn keyboard_layout_set(layout: KeyboardLayout) -> Result<(), String> {
    keyboard_layout::set(layout).map_err(|e| e.to_string())
}
