//! Chat mode: a conversation with the model outside any folder.
//!
//! Not a fourth [`ConversationMode`](super::conversation_mode::ConversationMode).
//! Those are narrower views of one agent working in the open folder; a chat
//! has no folder, so nothing of the agent's — its tools, its rules, its skills
//! — applies. What a chat has instead is a role: who the model is told to be,
//! and the tools that role is given. The tool set is the role's own, never
//! derived from the agent's: a DevOps role that works with Kubernetes gets
//! `kubectl`, not `readFile`.
//!
//! Adding a role is a variant here, its name, description, prompt and tools;
//! the window's role menu lists what [`ChatRole::ALL`] holds.

use serde::{Deserialize, Serialize};

use super::settings::Kubeconfig;
use super::tools::ToolName;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ChatRole {
    /// A general assistant that answers in text.
    #[default]
    Assistant,
    /// A Kubernetes engineer: clusters, manifests, Helm, debugging workloads.
    Kubernetes,
    /// The loop's own check of a role with tools, before any real role has
    /// one: a harmless tool that runs, and one that asks first.
    #[cfg(test)]
    Tester,
}

impl ChatRole {
    pub const ALL: &'static [ChatRole] = &[ChatRole::Assistant, ChatRole::Kubernetes];

    pub fn name(self) -> &'static str {
        match self {
            ChatRole::Assistant => "Assistant",
            ChatRole::Kubernetes => "Kubernetes",
            #[cfg(test)]
            ChatRole::Tester => "Tester",
        }
    }

    /// One line for the role's menu: what the model does in it.
    pub fn description(self) -> &'static str {
        match self {
            ChatRole::Assistant => "Answers in text; no files, commands or tools",
            ChatRole::Kubernetes => "A Kubernetes expert: clusters, manifests, Helm, failing pods",
            #[cfg(test)]
            ChatRole::Tester => "Tests the tool loop",
        }
    }

    /// What the model is told before the conversation.
    pub fn prompt(self) -> &'static str {
        match self {
            ChatRole::Assistant => {
                "You are a helpful assistant in a plain chat. You cannot read the user's files, run commands \
                 or change anything on their machine — answer from what the user writes here. When an answer \
                 depends on code or output you have not been shown, ask for it rather than guessing."
            }
            ChatRole::Kubernetes => KUBERNETES_PROMPT,
            #[cfg(test)]
            ChatRole::Tester => "You test the tool loop.",
        }
    }

    /// What the role is told about the user's own setup, after its prompt —
    /// `None` for a role that needs none. Settled before the conversation and the
    /// same every turn until the setup changes, so a provider's prompt cache keeps it.
    pub fn setup_note(self, kube: &KubeSetup) -> Option<String> {
        match self {
            ChatRole::Assistant => None,
            #[cfg(test)]
            ChatRole::Tester => None,
            ChatRole::Kubernetes => Some(match kube {
                KubeSetup::NotSet => "No kubeconfig is set up in Kibo yet. When the user asks about their own cluster, \
                    say once that they can add their kubeconfig file in Settings → Kubernetes and pick it on the tab above \
                    the message box; general Kubernetes questions need none of that."
                    .to_string(),
                KubeSetup::Missing(config) => format!(
                    "The user picked the kubeconfig \"{}\", but there is no file at {} any more. When they ask about \
                     their cluster, tell them to fix the path in Settings → Kubernetes.",
                    config.name, config.path
                ),
                KubeSetup::Ready(config) => format!(
                    "The user's cluster is \"{}\", its kubeconfig at {}. Commands you give for it name that file — \
                     `kubectl --kubeconfig {} …`, `helm --kubeconfig {} …` — so they reach this cluster and not \
                     whichever context is current. You still cannot run them yourself.",
                    config.name, config.path, config.path, config.path
                ),
            }),
        }
    }

    /// The tools this role may call. None yet: a plain chat only talks.
    pub fn tools(self) -> &'static [ToolName] {
        match self {
            // ponytail: talks only; `kubectl` and friends arrive here as tools once Chat runs them.
            ChatRole::Assistant | ChatRole::Kubernetes => &[],
            // `todo` runs, `deleteFile` asks first — and then finds no folder.
            #[cfg(test)]
            ChatRole::Tester => &[ToolName::Todo, ToolName::DeleteFile],
        }
    }
}

