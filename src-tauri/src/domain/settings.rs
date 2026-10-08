//! What the app persists between runs, and the errors that reading it can
//! produce.
//!
//! One file, `settings.json`, under `infra::app_dir`. Never any secret: an
//! API key lives sealed in `infra::llm_credentials_store`, keyed by the
//! provider id stored here. The split is what makes `settings.json` safe to
//! read, diff, back up and paste into a bug report.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// The context window assumed for a provider that never set one — large
/// enough for today's frontier models, so compaction runs rather than
/// staying off until someone looks for the setting.
pub const DEFAULT_CONTEXT_LIMIT: u32 = 260_000;

/// One configured LLM provider.
///
/// Unlike its counterpart in Alfa Atlas, every entry here is a complete
/// definition rather than an override of a compiled-in preset — this build
/// ships no presets (see `docs/06-port-plan.md`, F-1.12c), so `base_url` is a
/// plain `String` and a half-filled entry simply does not exist yet.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderConfig {
    pub id: String,
    /// Which wire protocol the endpoint speaks. Absent in every file written
    /// before Anthropic's was supported, and those were all OpenAI-compatible.
    #[serde(default)]
    pub kind: ProviderKind,
    pub base_url: String,
    /// Where the model list is asked for. `None` is `{base_url}/models`; set
    /// for an endpoint that lists them elsewhere — DeepSeek's Anthropic one
    /// answers `/messages` but lists its models only at the host's `/models`.
    #[serde(default)]
    pub models_url: Option<String>,
    /// The model to send. `None` means "whichever the provider lists first",
    /// resolved once and then written back here — see
    /// `services::llm_session::effective_model`.
    #[serde(default)]
    pub model: Option<String>,
    /// A certificate authority to trust *instead of* the public roots, for a
    /// gateway that sits behind a corporate CA. Without this the app is
    /// unusable in exactly the environment it was built for.
    #[serde(default)]
    pub trusted_cert_pem: Option<String>,
    /// Extra headers on every request to this provider — a gateway's own
    /// tracing or routing headers. `$uuid` as a value is replaced per request
    /// (see `infra::llm_providers::openai_compatible`).
    #[serde(default)]
    pub request_headers: HashMap<String, String>,
    /// `None` sends the parameter not at all, which is the only safe default
    /// for an endpoint we know nothing about: some reasoning models reject
    /// any temperature but their own, and a gateway that has never heard of
    /// `reasoning_effort` rejects the whole request rather than ignoring the
    /// field.
    #[serde(default)]
    pub temperature: Option<f32>,
    /// Nucleus sampling. `None` sends nothing, as with `temperature` — and
    /// several Anthropic models refuse a request that sets both.
    #[serde(default)]
    pub top_p: Option<f32>,
    #[serde(default)]
    pub max_tokens: Option<u32>,
    /// How big this model's context window is, in tokens. `None` means it
    /// was never set, and the session then assumes `DEFAULT_CONTEXT_LIMIT`
    /// (see `services::llm_session::resolve`).
    #[serde(default)]
    pub context_limit: Option<u32>,
    /// How hard a reasoning model should think — `"low"`/`"medium"`/`"high"`
    /// as that gateway spells it. A free string on purpose: gateways disagree
    /// on the vocabulary, and a value this app has never heard of must still
    /// be sendable without a rebuild.
    #[serde(default)]
    pub reasoning_effort: Option<String>,
    /// Whether the model is sent the pictures the user attaches. Off unless
    /// the user turns it on: a model that cannot see refuses a request with
    /// a picture in it, and every later request of that chat with it. Off,
    /// each picture goes as a line saying it was left out.
    #[serde(default)]
    pub supports_images: bool,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ProviderKind {
    /// `/chat/completions` — OpenAI itself and nearly every gateway.
    #[default]
    OpenAiCompatible,
    /// Anthropic's Messages API, `/messages`.
    Anthropic,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct LlmSettings {
    /// Which provider a turn uses. `None` falls back to the first configured
    /// one, so a single-provider setup needs no choice at all.
    pub active_provider_id: Option<String>,
    /// A `Vec`, not a map, so the file stays diffable and the order the user
    /// sees is the order they wrote. Lookup is linear over a handful of
    /// entries.
    pub providers: Vec<ProviderConfig>,
    /// Off by default: a conversation carries the contents of whatever files
    /// the agent read, and that is not something to write to disk unasked.
    /// See `infra::llm_debug_log`.
    pub debug_logging: bool,
    /// What the model writes its replies in.
    pub reply_language: ReplyLanguage,
    /// How far one agent turn may run before it stops and asks to continue.
    pub turn_limits: TurnLimits,
}

/// The ceilings of one agent turn's tool loop, raised by a user who knows a
/// task needs more — a long refactor, a big review — and lowered to keep a
/// model on a short leash.
///
/// Two of them, and the second is not a duplicate of the first: `budget` is
/// counted in `ToolName::loop_weight` units, so sixty cheap reads and sixty
/// repository-wide searches are not the same amount of work, and it is the
/// one that binds in practice. `rounds` is the backstop beside it — a tool
/// whose weight is misconfigured to zero would otherwise make the loop
/// unstoppable.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct TurnLimits {
    /// Model↔tool round trips.
    pub rounds: u32,
    /// Weighted tool calls.
    pub budget: u32,
}

