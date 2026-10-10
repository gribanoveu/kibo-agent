//! Everything one LLM call needs, resolved in one place: which provider, its
//! key, a client built from both, and the model to send.
//!
//! The sequence is short but has to be identical everywhere — a caller that
//! resolves the provider but forgets the model pin, or reads the key from the
//! wrong id, fails in a way that looks like a provider outage.

use std::sync::Arc;

use secrecy::SecretString;

use crate::domain::llm::{ChatRequest, ChatResponse, ChatStreamResult, LlmError, LlmModelInfo, LlmProvider};
use crate::domain::settings::{ProviderConfig, ReplyLanguage, SettingsError, TurnLimits, DEFAULT_CONTEXT_LIMIT};
use crate::infra::{daily_metrics, llm_credentials_store, llm_providers, settings_store};

pub struct LlmSession {
    pub provider: Arc<dyn LlmProvider>,
    /// Kept for the debug log and, later, rate-limit accounting: both are
    /// keyed by provider, and the client itself no longer knows which id it
    /// was built from.
    pub provider_id: String,
    pub model: String,
    /// Read here rather than at each call site so the turn loop cannot use a
    /// stale copy of the flag mid-turn.
    pub debug_logging: bool,
    /// The model's context window, when it is known. See
    /// `domain::compaction`.
    pub context_limit: Option<u32>,
    /// What the model is told to write in; `None` tells it nothing. See
    /// `domain::settings::ReplyLanguage`.
    pub reply_language: Option<&'static str>,
    /// How far an agent turn on this session may run.
    pub limits: TurnLimits,
}

/// The session for `provider_id`, or for the active provider when `None`.
///
/// Deliberately builds a fresh client per call rather than caching one: a
/// client is an HTTP agent and a handful of strings, a turn resolves once and
/// then reuses it for every round, and a cache keyed on the configuration is
/// the thing that later serves a rotated key from a stale entry.
pub fn resolve(provider_id: Option<&str>) -> Result<LlmSession, LlmError> {
    let settings = settings_store::load().map_err(settings_error)?.llm;
    let config = match provider_id {
        Some(id) => settings
            .provider(id)
            .ok_or_else(|| LlmError::Message(format!("no provider is configured with id \"{id}\"")))?,
        None => settings
            .active()
            .ok_or_else(|| LlmError::Message("no LLM provider is configured".to_string()))?,
    };

    let api_key = llm_credentials_store::get_api_key(&config.id);
    let provider: Arc<dyn LlmProvider> = Arc::from(llm_providers::provider_for(config, api_key)?);
    let model = effective_model(config, provider.as_ref())?;

    Ok(LlmSession {
        provider: Arc::new(Counted(provider, model.clone())),
        provider_id: config.id.clone(),
        model,
        debug_logging: settings.debug_logging,
        context_limit: Some(config.context_limit.unwrap_or(DEFAULT_CONTEXT_LIMIT)),
        reply_language: settings.reply_language.name(),
        limits: settings.turn_limits,
    })
}

/// The language the user chose for replies, for a message built before the
/// turn has a session — `/review`'s. Settings that cannot be read say none.
pub fn reply_language() -> Option<&'static str> {
    settings_store::load().ok()?.llm.reply_language.name()
}

/// The model to send: the pin when there is one, otherwise whatever the
/// provider lists first — which is then written back as the pin.
///
/// Writing it back is the point. Without a pin, "the first model" is a
/// property of the provider's catalogue on the day, so a project's answers
/// would start coming from a different model without anything having changed
/// on this side.
pub fn effective_model(
    config: &ProviderConfig,
    provider: &dyn LlmProvider,
) -> Result<String, LlmError> {
    if let Some(model) = &config.model {
        return Ok(model.clone());
    }
    let model = provider
        .list_models()?
        .into_iter()
        .next()
        .map(|m| m.id)
        .ok_or_else(|| {
            LlmError::Provider(format!("provider \"{}\" lists no models", config.id))
        })?;
    pin_model(&config.id, &model).map_err(settings_error)?;
    Ok(model)
}

