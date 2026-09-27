//! `settings.json`, read and written whole.

use std::fs;
use std::path::PathBuf;

use crate::domain::settings::{AppSettings, SettingsError};
use crate::infra::app_dir;

const FILE: &str = "settings.json";

pub fn path() -> Result<PathBuf, SettingsError> {
    Ok(app_dir::dir().map_err(SettingsError::AppDir)?.join(FILE))
}

/// A missing file is the first run, not an error.
pub fn load() -> Result<AppSettings, SettingsError> {
    let path = path()?;
    let text = match fs::read_to_string(&path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(AppSettings::default()),
        Err(e) => return Err(SettingsError::Read(e)),
    };
    // A file that does not parse is reported, never quietly replaced by the
    // defaults: the next save would then write those defaults over whatever
    // the user actually had, turning one bad character into a lost
    // configuration.
    serde_json::from_str(&text).map_err(SettingsError::Parse)
}

pub fn save(settings: &AppSettings) -> Result<(), SettingsError> {
    app_dir::ensure().map_err(SettingsError::AppDir)?;
    let text = serde_json::to_string_pretty(settings).map_err(SettingsError::Serialize)?;
    app_dir::write_private(&path()?, text.as_bytes()).map_err(SettingsError::Write)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::settings::{LlmSettings, ProviderConfig};
    use crate::testing::with_app_dir;

    fn settings() -> AppSettings {
        AppSettings {
            llm: LlmSettings {
                active_provider_id: Some("local".to_string()),
                providers: vec![ProviderConfig {
                    id: "local".to_string(),
                    base_url: "http://127.0.0.1:1234/v1".to_string(),
                    model: Some("qwen".to_string()),
                    ..Default::default()
                }],
                debug_logging: true,
                reply_language: crate::domain::settings::ReplyLanguage::Russian,
            },
            skills: crate::domain::settings::OptOut { disabled: vec!["release".to_string()] },
            skill_sources: crate::domain::settings::OptOut { disabled: vec!["agents".to_string()] },
            rules: crate::domain::settings::OptOut { disabled: vec!["/repo/AGENTS.md".to_string()] },
            tool_log: crate::domain::settings::ToolLogSettings { enabled: false },
            approval: crate::domain::settings::ApprovalMemory {
                remember: crate::domain::settings::RememberScope::Repository,
                auto_chats: vec!["c1".to_string()],
                auto_folders: vec!["/repo".to_string()],
            },
        }
    }

    #[test]
    fn nothing_saved_yet_reads_as_the_defaults() {
        with_app_dir("settings-empty", || {
            assert_eq!(load().unwrap(), AppSettings::default());
        });
    }

    #[test]
    fn settings_round_trip() {
        with_app_dir("settings-round-trip", || {
            save(&settings()).unwrap();
            assert_eq!(load().unwrap(), settings());
        });
    }

    /// Defaulting here would make the next save overwrite the file that
    /// failed to parse — one stray character, and the configuration is gone.
    #[test]
    fn a_damaged_file_is_an_error_not_a_silent_reset() {
        with_app_dir("settings-damaged", || {
            save(&settings()).unwrap();
            fs::write(path().unwrap(), "{ not json").unwrap();

            assert!(matches!(load(), Err(SettingsError::Parse(_))));
        });
    }

    /// Settings written by a newer build, or by hand, keep whatever this
    /// build knows and ignore the rest rather than refusing to start.
    #[test]
    fn unknown_fields_are_ignored() {
        with_app_dir("settings-unknown", || {
            app_dir::ensure().unwrap();
            fs::write(
                path().unwrap(),
                r#"{"llm":{"debugLogging":true},"somethingElse":{"a":1}}"#,
            )
            .unwrap();

            assert!(load().unwrap().llm.debug_logging);
        });
    }
}
