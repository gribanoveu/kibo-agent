//! Which web search a place uses, and setting it up for a turn
//! (`docs/24-web-search.md`) — what Settings → Web search chose, read the
//! way the turn and the context meter both need it.

use std::sync::Arc;

use crate::domain::embeddings::EmbeddingProvider;
use crate::domain::settings::{CredentialsError, WebSearchBackend, WebSearchPlace, DEFAULT_SEARXNG_URL};
use crate::domain::web_search::{searxng_base, Web, WebSetupError};
use crate::domain::features::{Feature, Features};
use crate::domain::provider_balance::is_deepseek;
use crate::infra::{deepseek_web, llm_credentials_store, open_web, settings_store, tavily};

/// Which search `webSearch` asks in `place`: the one chosen, or, never chosen,
/// what `AppSettings::web_search_in` says. An unreadable settings file is the
/// default's to answer, as for every setting read here. The saved keys are
/// asked only when the answer turns on them — a search never chosen, where a
/// Tavily key decides — so keys that cannot be read stop only what needs one.
pub fn backend(place: WebSearchPlace) -> Result<WebSearchBackend, CredentialsError> {
    let settings = settings_store::load().unwrap_or_default();
    let without = settings.web_search_in(place, false);
    if without == settings.web_search_in(place, true) {
        return Ok(without);
    }
    Ok(settings.web_search_in(place, tavily::has_saved_key()?))
}

/// Where the user's SearXNG answers, as saved or the default.
pub fn searxng_url() -> String {
    settings_store::load()
        .ok()
        .and_then(|settings| settings.searxng_url)
        .unwrap_or_else(|| DEFAULT_SEARXNG_URL.to_string())
}

/// Whether `place` is given the web tools: a search chosen there, and
/// Tavily's with a key.
///
/// For the context meter's estimate: keys that cannot be read count as off
/// here, and the turn itself says why ([`web_for`]).
pub fn is_on(place: WebSearchPlace) -> bool {
    let has_key = tavily::has_saved_key().unwrap_or(false);
    backend(place).is_ok_and(|backend| backend.is_on(has_key))
}

/// What `place` reaches the web with; `None` while it is off, or Tavily's has
/// no key. A search that is on but cannot be set up — a SearXNG address that is
/// not one, keys that cannot be read — is an error, and the turn stops with
/// it: quietly without the tools, the model only says it cannot search, and
/// nobody learns why.
pub fn web_for(place: WebSearchPlace, embeddings: &Arc<dyn EmbeddingProvider>) -> Result<Option<Web>, WebSetupError> {
    let client = |e: crate::infra::http_agent::TlsError| WebSetupError::Client(e.to_string());
    let search = match backend(place)? {
        WebSearchBackend::Off => return Ok(None),
        WebSearchBackend::Searxng => {
            let address = searxng_url();
            let base = searxng_base(&address).map_err(|reason| WebSetupError::Address { address, reason })?;
            open_web::searxng(base, Arc::clone(embeddings)).map_err(client)?
        }
        WebSearchBackend::Tavily => match tavily::saved()? {
            Some(search) => search,
            None => return Ok(None),
        },
    };
    open_web::web(search, Arc::clone(embeddings)).map(Some).map_err(client)
}