impl Default for TurnLimits {
    fn default() -> Self {
        // Two weeks of real chats (2026-10-08): half the turns took 5 rounds,
        // the heaviest — documenting a repository — 71 rounds and ~410 budget.
        // 60/250 stopped those mid-work; this keeps a margin over them and
        // still ends a model going round in circles.
        Self { rounds: 100, budget: 500 }
    }
}

/// The language the model is told to write in — answers, plans, findings,
/// summaries. `Auto` tells it nothing: it answers in the language it is
/// written to, and in English where nothing was written, as in `/review`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ReplyLanguage {
    #[default]
    Auto,
    English,
    Russian,
}

impl ReplyLanguage {
    /// The language's name as the prompt says it; `None` for `Auto`.
    pub fn name(self) -> Option<&'static str> {
        match self {
            ReplyLanguage::Auto => None,
            ReplyLanguage::English => Some("English"),
            ReplyLanguage::Russian => Some("Russian"),
        }
    }
}

impl LlmSettings {
    pub fn provider(&self, id: &str) -> Option<&ProviderConfig> {
        self.providers.iter().find(|p| p.id == id)
    }

    /// The provider to use when the caller has no opinion: the active one, or
    /// the only sensible default when nothing is pinned.
    pub fn active(&self) -> Option<&ProviderConfig> {
        match &self.active_provider_id {
            // A pin naming a provider that has since been removed resolves to
            // nothing, rather than silently to some other provider — the key,
            // the model and the endpoint would all be someone else's.
            Some(id) => self.provider(id),
            None => self.providers.first(),
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct AppSettings {
    pub llm: LlmSettings,
    /// Skills switched off, by name.
    pub skills: OptOut,
    /// Skills folders not read at all, by id: `project`, `app`, `agents`,
    /// `claude` — see `services::skills::SOURCES`.
    pub skill_sources: OptOut,
    /// Project instruction files switched off, by canonical path — per
    /// file rather than per name, so turning off one repository's
    /// `AGENTS.md` leaves every other repository's alone.
    pub rules: OptOut,
    /// Whether to trust each folder the user was asked about: its own skills
    /// and `/` commands are read only once trusted.
    pub folder_trust: FolderTrust,
    pub tool_log: ToolLogSettings,
    pub approval: ApprovalMemory,
    /// Chat mode's Kubernetes role: which kubeconfig files it knows of.
    pub kube: KubeSettings,
}

/// The user's answer per folder, by canonical path. A repository's skills and
/// commands are text its author wrote for the agent — a skill can tell it to
/// run a script of the repository's — so a folder cloned from somewhere is not
/// read until the user says it may be. As pi does
/// (`docs/27-pi-ideas.md`, item 5), a decision covers the folders inside it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct FolderTrust(std::collections::BTreeMap<String, bool>);

impl FolderTrust {
    /// The answer for `folder`: its own, or that of the nearest folder above it
    /// with one. `None` — never asked.
    pub fn decision(&self, folder: &std::path::Path) -> Option<bool> {
        folder.ancestors().find_map(|dir| self.0.get(&dir.display().to_string()).copied())
    }

    pub fn set(&mut self, folder: &std::path::Path, trusted: bool) {
        self.0.insert(folder.display().to_string(), trusted);
    }
}

/// The kubeconfig files the Kubernetes role can be pointed at — by path, never
/// by contents. The file stays where kubectl keeps it, with its credentials and
/// whatever refreshes them, and this file stays free of secrets.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct KubeSettings {
    pub configs: Vec<Kubeconfig>,
    /// The last one picked on a chat's tab, by name — where a new chat
    /// starts. `None` falls back to the first, as the active provider does.
    pub active: Option<String>,
    /// Namespaces typed on a chat's tab, per kubeconfig, newest first: a
    /// namespace the identity may work in but not list is typed once.
    #[serde(default)]
    pub typed_namespaces: std::collections::BTreeMap<String, Vec<String>>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Kubeconfig {
    /// What the user calls the cluster: `prod`, `staging`.
    pub name: String,
    pub path: String,
    /// The user's mark that this is a production cluster: every change there
    /// asks, whatever "Always allow" says, and its card says where it lands.
    #[serde(default)]
    pub production: bool,
}

/// How many typed namespaces a kubeconfig remembers.
pub const TYPED_NAMESPACES: usize = 10;

impl KubeSettings {
    /// Puts `namespace` first among `kubeconfig`'s typed ones.
    pub fn remember_namespace(&mut self, kubeconfig: &str, namespace: &str) {
        let typed = self.typed_namespaces.entry(kubeconfig.to_string()).or_default();
        typed.retain(|n| n != namespace);
        typed.insert(0, namespace.to_string());
        typed.truncate(TYPED_NAMESPACES);
    }

    /// The one in use: the picked one, or the first when none is picked. A pick
    /// naming a removed file is none, not some other cluster.
    pub fn active(&self) -> Option<&Kubeconfig> {
        match &self.active {
            Some(name) => self.configs.iter().find(|c| &c.name == name),
            None => self.configs.first(),
        }
    }

    /// Adds `config`, or replaces the one of that name where it stands —
    /// its path; the production mark is kept: a file moved is the same cluster.
    pub fn upsert(&mut self, config: Kubeconfig) {
        match self.configs.iter_mut().find(|c| c.name == config.name) {
            Some(existing) => existing.path = config.path,
            None => self.configs.push(config),
        }
    }

    /// Removes it, and the pick and its typed namespaces with it: the next
    /// falls back to the first.
    pub fn remove(&mut self, name: &str) {
        self.configs.retain(|c| c.name != name);
        self.typed_namespaces.remove(name);
        if self.active.as_deref() == Some(name) {
            self.active = None;
        }
    }
}

/// Where the composer's Ask/Auto is remembered. A chat or folder not listed
/// asks: Auto is what has to be chosen, never what is fallen into.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ApprovalMemory {
    pub remember: RememberScope,
    /// Chats left in Auto, by id. A deleted chat's id costs nothing.
    pub auto_chats: Vec<String>,
    /// Folders every chat of which runs in Auto, by the path the chats are
    /// saved under.
    pub auto_folders: Vec<String>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum RememberScope {
    /// Each chat keeps its own; a new chat starts in Ask.
    #[default]
    Chat,
    /// One choice for every chat in the open folder.
    Repository,
}

impl ApprovalMemory {
    /// Whether `chat` in `folder` runs in Auto. A chat not saved yet has no
    /// id, and under `Chat` asks.
    pub fn is_auto(&self, chat: Option<&str>, folder: &str) -> bool {
        match self.remember {
            RememberScope::Chat => chat.is_some_and(|id| self.auto_chats.iter().any(|c| c == id)),
            RememberScope::Repository => self.auto_folders.iter().any(|f| f == folder),
        }
    }

    /// Records the choice where the scope keeps it. `false` when there is
    /// nowhere yet — a chat with no id — so the caller can record it once
    /// there is.
    pub fn set_auto(&mut self, chat: Option<&str>, folder: &str, auto: bool) -> bool {
        let (list, key) = match (self.remember, chat) {
            (RememberScope::Chat, Some(id)) => (&mut self.auto_chats, id),
            (RememberScope::Chat, None) => return false,
            (RememberScope::Repository, _) => (&mut self.auto_folders, folder),
        };
        list.retain(|k| k != key);
        if auto {
            list.push(key.to_string());
        }
        true
    }
}

/// The tool-call log. On unless switched off: it keeps no file content, so
/// there is nothing in it to be careful about by default (CA-12.1).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ToolLogSettings {
    pub enabled: bool,
}

impl Default for ToolLogSettings {
    fn default() -> Self {
        ToolLogSettings { enabled: true }
    }
}

/// Things that are on until the user says otherwise: a skill dropped into
/// the folder, an `AGENTS.md` in a repository. Only the exceptions are
/// stored, and one whose thing is gone costs nothing.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct OptOut {
    pub disabled: Vec<String>,
}

impl OptOut {
    pub fn is_enabled(&self, key: &str) -> bool {
        !self.disabled.iter().any(|k| k == key)
    }