/// What `config` serves, asked before it is saved — the settings form's
/// model list. The key is the one typed into the form, or else the one stored
/// under the provider's name.
pub fn list_models_for(config: &ProviderConfig, api_key: Option<SecretString>) -> Result<Vec<String>, LlmError> {
    let api_key = api_key.or_else(|| llm_credentials_store::get_api_key(&config.id));
    let provider = llm_providers::provider_for(config, api_key)?;
    Ok(provider.list_models()?.into_iter().map(|m| m.id).collect())
}

/// Adds a provider, or replaces the one with the same id in place.
///
/// In place, rather than remove-and-append: an edit must not move the entry to
/// the end of a list the user arranged, and the first entry is what an
/// unpinned setup uses.
pub fn save_provider(config: ProviderConfig) -> Result<(), SettingsError> {
    let mut settings = settings_store::load()?;
    match settings
        .llm
        .providers
        .iter_mut()
        .find(|p| p.id == config.id)
    {
        Some(existing) => *existing = config,
        None => settings.llm.providers.push(config),
    }
    settings_store::save(&settings)
}

/// Removes a provider and the key sealed under its id.
///
/// The key goes with it: an id that is later reused for a different endpoint
/// would otherwise inherit a credential the user thought they had deleted.
pub fn remove_provider(id: &str) -> Result<(), SettingsError> {
    let mut settings = settings_store::load()?;
    settings.llm.providers.retain(|p| p.id != id);
    if settings.llm.active_provider_id.as_deref() == Some(id) {
        settings.llm.active_provider_id = None;
    }
    settings_store::save(&settings)?;
    // Best effort: settings are already written, and a key left behind is
    // unreachable rather than dangerous.
    let _ = llm_credentials_store::delete_api_key(id);
    Ok(())
}

/// Pins which provider a turn uses. `None` falls back to the first configured.
pub fn set_active_provider(id: Option<String>) -> Result<(), SettingsError> {
    let mut settings = settings_store::load()?;
    settings.llm.active_provider_id = id;
    settings_store::save(&settings)
}

pub fn set_debug_logging(enabled: bool) -> Result<(), SettingsError> {
    let mut settings = settings_store::load()?;
    settings.llm.debug_logging = enabled;
    settings_store::save(&settings)
}

pub fn set_reply_language(language: ReplyLanguage) -> Result<(), SettingsError> {
    let mut settings = settings_store::load()?;
    settings.llm.reply_language = language;
    settings_store::save(&settings)
}

pub fn set_turn_limits(limits: TurnLimits) -> Result<(), SettingsError> {
    let mut settings = settings_store::load()?;
    settings.llm.turn_limits = limits;
    settings_store::save(&settings)
}

/// Stores `model` as `provider_id`'s pin, leaving its other fields alone.
pub fn pin_model(provider_id: &str, model: &str) -> Result<(), SettingsError> {
    let mut settings = settings_store::load()?;
    let Some(config) = settings.llm.providers.iter_mut().find(|p| p.id == provider_id) else {
        // Nothing to pin it to. Adding an entry here would invent a provider
        // with no endpoint, which every later lookup would then have to
        // special-case.
        return Ok(());
    };
    config.model = Some(model.to_string());
    settings_store::save(&settings)
}

/// The provider every session in the app talks through, counting what each
/// answer says it spent into today's metrics, under the model it asked and
/// the hour it answered. Here rather than in the turn loop: a chat's title, a
/// commit message and a compaction summary ask the model too, and every one
/// of them comes through a resolved session.
struct Counted(Arc<dyn LlmProvider>, String);

impl Counted {
    fn count(&self, usage: Option<&crate::domain::llm::ChatUsage>) {
        if let Some(usage) = usage {
            let hour = chrono::Local::now().format("%H").to_string();
            daily_metrics::record(&crate::domain::metrics::of_usage(&self.1, &hour, usage));
        }
    }
}

impl LlmProvider for Counted {
    fn chat(&self, request: ChatRequest) -> Result<ChatResponse, LlmError> {
        let response = self.0.chat(request)?;
        self.count(response.usage.as_ref());
        Ok(response)
    }