/// What `deepseekSearch` and `deepseekFetch` reach the web with: DeepSeek's
/// own, with the key of the first provider on DeepSeek's address that has
/// one. `None` while the feature is off, or there is no such provider — the
/// tools then tell the model where the user adds one. Apart from `web_for`:
/// Settings → Web search does not decide it.
pub fn deepseek_for(features: &Features, embeddings: &Arc<dyn EmbeddingProvider>) -> Option<Web> {
    if !features.is_on(Feature::DeepseekSearch) {
        return None;
    }
    let settings = settings_store::load().ok()?.llm;
    settings.providers.iter().filter(|p| is_deepseek(&p.base_url)).find_map(|provider| {
        let key = llm_credentials_store::get_api_key(&provider.id).ok().flatten()?;
        deepseek_web::web(key, provider.trusted_cert_pem.as_deref(), Arc::clone(embeddings)).ok()
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::with_app_dir;

    struct NoModel;
    impl crate::domain::embeddings::EmbeddingProvider for NoModel {
        fn embed(&self, _: &[&str]) -> Result<Vec<crate::domain::embeddings::Embedding>, crate::domain::embeddings::EmbeddingError> {
            Err(crate::domain::embeddings::EmbeddingError::Invalid("no model in this test".into()))
        }
        fn dimensions(&self) -> usize {
            2
        }
    }


    /// A search turned on that cannot start stops the turn with what to fix,
    /// rather than leaving the model quietly without its tools.
    #[test]
    fn a_search_that_cannot_start_says_why() {
        with_app_dir("web-broken-address", || {
            let model: Arc<dyn EmbeddingProvider> = Arc::new(NoModel);
            let mut settings = settings_store::load().unwrap();
            settings.web_search = Some(WebSearchBackend::Searxng);
            settings.searxng_url = Some("not a url".into());
            settings_store::save(&settings).unwrap();

            let why = web_for(WebSearchPlace::Chat, &model).err().expect("a broken address started").to_string();
            assert!(why.contains("SearXNG") && why.contains("Settings → Web search"), "{why}");
        });
    }

    /// Keys that cannot be read stop what needs one — Tavily, or a search never
    /// chosen, which a Tavily key would turn on — and nothing else.
    #[test]
    fn unreadable_keys_stop_only_the_search_that_needs_one() {
        with_app_dir("web-broken-keys", || {
            let model: Arc<dyn EmbeddingProvider> = Arc::new(NoModel);
            let dir = crate::infra::app_dir::ensure().unwrap();
            crate::infra::app_dir::write_private(&dir.join("llm_credentials.enc"), b"ATLS not sealed by anyone").unwrap();

            assert!(web_for(WebSearchPlace::Agent, &model).unwrap().is_none(), "the agent's search is off by default");
            let why = web_for(WebSearchPlace::Chat, &model).err().expect("never chosen: the key decides").to_string();
            assert!(why.contains("does not open"), "{why}");
            assert!(tavily::has_saved_key().is_err(), "Settings is told, not shown \"no key\"");

            settings_store::update(|s| s.set_web_search(WebSearchPlace::Chat, WebSearchBackend::Searxng)).unwrap();
            assert!(web_for(WebSearchPlace::Chat, &model).unwrap().is_some(), "SearXNG needs no key");
            settings_store::update(|s| s.set_web_search(WebSearchPlace::Chat, WebSearchBackend::Tavily)).unwrap();
            assert!(web_for(WebSearchPlace::Chat, &model).is_err());
        });
    }

    /// Only while the feature is on, and only with a key of a provider on
    /// DeepSeek's own address — not one behind a gateway, whatever its name.
    #[test]
    fn deepseeks_web_needs_the_feature_and_a_deepseek_key() {
        with_app_dir("web-deepseek", || {
            use crate::domain::settings::ProviderConfig;
            let model: Arc<dyn EmbeddingProvider> = Arc::new(NoModel);
            let provider = |id: &str, base_url: &str| ProviderConfig { id: id.into(), base_url: base_url.into(), ..Default::default() };
            settings_store::update(|s| {
                s.llm.providers = vec![
                    provider("deepseek", "https://api.deepseek.com"),
                    provider("gateway", "https://gateway.example/deepseek/v1"),
                ]
            })
            .unwrap();
            llm_credentials_store::save_api_key("gateway", "sk-gateway").unwrap();
            let on = Features::from_settings(&[("deepseek-search".to_string(), true)].into());

            assert!(deepseek_for(&on, &model).is_none(), "the gateway's key is not DeepSeek's");
            llm_credentials_store::save_api_key("deepseek", "sk-ds").unwrap();
            assert!(deepseek_for(&on, &model).is_some());
            assert!(deepseek_for(&Features::default(), &model).is_none(), "off");
        });
    }
}
