//! Which cluster a chat of the Kubernetes role works with, and what the app
//! found there — the part of `docs/21-kubernetes-mode.md` (K-2) that is data
//! and rules. Talking to the cluster is `infra::kube_client`'s.

use serde::{Deserialize, Serialize};

use super::settings::Kubeconfig;

/// Where a chat works: a kubeconfig from Settings by name, one of its
/// contexts, a namespace. Kept with the chat, so switching the namespace in one
/// chat never moves another that was halfway through a diagnosis.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct KubePin {
    pub kubeconfig: String,
    /// `None` is the file's current context, as `kubectl` takes it.
    #[serde(default)]
    pub context: Option<String>,
    /// `None` is the context's own namespace, or `default`.
    #[serde(default)]
    pub namespace: Option<String>,
    /// Whether the chat's tools may change the cluster — "Changes" on its
    /// tab. Off unless the user turned it on, in this chat, this time it is open.
    #[serde(default)]
    pub writes: bool,
}

/// One context of a kubeconfig, as its menu lists it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct KubeContext {
    pub name: String,
    pub cluster: String,
    /// Its own namespace, when it sets one.
    pub namespace: Option<String>,
}

/// A kubeconfig's contexts, and the one it calls current.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct KubeContexts {
    pub contexts: Vec<KubeContext>,
    pub current: Option<String>,
}

/// What went wrong reaching a cluster. Worded for the chip's hint and the
/// model's note, which is where it ends up.
#[derive(Debug, thiserror::Error)]
pub enum KubeError {
    #[error("the kubeconfig could not be read: {0}")]
    Kubeconfig(String),
    #[error("the cluster did not answer within {0} seconds")]
    Timeout(u64),
    #[error("{0}")]
    Cluster(String),
    /// The server's 404, apart from its other answers: an apply asks whether
    /// the object is there, and an undone delete needs it not to be.
    #[error("{0}")]
    NotFound(String),
    /// A kind the server does not serve, by any of its names.
    #[error("the cluster serves no kind called `{0}` — check the spelling, or whether its CRD is installed")]
    UnknownKind(String),
    /// A kind served by more than one API group; which one was meant is the
    /// model's to say.
    #[error("`{asked}` is served by more than one API group — name one of: {options}")]
    AmbiguousKind { asked: String, options: String },
}

/// One kind the cluster serves, as discovery reported it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KubeKind {
    /// Empty for the core group.
    pub group: String,
    pub version: String,
    pub kind: String,
    pub plural: String,
    pub namespaced: bool,
}

impl KubeKind {
    /// A core-group kind every cluster serves, for the tools that need one
    /// by definition — events, pods — without asking discovery.
    pub fn core(kind: &str, plural: &str) -> KubeKind {
        KubeKind { group: String::new(), version: "v1".into(), kind: kind.into(), plural: plural.into(), namespaced: true }
    }

    /// `Deployment.apps`, or `Pod` for the core group — how the model names it
    /// back when a bare name is ambiguous.
    pub fn qualified(&self) -> String {
        if self.group.is_empty() { self.kind.clone() } else { format!("{}.{}", self.kind, self.group) }
    }
}

/// What a list asks for. `namespace: None` is every namespace, or a
/// cluster-scoped kind.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ListQuery {
    pub namespace: Option<String>,
    pub label_selector: Option<String>,
    pub field_selector: Option<String>,
    pub limit: Option<u32>,
    pub continue_token: Option<String>,
}

/// One page of a list: the objects as JSON, and how to ask for the next.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ListPage {
    pub items: Vec<serde_json::Value>,
    pub continue_token: Option<String>,
    /// How many are left after this page, when the server says.
    pub remaining: Option<i64>,
}

/// What a container's log is asked for. Lines always carry the server's
/// timestamps: several pods' logs are merged by them.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LogQuery {
    pub container: Option<String>,
    pub previous: bool,
    pub tail: Option<i64>,
    pub since_seconds: Option<i64>,
}

