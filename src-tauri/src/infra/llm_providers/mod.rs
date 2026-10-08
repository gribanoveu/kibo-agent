pub mod anthropic;
pub mod openai_compatible;

use std::io::{BufRead, BufReader};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::time::{Duration, Instant};

use secrecy::SecretString;
use serde_json::Value;

use crate::domain::llm::{ChatRequest, ChatResponse, ChatStreamResult, LlmError, LlmModelInfo, LlmProvider};
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
    let provider: Box<dyn LlmProvider> = match config.kind {
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
    };
    Ok(if config.supports_images { provider } else { Box::new(WithoutImages(provider)) })
}

/// A provider not set to accept pictures. Each one is replaced by a line
/// saying it was left out, so a chat that has some — started on another
/// model — still goes on here. In front of both wires, which then never need
/// to know about the setting.
struct WithoutImages(Box<dyn LlmProvider>);

impl LlmProvider for WithoutImages {
    fn chat(&self, request: ChatRequest) -> Result<ChatResponse, LlmError> {
        self.0.chat(without_images(request))
    }

    fn chat_stream(
        &self,
        request: ChatRequest,
        on_delta: &dyn Fn(&str),
        on_reasoning: &dyn Fn(&str),
        on_tool_call_delta: &dyn Fn(&str, &str, &str),
        cancelled: &dyn Fn() -> bool,
    ) -> Result<ChatStreamResult, LlmError> {
        self.0.chat_stream(without_images(request), on_delta, on_reasoning, on_tool_call_delta, cancelled)
    }

    fn list_models(&self) -> Result<Vec<LlmModelInfo>, LlmError> {
        self.0.list_models()
    }
}

fn without_images(mut request: ChatRequest) -> ChatRequest {
    for message in request.messages.iter_mut().filter(|m| !m.images.is_empty()) {
        let mut lines: Vec<String> = message.images.drain(..).map(|image| image.omitted_note()).collect();
        lines.extend(message.content.take().filter(|t| !t.is_empty()));
        message.content = Some(lines.join("\n\n"));
    }
    request
}

/// How soon a stop is seen while the provider says nothing.
const CANCEL_POLL: Duration = Duration::from_millis(100);

/// How long a stream may go without a line before it is taken for dead — a
/// proxy or NAT that dropped the connection without a word sends no FIN, so
/// nothing else would ever end the wait. Anthropic's `ping` events and any
/// other line reset it, so a model thinking for long still gets through.
const STALL: Duration = Duration::from_secs(300);

/// Ends the reader thread a stall left blocked in `read`: headers past this,
/// or a body that took longer than any real answer. A deadline of ureq's,
/// not a silence timer — its body timeout counts from the start.
const READER_HEADERS_LIMIT: Duration = Duration::from_secs(360);
const READER_BODY_LIMIT: Duration = Duration::from_secs(3600);

/// A streamed answer's lines, sent and read on a thread of its own. A model
/// that thinks before its first byte, or a proxy that holds the headers, keeps
/// that thread blocked for as long as it likes — the caller waits on the
/// channel instead, and a stop lands within `CANCEL_POLL`. The thread left
/// behind hangs up at its next line, once nobody takes it, or at ureq's
/// deadlines.
pub(super) struct StreamLines {
    lines: Receiver<Result<String, LlmError>>,
    stall: Duration,
}

impl StreamLines {
    pub(super) fn send(post: ureq::RequestBuilder<ureq::typestate::WithBody>, body: Value) -> Self {
        let (lines, taken) = mpsc::channel();
        let post = post
            .config()
            .timeout_recv_response(Some(READER_HEADERS_LIMIT))
            .timeout_recv_body(Some(READER_BODY_LIMIT))
            .build();
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
        Self { lines: taken, stall: STALL }
    }

