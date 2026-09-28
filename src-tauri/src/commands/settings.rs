//! Configuring the provider a turn talks to.
//!
//! The API key travels in one direction only. It goes in through
//! [`llm_api_key_save`], is sealed, and never comes back out: what the window
//! can learn is whether one exists. Everything else about a provider is
//! ordinary, readable configuration.

use std::sync::Arc;

use secrecy::SecretString;
use serde::Serialize;
use tauri::State;

use std::path::{Path, PathBuf};

use crate::domain::settings::{KubeSettings, Kubeconfig, ProviderConfig, ReplyLanguage};
use crate::infra::{http_agent, llm_credentials_store, settings_store};
use crate::services::{kubeconfigs, llm_session};

use super::chat::AgentState;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderView {
    #[serde(flatten)]
    config: ProviderConfig,
    /// Whether a key is stored — never the key itself.
    has_api_key: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LlmSettingsView {
    providers: Vec<ProviderView>,
    active_provider_id: Option<String>,
    debug_logging: bool,
    reply_language: ReplyLanguage,
}

#[tauri::command]
pub fn llm_settings_get() -> Result<LlmSettingsView, String> {
    let settings = settings_store::load().map_err(|e| e.to_string())?.llm;
    Ok(LlmSettingsView {
        providers: settings
            .providers
            .into_iter()
            .map(|config| ProviderView {
                has_api_key: llm_credentials_store::has_api_key(&config.id),
                config,
            })
            .collect(),
        active_provider_id: settings.active_provider_id,
        debug_logging: settings.debug_logging,
        reply_language: settings.reply_language,
    })
}

#[tauri::command]
pub fn llm_provider_save(mut provider: ProviderConfig) -> Result<(), String> {
    if provider.id.trim().is_empty() {
        return Err("a provider needs a name".to_string());
    }
    if provider.base_url.trim().is_empty() {
        return Err("a provider needs a base URL".to_string());
    }
    if provider.temperature.is_some_and(|t| !(0.0..=2.0).contains(&t)) {
        return Err("temperature is between 0 and 2".to_string());
    }
    if provider.top_p.is_some_and(|p| !(0.0..=1.0).contains(&p)) {
        return Err("top P is between 0 and 1".to_string());
    }
    // Refused here rather than at the first turn, where a certificate that
    // does not parse would read as a provider that does not answer.
    provider.trusted_cert_pem = provider.trusted_cert_pem.filter(|pem| !pem.trim().is_empty());
    if let Some(pem) = &provider.trusted_cert_pem {
        http_agent::parse_trusted_certs(pem).map_err(|e| format!("the certificate: {}", e.0))?;
    }
    llm_session::save_provider(provider).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn llm_provider_remove(id: String) -> Result<(), String> {
    llm_session::remove_provider(&id).map_err(|e| e.to_string())
}

/// Seals the key under the app master key. There is no command that reads one
/// back, which is the point.
#[tauri::command]
pub fn llm_api_key_save(id: String, key: String) -> Result<(), String> {
    if key.trim().is_empty() {
        return llm_credentials_store::delete_api_key(&id);
    }
    llm_credentials_store::save_api_key(&id, key.trim())
}

#[tauri::command]
pub fn llm_active_provider_set(id: Option<String>) -> Result<(), String> {
    llm_session::set_active_provider(id).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn llm_debug_logging_set(enabled: bool) -> Result<(), String> {
    llm_session::set_debug_logging(enabled).map_err(|e| e.to_string())
}

/// The language replies are asked in, from the next turn on.
#[tauri::command]
pub fn llm_reply_language_set(language: ReplyLanguage) -> Result<(), String> {
    llm_session::set_reply_language(language).map_err(|e| e.to_string())
}

/// The kubeconfig files Chat mode's Kubernetes role knows of, and the one picked.
#[tauri::command]
pub fn kube_settings_get() -> Result<KubeSettings, String> {
    kubeconfigs::list().map_err(|e| e.to_string())
}

/// Adds a kubeconfig, or replaces the one of that name. Refused unless a file is
/// there now: a typo found here beats one found as a command the model wrote fails.
#[tauri::command]
pub fn kubeconfig_save(name: String, path: String) -> Result<(), String> {
    let name = name.trim();
    if name.is_empty() {
        return Err("a kubeconfig needs a name".to_string());
    }
    let path = expand_home(path.trim()).ok_or("the path starts with ~, and there is no home folder")?;
    if !path.is_absolute() {
        return Err("give the whole path, from / or ~".to_string());
    }
    if !path.is_file() {
        return Err(format!("there is no file at {}", path.display()));
    }
    let config = Kubeconfig { name: name.to_string(), path: path.to_string_lossy().into_owned() };
    kubeconfigs::save(config).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn kubeconfig_remove(name: String) -> Result<(), String> {
    kubeconfigs::remove(&name).map_err(|e| e.to_string())
}

/// The one the Kubernetes role works with, from its next reply on.
#[tauri::command]
pub fn kubeconfig_pick(name: Option<String>) -> Result<(), String> {
    kubeconfigs::pick(name).map_err(|e| e.to_string())
}

/// `~/.kube/config` as the shell would read it; any other path as it is.
fn expand_home(path: &str) -> Option<PathBuf> {
    match path.strip_prefix('~') {
        Some(rest) if rest.is_empty() || rest.starts_with('/') || rest.starts_with('\\') => {
            Some(dirs::home_dir()?.join(rest.trim_start_matches(['/', '\\'])))
        }
        _ => Some(Path::new(path).to_path_buf()),
    }
}

/// Asks the provider what it serves. A live call, so it is also the one thing
/// that proves the base URL and the key are both right.
#[tauri::command]
pub async fn llm_models_list(id: Option<String>) -> Result<Vec<String>, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let session = llm_session::resolve(id.as_deref()).map_err(|e| e.to_string())?;
        session
            .provider
            .list_models()
            .map(|models| models.into_iter().map(|m| m.id).collect())
            .map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| format!("the request thread failed: {e}"))?
}

/// What a provider serves, asked from the settings form before it is saved —
/// so a refresh reflects the URL, key and certificate as typed. `api_key`
/// `None` uses the stored one.
#[tauri::command]
pub async fn llm_models_probe(provider: ProviderConfig, api_key: Option<String>) -> Result<Vec<String>, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let api_key = api_key.filter(|k| !k.trim().is_empty()).map(|k| SecretString::from(k.trim().to_string()));
        llm_session::list_models_for(&provider, api_key).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| format!("the request thread failed: {e}"))?
}