/// A cluster the chat is pinned to, as the tools read it: its kubeconfig and
/// context already chosen, so no call can name another. Implemented by
/// `infra::kube_client::ClusterApi`; a test's double answers from JSON.
pub trait KubeApi: Send + Sync {
    /// Every kind the server serves that can be listed.
    fn kinds(&self) -> Result<Vec<KubeKind>, KubeError>;
    fn list(&self, kind: &KubeKind, query: &ListQuery) -> Result<ListPage, KubeError>;
    /// `namespace` is ignored for a cluster-scoped kind.
    fn get(&self, kind: &KubeKind, namespace: &str, name: &str) -> Result<serde_json::Value, KubeError>;
    fn logs(&self, namespace: &str, pod: &str, query: &LogQuery) -> Result<String, KubeError>;
    /// Merges `patch` into the object and returns it as the server then has
    /// it. With `dry_run` the server checks and answers, and keeps nothing.
    fn patch(
        &self,
        kind: &KubeKind,
        namespace: &str,
        name: &str,
        patch: &serde_json::Value,
        dry_run: bool,
    ) -> Result<serde_json::Value, KubeError>;
    /// Server-side apply of a whole `object` — created when it is not there —
    /// under the app's name, taking over fields others set: the user has
    /// approved the difference. Returns it as the server then has it.
    fn apply(&self, kind: &KubeKind, namespace: &str, name: &str, object: &serde_json::Value, dry_run: bool) -> Result<serde_json::Value, KubeError>;
    fn delete(&self, kind: &KubeKind, namespace: &str, name: &str, dry_run: bool) -> Result<(), KubeError>;
    /// Shows `seen` the object as it is, and again after every change to it,
    /// until `seen` says it has seen enough, `timeout` passes or `stop` says
    /// the turn is over. A client that cannot watch shows it once.
    fn watch(
        &self,
        kind: &KubeKind,
        namespace: &str,
        name: &str,
        _timeout: std::time::Duration,
        _stop: &dyn Fn() -> bool,
        seen: &mut dyn FnMut(&serde_json::Value) -> bool,
    ) -> Result<(), KubeError> {
        seen(&self.get(kind, namespace, name)?);
        Ok(())
    }
}

/// One change a tool made to a cluster, as the audit keeps it
/// (`docs/21-kubernetes-mode.md`, decisions 6 and 9): where, what, and the
/// spec's generation on either side — the undo's test of "nobody else
/// changed it since".
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct KubeChange {
    /// Names the backup, and is what `kubeUndo` takes.
    pub id: String,
    /// RFC 3339, UTC.
    pub at: String,
    pub kubeconfig: String,
    pub context: String,
    pub namespace: String,
    pub group: String,
    pub version: String,
    pub kind: String,
    pub plural: String,
    pub name: String,
    pub tool: String,
    /// What it did, in words: `3 → 0 replicas`.
    pub summary: String,
    pub generation_before: Option<i64>,
    /// `None` until the change ran, and when it failed.
    pub generation_after: Option<i64>,
    /// `metadata.resourceVersion` after it, for a kind with no generation — a
    /// ConfigMap, a Secret, a Service — where any change at all is a change.
    #[serde(default)]
    pub version_after: Option<String>,
    /// Why it failed; `None` when it did not.
    pub error: Option<String>,
    /// The change this one put back, when it is an undo — itself a change,
    /// with a backup, so an undo can be undone.
    #[serde(default)]
    pub undoes: Option<String>,
}

impl KubeChange {
    fn same_object(&self, other: &KubeChange) -> bool {
        let key = |c: &'_ KubeChange| (c.kubeconfig.clone(), c.context.clone(), c.namespace.clone(), c.group.clone(), c.kind.clone(), c.name.clone());
        key(self) == key(other)
    }
}

/// The one change that cannot be put back: by the time it is made, the pods
/// it replaced are gone.
pub const ROLLOUT_RESTART: &str = "kubeRolloutRestart";

/// The merge patch that turns `from` into `to`: what differs, and `null` for
/// what `to` lacks — so a whole section is replaced rather than merged into.
/// `None` when nothing differs. A list is one value, as a merge patch has it.
pub fn merge_diff(from: &serde_json::Value, to: &serde_json::Value) -> Option<serde_json::Value> {
    use serde_json::Value;
    let (Value::Object(from), Value::Object(to)) = (from, to) else {
        return (from != to).then(|| to.clone());
    };
    let mut patch = serde_json::Map::new();
    for (key, wanted) in to {
        let changed = match from.get(key) {
            Some(had) => merge_diff(had, wanted),
            None => Some(wanted.clone()),
        };
        if let Some(changed) = changed {
            patch.insert(key.clone(), changed);
        }
    }
    for key in from.keys().filter(|key| !to.contains_key(*key)) {
        patch.insert(key.clone(), Value::Null);
    }
    (!patch.is_empty()).then_some(Value::Object(patch))
}