/// The kubeconfig the Kubernetes role works with, as far as the app can tell.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KubeSetup {
    NotSet,
    /// Picked, but its file is gone.
    Missing(Kubeconfig),
    Ready(Kubeconfig),
}

const KUBERNETES_PROMPT: &str = "\
You are a senior Kubernetes engineer (SRE/platform level) in a chat. You know Kubernetes itself — \
workloads, scheduling, networking, storage, RBAC, the control plane — and what surrounds it: kubectl, \
Helm, Kustomize, container images, ingress controllers and service meshes, GitOps with Argo CD or Flux, \
Prometheus and Grafana, and the managed flavours (EKS, GKE, AKS, OpenShift).

You cannot reach the user's cluster, files or terminal: you know only what they paste here. So:
- To diagnose, ask for the output you need and give the exact command that produces it \
  (`kubectl describe pod <name> -n <ns>`, `kubectl logs <pod> --previous`, `kubectl get events -n <ns> \
  --sort-by=.lastTimestamp`). Do not invent cluster state, names or output.
- Work a failure from its symptom to its cause: status and events first (CrashLoopBackOff, \
  ImagePullBackOff, Pending, OOMKilled, failing probes), then logs, then configuration. Say what each \
  step rules out.
- When the version matters (API removals, feature gates, Helm chart values), say which version your \
  answer assumes, or ask.

When you write manifests or commands:
- Give complete, valid YAML in fenced blocks, with the apiVersion current for supported Kubernetes \
  releases, and the namespace explicit.
- Default to production practice: resource requests and limits, liveness/readiness probes, a non-root \
  securityContext, least-privilege RBAC, Secrets not baked into images or ConfigMaps, labels that match \
  their selectors. Mention a default you left out and why.
- Mark any command that changes or deletes something (`delete`, `drain`, `apply --force`, `helm \
  uninstall`, `rollout restart` in production) as such, say what it affects, and give a dry run \
  (`--dry-run=server`, `kubectl diff`, `helm diff`) or a way back when there is one.

Keep answers direct: the likely cause or the recommended approach first, then the steps. For a \
question outside Kubernetes and its ecosystem, answer briefly and say it is outside your focus.";

#[cfg(test)]
mod tests {
    use super::*;

    /// The promise of Chat mode as it ships: no role reaches the user's files.
    #[test]
    fn no_role_has_tools_yet() {
        for role in ChatRole::ALL {
            assert!(role.tools().is_empty(), "{role:?}");
        }
    }

    /// The window sends the role by this name; a rename is a role it can no longer pick.
    #[test]
    fn the_wire_name_is_the_one_the_window_sends() {
        assert_eq!(serde_json::to_string(&ChatRole::Assistant).unwrap(), "\"assistant\"");
        assert_eq!(serde_json::to_string(&ChatRole::Kubernetes).unwrap(), "\"kubernetes\"");
        assert_eq!(ChatRole::default(), ChatRole::Assistant);
    }

    fn prod() -> Kubeconfig {
        Kubeconfig { name: "prod".to_string(), path: "/home/me/.kube/prod".to_string() }
    }

    #[test]
    fn only_the_kubernetes_role_is_told_about_the_kubeconfig() {
        assert_eq!(ChatRole::Assistant.setup_note(&KubeSetup::Ready(prod())), None);
        let ready = ChatRole::Kubernetes.setup_note(&KubeSetup::Ready(prod())).unwrap();
        assert!(ready.contains("\"prod\"") && ready.contains("--kubeconfig /home/me/.kube/prod"), "{ready}");
    }

    /// Each state tells the model something different to say — the one thing
    /// an unconfigured role is there for.
    #[test]
    fn a_missing_or_absent_kubeconfig_sends_the_user_to_settings() {
        let not_set = ChatRole::Kubernetes.setup_note(&KubeSetup::NotSet).unwrap();
        assert!(not_set.contains("No kubeconfig") && not_set.contains("Settings → Kubernetes"), "{not_set}");
        let missing = ChatRole::Kubernetes.setup_note(&KubeSetup::Missing(prod())).unwrap();
        assert!(missing.contains("no file at /home/me/.kube/prod") && missing.contains("Settings → Kubernetes"), "{missing}");
    }
}
