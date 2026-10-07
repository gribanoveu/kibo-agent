//! Optional, off-by-default log of what was sent to and received from a
//! provider, for the times an opaque gateway error has to be correlated with
//! the payload that produced it.
//!
//! Off by default because a conversation carries the contents of whatever
//! files the agent read, and writing those to disk is the user's decision, not
//! ours. Enabled through `LlmSettings.debug_logging`; callers pass that flag
//! straight through rather than checking it themselves, so there is one gate
//! rather than one per call site.
//!
//! JSON Lines in `<app dir>/logs/llm.jsonl`, one entry per request or
//! outcome.
//!
//! **What may be logged is fixed by the types.** Every entry's payload is a
//! `ChatRequest`, a `ChatStreamResult` or an error message — none of which can
//! carry a credential, because the API key never reaches them: it lives inside
//! the provider and goes out as an `Authorization` header the domain types
//! never see. Logging the HTTP layer instead of these typed values is what
//! would put the key in the file, which is why this logs the typed values even
//! though they are one step removed from the bytes on the wire.

use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::Serialize;

use crate::domain::llm::{ChatRequest, LlmError};
use crate::infra::app_dir;

const FILE: &str = "llm.jsonl";

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct LogEntry<'a> {
    ts: u128,
    provider_id: &'a str,
    round: u32,
    direction: &'static str,
    payload: serde_json::Value,
}

fn log_path() -> Result<PathBuf, String> {
    Ok(app_dir::dir()?.join("logs").join(FILE))
}

/// The request about to be sent for one round. No-op when disabled.
pub fn log_request(enabled: bool, provider_id: &str, round: u32, request: &ChatRequest) {
    if !enabled {
        return;
    }
    let Ok(path) = log_path() else { return };
    append(&path, provider_id, round, "request", &without_pixels(request));
}

/// Each picture's base64 replaced by its length. Every round resends every
/// picture of the conversation, so a turn of ten rounds over five screenshots
/// would write tens of megabytes here — and a gateway's complaint is matched
/// against a payload by its shape, its type and size, not by its pixels. The
/// picture itself is in the chat, sanitized.
fn without_pixels(request: &ChatRequest) -> ChatRequest {
    let mut request = request.clone();
    for image in request.messages.iter_mut().flat_map(|m| m.images.iter_mut()) {
        image.data = format!("<{} bytes of base64>", image.data.len());
    }
    request
}

/// What that round produced — the result, or the error's message, which for
/// an HTTP status already carries the provider's own explanation.
///
/// Generic over the response type because a round is not the only thing that
/// talks to the provider: the compaction pass sends an ordinary, unstreamed
/// request, and a log that omits the request which rewrote the history cannot
/// explain what the model was reading afterwards.
pub fn log_response<T: Serialize>(
    enabled: bool,
    provider_id: &str,
    round: u32,
    result: &Result<T, LlmError>,
) {
    if !enabled {
        return;
    }
    let Ok(path) = log_path() else { return };
    match result {
        Ok(response) => append(&path, provider_id, round, "response", response),
        Err(error) => append(&path, provider_id, round, "error", &error.to_string()),
    }
}

