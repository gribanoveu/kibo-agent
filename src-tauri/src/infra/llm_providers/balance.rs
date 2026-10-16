//! DeepSeek's `GET /user/balance` (api-docs.deepseek.com/api/get-user-balance).

use secrecy::{ExposeSecret, SecretString};
use serde::Deserialize;

use super::openai_compatible::ok_or_status_error;
use crate::domain::llm::LlmError;
use crate::domain::provider_balance::{BalanceAmount, ProviderBalance};
use crate::infra::http_agent;

#[derive(Deserialize)]
struct Wire {
    is_available: bool,
    #[serde(default)]
    balance_infos: Vec<WireAmount>,
}

#[derive(Deserialize)]
struct WireAmount {
    currency: String,
    total_balance: String,
    #[serde(default)]
    granted_balance: String,
    #[serde(default)]
    topped_up_balance: String,
}

pub fn deepseek(url: &str, api_key: &SecretString, trusted_cert_pem: Option<&str>) -> Result<ProviderBalance, LlmError> {
    let agent = http_agent::build_agent(trusted_cert_pem).map_err(|e| LlmError::Tls(e.0))?;
    let response = agent
        .get(url)
        .header("Authorization", format!("Bearer {}", api_key.expose_secret()).as_str())
        .call()
        .map_err(|e| LlmError::Http(e.to_string()))?;
    let wire: Wire = ok_or_status_error(response)?
        .body_mut()
        .read_json()
        .map_err(|e| LlmError::Http(e.to_string()))?;
    Ok(ProviderBalance {
        available: wire.is_available,
        amounts: wire
            .balance_infos
            .into_iter()
            .map(|a| BalanceAmount {
                currency: a.currency,
                total: a.total_balance,
                granted: a.granted_balance,
                topped_up: a.topped_up_balance,
            })
            .collect(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::infra::llm_providers::openai_compatible::tests::serve_capturing;

    #[test]
    fn the_balance_is_asked_with_the_key_and_read_as_written() {
        let (url, server) = serve_capturing(
            r#"{"is_available":true,"balance_infos":[{"currency":"CNY","total_balance":"110.00","granted_balance":"10.00","topped_up_balance":"100.00"}]}"#
                .to_string(),
        );
        let balance = deepseek(&format!("{url}/user/balance"), &SecretString::from("sk-test"), None).expect("read");
        let sent = server.join().expect("served");
        assert!(sent.starts_with("GET /user/balance HTTP/1.1"), "{sent}");
        assert!(sent.to_lowercase().contains("authorization: bearer sk-test"), "{sent}");
        assert_eq!(
            balance,
            ProviderBalance {
                available: true,
                amounts: vec![BalanceAmount {
                    currency: "CNY".into(),
                    total: "110.00".into(),
                    granted: "10.00".into(),
                    topped_up: "100.00".into(),
                }],
            }
        );
    }

    /// No currencies is still an answer, not a failure to read one.
    #[test]
    fn a_spent_account_says_it_does_not_cover_a_request() {
        let (url, server) = serve_capturing(r#"{"is_available":false,"balance_infos":[]}"#.to_string());
        let balance = deepseek(&url, &SecretString::from("k"), None).expect("read");
        server.join().expect("served");
        assert_eq!(balance, ProviderBalance { available: false, amounts: vec![] });
    }
}