    /// The next line; `None` at the end of the stream or once `cancelled`.
    /// `Unavailable` after `stall` without one — retried by the turn while
    /// nothing has reached the user yet, reported once something has.
    pub(super) fn next(&self, cancelled: &dyn Fn() -> bool) -> Result<Option<String>, LlmError> {
        let deadline = Instant::now() + self.stall;
        loop {
            if cancelled() {
                return Ok(None);
            }
            if Instant::now() >= deadline {
                return Err(LlmError::Unavailable {
                    retry_after_seconds: None,
                    message: format!("the provider sent nothing for {} s", self.stall.as_secs()),
                });
            }
            match self.lines.recv_timeout(CANCEL_POLL) {
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

    /// A connection that went quiet without closing — a proxy that dropped
    /// it says nothing — ends the wait. Lines spaced closer than the stall
    /// keep it alive, so it is silence that counts, not the stream's length.
    #[test]
    fn a_stream_that_goes_silent_is_given_up_on() {
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("binds");
        let url = format!("http://127.0.0.1:{}", listener.local_addr().expect("addr").port());
        std::thread::spawn(move || {
            let (mut socket, _) = listener.accept().expect("accepts");
            let _ = socket.read(&mut [0; 4096]);
            let _ = socket.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\n\r\n");
            for i in 0..4 {
                let _ = socket.write_all(format!("data: {i}\n").as_bytes());
                std::thread::sleep(Duration::from_millis(150));
            }
            // Open and silent, past the test's own timing check.
            std::thread::sleep(Duration::from_secs(5));
        });
        let post = http_agent::build_agent(None).expect("agent").post(url);
        let mut lines = StreamLines::send(post, serde_json::json!({}));
        lines.stall = Duration::from_millis(400);

        let started = Instant::now();
        for i in 0..4 {
            assert_eq!(lines.next(&|| false).expect("a line"), Some(format!("data: {i}")));
        }
        let err = lines.next(&|| false).expect_err("silence is an error");
        assert!(matches!(err, LlmError::Unavailable { retry_after_seconds: None, .. }), "{err:?}");
        assert!(started.elapsed() < Duration::from_secs(3), "waited for the provider instead");
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

    fn picture(width: u32) -> crate::domain::image::ImagePart {
        use crate::domain::image::{ImageMediaType, ImagePart};
        ImagePart { media_type: ImageMediaType::Png, data: "PIXELS".into(), width, height: 10 }
    }

    /// The setting decides what reaches the wire, for either protocol: the
    /// picture itself, or a line saying it was left out.
    #[test]
    fn a_provider_sends_pictures_only_when_set_to() {
        use crate::domain::llm::{ChatRequest, LlmMessage};
        use crate::infra::llm_providers::openai_compatible::tests::serve_capturing;
        let sent = |kind, supports_images, stream: bool| {
            let (url, server) = serve_capturing("{}".to_string());
            let config = ProviderConfig { id: "p".into(), kind, base_url: url, supports_images, ..Default::default() };
            let request = ChatRequest {
                messages: vec![LlmMessage::user_with_images("look", vec![picture(20)])],
                tools: vec![],
                model: "m".into(),
            };
            let provider = provider_for(&config, Some(SecretString::from("k"))).expect("builds");
            if stream {
                let _ = provider.chat_stream(request, &|_| {}, &|_| {}, &|_, _, _| {}, &|| false);
            } else {
                let _ = provider.chat(request);
            }
            server.join().expect("served")
        };
        for (kind, stream) in [
            (ProviderKind::OpenAiCompatible, true),
            (ProviderKind::OpenAiCompatible, false),
            (ProviderKind::Anthropic, true),
        ] {
            let with = sent(kind, true, stream);
            assert!(with.contains("PIXELS") && !with.contains("omitted"), "{kind:?} on: {with}");
            let without = sent(kind, false, stream);
            assert!(!without.contains("PIXELS"), "{kind:?} off: {without}");
            assert!(without.contains("[image 20×10 omitted: this provider is not set to accept images]"), "{kind:?}");
        }
    }

    #[test]
    fn a_left_out_picture_becomes_a_line_before_the_words() {
        use crate::domain::llm::{ChatRequest, LlmMessage};
        let request = ChatRequest {
            messages: vec![
                LlmMessage::user("no picture"),
                LlmMessage::user_with_images("and this?", vec![picture(1), picture(2)]),
                LlmMessage::user_with_images("", vec![picture(3)]),
            ],
            tools: vec![],
            model: "m".into(),
        };
        let messages = without_images(request).messages;
        assert_eq!(messages[0], LlmMessage::user("no picture"), "untouched");
        assert_eq!(
            messages[1],
            LlmMessage::user(
                "[image 1×10 omitted: this provider is not set to accept images]\n\n\
                 [image 2×10 omitted: this provider is not set to accept images]\n\nand this?"
            )
        );
        assert_eq!(messages[2], LlmMessage::user("[image 3×10 omitted: this provider is not set to accept images]"));
    }

    #[test]
    fn the_public_trust_store_needs_no_certificate() {
        assert!(provider_for(&config(None), Some(SecretString::from("k"))).is_ok());
    }
}
