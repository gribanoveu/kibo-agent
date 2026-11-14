//! The arguments of the Kubernetes role's tools.

use super::*;

/// `kubeList`. `namespace` is the chat's when absent, and `*` is all of them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct KubeListArgs {
    pub kind: String,
    #[serde(default)]
    pub namespace: Option<String>,
    #[serde(default)]
    pub label_selector: Option<String>,
    #[serde(default)]
    pub field_selector: Option<String>,
    /// Paths into each object, shown as extra columns.
    #[serde(default, deserialize_with = "crate::domain::flexible_args::opt_string_list")]
    pub fields: Option<Vec<String>>,
    #[serde(default, deserialize_with = "crate::domain::flexible_args::opt_u32")]
    pub limit: Option<u32>,
    /// The token a page ended with, for the next one.
    #[serde(default, rename = "continue")]
    pub continue_token: Option<String>,
}

/// `kubeGet`: one object, whole or cut to its top-level `sections`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct KubeGetArgs {
    pub kind: String,
    pub name: String,
    #[serde(default)]
    pub namespace: Option<String>,
    #[serde(default, deserialize_with = "crate::domain::flexible_args::opt_string_list")]
    pub sections: Option<Vec<String>>,
}

/// `kubeEvents`: a namespace's, or one object's.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct KubeEventsArgs {
    #[serde(default)]
    pub namespace: Option<String>,
    #[serde(default)]
    pub kind: Option<String>,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default, deserialize_with = "crate::domain::flexible_args::opt_bool")]
    pub warnings_only: Option<bool>,
}

/// `kubeLogs`: a pod, an owner's pods (`kind` and `name`), or a selector's.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct KubeLogsArgs {
    #[serde(default)]
    pub pod: Option<String>,
    #[serde(default)]
    pub kind: Option<String>,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub label_selector: Option<String>,
    #[serde(default)]
    pub namespace: Option<String>,
    #[serde(default)]
    pub container: Option<String>,
    #[serde(default, deserialize_with = "crate::domain::flexible_args::opt_bool")]
    pub previous: Option<bool>,
    #[serde(default, deserialize_with = "crate::domain::flexible_args::opt_u32")]
    pub tail: Option<u32>,
    #[serde(default)]
    pub since: Option<String>,
    #[serde(default)]
    pub grep: Option<String>,
}

/// `kubeTop`: `pods` or `nodes`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct KubeTopArgs {
    pub kind: String,
    #[serde(default)]
    pub namespace: Option<String>,
}

/// `kubeFieldHistory`: who set an object's fields, cut to `paths`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct KubeFieldHistoryArgs {
    pub kind: String,
    pub name: String,
    #[serde(default)]
    pub namespace: Option<String>,
    #[serde(default, deserialize_with = "crate::domain::flexible_args::opt_string_list")]
    pub paths: Option<Vec<String>>,
}

/// `kubeDiagnose`: a workload, or one pod.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct KubeDiagnoseArgs {
    pub kind: String,
    pub name: String,
    #[serde(default)]
    pub namespace: Option<String>,
}

/// `kubeWaitRollout`: a workload in the chat's own namespace, and how long
/// to wait for it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct KubeWaitRolloutArgs {
    pub kind: String,
    pub name: String,
    #[serde(default)]
    pub timeout_seconds: Option<u64>,
}

/// `kubeProbe`: where from — a pod, or a workload for one of its pods, in
/// the chat's own namespace — and what to reach.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct KubeProbeArgs {
    pub kind: String,
    pub name: String,
    #[serde(default)]
    pub container: Option<String>,
    pub target: String,
}

/// `kubeRunbook`: one runbook, by the name the prompt listed it under.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct KubeRunbookArgs {
    pub name: String,
}

/// `kubeScale`: a workload's replicas, in the chat's own namespace — a
/// change is never made in another. `replicas` is optional in the type so a
/// missing number is the tool's error, worded for the model.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct KubeScaleArgs {
    pub kind: String,
    pub name: String,
    #[serde(default, deserialize_with = "crate::domain::flexible_args::opt_u32")]
    pub replicas: Option<u32>,
    /// The user has said losing the data is acceptable: asked for only when
    /// the scale destroys some.
    #[serde(default, deserialize_with = "crate::domain::flexible_args::opt_bool")]
    pub confirm_data_loss: Option<bool>,
}

/// `kubeSuspend`: a CronJob's or Job's `spec.suspend`. Optional in the type
/// so a missing answer is the tool's error, worded for the model.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct KubeSuspendArgs {
    pub kind: String,
    pub name: String,
    #[serde(default, deserialize_with = "crate::domain::flexible_args::opt_bool")]
    pub suspend: Option<bool>,
}

/// `kubeRolloutRestart`: the workload whose pods are replaced.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct KubeRolloutRestartArgs {
    pub kind: String,
    pub name: String,
}

/// `kubeRolloutUndo`: a Deployment, and the revision to go back to — the
/// previous one when none is named.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct KubeRolloutUndoArgs {
    pub kind: String,
    pub name: String,
    #[serde(default, deserialize_with = "crate::domain::flexible_args::opt_u32")]
    pub to_revision: Option<u32>,
}

/// `kubeApply`: one or more objects as YAML, `---` between them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct KubeApplyArgs {
    pub manifest: String,
}

/// `kubeDelete`: one object of the chat's own namespace.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct KubeDeleteArgs {
    pub kind: String,
    pub name: String,
    /// As `kubeScale`'s.
    #[serde(default, deserialize_with = "crate::domain::flexible_args::opt_bool")]
    pub confirm_data_loss: Option<bool>,
}

/// `kubeUndo`: a change by the id its tool answered with.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct KubeUndoArgs {
    pub change_id: String,
}
