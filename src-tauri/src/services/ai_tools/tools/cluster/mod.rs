//! The Kubernetes role's tools (`docs/21-kubernetes-mode.md`). Each works on
//! the cluster the chat is pinned to — none takes a kubeconfig or a context —
//! through `domain::kube::KubeApi`, and shapes what it read with
//! `domain::kube_view` before the model sees it. `read` holds the read tools,
//! `change` the ones that change the cluster, `definitions` what the model is
//! told about each; the helpers they share are here.

mod change;
mod definitions;
mod read;
#[cfg(test)]
mod tests;

pub use change::*;
pub(super) use definitions::*;
pub use read::*;

use chrono::Utc;
use serde::Deserialize;
use serde_json::Value;

use crate::services::text_diff::diff_stats;

use crate::domain::kube::{merge_diff, resolve_kind, undoable, KubeChange, KubeError, ROLLOUT_RESTART, KubeKind, ListQuery, LogQuery, PinnedCluster};
use crate::domain::kube_probe;
use crate::domain::kube_view::{self, cap, FieldPath, PodLog, Rollout};
use crate::domain::llm::LlmToolDefinition;
use crate::domain::tools::{
    ChangeDiff, FileDiffStats, KubeApplyArgs, KubeDeleteArgs,
    KubeDiagnoseArgs, KubeEventsArgs, KubeFieldHistoryArgs, KubeGetArgs, KubeListArgs, KubeLogsArgs, KubeProbeArgs, KubeRolloutRestartArgs, KubeRolloutUndoArgs, KubeScaleArgs, KubeSuspendArgs, KubeTopArgs, KubeWaitRolloutArgs,
    ToolCall, ToolError, ToolPreview, ToolResult,
};

/// A page of a list when the model asks for none; the most it may ask for.
const LIST_PAGE: u32 = 200;
const LIST_MOST: u32 = 500;
const LOG_TAIL: usize = 200;
const LOG_MOST: usize = 2_000;
/// Lines read from each pod when `grep` will throw most of them away.
const LOG_GREP_WINDOW: i64 = 5_000;
/// Pods one `kubeLogs` reads; a selector matching more says how many it left.
const LOG_PODS: usize = 10;
const EVENT_ROWS: usize = 100;
/// `kubeDiagnose`'s share of each: a report, not every row there is.
const DIAGNOSE_PODS: usize = 10;
const DIAGNOSE_EVENTS: usize = 30;
const DIAGNOSE_LOG_TAIL: usize = 40;

fn cluster(kube: Option<PinnedCluster>) -> Result<PinnedCluster, ToolError> {
    kube.ok_or(ToolError::NoCluster)
}

fn invalid(tool: &str, reason: impl Into<String>) -> ToolError {
    ToolError::InvalidArguments { tool: tool.to_string(), reason: reason.into() }
}

/// The namespace a call means: the chat's when it names none; `None` for
/// all of them (`*`), where `spans` allows it.
fn namespace(tool: &str, asked: Option<&str>, cluster: &PinnedCluster, spans: bool) -> Result<Option<String>, ToolError> {
    match asked.map(str::trim).filter(|n| !n.is_empty()) {
        None => Ok(Some(cluster.namespace.to_string())),
        Some("*" | "all") if spans => Ok(None),
        Some("*" | "all") => Err(invalid(tool, "namespace `*` is for lists — name one namespace")),
        Some(named) => Ok(Some(named.to_string())),
    }
}

fn kind(cluster: &PinnedCluster, asked: &str) -> Result<KubeKind, ToolError> {
    Ok(resolve_kind(&cluster.api.kinds()?, asked)?.clone())
}

fn place(kind: &KubeKind, namespace: Option<&str>) -> String {
    match namespace {
        _ if !kind.namespaced => "the cluster".to_string(),
        Some(namespace) => format!("namespace {namespace}"),
        None => "all namespaces".to_string(),
    }
}

fn result(text: String, summary: String, hint: &str) -> Result<ToolResult, ToolError> {
    Ok(ToolResult::Kube { text: cap(text, hint), summary })
}