/// Whether a turn could start right now, and if not, what is missing. Asked
/// before sending rather than discovered as a failed turn.
#[tauri::command]
pub fn agent_readiness(state: State<'_, Arc<AgentState>>) -> Readiness {
    let workspace = super::chat::workspace_current(state);
    let provider = settings_store::load()
        .ok()
        .and_then(|settings| settings.llm.active().cloned());
    Readiness {
        has_key: provider
            .as_ref()
            .map(|p| llm_credentials_store::has_api_key(&p.id))
            .unwrap_or(false),
        provider: provider.map(|p| p.id),
        workspace,
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Readiness {
    pub workspace: Option<String>,
    pub provider: Option<String>,
    pub has_key: bool,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::with_app_dir;

    fn provider(id: &str) -> ProviderConfig {
        ProviderConfig {
            id: id.to_string(),
            base_url: "https://gateway.example/v1".to_string(),
            ..Default::default()
        }
    }

    /// The whole path the settings window drives, in order: configure a
    /// provider, give it a key, ask what is configured. Each piece is tested
    /// where it lives; what this adds is that the three commands agree —
    /// which is the part a window can be wrong about and a unit test cannot.
    #[test]
    fn a_saved_key_is_reported_as_stored() {
        with_app_dir("cmd-settings-key", || {
            llm_provider_save(provider("gateway")).unwrap();
            llm_api_key_save("gateway".to_string(), "sk-live".to_string()).unwrap();
            llm_active_provider_set(Some("gateway".to_string())).unwrap();

            let view = llm_settings_get().unwrap();
            assert_eq!(view.active_provider_id.as_deref(), Some("gateway"));
            assert_eq!(view.providers.len(), 1);
            assert!(view.providers[0].has_api_key, "the key did not survive");

            // And it is the key itself that survived, not merely a flag.
            let stored = llm_credentials_store::get_api_key("gateway").expect("stored");
            assert_eq!(secrecy::ExposeSecret::expose_secret(&stored), "sk-live");
        });
    }

    /// The window sends the whole provider back, including fields it added
    /// itself. A refusal here would take the key with it: the key is saved
    /// after the provider, in the same click.
    #[test]
    fn the_shape_the_window_sends_is_accepted() {
        with_app_dir("cmd-settings-wire", || {
            let wire = serde_json::json!({
                "id": "gateway",
                "baseUrl": "https://gateway.example/v1",
                "model": null,
                "hasApiKey": true,
            });
            let parsed: ProviderConfig = serde_json::from_value(wire).expect("accepted");

            llm_provider_save(parsed).unwrap();
            assert_eq!(llm_settings_get().unwrap().providers.len(), 1);
        });
    }

    /// An empty box means "delete the stored key" — and it must not leave the
    /// provider believing it still has one.
    #[test]
    fn an_emptied_key_box_removes_the_key() {
        with_app_dir("cmd-settings-clear", || {
            llm_provider_save(provider("gateway")).unwrap();
            llm_api_key_save("gateway".to_string(), "sk-live".to_string()).unwrap();
            llm_api_key_save("gateway".to_string(), "  ".to_string()).unwrap();

            assert!(!llm_settings_get().unwrap().providers[0].has_api_key);
        });
    }

    #[test]
    fn sampling_out_of_range_is_refused_and_in_range_kept() {
        with_app_dir("cmd-settings-sampling", || {
            let with = |temperature, top_p| ProviderConfig { temperature, top_p, ..provider("gateway") };
            assert!(llm_provider_save(with(Some(2.5), None)).is_err());
            assert!(llm_provider_save(with(Some(-0.1), None)).is_err());
            assert!(llm_provider_save(with(None, Some(1.5))).is_err());
            llm_provider_save(with(Some(2.0), Some(0.0))).unwrap();
            let saved = &llm_settings_get().unwrap().providers[0].config;
            assert_eq!((saved.temperature, saved.top_p), (Some(2.0), Some(0.0)));
        });
    }

    /// A blank box is no certificate; a damaged one is said so at save.
    #[test]
    fn a_certificate_is_checked_at_save_and_a_blank_one_is_none() {
        with_app_dir("cmd-settings-cert", || {
            let with = |pem: &str| ProviderConfig { trusted_cert_pem: Some(pem.to_string()), ..provider("gateway") };
            let err = llm_provider_save(with("not a certificate")).expect_err("refused");
            assert!(err.contains("certificate"), "{err}");
            llm_provider_save(with("  \n")).unwrap();
            assert_eq!(llm_settings_get().unwrap().providers[0].config.trusted_cert_pem, None);
        });
    }

    #[test]
    fn a_provider_without_a_name_or_a_url_is_refused() {
        with_app_dir("cmd-settings-blank", || {
            assert!(llm_provider_save(provider("  ")).is_err());
            assert!(llm_provider_save(ProviderConfig {
                id: "gateway".to_string(),
                base_url: " ".to_string(),
                ..Default::default()
            })
            .is_err());
        });
    }

    #[test]
    fn a_kubeconfig_is_saved_only_where_a_file_is() {
        with_app_dir("cmd-kubeconfig", || {
            let file = crate::infra::app_dir::dir().unwrap().join("prod.yaml");
            let path = file.to_string_lossy().into_owned();
            let err = kubeconfig_save("prod".to_string(), path.clone()).unwrap_err();
            assert!(err.contains("no file"), "{err}");
            assert!(kubeconfig_save("prod".to_string(), "kube/config".to_string()).unwrap_err().contains("whole path"));

            std::fs::write(&file, "apiVersion: v1\n").unwrap();
            assert!(kubeconfig_save("  ".to_string(), path.clone()).unwrap_err().contains("name"));
            kubeconfig_save(" prod ".to_string(), path.clone()).unwrap();
            let kube = kube_settings_get().unwrap();
            assert_eq!(kube.configs, vec![Kubeconfig { name: "prod".to_string(), path }]);
        });
    }

    #[test]
    fn a_tilde_is_the_home_folder() {
        let home = dirs::home_dir().unwrap();
        assert_eq!(expand_home("~/.kube/config"), Some(home.join(".kube/config")));
        assert_eq!(expand_home("~"), Some(home));
        assert_eq!(expand_home("~bob/config"), Some(PathBuf::from("~bob/config")), "another user's home is not ours");
        assert_eq!(expand_home("/etc/kube"), Some(PathBuf::from("/etc/kube")));
    }
}
