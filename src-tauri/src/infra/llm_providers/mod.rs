pub mod anthropic;
pub mod openai_compatible;

use std::io::{BufRead, BufReader};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::time::Duration;

use secrecy::SecretString;
use serde_json::Value;

use crate::domain::llm::{LlmError, LlmProvider};
use crate::domain::settings::{ProviderConfig, ProviderKind};
use crate::infra::http_agent;

/// The one place a configuration becomes a client. Callers work against the
/// trait afterwards, never against `OpenAiCompatibleProvider` directly — which
/// is what keeps a second protocol (Anthropic's own, say) to a branch here
/// rather than a change at every call site.
pub fn provider_for(
    config: &ProviderConfig,
    api_key: Option<SecretString>,
) -> Result<Box<dyn LlmProvider>, LlmError> {
    let api_key = api_key.ok_or_else(|| {
        LlmError::Message(format!("no API key is stored for provider \"{}\"", config.id))
    })?;
    // A blank one, hand-written into the file, is the default rather than a
    // request to "".
    let models_url = config.models_url.clone().filter(|u| !u.trim().is_empty());
    let agent = http_agent::build_agent(config.trusted_cert_pem.as_deref())
        .map_err(|e| LlmError::Tls(e.0))?;
    Ok(match config.kind {
        ProviderKind::OpenAiCompatible => Box::new(openai_compatible::OpenAiCompatibleProvider::new(
            agent,
            config.base_url.clone(),
            api_key,
            config.request_headers.clone(),
            config.temperature,
            config.top_p,
            config.max_tokens,
            config.reasoning_effort.clone(),
            models_url.clone(),
        )),
        ProviderKind::Anthropic => Box::new(anthropic::AnthropicProvider::new(
            agent,
            config.base_url.clone(),
            api_key,
            config.request_headers.clone(),
            config.temperature,
            config.top_p,
            config.max_tokens,
            config.reasoning_effort.clone(),
            models_url.clone(),
        )),
    })
}

/// How soon a stop is seen while the provider says nothing.
const CANCEL_POLL: Duration = Duration::from_millis(100);

/// A streamed answer's lines, sent and read on a thread of its own. A model
/// that thinks before its first byte, or a proxy that holds the headers, keeps
/// that thread blocked for as long as it likes — the caller waits on the
/// channel instead, and a stop lands within `CANCEL_POLL`. The thread left
/// behind hangs up at its next line, once nobody takes it.
pub(super) struct StreamLines(Receiver<Result<String, LlmError>>);

impl StreamLines {
    pub(super) fn send(post: ureq::RequestBuilder<ureq::typestate::WithBody>, body: Value) -> Self {
        let (lines, taken) = mpsc::channel();
        std::thread::spawn(move || {
            let response = post
                .send_json(&body)
                .map_err(|e| LlmError::Http(e.to_string()))
                .and_then(openai_compatible::ok_or_status_error);
            let response = match response {
                Ok(response) => response,
                Err(e) => {
                    let _ = lines.send(Err(e));
                    return;
                }
            };
            for line in BufReader::new(response.into_body().into_reader()).lines() {
                let line = line.map_err(|e| LlmError::Http(e.to_string()));
                let failed = line.is_err();
                if lines.send(line).is_err() || failed {
                    return;
                }
            }
        });
        Self(taken)
    }

    /// The next line; `None` at the end of the stream or once `cancelled`.
    pub(super) fn next(&self, cancelled: &dyn Fn() -> bool) -> Result<Option<String>, LlmError> {
        loop {
            if cancelled() {
                return Ok(None);
            }
            match self.0.recv_timeout(CANCEL_POLL) {
                Ok(line) => return line.map(Some),
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => return Ok(None),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The provider has taken the request and says nothing yet — the Stop
    /// button still has to work.
    #[test]
    fn a_stop_lands_while_the_provider_is_still_silent() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("binds");
        let url = format!("http://127.0.0.1:{}", listener.local_addr().expect("addr").port());
        // Silent for 3 s, then gone: a wait that ignores the stop fails the
        // timing check below instead of hanging the run.
        std::thread::spawn(move || {
            let socket = listener.accept();
            std::thread::sleep(Duration::from_secs(3));
            drop(socket);
        });
        let post = http_agent::build_agent(None).expect("agent").post(url);
        let lines = StreamLines::send(post, serde_json::json!({}));

        let started = std::time::Instant::now();
        let stop_after = started + Duration::from_millis(200);
        let next = lines.next(&|| std::time::Instant::now() > stop_after);
        assert!(started.elapsed() < Duration::from_secs(2), "waited for the provider instead");
        assert_eq!(next.expect("a stop is not an error"), None);
    }

    fn config(trusted_cert_pem: Option<&str>) -> ProviderConfig {
        ProviderConfig {
            id: "local".to_string(),
            base_url: "https://example.internal/v1".to_string(),
            trusted_cert_pem: trusted_cert_pem.map(|s| s.to_string()),
            ..Default::default()
        }
    }

    /// The most common way a provider is "configured but broken", and the
    /// message has to say which of the two halves is missing.
    #[test]
    fn a_provider_without_a_key_says_so() {
        let Err(err) = provider_for(&config(None), None) else {
            panic!("expected an error");
        };
        assert!(err.to_string().contains("local"), "{err}");
    }

    #[test]
    fn a_damaged_trust_certificate_is_a_tls_error_not_a_key_error() {
        let Err(err) = provider_for(&config(Some("not a pem")), Some(SecretString::from("k")))
        else {
            panic!("expected an error")
        };
        assert!(matches!(err, LlmError::Tls(_)), "{err}");
    }

    /// Where the model list is asked for, as the request line shows it:
    /// `{base_url}/models` unless the provider names another — or names a
    /// blank one, which is the default too.
    #[test]
    fn the_model_list_is_asked_where_the_provider_says() {
        use crate::infra::llm_providers::openai_compatible::tests::serve_capturing;
        let asked = |kind, models_url: Option<&str>| {
            let (url, server) = serve_capturing(r#"{"data":[]}"#.to_string());
            let config = ProviderConfig {
                id: "p".into(),
                kind,
                base_url: format!("{url}/anthropic/"),
                models_url: models_url.map(|u| u.replace("{url}", &url)),
                ..Default::default()
            };
            provider_for(&config, Some(SecretString::from("k"))).expect("builds").list_models().expect("lists");
            let sent = server.join().expect("served");
            sent.lines().next().unwrap_or_default().to_string()
        };

        assert_eq!(asked(ProviderKind::OpenAiCompatible, None), "GET /anthropic/models HTTP/1.1");
        assert_eq!(asked(ProviderKind::OpenAiCompatible, Some("  ")), "GET /anthropic/models HTTP/1.1");
        assert_eq!(asked(ProviderKind::OpenAiCompatible, Some("{url}/models")), "GET /models HTTP/1.1");
        assert_eq!(asked(ProviderKind::Anthropic, None), "GET /anthropic/models?limit=1000 HTTP/1.1");
        assert_eq!(asked(ProviderKind::Anthropic, Some("{url}/models")), "GET /models?limit=1000 HTTP/1.1");
    }

    #[test]
    fn the_public_trust_store_needs_no_certificate() {
        assert!(provider_for(&config(None), Some(SecretString::from("k"))).is_ok());
    }
}
