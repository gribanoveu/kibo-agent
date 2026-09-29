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