    fn chat_stream(
        &self,
        request: ChatRequest,
        on_delta: &dyn Fn(&str),
        on_reasoning: &dyn Fn(&str),
        on_tool_call_delta: &dyn Fn(&str, &str, &str),
        cancelled: &dyn Fn() -> bool,
    ) -> Result<ChatStreamResult, LlmError> {
        let result = self.0.chat_stream(request, on_delta, on_reasoning, on_tool_call_delta, cancelled)?;
        self.count(result.usage.as_ref());
        Ok(result)
    }

    fn list_models(&self) -> Result<Vec<LlmModelInfo>, LlmError> {
        self.0.list_models()
    }
}

fn settings_error(e: SettingsError) -> LlmError {
    LlmError::Message(e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::llm::ChatUsage;
    use crate::domain::metrics::{DailyMetric, Metric};
    use crate::domain::settings::{AppSettings, LlmSettings};
    use crate::infra::llm_credentials_store;
    use crate::testing::with_app_dir;

    /// A provider that answers `list_models` and nothing else — enough for
    /// everything resolution does without a network.
    struct Catalogue(Vec<&'static str>);

    impl LlmProvider for Catalogue {
        fn chat(&self, _: ChatRequest) -> Result<ChatResponse, LlmError> {
            unreachable!("resolution never sends a completion")
        }

        fn chat_stream(
            &self,
            _: ChatRequest,
            _: &dyn Fn(&str),
            _: &dyn Fn(&str),
            _: &dyn Fn(&str, &str, &str),
            _: &dyn Fn() -> bool,
        ) -> Result<ChatStreamResult, LlmError> {
            unreachable!("resolution never streams")
        }

        fn list_models(&self) -> Result<Vec<LlmModelInfo>, LlmError> {
            Ok(self
                .0
                .iter()
                .map(|id| LlmModelInfo { id: id.to_string() })
                .collect())
        }
    }

    fn configure(providers: Vec<ProviderConfig>, active: Option<&str>) {
        settings_store::save(&AppSettings {
            llm: LlmSettings {
                active_provider_id: active.map(|s| s.to_string()),
                providers,
                ..Default::default()
            },
            ..Default::default()
        })
        .unwrap();
    }

    fn provider(id: &str, model: Option<&str>) -> ProviderConfig {
        ProviderConfig {
            id: id.to_string(),
            base_url: format!("https://{id}.example/v1"),
            model: model.map(|s| s.to_string()),
            ..Default::default()
        }
    }

    #[test]
    fn nothing_configured_says_so_rather_than_failing_obscurely() {
        with_app_dir("session-empty", || {
            let Err(err) = resolve(None) else { panic!("expected an error") };
            assert!(err.to_string().contains("no LLM provider"), "{err}");
        });
    }

    #[test]
    fn an_unknown_id_names_the_id() {
        with_app_dir("session-unknown", || {
            configure(vec![provider("local", Some("qwen"))], None);
            let Err(err) = resolve(Some("openai")) else { panic!("expected an error") };
            assert!(err.to_string().contains("openai"), "{err}");
        });
    }

    #[test]
    fn a_configured_provider_without_a_key_does_not_resolve() {
        with_app_dir("session-keyless", || {
            configure(vec![provider("local", Some("qwen"))], None);
            let Err(err) = resolve(None) else { panic!("expected an error") };
            assert!(err.to_string().contains("API key"), "{err}");
        });
    }

    #[test]
    fn resolution_uses_the_pinned_model_and_the_key_of_that_provider() {
        with_app_dir("session-resolve", || {
            configure(
                vec![provider("local", Some("qwen")), provider("openai", Some("gpt"))],
                Some("openai"),
            );
            llm_credentials_store::save_api_key("openai", "sk-openai").unwrap();

            let session = resolve(None).expect("resolves the active provider");
            assert_eq!(session.provider_id, "openai");
            assert_eq!(session.model, "gpt");
        });
    }

    #[test]
    fn the_debug_flag_comes_from_settings() {
        with_app_dir("session-debug-flag", || {
            configure(vec![provider("local", Some("qwen"))], None);
            let mut settings = settings_store::load().unwrap();
            settings.llm.debug_logging = true;
            settings_store::save(&settings).unwrap();
            llm_credentials_store::save_api_key("local", "sk").unwrap();

            assert!(resolve(None).unwrap().debug_logging);
        });
    }

    #[test]
    fn the_reply_language_comes_from_settings_and_auto_says_nothing() {
        with_app_dir("session-language", || {
            configure(vec![provider("local", Some("qwen"))], None);
            llm_credentials_store::save_api_key("local", "sk").unwrap();
            assert_eq!(resolve(None).unwrap().reply_language, None, "Auto by default");

            let mut settings = settings_store::load().unwrap();
            settings.llm.reply_language = ReplyLanguage::Russian;
            settings_store::save(&settings).unwrap();
            assert_eq!(resolve(None).unwrap().reply_language, Some("Russian"));
        });
    }

    /// Without the write-back, every turn would take whatever the catalogue
    /// happened to list first that day.
    #[test]
    fn an_unpinned_model_is_discovered_once_and_then_pinned() {
        with_app_dir("session-pin", || {
            configure(vec![provider("local", None)], None);

            let model = effective_model(&provider("local", None), &Catalogue(vec!["a", "b"]))
                .expect("discovers a model");
            assert_eq!(model, "a");

            let stored = settings_store::load().unwrap();
            assert_eq!(stored.llm.provider("local").unwrap().model.as_deref(), Some("a"));
        });
    }

    #[test]
    fn a_pinned_model_is_used_without_asking_the_provider() {
        with_app_dir("session-pinned", || {
            configure(vec![provider("local", Some("qwen"))], None);

            let model =
                effective_model(&provider("local", Some("qwen")), &Catalogue(vec![])).unwrap();
            assert_eq!(model, "qwen");
        });
    }

    #[test]
    fn a_provider_with_an_empty_catalogue_is_an_error_not_an_empty_model() {
        with_app_dir("session-no-models", || {
            configure(vec![provider("local", None)], None);

            let err = effective_model(&provider("local", None), &Catalogue(vec![]))
                .expect_err("no models");
            assert!(err.to_string().contains("no models"), "{err}");
        });
    }

    /// Pinning against an id that is not in settings must not conjure an
    /// entry with no endpoint.
    #[test]
    fn pinning_an_unconfigured_provider_changes_nothing() {
        with_app_dir("session-pin-unknown", || {
            configure(vec![provider("local", Some("qwen"))], None);

            pin_model("gone", "whatever").unwrap();

            let stored = settings_store::load().unwrap();
            assert_eq!(stored.llm.providers.len(), 1);
            assert_eq!(stored.llm.provider("local").unwrap().model.as_deref(), Some("qwen"));
        });
    }

    // ------------------------------------------------------ managing providers

    #[test]
    fn a_provider_is_added_once_and_edited_in_place() {
        with_app_dir("providers-upsert", || {
            save_provider(provider("local", None)).unwrap();
            save_provider(provider("openai", Some("gpt"))).unwrap();
            save_provider(provider("local", Some("qwen"))).unwrap();

            let providers = settings_store::load().unwrap().llm.providers;
            assert_eq!(
                providers.iter().map(|p| p.id.as_str()).collect::<Vec<_>>(),
                ["local", "openai"],
                "an edit must not move the entry to the end of a list the user arranged"
            );
            assert_eq!(providers[0].model.as_deref(), Some("qwen"));
        });
    }

    /// An id reused later for a different endpoint would otherwise inherit a
    /// credential the user believed they had deleted.
    #[test]
    fn removing_a_provider_takes_its_key_with_it() {
        with_app_dir("providers-remove", || {
            save_provider(provider("local", Some("qwen"))).unwrap();
            llm_credentials_store::save_api_key("local", "sk-secret").unwrap();

            remove_provider("local").unwrap();

            assert!(settings_store::load().unwrap().llm.providers.is_empty());
            assert!(!llm_credentials_store::has_api_key("local"));
        });
    }

    /// Otherwise the pin points at nothing and every turn refuses to start,
    /// with no obvious way for the user to see why.
    #[test]
    fn removing_the_active_provider_clears_the_pin() {
        with_app_dir("providers-remove-active", || {
            save_provider(provider("local", Some("qwen"))).unwrap();
            save_provider(provider("openai", Some("gpt"))).unwrap();
            set_active_provider(Some("local".to_string())).unwrap();

            remove_provider("local").unwrap();

            let settings = settings_store::load().unwrap().llm;
            assert_eq!(settings.active_provider_id, None);
            assert_eq!(settings.active().unwrap().id, "openai", "and the other one takes over");
        });
    }

    #[test]
    fn the_active_provider_and_the_debug_flag_are_remembered() {
        with_app_dir("providers-flags", || {
            save_provider(provider("local", Some("qwen"))).unwrap();
            set_active_provider(Some("local".to_string())).unwrap();
            set_debug_logging(true).unwrap();

            let settings = settings_store::load().unwrap().llm;
            assert_eq!(settings.active_provider_id.as_deref(), Some("local"));
            assert!(settings.debug_logging);
        });
    }

    /// Editing a provider must not silently disarm the log the user turned on,
    /// or arm one they did not.
    #[test]
    fn saving_a_provider_leaves_the_other_settings_alone() {
        with_app_dir("providers-preserve", || {
            set_debug_logging(true).unwrap();
            set_active_provider(Some("local".to_string())).unwrap();

            save_provider(provider("local", Some("qwen"))).unwrap();

            let settings = settings_store::load().unwrap().llm;
            assert!(settings.debug_logging);
            assert_eq!(settings.active_provider_id.as_deref(), Some("local"));
        });
    }

    /// Answers with a fixed usage, streamed or not.
    struct Spends(ChatUsage);

    impl LlmProvider for Spends {
        fn chat(&self, _: ChatRequest) -> Result<ChatResponse, LlmError> {
            Ok(ChatResponse { content: Some("a title".into()), tool_calls: Vec::new(), usage: Some(self.0) })
        }

        fn chat_stream(
            &self,
            _: ChatRequest,
            _: &dyn Fn(&str),
            _: &dyn Fn(&str),
            _: &dyn Fn(&str, &str, &str),
            _: &dyn Fn() -> bool,
        ) -> Result<ChatStreamResult, LlmError> {
            Ok(ChatStreamResult { usage: Some(self.0), ..Default::default() })
        }

        fn list_models(&self) -> Result<Vec<LlmModelInfo>, LlmError> {
            Ok(Vec::new())
        }
    }

    #[test]
    fn every_answer_through_a_session_counts_for_today() {
        with_app_dir("session-counted", || {
            let usage = ChatUsage { prompt_tokens: 100, completion_tokens: 7, total_tokens: 107, cached_tokens: 90 };
            let counted = Counted(Arc::new(Spends(usage)), "qwen".into());
            let request = || ChatRequest { model: "m".into(), messages: Vec::new(), tools: Vec::new() };

            counted.chat(request()).unwrap();
            counted.chat_stream(request(), &|_| {}, &|_| {}, &|_, _, _| {}, &|| false).unwrap();

            let today = chrono::Local::now().format("%Y-%m-%d").to_string();
            let row = |metric, key: &str, value| DailyMetric { day: today.clone(), metric, key: key.into(), value };
            let read = daily_metrics::read(&[Metric::PromptTokens, Metric::CachedTokens, Metric::CompletionTokens, Metric::ModelTokens]);
            assert_eq!(
                read.unwrap(),
                [
                    row(Metric::CachedTokens, "", 180),
                    row(Metric::CompletionTokens, "", 14),
                    row(Metric::ModelTokens, "qwen", 214),
                    row(Metric::PromptTokens, "", 200),
                ]
            );
            let hours = daily_metrics::read(&[Metric::HourTokens]).unwrap();
            assert_eq!(hours.iter().map(|r| r.value).sum::<u64>(), 214, "{hours:?}");
        });
    }
}
