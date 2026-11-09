//! Helpers shared by unit tests across modules. Compiled only under `cfg(test)`.

use std::fs::File;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

/// A fresh directory under this test run's folder, unique per call.
///
/// No cleanup guard: a test that fails leaves its directory behind on purpose —
/// on a filesystem test that is usually the only evidence of what went wrong.
/// It stays until the next test run starts, which removes the folders of every
/// run that is over (see `run_root`).
pub fn temp_dir(label: &str) -> PathBuf {
    use std::sync::atomic::{AtomicU32, Ordering};
    static N: AtomicU32 = AtomicU32::new(0);

    let n = N.fetch_add(1, Ordering::Relaxed);
    let dir = run_root().join(format!("{label}-{n}"));
    std::fs::create_dir_all(&dir).expect("temp dir is creatable");
    dir
}

/// `kibo-tests/<pid>-<nanos>` under the system temp dir: this process's folder,
/// holding a locked `.lock` for as long as the process lives.
///
/// The first call sweeps the folders whose lock nobody holds — runs that ended,
/// passed or crashed, since the OS drops a lock with its process. Without it each
/// `cargo test` left a thousand folders behind, and a mutation run one set per mutant.
fn run_root() -> &'static Path {
    static ROOT: OnceLock<(PathBuf, File)> = OnceLock::new();
    &ROOT
        .get_or_init(|| {
            let base = std::env::temp_dir().join("kibo-tests");
            for entry in std::fs::read_dir(&base).into_iter().flatten().flatten() {
                // No `.lock` yet: a run between creating its folder and locking it.
                let Ok(lock) = File::open(entry.path().join(".lock")) else { continue };
                if lock.try_lock().is_ok() {
                    drop(lock);
                    let _ = std::fs::remove_dir_all(entry.path());
                }
            }
            let nanos = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("clock is after the epoch")
                .as_nanos();
            let root = base.join(format!("{}-{nanos}", std::process::id()));
            std::fs::create_dir_all(&root).expect("test run dir is creatable");
            let lock = File::create(root.join(".lock")).expect("test run lock is creatable");
            lock.lock().expect("test run lock is takeable");
            (root, lock)
        })
        .0
}

/// Runs `f` against a throwaway app directory with a working keychain, holding
/// the app-directory lock for the whole call.
///
/// Everything under `infra::app_dir` — settings, sealed credentials, logs — is
/// process-global state, so tests that touch it cannot run beside each other.
/// One helper rather than a copy per store, for the reason given on
/// `app_dir::test_support`.
pub fn with_app_dir<T>(label: &str, f: impl FnOnce() -> T) -> T {
    let _guard = crate::infra::app_dir::test_support::lock();
    crate::infra::app_dir::test_support::install(temp_dir(label));
    crate::infra::master_key::install_working_keychain_for_tests();
    f()
}

/// The event-stream body the spending stub answers every request with:
/// `reply` as the content, then a usage of `prompt` and `completion` tokens.
pub fn spending_reply(reply: &str, prompt: u32, completion: u32) -> String {
    let content = serde_json::json!({ "choices": [{ "delta": { "content": reply } }] });
    let usage = serde_json::json!({
        "choices": [],
        "usage": { "prompt_tokens": prompt, "completion_tokens": completion, "total_tokens": prompt + completion },
    });
    format!("data: {content}\n\ndata: {usage}\n\ndata: [DONE]\n\n")
}

/// Points the settings' only provider at a server of the test's own on
/// `port`, with a key for it. For tests that hold the provider's answer —
/// a turn mid-flight is theirs to release.
pub fn spending_provider_on(port: u16) {
    let mut settings = crate::infra::settings_store::load().expect("settings");
    settings.llm.providers = vec![crate::domain::settings::ProviderConfig {
        id: "local".into(),
        base_url: format!("http://127.0.0.1:{port}/v1"),
        model: Some("qwen".into()),
        ..Default::default()
    }];
    crate::infra::settings_store::save(&settings).expect("saves");
    crate::infra::llm_credentials_store::save_api_key("local", "sk").expect("key");
}

/// Makes `local` the only provider, served by a thread on this machine that
/// answers every request with `reply` and a usage of `prompt` and `completion`
/// tokens. Call it inside `with_app_dir`: it writes the settings and the key.
/// For tests of what a call is counted under — none of them reaches a network.
pub fn spending_provider(reply: &str, prompt: u32, completion: u32) {
    spending_provider_on(crate::infra::llm_providers::stub_server(spending_reply(reply, prompt, completion)));
}

/// What `chat` has been counted as spending, every day together.
pub fn chat_tokens(chat: &str) -> u64 {
    use crate::domain::metrics::Metric;
    crate::infra::daily_metrics::read(&[Metric::ChatTokens])
        .expect("metrics")
        .into_iter()
        .filter(|row| row.key == chat)
        .map(|row| row.value)
        .sum()
}

/// A model that answers a chat turn from a script, one reply per request: for the
/// checks of a live test's or a bench's check, which must pass the right turn and
/// fail the wrong ones without a provider.
pub fn scripted_session(replies: Vec<crate::domain::llm::ChatStreamResult>) -> crate::services::llm_session::LlmSession {
    use crate::domain::llm::{ChatRequest, ChatResponse, ChatStreamResult, LlmError, LlmModelInfo, LlmProvider, StreamEvent};
    use std::collections::VecDeque;
    use std::sync::Mutex;

    struct Scripted(Mutex<VecDeque<ChatStreamResult>>);

    impl LlmProvider for Scripted {
        fn chat(&self, _: ChatRequest) -> Result<ChatResponse, LlmError> {
            unreachable!("a chat turn is streamed")
        }

        fn chat_stream(&self, _: ChatRequest, _: &dyn Fn(StreamEvent), _: &dyn Fn() -> bool) -> Result<ChatStreamResult, LlmError> {
            Ok(self.0.lock().unwrap().pop_front().expect("a reply left in the script"))
        }

        fn list_models(&self) -> Result<Vec<LlmModelInfo>, LlmError> {
            unreachable!("a chat turn never lists models")
        }
    }

    crate::services::llm_session::LlmSession {
        provider: std::sync::Arc::new(Scripted(Mutex::new(replies.into()))),
        provider_id: "test".into(),
        model: "m".into(),
        debug_logging: false,
        context_limit: None,
        reply_language: None,
        limits: Default::default(),
    }
}