    pub fn set_enabled(&mut self, key: &str, enabled: bool) {
        self.disabled.retain(|k| k != key);
        if !enabled {
            self.disabled.push(key.to_string());
        }
    }
}

#[derive(Debug, Error)]
pub enum SettingsError {
    #[error("{0}")]
    AppDir(String),
    #[error("failed to read settings: {0}")]
    Read(#[source] std::io::Error),
    #[error("failed to write settings: {0}")]
    Write(String),
    #[error("settings.json is not valid: {0}")]
    Parse(#[source] serde_json::Error),
    #[error("failed to serialize settings: {0}")]
    Serialize(#[source] serde_json::Error),
}

#[cfg(test)]
mod tests {
    use super::*;

    fn provider(id: &str) -> ProviderConfig {
        ProviderConfig {
            id: id.to_string(),
            base_url: format!("https://{id}.example/v1"),
            ..Default::default()
        }
    }

    #[test]
    fn auto_is_remembered_per_chat_or_per_folder_and_asks_otherwise() {
        let mut memory = ApprovalMemory::default();
        assert!(!memory.is_auto(Some("c1"), "/repo"), "nothing chosen asks");
        assert!(!memory.set_auto(None, "/repo", true), "an unsaved chat has nowhere to keep it");
        assert!(memory.set_auto(Some("c1"), "/repo", true));
        memory.set_auto(Some("c1"), "/repo", true);
        assert_eq!(memory.auto_chats, ["c1"], "once");
        assert!(memory.is_auto(Some("c1"), "/repo"));
        assert!(!memory.is_auto(Some("c2"), "/repo"), "another chat asks");
        assert!(!memory.is_auto(None, "/repo"), "a new chat asks");

        memory.remember = RememberScope::Repository;
        assert!(!memory.is_auto(Some("c1"), "/repo"), "the chat's choice is not the folder's");
        assert!(memory.set_auto(None, "/repo", true));
        assert!(memory.is_auto(None, "/repo") && memory.is_auto(Some("c9"), "/repo"));
        assert!(!memory.is_auto(Some("c1"), "/other"));

        memory.set_auto(Some("c1"), "/repo", false);
        assert!(memory.auto_folders.is_empty(), "back to Ask leaves no trace");
        memory.remember = RememberScope::Chat;
        assert!(memory.is_auto(Some("c1"), "/repo"), "each scope keeps its own list");
    }

    #[test]
    fn a_thing_is_on_until_switched_off_and_back_on_leaves_no_trace() {
        let mut settings = OptOut::default();
        assert!(settings.is_enabled("release"));
        settings.set_enabled("release", false);
        settings.set_enabled("release", false);
        assert!(!settings.is_enabled("release"));
        assert!(settings.is_enabled("review"));
        assert_eq!(settings.disabled, ["release"]);
        settings.set_enabled("release", true);
        assert!(settings.disabled.is_empty());
    }

    /// A default of `false` from `#[derive(Default)]` would switch the log
    /// off for everyone whose settings predate it.
    #[test]
    fn the_tool_log_is_on_when_nothing_says_otherwise() {
        assert!(AppSettings::default().tool_log.enabled);
        let old: AppSettings = serde_json::from_str(r#"{"llm":{},"toolLog":{}}"#).unwrap();
        assert!(old.tool_log.enabled);
    }

    #[test]
    fn an_empty_file_is_valid_settings() {
        let settings: AppSettings = serde_json::from_str("{}").unwrap();
        assert_eq!(settings, AppSettings::default());
        assert!(settings.llm.active().is_none());
    }

    #[test]
    fn a_provider_entry_needs_only_an_id_and_a_url() {
        let settings: AppSettings = serde_json::from_str(
            r#"{"llm":{"providers":[{"id":"local","baseUrl":"http://127.0.0.1:1234/v1"}]}}"#,
        )
        .unwrap();
        let active = settings.llm.active().expect("the only provider is active");
        assert_eq!(active.id, "local");
        assert_eq!(active.model, None);
        assert!(active.request_headers.is_empty());
    }

    #[test]
    fn with_nothing_pinned_the_first_provider_is_active() {
        let settings = LlmSettings {
            providers: vec![provider("one"), provider("two")],
            ..Default::default()
        };
        assert_eq!(settings.active().unwrap().id, "one");
    }

    #[test]
    fn a_pin_wins_over_the_order() {
        let settings = LlmSettings {
            active_provider_id: Some("two".to_string()),
            providers: vec![provider("one"), provider("two")],
            ..Default::default()
        };
        assert_eq!(settings.active().unwrap().id, "two");
    }

    /// Falling back to "the first one" here would send the conversation to a
    /// different endpoint, under a different key, without saying so.
    #[test]
    fn a_pin_naming_a_removed_provider_resolves_to_nothing() {
        let settings = LlmSettings {
            active_provider_id: Some("gone".to_string()),
            providers: vec![provider("one")],
            ..Default::default()
        };
        assert!(settings.active().is_none());
    }

    /// The one thing `settings.json` must never contain.
    #[test]
    fn a_provider_entry_has_nowhere_to_put_an_api_key() {
        let written = serde_json::to_string(&ProviderConfig {
            id: "openai".to_string(),
            base_url: "https://api.openai.com/v1".to_string(),
            ..Default::default()
        })
        .unwrap();
        assert!(!written.to_lowercase().contains("key"), "{written}");
    }

    /// Every settings file written before the field existed.
    #[test]
    fn a_provider_without_a_kind_is_openai_compatible() {
        let config: ProviderConfig =
            serde_json::from_str(r#"{"id":"a","baseUrl":"https://x/v1"}"#).unwrap();
        assert_eq!(config.kind, ProviderKind::OpenAiCompatible);
        let anthropic: ProviderConfig =
            serde_json::from_str(r#"{"id":"a","baseUrl":"https://x/v1","kind":"anthropic"}"#).unwrap();
        assert_eq!(anthropic.kind, ProviderKind::Anthropic);
    }

    fn config(name: &str) -> Kubeconfig {
        Kubeconfig { name: name.to_string(), path: format!("/home/me/.kube/{name}"), production: false }
    }

    #[test]
    fn the_first_kubeconfig_is_in_use_until_another_is_picked() {
        let mut kube = KubeSettings::default();
        assert_eq!(kube.active(), None);
        kube.upsert(config("prod"));
        kube.upsert(config("staging"));
        assert_eq!(kube.active().map(|c| c.name.as_str()), Some("prod"));
        kube.active = Some("staging".to_string());
        assert_eq!(kube.active().map(|c| c.name.as_str()), Some("staging"));
        kube.active = Some("gone".to_string());
        assert_eq!(kube.active(), None, "a pick naming a removed file is not some other cluster");
    }

    #[test]
    fn saving_a_kubeconfig_under_its_name_replaces_it_in_place() {
        let mut kube = KubeSettings { configs: vec![config("prod"), config("staging")], ..Default::default() };
        kube.configs[0].production = true;
        kube.upsert(Kubeconfig { name: "prod".to_string(), path: "/elsewhere".to_string(), production: false });
        assert!(kube.configs[0].production, "the mark stays with the name");
        kube.configs[0].production = false;
        assert_eq!(kube.configs.len(), 2);
        assert_eq!(kube.configs[0].path, "/elsewhere");
    }

    #[test]
    fn removing_the_picked_kubeconfig_drops_the_pick() {
        let mut kube = KubeSettings {
            configs: vec![config("prod"), config("staging")],
            active: Some("staging".to_string()),
            ..Default::default()
        };
        kube.remember_namespace("prod", "payments");
        kube.remove("prod");
        assert_eq!(kube.active.as_deref(), Some("staging"), "another's removal keeps the pick");
        assert!(kube.typed_namespaces.is_empty(), "its typed namespaces outlived it");
        kube.remove("staging");
        assert!(kube.configs.is_empty());
        assert_eq!(kube.active, None);
    }

    #[test]
    fn a_typed_namespace_goes_first_once_and_the_oldest_drop_off() {
        let mut kube = KubeSettings::default();
        for n in 0..TYPED_NAMESPACES + 2 {
            kube.remember_namespace("prod", &format!("ns{n}"));
        }
        kube.remember_namespace("prod", "ns5");
        let typed = &kube.typed_namespaces["prod"];
        assert_eq!(typed.len(), TYPED_NAMESPACES);
        assert_eq!(typed[0], "ns5");
        assert_eq!(typed.iter().filter(|n| *n == "ns5").count(), 1);
        assert!(!typed.contains(&"ns0".to_string()), "the oldest stayed");
    }

    #[test]
    fn a_folder_takes_its_own_answer_or_the_nearest_one_above_it() {
        use std::path::Path;
        let mut trust = FolderTrust::default();
        assert_eq!(trust.decision(Path::new("/work/app")), None, "never asked");
        trust.set(Path::new("/work"), true);
        trust.set(Path::new("/work/vendor"), false);
        assert_eq!(trust.decision(Path::new("/work")), Some(true));
        assert_eq!(trust.decision(Path::new("/work/app/src")), Some(true), "inside a trusted one");
        assert_eq!(trust.decision(Path::new("/work/vendor/lib")), Some(false), "the nearest answer wins");
        assert_eq!(trust.decision(Path::new("/elsewhere")), None);
        assert_eq!(trust.decision(Path::new("/workshop")), None, "a sibling with the same prefix is not inside");
        trust.set(Path::new("/work/vendor"), true);
        assert_eq!(trust.decision(Path::new("/work/vendor")), Some(true), "an answer changed is replaced");
    }

    #[test]
    fn the_answers_are_saved_as_a_map_of_paths() {
        let mut settings = AppSettings::default();
        settings.folder_trust.set(std::path::Path::new("/work"), false);
        let json = serde_json::to_value(&settings).unwrap();
        assert_eq!(json["folderTrust"], serde_json::json!({ "/work": false }));
        let back: AppSettings = serde_json::from_value(json).unwrap();
        assert_eq!(back, settings);
    }
}