/// Whether the change `id` is the one to undo now, by the audit — `history`,
/// oldest first: it happened, and nothing of the app's changed its object
/// after it. Changes of one object come off last to first, as file edits do;
/// an undo is the newest change of its object, so what it undid is no longer
/// the last. Returns the audit's record — the one that knows the generation
/// the change left. What anyone else did since is the generation's to tell.
pub fn undoable<'h>(history: &'h [KubeChange], id: &str) -> Result<&'h KubeChange, String> {
    let change = history.iter().rev().find(|c| c.id == id).ok_or_else(|| format!("the change {id} is not on record"))?;
    if let Some(error) = &change.error {
        return Err(format!("the change {id} was refused by the cluster and changed nothing: {error}"));
    }
    let last = history.iter().rev().find(|c| c.error.is_none() && c.same_object(change)).unwrap_or(change);
    if last.id == change.id {
        return Ok(change);
    }
    Err(if last.undoes.as_deref() == Some(id) {
        format!("the change {id} is already undone, by {} — undoing that one makes the change again", last.id)
    } else if last.tool == ROLLOUT_RESTART {
        format!(
            "{}/{} was restarted after {id} ({}), and a restart cannot be undone — set what is wanted with the changing tools instead",
            change.kind, change.name, last.id
        )
    } else {
        format!("{}/{} was changed again after {id}, by {} ({}) — undo that one first", change.kind, change.name, last.id, last.summary)
    })
}

/// Where changes are recorded. A change that cannot be backed up does not
/// run — the opposite of a file edit's best-effort copy: a cluster is not a
/// working tree, and nothing there changes without a way back on record.
pub trait KubeChanges: Send + Sync {
    /// Keeps `object` — the live one, whole — under the change's id.
    fn backup(&self, change: &KubeChange, object: &serde_json::Value) -> Result<(), String>;
    /// Adds the change, with how it ended, to the audit.
    fn audit(&self, change: &KubeChange) -> Result<(), String>;
    /// The change `id` as it was planned, and the object as it was before it.
    fn load(&self, id: &str) -> Result<(KubeChange, serde_json::Value), String>;
    /// Every change on record, oldest first.
    fn history(&self) -> Result<Vec<KubeChange>, String>;
}

/// What a Kubernetes chat's tools read: its cluster, and the namespace it is
/// pinned to — the default of every call that names none.
#[derive(Clone, Copy)]
pub struct PinnedCluster<'a> {
    pub api: &'a dyn KubeApi,
    pub namespace: &'a str,
    /// The kubeconfig's name in Settings and its context, for the audit
    /// and the approval card.
    pub kubeconfig: &'a str,
    pub context: &'a str,
    /// "Changes" is on for this chat.
    pub writes: bool,
    /// The user marked this kubeconfig as production: said on every card.
    pub production: bool,
    /// `None` where nothing can be recorded — and so nothing changed.
    pub changes: Option<&'a dyn KubeChanges>,
}

/// `kubectl`'s short names, for the kinds that have one — discovery in this
/// client does not carry them.
const SHORT_NAMES: &[(&str, &str)] = &[
    ("po", "pods"),
    ("svc", "services"),
    ("deploy", "deployments"),
    ("rs", "replicasets"),
    ("sts", "statefulsets"),
    ("ds", "daemonsets"),
    ("cj", "cronjobs"),
    ("cm", "configmaps"),
    ("ns", "namespaces"),
    ("no", "nodes"),
    ("ing", "ingresses"),
    ("pvc", "persistentvolumeclaims"),
    ("pv", "persistentvolumes"),
    ("sa", "serviceaccounts"),
    ("hpa", "horizontalpodautoscalers"),
    ("ep", "endpoints"),
    ("ev", "events"),
    ("netpol", "networkpolicies"),
    ("pdb", "poddisruptionbudgets"),
    ("crd", "customresourcedefinitions"),
    ("vs", "virtualservices"),
    ("dr", "destinationrules"),
    ("se", "serviceentries"),
];

