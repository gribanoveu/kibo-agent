//! The balance chip's one question (`domain::provider_balance`): what is
//! left on `provider_id`'s account — nothing while the feature is off or the
//! provider does not say.

use crate::domain::features::Feature;
use crate::domain::llm::LlmError;
use crate::domain::provider_balance::{balance_url, ProviderBalance};
use crate::infra::{llm_credentials_store, llm_providers, settings_store};
use crate::services::features;

pub fn balance(provider_id: &str) -> Result<Option<ProviderBalance>, LlmError> {
    if !features::enabled().is_on(Feature::ProviderBalance) {
        return Ok(None);
    }
    let settings = settings_store::load().map_err(|e| LlmError::Message(e.to_string()))?.llm;
    let Some(config) = settings.provider(provider_id) else {
        return Ok(None);
    };
    let Some(url) = balance_url(&config.base_url) else {
        return Ok(None);
    };
    let api_key = llm_credentials_store::get_api_key(&config.id)
        .map_err(|e| LlmError::Message(e.to_string()))?
        .ok_or_else(|| LlmError::Message(format!("no API key is stored for provider \"{}\"", config.id)))?;
    llm_providers::balance::deepseek(url, &api_key, config.trusted_cert_pem.as_deref()).map(Some)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::settings::{AppSettings, LlmSettings, ProviderConfig};
    use crate::testing::with_app_dir;

    fn configure(base_url: &str, on: bool) {
        let mut settings = AppSettings {
            llm: LlmSettings {
                providers: vec![ProviderConfig { id: "ds".into(), base_url: base_url.into(), ..Default::default() }],
                ..Default::default()
            },
            ..Default::default()
        };
        settings.features.insert("provider-balance".into(), on);
        settings_store::save(&settings).unwrap();
    }

    /// Each `None` is said before the network: the keyless DeepSeek below
    /// proves the same path otherwise goes on to ask.
    #[test]
    fn only_deepseek_is_asked_and_only_while_the_feature_is_on() {
        with_app_dir("balance-gate", || {
            configure("https://api.deepseek.com", false);
            assert_eq!(balance("ds").unwrap(), None, "off");

            configure("https://openrouter.ai/api/v1", true);
            assert_eq!(balance("ds").unwrap(), None, "not DeepSeek");
            assert_eq!(balance("nope").unwrap(), None, "not configured");

            configure("https://api.deepseek.com", true);
            let err = balance("ds").unwrap_err();
            assert!(err.to_string().contains("API key"), "{err}");
        });
    }
}