/// Best-effort throughout: a full disk or a read-only directory must never
/// take down a real turn, so every fallible step drops the entry instead of
/// propagating. `path` is a parameter so tests write somewhere disposable.
fn append<T: Serialize>(
    path: &Path,
    provider_id: &str,
    round: u32,
    direction: &'static str,
    payload: &T,
) {
    let Some(dir) = path.parent() else { return };
    if std::fs::create_dir_all(dir).is_err() {
        return;
    }
    let Ok(payload) = serde_json::to_value(payload) else {
        return;
    };
    let Ok(mut file) = OpenOptions::new().create(true).append(true).open(path) else {
        return;
    };
    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    let entry = LogEntry {
        ts,
        provider_id,
        round,
        direction,
        payload,
    };
    if let Ok(line) = serde_json::to_string(&entry) {
        let _ = writeln!(file, "{line}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::llm::ChatStreamResult;
    use crate::domain::llm::LlmMessage;
    use crate::testing::{temp_dir, with_app_dir};

    fn request() -> ChatRequest {
        ChatRequest {
            messages: vec![LlmMessage::user("hi")],
            tools: vec![],
            model: "qwen".to_string(),
        }
    }

    fn lines(path: &Path) -> Vec<serde_json::Value> {
        std::fs::read_to_string(path)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect()
    }

    #[test]
    fn an_entry_lands_as_one_json_line_in_a_created_directory() {
        let path = temp_dir("debug-log").join("logs").join(FILE);
        append(&path, "local", 1, "request", &request());

        let entries = lines(&path);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0]["providerId"], "local");
        assert_eq!(entries[0]["round"], 1);
        assert_eq!(entries[0]["direction"], "request");
        assert_eq!(entries[0]["payload"]["model"], "qwen");
    }

    #[test]
    fn rounds_append_in_order() {
        let path = temp_dir("debug-log-order").join(FILE);
        append(&path, "local", 1, "request", &request());
        append(&path, "local", 2, "request", &request());

        let entries = lines(&path);
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0]["round"], 1);
        assert_eq!(entries[1]["round"], 2);
    }

    #[test]
    fn a_failed_round_logs_the_providers_own_message() {
        with_app_dir("debug-log-error", || {
            let result: Result<ChatStreamResult, LlmError> =
                Err(LlmError::Provider("400: model not found".to_string()));
            log_response(true, "local", 3, &result);

            let entries = lines(&log_path().unwrap());
            assert_eq!(entries[0]["direction"], "error");
            assert_eq!(entries[0]["round"], 3);
            assert_eq!(
                entries[0]["payload"],
                "provider error: 400: model not found",
                "the provider's own explanation is what makes the entry worth having"
            );
        });
    }

    /// The gate, exercised through the real entry point and against the real
    /// log path: disabled must write nothing at all, not an empty file.
    #[test]
    fn disabled_writes_nothing() {
        with_app_dir("debug-log-off", || {
            log_request(false, "local", 1, &request());
            log_response::<ChatStreamResult>(
                false,
                "local",
                1,
                &Err(LlmError::Http("boom".to_string())),
            );

            assert!(!log_path().unwrap().exists());
        });
    }

    /// A picture is logged by its type and size, never by its pixels.
    #[test]
    fn a_picture_is_logged_without_its_pixels() {
        use crate::domain::image::{ImageMediaType, ImagePart};
        with_app_dir("debug-log-pixels", || {
            let picture = ImagePart { media_type: ImageMediaType::Png, data: "PIXELS".repeat(1000), width: 40, height: 30 };
            let mut sent = request();
            sent.messages.push(crate::domain::llm::LlmMessage::user_with_images("look", vec![picture]));
            log_request(true, "local", 1, &sent);

            let line = std::fs::read_to_string(log_path().unwrap()).unwrap();
            assert!(!line.contains("PIXELS"), "{line}");
            let payload = lines(&log_path().unwrap()).remove(0)["payload"].clone();
            let logged = &payload["messages"].as_array().unwrap().last().unwrap()["images"][0];
            assert_eq!(logged["data"], "<6000 bytes of base64>");
            assert_eq!((logged["mediaType"].as_str(), logged["width"].as_u64()), (Some("image/png"), Some(40)));
            assert_eq!(sent.messages.last().unwrap().images[0].data.len(), 6000, "the request itself is untouched");
        });
    }

    #[test]
    fn enabled_writes_to_the_app_directory() {
        with_app_dir("debug-log-on", || {
            log_request(true, "local", 1, &request());

            assert_eq!(lines(&log_path().unwrap()).len(), 1);
        });
    }

    /// The payload is a `ChatRequest` and nothing else. A later change that
    /// logs the provider configuration or the outgoing headers alongside it is
    /// how the API key ends up in this file, and it fails here first.
    ///
    /// Through `log_request`, not `append`: the entry point is what a future
    /// edit changes, and a test that assembles the entry itself would keep
    /// passing while the real one started carrying the key.
    #[test]
    fn a_logged_request_carries_only_the_request() {
        with_app_dir("debug-log-shape", || {
            log_request(true, "local", 1, &request());

            let payload = lines(&log_path().unwrap()).remove(0)["payload"].clone();
            let mut fields: Vec<String> = payload
                .as_object()
                .unwrap_or_else(|| panic!("the payload is no longer a request: {payload}"))
                .keys()
                .cloned()
                .collect();
            fields.sort();
            assert_eq!(fields, ["messages", "model", "tools"]);
        });
    }
}