/// The kind the model means by `asked`: `Deployment`, `deployments`, `deploy`,
/// or with its group, `Gateway.networking.istio.io`. A name several groups
/// serve is the core group's when it has one — `Event` is `v1` — and
/// otherwise the model is asked to say which.
pub fn resolve_kind<'k>(kinds: &'k [KubeKind], asked: &str) -> Result<&'k KubeKind, KubeError> {
    let asked = asked.trim();
    let (name, group) = match asked.split_once('.') {
        Some((name, group)) => (name.to_lowercase(), Some(group.to_lowercase())),
        None => (asked.to_lowercase(), None),
    };
    let plural = SHORT_NAMES.iter().find(|(short, _)| *short == name).map_or(name.as_str(), |(_, plural)| plural);
    let found: Vec<&KubeKind> = kinds
        .iter()
        .filter(|k| k.kind.to_lowercase() == name || k.plural == name || k.plural == plural)
        .filter(|k| group.as_ref().is_none_or(|g| &k.group == g))
        .collect();
    match found.as_slice() {
        [] => Err(KubeError::UnknownKind(asked.to_string())),
        [one] => Ok(one),
        many => many.iter().find(|k| k.group.is_empty()).copied().ok_or_else(|| KubeError::AmbiguousKind {
            asked: asked.to_string(),
            options: many.iter().map(|k| k.qualified()).collect::<Vec<_>>().join(", "),
        }),
    }
}

/// The namespace `kubectl` would use with no `-n`.
pub const DEFAULT_NAMESPACE: &str = "default";

impl KubeContexts {
    /// The context a pin names, or the current one when it names none.
    pub fn chosen(&self, pin: &KubePin) -> Option<&KubeContext> {
        let name = pin.context.as_ref().or(self.current.as_ref())?;
        self.contexts.iter().find(|c| &c.name == name)
    }
}

impl KubeContext {
    /// The namespace a pin names, or this context's, or `default`.
    pub fn namespace_for(&self, pin: &KubePin) -> String {
        pin.namespace.clone().or_else(|| self.namespace.clone()).unwrap_or_else(|| DEFAULT_NAMESPACE.to_string())
    }
}

/// The kubeconfig the Kubernetes role works with, as far as the app can tell.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KubeSetup {
    NotSet,
    /// Picked, but its file is gone.
    Missing(Kubeconfig),
    /// The file is there and is not a kubeconfig this client can read.
    Unreadable { config: Kubeconfig, reason: String },
    /// The file is there; the context the chat is pinned to is not in it.
    NoContext { config: Kubeconfig, context: String },
    Pinned(KubeTarget),
}

/// The cluster, context and namespace a chat is pinned to, and what the
/// cluster said when asked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KubeTarget {
    pub config: Kubeconfig,
    pub context: String,
    pub cluster: String,
    pub namespace: String,
    pub reach: Reach,
    /// The pin's "Changes": whether a tool may change anything here.
    pub writes: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Reach {
    /// The API server's version, and what this identity may do in the namespace.
    Answered { version: String, access: Access },
    /// Why not: no route, a refused login, a plugin that did not answer.
    Unreachable(String),
}

/// What the kubeconfig's identity may do in the namespace, coarsely — enough
/// for the model to know whether a change it suggests could even be made.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Access {
    /// Allowed to do nothing the review listed — not even read.
    None,
    ReadOnly,
    /// Can create, change or delete something.
    Changes,
    /// Every verb on every resource: an admin's kubeconfig.
    Everything,
}

/// One rule of a `SelfSubjectRulesReview`, as far as [`access_of`] reads it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Rule {
    pub verbs: Vec<String>,
    pub api_groups: Vec<String>,
    pub resources: Vec<String>,
}

const CHANGING: &[&str] = &["create", "update", "patch", "delete", "deletecollection", "*"];
const READING: &[&str] = &["get", "list", "watch", "*"];
/// Everyone may ask what they may do; that is not access to anything.
const ABOUT_ONESELF: &[&str] = &["authorization.k8s.io", "authentication.k8s.io"];

