//! The HTTP page (`docs/45-http-client.md`). Its collections live in the window
//! until HC-2 keeps them here, so the one command writes the file the page built.

use std::path::Path;

use super::blocking;

/// A Postman collection the page built, written at `dest` from the system's save dialog.
// ponytail: the page's own text, checked to be a JSON file and nothing else — HC-2
// writes the stored collection by its id, and this goes.
#[tauri::command]
pub async fn http_export(dest: String, text: String) -> Result<(), String> {
    blocking(move || write_json(Path::new(&dest), &text)).await
}

fn write_json(dest: &Path, text: &str) -> Result<(), String> {
    if dest.extension().and_then(|e| e.to_str()) != Some("json") {
        return Err(format!("{} is not a .json file", dest.display()));
    }
    serde_json::from_str::<serde_json::Value>(text).map_err(|e| format!("not JSON: {e}"))?;
    std::fs::write(dest, text).map_err(|e| format!("{}: {e}", dest.display()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::temp_dir;

    #[test]
    fn writes_json_to_a_json_file_and_nothing_else() {
        let dir = temp_dir("http-export");
        let file = dir.join("orders.postman_collection.json");
        write_json(&file, "{\"info\": {}}").unwrap();
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "{\"info\": {}}");

        let script = dir.join("run.sh");
        assert!(write_json(&script, "{}").is_err());
        assert!(!script.exists());
        assert!(write_json(&file, "echo hi").is_err());
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "{\"info\": {}}");
    }
}
