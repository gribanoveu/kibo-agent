//! What is left on the account a provider bills, for the chip beside the
//! model — `Feature::ProviderBalance`, turned on from the Catalog. Only
//! DeepSeek says it today, at `/user/balance` on its own host.

use serde::Serialize;
use url::Url;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderBalance {
    /// The provider's own word on whether it still covers a request.
    pub available: bool,
    /// One per currency the account holds.
    pub amounts: Vec<BalanceAmount>,
}

/// Amounts as the provider writes them (`"110.00"`): shown, never added up.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BalanceAmount {
    pub currency: String,
    pub total: String,
    pub granted: String,
    pub topped_up: String,
}

const DEEPSEEK_BALANCE: &str = "https://api.deepseek.com/user/balance";

/// Where the provider at `base_url` tells its balance; `None` for one that
/// does not. Known by the host, not by the provider's name: the name is the
/// user's, and a gateway in front of DeepSeek has no `/user/balance`. Only
/// that host — the request carries the key.
pub fn balance_url(base_url: &str) -> Option<&'static str> {
    let url = Url::parse(base_url.trim()).ok()?;
    (url.host_str() == Some("api.deepseek.com")).then_some(DEEPSEEK_BALANCE)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deepseek_is_known_by_its_host_on_either_wire() {
        for base in ["https://api.deepseek.com", "https://api.deepseek.com/v1/", "https://api.deepseek.com/anthropic", " https://API.deepseek.com "] {
            assert_eq!(balance_url(base), Some(DEEPSEEK_BALANCE), "{base}");
        }
    }

    #[test]
    fn any_other_host_is_not_asked() {
        for base in [
            "https://openrouter.ai/api/v1",
            "https://api.deepseek.com.evil.example/v1",
            "https://gateway.example/api.deepseek.com",
            "https://deepseek.com",
            "not a url",
            "",
        ] {
            assert_eq!(balance_url(base), None, "{base}");
        }
    }
}