/// Sums up the rules a review returned.
pub fn access_of(rules: &[Rule]) -> Access {
    let has = |list: &[String], word: &str| list.iter().any(|v| v == word);
    let counted: Vec<&Rule> = rules
        .iter()
        .filter(|rule| !rule.resources.is_empty())
        .filter(|rule| rule.api_groups.is_empty() || !rule.api_groups.iter().all(|group| ABOUT_ONESELF.contains(&group.as_str())))
        .collect();
    if counted.iter().any(|rule| has(&rule.verbs, "*") && has(&rule.resources, "*")) {
        return Access::Everything;
    }
    if counted.iter().any(|rule| rule.verbs.iter().any(|v| CHANGING.contains(&v.as_str()))) {
        return Access::Changes;
    }
    if counted.iter().any(|rule| rule.verbs.iter().any(|v| READING.contains(&v.as_str()))) {
        return Access::ReadOnly;
    }
    Access::None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rule(verbs: &[&str], groups: &[&str], resources: &[&str]) -> Rule {
        let own = |list: &[&str]| list.iter().map(|s| s.to_string()).collect();
        Rule { verbs: own(verbs), api_groups: own(groups), resources: own(resources) }
    }

    /// What every identity is given: asking about itself. Not access.
    fn self_review() -> Rule {
        rule(&["create"], &["authorization.k8s.io"], &["selfsubjectaccessreviews", "selfsubjectrulesreviews"])
    }

    fn kind(group: &str, kind: &str, plural: &str) -> KubeKind {
        KubeKind { group: group.into(), version: "v1".into(), kind: kind.into(), plural: plural.into(), namespaced: true }
    }

    fn change(id: &str, name: &str) -> KubeChange {
        KubeChange {
            id: id.into(),
            at: "2026-09-30T10:00:00Z".into(),
            kubeconfig: "prod".into(),
            context: "eks".into(),
            namespace: "orders".into(),
            group: "apps".into(),
            version: "v1".into(),
            kind: "Deployment".into(),
            plural: "deployments".into(),
            name: name.into(),
            tool: "kubeScale".into(),
            summary: "3 → 0 replicas".into(),
            generation_before: Some(4),
            generation_after: Some(5),
            version_after: None,
            error: None,
            undoes: None,
        }
    }

    /// Last to first: only the newest change of an object comes off, and an
    /// undo is itself the newest — undoing it makes the change again.
    #[test]
    fn only_the_last_change_of_an_object_is_undone() {
        let failed = KubeChange { error: Some("conflict".into()), generation_after: None, ..change("kc-4", "api") };
        let elsewhere = KubeChange { namespace: "payments".into(), ..change("kc-5", "api") };
        let history = [change("kc-1", "api"), change("kc-2", "web"), change("kc-3", "api"), failed, elsewhere];
        assert_eq!(undoable(&history, "kc-3").unwrap().id, "kc-3", "a failed change and another namespace's are not after it");
        assert_eq!(undoable(&history, "kc-2").unwrap().id, "kc-2");
        assert!(undoable(&history, "kc-1").unwrap_err().contains("by kc-3 (3 → 0 replicas) — undo that one first"));
        assert!(undoable(&history, "kc-4").unwrap_err().contains("changed nothing: conflict"));
        assert!(undoable(&history, "kc-9").unwrap_err().contains("not on record"));

        let restart = KubeChange { tool: ROLLOUT_RESTART.into(), ..change("kc-7", "api") };
        let history = [change("kc-1", "api"), restart];
        assert!(undoable(&history, "kc-1").unwrap_err().contains("was restarted after kc-1 (kc-7), and a restart cannot be undone"));

        let undo = KubeChange { undoes: Some("kc-2".into()), ..change("kc-6", "web") };
        let history = [change("kc-2", "web"), undo];
        assert!(undoable(&history, "kc-2").unwrap_err().contains("already undone, by kc-6"));
        assert_eq!(undoable(&history, "kc-6").unwrap().id, "kc-6", "an undo is undone like any change");
    }

    /// A rollback replaces the pod template: what the old one lacks goes,
    /// what it had comes back, and a list is swapped whole.
    #[test]
    fn a_merge_diff_takes_out_what_the_target_lacks() {
        use serde_json::json;
        let now = json!({"metadata": {"labels": {"app": "api", "new": "x"}}, "spec": {"containers": [{"image": "api:2"}], "nodeSelector": {"gpu": "yes"}}});
        let was = json!({"metadata": {"labels": {"app": "api"}, "annotations": {"a": "1"}}, "spec": {"containers": [{"image": "api:1"}]}});
        assert_eq!(
            merge_diff(&now, &was).unwrap(),
            json!({"metadata": {"labels": {"new": null}, "annotations": {"a": "1"}}, "spec": {"containers": [{"image": "api:1"}], "nodeSelector": null}})
        );
        assert_eq!(merge_diff(&now, &now), None);
        assert_eq!(merge_diff(&json!(3), &json!(0)), Some(json!(0)));
        assert_eq!(merge_diff(&json!(null), &json!({"a": 1})), Some(json!({"a": 1})));
    }

    #[test]
    fn a_kind_is_found_by_any_of_its_names() {
        let kinds = [kind("apps", "Deployment", "deployments"), kind("", "Pod", "pods")];
        for asked in ["Deployment", "deployment", "deployments", "deploy", "Deployment.apps", " deploy "] {
            assert_eq!(resolve_kind(&kinds, asked).unwrap().kind, "Deployment", "{asked}");
        }
        assert_eq!(resolve_kind(&kinds, "po").unwrap().kind, "Pod");
        assert!(matches!(resolve_kind(&kinds, "Deployment.batch"), Err(KubeError::UnknownKind(_))));
        assert!(matches!(resolve_kind(&kinds, "VirtualService"), Err(KubeError::UnknownKind(_))));
    }

    /// `Event` is core's and `events.k8s.io`'s: core wins, as in `kubectl`.
    /// Two CRDs of one name have no such winner.
    #[test]
    fn a_name_two_groups_serve_is_cores_or_the_models_to_choose() {
        let kinds = [
            kind("events.k8s.io", "Event", "events"),
            kind("", "Event", "events"),
            kind("gateway.networking.k8s.io", "Gateway", "gateways"),
            kind("networking.istio.io", "Gateway", "gateways"),
        ];
        assert_eq!(resolve_kind(&kinds, "events").unwrap().group, "");
        let Err(KubeError::AmbiguousKind { options, .. }) = resolve_kind(&kinds, "Gateway") else { panic!("not ambiguous") };
        assert_eq!(options, "Gateway.gateway.networking.k8s.io, Gateway.networking.istio.io");
        assert_eq!(resolve_kind(&kinds, "Gateway.networking.istio.io").unwrap().group, "networking.istio.io");
    }

    #[test]
    fn asking_about_oneself_is_no_access() {
        assert_eq!(access_of(&[self_review()]), Access::None);
    }

    #[test]
    fn reading_is_read_only_and_one_changing_verb_is_changes() {
        let read = rule(&["get", "list", "watch"], &["", "apps"], &["pods", "deployments"]);
        assert_eq!(access_of(&[self_review(), read.clone()]), Access::ReadOnly);
        let scale = rule(&["patch"], &["apps"], &["deployments/scale"]);
        assert_eq!(access_of(&[self_review(), read, scale]), Access::Changes);
    }

    #[test]
    fn every_verb_on_every_resource_is_everything() {
        assert_eq!(access_of(&[rule(&["*"], &["*"], &["*"])]), Access::Everything);
        // A wildcard verb on named resources is a lot, but not all.
        assert_eq!(access_of(&[rule(&["*"], &["apps"], &["deployments"])]), Access::Changes);
    }

    /// A pin names what it chose; what it leaves out is what `kubectl` would take.
    #[test]
    fn an_empty_pin_is_the_current_context_and_its_namespace() {
        let contexts = KubeContexts {
            contexts: vec![
                KubeContext { name: "prod".into(), cluster: "eks".into(), namespace: Some("payments".into()) },
                KubeContext { name: "stg".into(), cluster: "stg".into(), namespace: None },
            ],
            current: Some("prod".into()),
        };
        let pin = KubePin { kubeconfig: "k".into(), ..Default::default() };
        let chosen = contexts.chosen(&pin).unwrap();
        assert_eq!((chosen.name.as_str(), chosen.namespace_for(&pin).as_str()), ("prod", "payments"));

        let staging = KubePin { context: Some("stg".into()), ..pin.clone() };
        assert_eq!(contexts.chosen(&staging).unwrap().namespace_for(&staging), DEFAULT_NAMESPACE);
        let named = KubePin { namespace: Some("orders".into()), ..staging.clone() };
        assert_eq!(contexts.chosen(&named).unwrap().namespace_for(&named), "orders");
        let gone = KubePin { context: Some("old".into()), ..pin };
        assert_eq!(contexts.chosen(&gone), None);
    }
}
