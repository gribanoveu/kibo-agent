//! The features the Catalog page turns on and off (`docs/33-features.md`):
//! the choice in `settings.json`, what each one adds as its card says it, and
//! its data removed on request.

use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::domain::chat_role::ChatRole;
use crate::domain::conversation_mode::{self, ConversationMode};
use crate::domain::features::{Feature, Features};
use crate::domain::settings::AppSettings;
use crate::infra::{app_dir, settings_store};
use crate::services::ai_tools::tools::tool_definitions;

/// One feature as its Catalog card shows it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FeatureView {
    pub id: String,
    pub name: String,
    pub description: String,
    pub on: bool,
    /// What the model is given, as it is told.
    pub tools: Vec<FeatureTool>,
    /// Where the model is given them: a chat's role, a mode in a folder.
    pub places: Vec<String>,
    /// Its file, as a path under the home folder reads; `None` for a feature with none.
    pub data: Option<String>,
    pub has_data: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct FeatureTool {
    pub name: String,
    pub description: String,
}

/// What a turn reads once: an unreadable settings file is the defaults, as for
/// every setting a turn reads.
pub fn enabled() -> Features {
    Features::from_settings(&settings_store::load().unwrap_or_default().features)
}

pub fn list() -> Vec<FeatureView> {
    let settings = settings_store::load().unwrap_or_default();
    views(&settings, app_dir::dir().ok().as_deref())
}

pub fn set(id: &str, on: bool) -> Result<(), String> {
    let feature = Feature::from_id(id).ok_or_else(|| format!("no feature \"{id}\""))?;
    let mut settings = settings_store::load().map_err(|e| e.to_string())?;
    settings.features.insert(feature.manifest().id.to_string(), on);
    settings_store::save(&settings).map_err(|e| e.to_string())
}

/// Removes the feature's file. Only while it is off: the model could be
/// writing to it.
pub fn delete_data(id: &str) -> Result<(), String> {
    let feature = Feature::from_id(id).ok_or_else(|| format!("no feature \"{id}\""))?;
    if enabled().is_on(feature) {
        return Err(format!("turn {} off before deleting its data", feature.manifest().name));
    }
    let Some(path) = data_path(feature, app_dir::dir().ok().as_deref()) else {
        return Ok(());
    };
    match std::fs::remove_file(&path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(format!("could not delete {}: {e}", path.display())),
    }
}

fn data_path(feature: Feature, app_dir: Option<&Path>) -> Option<PathBuf> {
    Some(app_dir?.join(feature.manifest().data?))
}

fn views(settings: &AppSettings, app_dir: Option<&Path>) -> Vec<FeatureView> {
    let on = Features::from_settings(&settings.features);
    let definitions = tool_definitions();
    Feature::ALL
        .iter()
        .map(|&feature| {
            let manifest = feature.manifest();
            let tools = manifest
                .tools
                .iter()
                .filter_map(|tool| definitions.iter().find(|d| d.name == tool.wire_name()))
                .map(|d| FeatureTool { name: d.name.clone(), description: d.description.clone() })
                .collect();
            FeatureView {
                id: manifest.id.to_string(),
                name: manifest.name.to_string(),
                description: manifest.description.to_string(),
                on: on.is_on(feature),
                tools,
                places: places(manifest.tools),
                data: manifest.data.map(|file| format!("~/.kibo/{file}")),
                has_data: data_path(feature, app_dir).is_some_and(|path| path.exists()),
            }
        })
        .collect()
}

/// "Assistant chat", then the user's modes in a folder that list any of `tools`.
fn places(tools: &[crate::domain::tools::ToolName]) -> Vec<String> {
    let roles = ChatRole::ALL
        .iter()
        .filter(|role| tools.iter().any(|tool| role.tools().contains(tool)))
        .map(|role| format!("{} chat", role.name()));
    let modes = [(ConversationMode::Agent, "Agent"), (ConversationMode::Plan, "Plan"), (ConversationMode::Ask, "Ask")]
        .into_iter()
        .filter(|(mode, _)| tools.iter().any(|&tool| conversation_mode::offers(*mode, tool)))
        .map(|(_, name)| name.to_string());
    roles.chain(modes).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::temp_dir;

    #[test]
    fn the_board_card_says_what_it_adds_and_where() {
        let dir = temp_dir("features-views");
        let off = views(&AppSettings::default(), Some(&dir));
        let board = &off[0];
        assert_eq!((board.id.as_str(), board.on, board.has_data), ("board", false, false));
        assert_eq!(board.tools.iter().map(|t| t.name.as_str()).collect::<Vec<_>>(), ["board"]);
        assert!(board.tools[0].description.contains("remind"), "{}", board.tools[0].description);
        assert_eq!(board.places, ["Assistant chat", "Agent", "Plan", "Ask"]);
        assert_eq!(board.data.as_deref(), Some("~/.kibo/board.json"));

        std::fs::write(dir.join("board.json"), "[]").unwrap();
        let mut settings = AppSettings::default();
        settings.features.insert("board".into(), true);
        let on = views(&settings, Some(&dir));
        assert!(on[0].on && on[0].has_data);
    }

    /// A mode is named only when it lists one of the tools: `runCommand`
    /// is the agent's alone, and no chat role has it.
    #[test]
    fn the_places_are_where_the_tools_are_listed() {
        use crate::domain::tools::ToolName;
        assert_eq!(places(&[ToolName::RunCommand]), ["Agent"]);
        assert_eq!(places(&[ToolName::KubeList]), ["Kubernetes chat"]);
    }

    #[test]
    fn a_choice_is_saved_and_its_data_deleted_only_while_off() {
        crate::testing::with_app_dir("features-set", || {
            let dir = app_dir::dir().unwrap();
            assert!(!enabled().is_on(Feature::Board));
            set("board", true).unwrap();
            assert!(enabled().is_on(Feature::Board));

            std::fs::write(dir.join("board.json"), "[]").unwrap();
            assert!(delete_data("board").unwrap_err().contains("turn Board off"));
            assert!(dir.join("board.json").exists());

            set("board", false).unwrap();
            delete_data("board").unwrap();
            assert!(!dir.join("board.json").exists());
            // Nothing there is nothing to delete, not a failure.
            delete_data("board").unwrap();
            assert!(set("nope", true).is_err());
        });
    }
}
