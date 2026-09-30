//! The Kubernetes role's read tools (`docs/21-kubernetes-mode.md`, K-3):
//! `kubeList`, `kubeGet`, `kubeEvents`, `kubeLogs`, `kubeTop`,
//! `kubeFieldHistory`. Each reads the cluster the chat is pinned to — none
//! takes a kubeconfig or a context — through `domain::kube::KubeApi`, and
//! shapes what it read with `domain::kube_view` before the model sees it.

use chrono::Utc;
use serde::Deserialize;
use serde_json::Value;

use crate::services::text_diff::diff_stats;

use crate::domain::kube::{merge_diff, resolve_kind, undoable, KubeChange, KubeError, ROLLOUT_RESTART, KubeKind, ListQuery, LogQuery, PinnedCluster};
use crate::domain::kube_view::{self, cap, FieldPath, PodLog, Rollout};
use crate::domain::llm::LlmToolDefinition;
use crate::domain::tools::{
    ChangeDiff, FileDiffStats, KubeApplyArgs, KubeDeleteArgs,
    KubeDiagnoseArgs, KubeEventsArgs, KubeFieldHistoryArgs, KubeGetArgs, KubeListArgs, KubeLogsArgs, KubeRolloutRestartArgs, KubeRolloutUndoArgs, KubeScaleArgs, KubeSuspendArgs, KubeTopArgs, KubeWaitRolloutArgs,
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

pub fn kube_list(kube: Option<PinnedCluster>, args: &KubeListArgs) -> Result<ToolResult, ToolError> {
    let cluster = cluster(kube)?;
    let kind = kind(&cluster, &args.kind)?;
    let fields = args
        .fields
        .iter()
        .flatten()
        .map(|path| FieldPath::parse(path))
        .collect::<Result<Vec<_>, _>>()
        .map_err(|reason| invalid("kubeList", reason))?;
    let namespace = namespace("kubeList", args.namespace.as_deref(), &cluster, true)?.filter(|_| kind.namespaced);
    let query = ListQuery {
        namespace: namespace.clone(),
        label_selector: args.label_selector.clone(),
        field_selector: args.field_selector.clone(),
        limit: Some(args.limit.unwrap_or(LIST_PAGE).clamp(1, LIST_MOST)),
        continue_token: args.continue_token.clone(),
    };
    let mut page = cluster.api.list(&kind, &query)?;
    for item in &mut page.items {
        kube_view::redact(&kind.kind, item);
    }
    let place = place(&kind, namespace.as_deref());
    let filters: String = [("labelSelector", &args.label_selector), ("fieldSelector", &args.field_selector)]
        .iter()
        .filter_map(|(name, value)| value.as_ref().map(|v| format!(", {name} {v}")))
        .collect();
    let count = page.items.len();
    let summary = format!("{count} {}", kind.plural);
    if count == 0 {
        return result(format!("No {} in {place}{filters}.", kind.plural), summary, "");
    }
    let all = kind.namespaced && namespace.is_none();
    let mut text = format!(
        "{count} {} in {place}{filters}:\n{}",
        kind.plural,
        kube_view::table(&kind.kind, &page.items, all, &fields, Utc::now())
    );
    if let Some(token) = page.continue_token {
        let left = page.remaining.map_or_else(String::new, |n| format!(" ({n} left)"));
        text.push_str(&format!("\nMore{left} — call again with continue: \"{token}\"."));
    }
    result(text, summary, "narrow with labelSelector, fieldSelector or limit")
}

pub fn kube_get(kube: Option<PinnedCluster>, args: &KubeGetArgs) -> Result<ToolResult, ToolError> {
    let cluster = cluster(kube)?;
    let kind = kind(&cluster, &args.kind)?;
    let namespace = namespace("kubeGet", args.namespace.as_deref(), &cluster, false)?.unwrap_or_default();
    let mut object = cluster.api.get(&kind, &namespace, &args.name)?;
    kube_view::redact(&kind.kind, &mut object);
    let sections = args.sections.clone().unwrap_or_default();
    let summary = format!("{}/{}", kind.kind, args.name);
    result(kube_view::object_yaml(object, &sections), summary, "ask for fewer sections, e.g. sections: [\"spec\"]")
}

pub fn kube_events(kube: Option<PinnedCluster>, args: &KubeEventsArgs) -> Result<ToolResult, ToolError> {
    let cluster = cluster(kube)?;
    let namespace = namespace("kubeEvents", args.namespace.as_deref(), &cluster, true)?;
    let mut selectors = Vec::new();
    // The object's kind as the server spells it: `deploy` finds nothing.
    if let Some(asked) = &args.kind {
        selectors.push(format!("involvedObject.kind={}", kind(&cluster, asked)?.kind));
    }
    if let Some(name) = &args.name {
        selectors.push(format!("involvedObject.name={name}"));
    }
    if args.warnings_only == Some(true) {
        selectors.push("type=Warning".to_string());
    }
    let events = KubeKind::core("Event", "events");
    let query = ListQuery {
        namespace: namespace.clone(),
        field_selector: (!selectors.is_empty()).then(|| selectors.join(",")),
        limit: Some(LIST_MOST),
        ..Default::default()
    };
    let page = cluster.api.list(&events, &query)?;
    let about = match (&args.kind, &args.name) {
        (Some(kind), Some(name)) => format!(" for {kind}/{name}"),
        (None, Some(name)) => format!(" for {name}"),
        _ => String::new(),
    };
    let summary = format!("{} events", page.items.len());
    if page.items.is_empty() {
        return result(
            format!(
                "No events{about} in {} — the cluster keeps events about an hour, so an older failure leaves none. \
                 A Deployment's own events are about scaling; its pods' are under each Pod.",
                place(&events, namespace.as_deref())
            ),
            summary,
            "",
        );
    }
    let table = kube_view::events(&page.items, Utc::now(), EVENT_ROWS);
    result(format!("Events{about} in {}:\n{table}", place(&events, namespace.as_deref())), summary, "narrow with kind and name, or warningsOnly")
}

/// A selector's text, from an owner's `spec.selector` — a Deployment's
/// `matchLabels` and `matchExpressions`, or a Service's plain map.
fn selector_of(owner: &Value) -> Option<String> {
    let selector = owner.pointer("/spec/selector")?.as_object()?;
    let structured = selector.contains_key("matchLabels") || selector.contains_key("matchExpressions");
    let labels = if structured { selector.get("matchLabels").and_then(Value::as_object) } else { Some(selector) };
    let mut terms: Vec<String> =
        labels.into_iter().flatten().filter_map(|(k, v)| Some(format!("{k}={}", v.as_str()?))).collect();
    for expression in selector.get("matchExpressions").and_then(Value::as_array).into_iter().flatten() {
        let key = expression["key"].as_str()?;
        let values = || {
            let values: Vec<&str> = expression["values"].as_array().into_iter().flatten().filter_map(Value::as_str).collect();
            values.join(",")
        };
        terms.push(match expression["operator"].as_str()? {
            "In" => format!("{key} in ({})", values()),
            "NotIn" => format!("{key} notin ({})", values()),
            "Exists" => key.to_string(),
            "DoesNotExist" => format!("!{key}"),
            _ => return None,
        });
    }
    (!terms.is_empty()).then(|| terms.join(","))
}

/// The container a pod's log is read from: the one asked for, the pod's
/// default, or its first — and the pod's other containers, to say so.
fn container_of(pod: &Value, asked: Option<&str>) -> (Option<String>, Vec<String>) {
    let names: Vec<String> = pod
        .pointer("/spec/containers")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|c| c["name"].as_str().map(str::to_string))
        .collect();
    let default = pod.pointer("/metadata/annotations/kubectl.kubernetes.io~1default-container").and_then(Value::as_str);
    let chosen = asked.or(default).map(str::to_string).or_else(|| names.first().cloned());
    let others = names.into_iter().filter(|n| Some(n) != chosen.as_ref()).collect();
    (chosen, others)
}

pub fn kube_logs(kube: Option<PinnedCluster>, args: &KubeLogsArgs) -> Result<ToolResult, ToolError> {
    let cluster = cluster(kube)?;
    let namespace = namespace("kubeLogs", args.namespace.as_deref(), &cluster, false)?.unwrap_or_default();
    let pods_kind = KubeKind::core("Pod", "pods");
    let list = |selector: String| -> Result<Vec<Value>, ToolError> {
        let query = ListQuery { namespace: Some(namespace.clone()), label_selector: Some(selector), ..Default::default() };
        Ok(cluster.api.list(&pods_kind, &query)?.items)
    };
    let (source, pods) = match (&args.pod, &args.kind, &args.name, &args.label_selector) {
        (Some(pod), ..) => (format!("pod {pod}"), vec![cluster.api.get(&pods_kind, &namespace, pod)?]),
        (None, Some(asked), Some(name), _) => {
            let kind = kind(&cluster, asked)?;
            let owner = cluster.api.get(&kind, &namespace, name)?;
            let source = format!("{}/{name}", kind.kind);
            if kind.kind == "Pod" {
                (source, vec![owner])
            } else {
                let selector = selector_of(&owner).ok_or_else(|| {
                    invalid("kubeLogs", format!("{source} selects no pods of its own — find them with kubeList and read by pod or labelSelector"))
                })?;
                (source, list(selector)?)
            }
        }
        (None, None, None, Some(selector)) => (format!("pods with {selector}"), list(selector.clone())?),
        _ => return Err(invalid("kubeLogs", "name a pod, or kind and name of its owner, or a labelSelector")),
    };
    if pods.is_empty() {
        return result(format!("No pods for {source} in namespace {namespace}."), "0 pods".to_string(), "");
    }
    let grep = match &args.grep {
        Some(pattern) => Some(
            regex::RegexBuilder::new(pattern).case_insensitive(true).build().map_err(|e| ToolError::InvalidRegex(e.to_string()))?,
        ),
        None => None,
    };
    let since_seconds =
        args.since.as_deref().map(kube_view::parse_since).transpose().map_err(|reason| invalid("kubeLogs", reason))?;
    let tail = args.tail.map_or(LOG_TAIL, |t| (t as usize).clamp(1, LOG_MOST));
    let read = pods.len().min(LOG_PODS);
    let mut logs = Vec::new();
    let mut failed = Vec::new();
    let mut containers = (None, Vec::new());
    for pod in &pods[..read] {
        let name = pod.pointer("/metadata/name").and_then(Value::as_str).unwrap_or_default();
        let (container, others) = container_of(pod, args.container.as_deref());
        let query = LogQuery {
            container: container.clone(),
            previous: args.previous == Some(true),
            tail: Some(if grep.is_some() { LOG_GREP_WINDOW } else { tail as i64 }),
            since_seconds,
        };
        match cluster.api.logs(&namespace, name, &query) {
            Ok(text) => logs.push(PodLog { pod: name.to_string(), text }),
            Err(e) => failed.push(format!("{name}: {e}")),
        }
        containers = (container, others);
    }
    // One pod, and it failed: that is the answer, not an empty log.
    if logs.is_empty() && failed.len() == 1 {
        return Err(ToolError::Kube(crate::domain::kube::KubeError::Cluster(failed.remove(0))));
    }
    let lines = kube_view::merge_logs(&logs, tail, grep.as_ref(), pods.len() > 1);
    let mut head = format!("Logs of {source}: {} pod{}", read, if read == 1 { "" } else { "s" });
    if read < pods.len() {
        head.push_str(&format!(" of {} — narrow with pod or labelSelector to read the rest", pods.len()));
    }
    if let (Some(container), others) = &containers {
        head.push_str(&format!(", container {container}"));
        if !others.is_empty() {
            head.push_str(&format!(" (also: {})", others.join(", ")));
        }
    }
    if args.previous == Some(true) {
        head.push_str(", before the last restart");
    }
    if let Some(since) = &args.since {
        head.push_str(&format!(", last {since}"));
    }
    if let Some(pattern) = &args.grep {
        head.push_str(&format!(", lines matching /{pattern}/"));
    }
    let body = if lines.is_empty() { "No log lines.".to_string() } else { format!("last {} lines, UTC:\n{}", lines.len(), lines.join("\n")) };
    let mut text = format!("{head}; {body}");
    if !failed.is_empty() {
        text.push_str(&format!("\nNot read: {}", failed.join("; ")));
    }
    result(text, format!("{} lines · {read} pods", lines.len()), "narrow with grep, since or tail")
}

pub fn kube_top(kube: Option<PinnedCluster>, args: &KubeTopArgs) -> Result<ToolResult, ToolError> {
    let cluster = cluster(kube)?;
    let metrics = match args.kind.trim().to_lowercase().as_str() {
        "pods" | "pod" | "po" => "PodMetrics.metrics.k8s.io",
        "nodes" | "node" | "no" => "NodeMetrics.metrics.k8s.io",
        _ => return Err(invalid("kubeTop", "kind is `pods` or `nodes`")),
    };
    let kind = match resolve_kind(&cluster.api.kinds()?, metrics) {
        Ok(kind) => kind.clone(),
        Err(_) => {
            return result(
                "This cluster has no metrics API (metrics.k8s.io) — metrics-server is not installed, so there is no CPU \
                 or memory usage to read. Requests and limits are in the pod spec (kubeGet)."
                    .to_string(),
                "no metrics".to_string(),
                "",
            )
        }
    };
    let namespace = namespace("kubeTop", args.namespace.as_deref(), &cluster, true)?.filter(|_| kind.namespaced);
    let query = ListQuery { namespace: namespace.clone(), limit: Some(LIST_MOST), ..Default::default() };
    let items = cluster.api.list(&kind, &query)?.items;
    let summary = format!("{} {}", items.len(), args.kind);
    if items.is_empty() {
        return result(format!("No usage reported in {}.", place(&kind, namespace.as_deref())), summary, "");
    }
    let all = kind.namespaced && namespace.is_none();
    let text = format!(
        "Usage now in {} (no history — metrics-server keeps only the latest):\n{}",
        place(&kind, namespace.as_deref()),
        kube_view::top(&items, all)
    );
    result(text, summary, "narrow with namespace")
}

pub fn kube_field_history(kube: Option<PinnedCluster>, args: &KubeFieldHistoryArgs) -> Result<ToolResult, ToolError> {
    let cluster = cluster(kube)?;
    let kind = kind(&cluster, &args.kind)?;
    let namespace = namespace("kubeFieldHistory", args.namespace.as_deref(), &cluster, false)?.unwrap_or_default();
    let object = cluster.api.get(&kind, &namespace, &args.name)?;
    let paths = args.paths.clone().unwrap_or_default();
    let summary = format!("{}/{}", kind.kind, args.name);
    let Some(table) = kube_view::field_history(&object, &paths) else {
        return result(format!("{summary} has no managedFields — the server did not record who set its fields."), summary, "");
    };
    let text = format!(
        "Who set the fields of {summary}. TIME is when that manager last wrote any of its fields, not when this one \
         was set.\n{table}"
    );
    result(text, summary, "narrow with paths")
}

/// A workload's state in one call (`docs/21-kubernetes-mode.md`, K-4): its
/// own status, its pods and what is wrong with each, the events of it, its
/// ReplicaSets and pods, and the log of the container that explains it — the
/// crash before the last restart where there was one. Facts only; and what
/// the cluster no longer shows, said. Only the object itself must be read:
/// events or a log the identity may not read are named, not fatal.
pub fn kube_diagnose(kube: Option<PinnedCluster>, args: &KubeDiagnoseArgs) -> Result<ToolResult, ToolError> {
    let cluster = cluster(kube)?;
    let kind = kind(&cluster, &args.kind)?;
    let namespace = namespace("kubeDiagnose", args.namespace.as_deref(), &cluster, false)?.unwrap_or_default();
    let owner = cluster.api.get(&kind, &namespace, &args.name)?;
    let now = Utc::now();
    let list = |kind: &KubeKind, selector: &str| -> Result<Vec<Value>, ToolError> {
        let query = ListQuery { namespace: Some(namespace.clone()), label_selector: Some(selector.to_string()), ..Default::default() };
        Ok(cluster.api.list(kind, &query)?.items)
    };
    let name_of = |object: &Value| object.pointer("/metadata/name").and_then(Value::as_str).unwrap_or_default().to_string();

    let mut out = vec![format!("{}/{} in namespace {namespace}", kind.kind, args.name)];
    let mut involved = vec![args.name.clone()];
    let pods = if kind.kind == "Pod" {
        vec![owner.clone()]
    } else {
        out.extend(kube_view::owner_status(&kind.kind, &owner));
        let selector = selector_of(&owner).ok_or_else(|| {
            invalid("kubeDiagnose", format!("{}/{} has no pods of its own — diagnose what it runs: a CronJob's latest Job, a Service's Deployment", kind.kind, args.name))
        })?;
        // A Deployment's ReplicaSets carry what stopped pods being made: a
        // quota, an admission webhook (FailedCreate).
        if kind.kind == "Deployment" {
            let sets = KubeKind { group: "apps".into(), version: "v1".into(), kind: "ReplicaSet".into(), plural: "replicasets".into(), namespaced: true };
            involved.extend(list(&sets, &selector).unwrap_or_default().iter().map(name_of));
        }
        list(&KubeKind::core("Pod", "pods"), &selector)?
    };
    involved.extend(pods.iter().map(name_of));

    let troubled: Vec<(&Value, Vec<String>)> =
        pods.iter().map(|pod| (pod, kube_view::pod_findings(pod, now))).filter(|(_, found)| !found.is_empty()).collect();
    out.push(String::new());
    if pods.is_empty() {
        out.push("No pods: none were made, or they are gone — the events below say which.".to_string());
    } else {
        out.push(format!("Pods: {}, {} with problems.", pods.len(), troubled.len()));
        let shown: Vec<Value> = if troubled.is_empty() {
            pods.iter().take(DIAGNOSE_PODS).cloned().collect()
        } else {
            troubled.iter().take(DIAGNOSE_PODS).map(|(pod, _)| (*pod).clone()).collect()
        };
        out.push(kube_view::table("Pod", &shown, false, &[], now));
        for (pod, found) in troubled.iter().take(DIAGNOSE_PODS) {
            let name = name_of(pod);
            out.extend(found.iter().map(|fact| format!("{name}: {fact}")));
            if pod.pointer("/status/phase").and_then(Value::as_str) == Some("Pending") {
                out.push(format!("{name}: {}", kube_view::requests_of(pod)));
                if let Some(selector) = pod.pointer("/spec/nodeSelector").filter(|s| s.as_object().is_some_and(|m| !m.is_empty())) {
                    out.push(format!("{name}: nodeSelector {selector}"));
                }
                for claim in kube_view::claims_of(pod) {
                    let claims = KubeKind::core("PersistentVolumeClaim", "persistentvolumeclaims");
                    let phase = match cluster.api.get(&claims, &namespace, &claim) {
                        Ok(pvc) => pvc.pointer("/status/phase").and_then(Value::as_str).unwrap_or("unknown").to_string(),
                        Err(e) => format!("not read: {e}"),
                    };
                    if phase != "Bound" {
                        out.push(format!("{name}: claim {claim} is {phase}"));
                    }
                }
            }
        }
    }

    out.push(String::new());
    let events = KubeKind::core("Event", "events");
    let query = ListQuery { namespace: Some(namespace.clone()), limit: Some(LIST_MOST), ..Default::default() };
    match cluster.api.list(&events, &query) {
        Ok(page) => {
            let about: Vec<Value> = page
                .items
                .into_iter()
                .filter(|e| e.pointer("/involvedObject/name").and_then(Value::as_str).is_some_and(|n| involved.iter().any(|i| i == n)))
                .collect();
            if about.is_empty() {
                out.push("No events of it or its pods.".to_string());
            } else {
                out.push(format!("Events of it and its pods:\n{}", kube_view::events(&about, now, DIAGNOSE_EVENTS)));
            }
        }
        Err(e) => out.push(format!("Events not read: {e}")),
    }

    let telling = troubled.iter().find_map(|(pod, _)| kube_view::problem_container(pod).map(|c| (*pod, c)));
    if let Some((pod, (container, previous))) = telling {
        let name = name_of(pod);
        let query = LogQuery { container: Some(container.clone()), previous, tail: Some(DIAGNOSE_LOG_TAIL as i64), since_seconds: None };
        let before = if previous { ", before its last restart" } else { "" };
        out.push(String::new());
        match cluster.api.logs(&namespace, &name, &query) {
            Ok(text) => {
                let lines = kube_view::merge_logs(&[PodLog { pod: name.clone(), text }], DIAGNOSE_LOG_TAIL, None, false);
                let body = if lines.is_empty() { " empty.".to_string() } else { format!(" last {} lines, UTC:\n{}", lines.len(), lines.join("\n")) };
                out.push(format!("Log of {name}, container {container}{before}:{body}"));
            }
            Err(e) => out.push(format!("Log of {name}, container {container}{before}, not read: {e}")),
        }
        let more = troubled.iter().filter(|(p, _)| kube_view::problem_container(p).is_some()).count() - 1;
        if more > 0 {
            out.push(format!("{more} more pods have a container in trouble — kubeLogs reads them."));
        }
    }

    out.push(String::new());
    out.push(
        "Not visible here: events older than about an hour, logs from before the last restart, past CPU and memory. \
         A cause outside the cluster — a database, an external API — shows only as connection errors in the logs."
            .to_string(),
    );
    let summary = format!("{} pods, {} with problems", pods.len(), troubled.len());
    result(out.join("\n"), summary, "diagnose one pod, or read events and logs on their own")
}

/// How long `kubeWaitRollout` waits when not told, and the most it will.
const WAIT_SECONDS: u64 = 120;
const WAIT_MOST: u64 = 600;

/// `kubeWaitRollout`: watches a workload until its rollout is done, the
/// controller gives up, the time is out or the turn is stopped. Done is one
/// line; anything else is that line and the diagnosis — why is the question
/// the model asks next.
pub fn kube_wait_rollout(kube: Option<PinnedCluster>, cancelled: Option<&dyn Fn() -> bool>, args: &KubeWaitRolloutArgs) -> Result<ToolResult, ToolError> {
    let cluster = cluster(kube)?;
    let kind = kind(&cluster, &args.kind)?;
    let what = format!("{}/{}", kind.kind, args.name);
    if !RESTARTABLE.contains(&kind.kind.as_str()) {
        return Err(invalid("kubeWaitRollout", format!("{what} has no rollout to wait for — a Deployment, StatefulSet or DaemonSet has; read anything else with kubeGet")));
    }
    let mut stands = Rollout::Going(String::new());
    let seconds = args.timeout_seconds.unwrap_or(WAIT_SECONDS).clamp(1, WAIT_MOST);
    let stop = || cancelled.is_some_and(|cancelled| cancelled());
    cluster.api.watch(&kind, cluster.namespace, &args.name, std::time::Duration::from_secs(seconds), &stop, &mut |object| {
        stands = kube_view::rollout(&kind.kind, object);
        !matches!(stands, Rollout::Going(_))
    })?;
    let (summary, line) = match stands {
        Rollout::Done(numbers) => return result(format!("Rollout of {what} is done: {numbers}."), "done".to_string(), ""),
        Rollout::Stuck(why) => ("stuck", format!("Rollout of {what} is stuck: {why}.")),
        Rollout::Going(numbers) if stop() => ("stopped", format!("Stopped waiting for {what} — the user stopped the turn: {numbers}.")),
        Rollout::Going(numbers) => {
            ("not done", format!("Rollout of {what} is not done after {seconds}s: {numbers}. It may still finish — wait again, or read why below."))
        }
    };
    let diagnose = KubeDiagnoseArgs { kind: args.kind.clone(), name: args.name.clone(), namespace: None };
    let why = match kube_diagnose(Some(cluster), &diagnose) {
        Ok(ToolResult::Kube { text, .. }) => text,
        Ok(_) => String::new(),
        Err(e) => format!("Not diagnosed: {e}"),
    };
    result(format!("{line}\n\n{why}"), summary.to_string(), "kubeDiagnose reads it on its own")
}

/// What has replicas to set. The rest are told how they are stopped instead.
const SCALABLE: &[&str] = &["Deployment", "StatefulSet", "ReplicaSet"];
const SUSPENDABLE: &[&str] = &["CronJob", "Job"];
const RESTARTABLE: &[&str] = &["Deployment", "StatefulSet", "DaemonSet"];
/// Where a Deployment and its ReplicaSets keep the revision's number.
const REVISION: &str = "/metadata/annotations/deployment.kubernetes.io~1revision";

/// A change the server has accepted as a dry run: the object as it is, the
/// patch that changes it, and what that does in words — `3 → 0 replicas`.
struct Plan {
    /// The kind of change, and so how it is put back: a changing tool's name.
    tool: &'static str,
    kind: KubeKind,
    name: String,
    /// As it is now; `Null` when it is not there.
    object: Value,
    act: Act,
    summary: String,
    /// Its YAML now against its YAML after, where the words do not say it all.
    diff: Option<FileDiffStats>,
    /// What else the card says before anyone agrees: what is lost, and what
    /// will put the object back by itself.
    warnings: Vec<String>,
}

/// What a change does to its object.
enum Act {
    /// A merge patch: one field, or one section replaced.
    Patch(Value),
    /// The object whole, created when it is not there.
    Apply(Value),
    Delete,
}

impl Plan {
    fn patching(tool: &'static str, kind: KubeKind, name: &str, object: Value, patch: Value, summary: String) -> Plan {
        Plan { tool, kind, name: name.to_string(), object, act: Act::Patch(patch), summary, diff: None, warnings: Vec::new() }
    }

    /// Asks the server, for real or as a dry run. The object afterwards;
    /// `Null` once it is deleted.
    fn run(&self, cluster: &PinnedCluster, dry_run: bool) -> Result<Value, KubeError> {
        let (api, namespace) = (cluster.api, cluster.namespace);
        match &self.act {
            Act::Patch(patch) => api.patch(&self.kind, namespace, &self.name, patch, dry_run),
            Act::Apply(object) => api.apply(&self.kind, namespace, &self.name, object, dry_run),
            Act::Delete => api.delete(&self.kind, namespace, &self.name, dry_run).map(|()| Value::Null),
        }
    }
}

/// Whether a change of this kind can be put back from its backup. A restart
/// cannot: its pods are already replaced, and the old ones do not come back.
fn can_be_undone(tool: &str) -> bool {
    tool != ROLLOUT_RESTART
}

/// What every change starts with, in the order it is cheapest to know: the
/// tab's switch, the kind — one of `kinds`, or `other` says what to do
/// instead — and the object, in the chat's own namespace.
fn target(
    cluster: &PinnedCluster,
    tool: &str,
    asked: &str,
    name: &str,
    kinds: &[&str],
    other: impl Fn(&str) -> String,
) -> Result<(KubeKind, Value), ToolError> {
    if !cluster.writes {
        return Err(ToolError::KubeReadOnly);
    }
    let kind = kind(cluster, asked)?;
    if !kinds.contains(&kind.kind.as_str()) {
        return Err(invalid(tool, other(&kind.kind)));
    }
    let object = cluster.api.get(&kind, cluster.namespace, name)?;
    Ok((kind, object))
}

/// The last thing that can refuse a change: the server's own check — RBAC,
/// admission, quota — as a dry run. Run before the card, for the card, and
/// again before the change: the cluster may have moved while the user read.
fn checked(cluster: &PinnedCluster, plan: Plan) -> Result<Plan, ToolError> {
    plan.run(cluster, true)?;
    Ok(plan)
}

/// The object, or `Null` when the cluster has none of that name.
fn found(cluster: &PinnedCluster, kind: &KubeKind, name: &str) -> Result<Value, ToolError> {
    match cluster.api.get(kind, cluster.namespace, name) {
        Err(KubeError::NotFound(_)) => Ok(Value::Null),
        other => Ok(other?),
    }
}

/// An object as a manifest says it, for a diff and for putting back: without
/// what the server writes itself — its ids, counters, timestamps and status.
fn declared(mut object: Value) -> Value {
    if let Some(top) = object.as_object_mut() {
        top.remove("status");
    }
    if let Some(metadata) = object.get_mut("metadata").and_then(Value::as_object_mut) {
        for written in ["uid", "resourceVersion", "generation", "creationTimestamp", "managedFields", "selfLink", "deletionTimestamp", "deletionGracePeriodSeconds"] {
            metadata.remove(written);
        }
    }
    object
}

/// What the card shows of an object: declared, and with a Secret's values
/// and credentials in `env` taken out, as everywhere else.
fn shown(kind: &KubeKind, object: &Value) -> String {
    if object.is_null() {
        return String::new();
    }
    let mut object = declared(object.clone());
    kube_view::redact(&kind.kind, &mut object);
    kube_view::object_yaml(object, &[])
}

/// A manifest's object: its kind as the cluster serves it, in the chat's own
/// namespace and no other.
fn addressed(cluster: &PinnedCluster, tool: &str, document: &mut Value) -> Result<(KubeKind, String), ToolError> {
    let text = |path: &str| document.pointer(path).and_then(Value::as_str).map(str::to_string);
    let (Some(named), Some(api_version), Some(name)) = (text("/kind"), text("/apiVersion"), text("/metadata/name")) else {
        return Err(invalid(tool, "every object needs apiVersion, kind and metadata.name"));
    };
    let (group, version) = api_version.rsplit_once('/').unwrap_or(("", &api_version));
    let kind = kind(cluster, &if group.is_empty() { named.clone() } else { format!("{named}.{group}") })?;
    if kind.version != version || (group.is_empty() && !kind.group.is_empty()) {
        let served = if kind.group.is_empty() { kind.version.clone() } else { format!("{}/{}", kind.group, kind.version) };
        return Err(invalid(tool, format!("the cluster serves {named} as apiVersion {served}, not {api_version}")));
    }
    if !kind.namespaced {
        return Err(invalid(tool, format!("a {named} is cluster-wide, and a chat changes only its own namespace — give the user the command instead")));
    }
    match text("/metadata/namespace") {
        Some(other) if other != cluster.namespace => {
            return Err(invalid(tool, format!("{named}/{name} names namespace {other}; this chat changes only {} — the user can open a chat pinned there", cluster.namespace)))
        }
        _ => document["metadata"]["namespace"] = Value::String(cluster.namespace.to_string()),
    }
    Ok((kind, name))
}

/// Objects one `kubeApply` takes: a card that long is not read.
const APPLY_MOST: usize = 20;

/// `kubectl apply`: each object of the manifest as the server would have it,
/// against what is there. One that would not change is left out; a manifest
/// that changes nothing is refused.
fn plan_apply(cluster: &PinnedCluster, args: &KubeApplyArgs) -> Result<Vec<Plan>, ToolError> {
    let tool = "kubeApply";
    if !cluster.writes {
        return Err(ToolError::KubeReadOnly);
    }
    let mut documents = Vec::new();
    for document in yaml_serde::Deserializer::from_str(&args.manifest) {
        let document = Value::deserialize(document).map_err(|e| invalid(tool, format!("the manifest is not YAML: {e}")))?;
        if !document.is_null() {
            documents.push(document);
        }
    }
    if documents.is_empty() || documents.len() > APPLY_MOST {
        return Err(invalid(tool, format!("a manifest is 1 to {APPLY_MOST} objects, `---` between them — this one has {}", documents.len())));
    }
    let mut plans = Vec::new();
    for mut document in documents {
        let (kind, name) = addressed(cluster, tool, &mut document)?;
        let object = found(cluster, &kind, &name)?;
        let mut plan = Plan { tool, kind, name, object, act: Act::Apply(document), summary: String::new(), diff: None, warnings: Vec::new() };
        let after = plan.run(cluster, true)?;
        let diff = diff_stats(&shown(&plan.kind, &plan.object), &shown(&plan.kind, &after));
        if declared(plan.object.clone()) == declared(after) {
            continue;
        }
        plan.summary = if plan.object.is_null() {
            "create".to_string()
        } else {
            format!("update (+{} −{} lines)", diff.lines_added, diff.lines_removed)
        };
        plan.diff = Some(diff);
        plans.push(plan);
    }
    if plans.is_empty() {
        return Err(invalid(tool, "the cluster already has these objects exactly as the manifest says — there is nothing to change"));
    }
    Ok(plans)
}

/// What deleting a claim takes with it, when its volume is not kept.
fn claim_warning(cluster: &PinnedCluster, claim: &Value) -> Option<String> {
    let volume = claim.pointer("/spec/volumeName").and_then(Value::as_str)?;
    let policy = kind(cluster, "persistentvolumes")
        .and_then(|volumes| Ok(cluster.api.get(&volumes, "", volume)?))
        .ok()
        .and_then(|volume| volume.pointer("/spec/persistentVolumeReclaimPolicy").and_then(Value::as_str).map(str::to_string));
    match policy.as_deref() {
        Some("Retain") => None,
        Some(_) => Some(format!("Its volume {volume} is deleted with it: the DATA IS LOST, and the backup does not bring it back.")),
        None => Some(format!("Whether its volume {volume} is kept could not be read: the data may be lost with it.")),
    }
}

fn plan_delete(cluster: &PinnedCluster, args: &KubeDeleteArgs) -> Result<Plan, ToolError> {
    let tool = "kubeDelete";
    if !cluster.writes {
        return Err(ToolError::KubeReadOnly);
    }
    let kind = kind(cluster, &args.kind)?;
    if !kind.namespaced {
        return Err(invalid(tool, format!("a {} is cluster-wide, and a chat changes only its own namespace — give the user the command instead", kind.kind)));
    }
    let object = cluster.api.get(&kind, cluster.namespace, &args.name)?;
    let warning = (kind.kind == "PersistentVolumeClaim").then(|| claim_warning(cluster, &object)).flatten();
    let warnings = warning.into_iter().collect();
    let plan = Plan { tool, kind, name: args.name.clone(), object, act: Act::Delete, summary: "delete".to_string(), diff: None, warnings };
    checked(cluster, plan)
}

fn plan_scale(cluster: &PinnedCluster, args: &KubeScaleArgs) -> Result<Plan, ToolError> {
    let tool = "kubeScale";
    // Refused before the arguments are: a read-only chat is told to switch, not to fix a call.
    if !cluster.writes {
        return Err(ToolError::KubeReadOnly);
    }
    let after = args.replicas.ok_or_else(|| invalid(tool, "replicas is required: the number to scale to"))?;
    let (kind, object) = target(cluster, tool, &args.kind, &args.name, SCALABLE, |kind| {
        let how = match kind {
            "DaemonSet" => "a DaemonSet runs a pod on every node and has no replicas — it stops only by deleting it, or by a nodeSelector no node matches",
            "CronJob" => "a CronJob has no replicas — it is stopped with kubeSuspend",
            "Job" => "a Job has no replicas — it is stopped with kubeSuspend, or by deleting it",
            _ => "only a Deployment, StatefulSet or ReplicaSet has replicas to set",
        };
        format!("{how}. Say what is still running.")
    })?;
    let before = object.pointer("/spec/replicas").and_then(Value::as_i64).unwrap_or(1);
    if before == i64::from(after) {
        return Err(invalid(tool, format!("{}/{} already has {after} replicas — there is nothing to change", kind.kind, args.name)));
    }
    let patch = serde_json::json!({"spec": {"replicas": after}});
    checked(cluster, Plan::patching(tool, kind, &args.name, object, patch, format!("{before} → {after} replicas")))
}

fn plan_suspend(cluster: &PinnedCluster, args: &KubeSuspendArgs) -> Result<Plan, ToolError> {
    let tool = "kubeSuspend";
    if !cluster.writes {
        return Err(ToolError::KubeReadOnly);
    }
    let after = args.suspend.ok_or_else(|| invalid(tool, "suspend is required: true to stop it, false to let it run again"))?;
    let (kind, object) = target(cluster, tool, &args.kind, &args.name, SUSPENDABLE, |_| {
        "only a CronJob or a Job is suspended — a Deployment, StatefulSet or ReplicaSet is stopped with kubeScale to 0".to_string()
    })?;
    let before = object.pointer("/spec/suspend").and_then(Value::as_bool).unwrap_or(false);
    let word = |suspended: bool| if suspended { "suspended" } else { "active" };
    if before == after {
        return Err(invalid(tool, format!("{}/{} is already {} — there is nothing to change", kind.kind, args.name, word(after))));
    }
    let patch = serde_json::json!({"spec": {"suspend": after}});
    checked(cluster, Plan::patching(tool, kind, &args.name, object, patch, format!("{} → {}", word(before), word(after))))
}

/// `kubectl rollout restart`: the pod template's `restartedAt`, which makes
/// the controller replace every pod, a few at a time.
fn plan_restart(cluster: &PinnedCluster, args: &KubeRolloutRestartArgs) -> Result<Plan, ToolError> {
    let tool = ROLLOUT_RESTART;
    let (kind, object) = target(cluster, tool, &args.kind, &args.name, RESTARTABLE, |_| {
        "only a Deployment, StatefulSet or DaemonSet is restarted — a single pod restarts when it is deleted, which the user does".to_string()
    })?;
    let at = Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
    let patch = serde_json::json!({"spec": {"template": {"metadata": {"annotations": {"kubectl.kubernetes.io/restartedAt": at}}}}});
    checked(cluster, Plan::patching(tool, kind, &args.name, object, patch, "rollout restart — every pod is replaced".to_string()))
}

fn images(template: &Value) -> String {
    let containers = template.pointer("/spec/containers").and_then(Value::as_array);
    let images: Vec<&str> = containers.into_iter().flatten().filter_map(|c| c["image"].as_str()).collect();
    images.join(", ")
}

/// A Deployment's pod template replaced by `wanted`, whole: what `wanted`
/// lacks is taken out, not left. `label` is what the change is called; the
/// images on either side are added when they differ — what a rollback is
/// usually for.
fn plan_template(cluster: &PinnedCluster, kind: KubeKind, name: &str, object: Value, wanted: &Value, label: String) -> Result<Plan, ToolError> {
    let tool = "kubeRolloutUndo";
    let current = object.pointer("/spec/template").cloned().unwrap_or(Value::Null);
    let Some(difference) = merge_diff(&current, wanted) else {
        return Err(invalid(tool, format!("{}/{name} already has that pod template — there is nothing to change", kind.kind)));
    };
    let (was, will) = (images(&current), images(wanted));
    let summary = if was == will { label } else { format!("{label}, image {was} → {will}") };
    let patch = serde_json::json!({"spec": {"template": difference}});
    checked(cluster, Plan::patching(tool, kind, name, object, patch, summary))
}

/// `kubectl rollout undo`: the pod template of an earlier revision, read
/// from the ReplicaSet that still holds it — the one before the current
/// unless `toRevision` names another.
fn plan_rollout_undo(cluster: &PinnedCluster, args: &KubeRolloutUndoArgs) -> Result<Plan, ToolError> {
    let tool = "kubeRolloutUndo";
    let (kind, object) = target(cluster, tool, &args.kind, &args.name, &["Deployment"], |_| {
        "only a Deployment is rolled back here — for a StatefulSet or DaemonSet give the user `kubectl rollout undo`".to_string()
    })?;
    let revision = |o: &Value| o.pointer(REVISION).and_then(Value::as_str).and_then(|r| r.parse::<u32>().ok());
    let current = revision(&object);
    let sets = self::kind(cluster, "replicasets.apps")?;
    let query = ListQuery { namespace: Some(cluster.namespace.to_string()), label_selector: selector_of(&object), ..Default::default() };
    let uid = &object["metadata"]["uid"];
    let mut earlier: Vec<(u32, Value)> = cluster
        .api
        .list(&sets, &query)?
        .items
        .into_iter()
        .filter(|set| set.pointer("/metadata/ownerReferences").and_then(Value::as_array).is_some_and(|owners| owners.iter().any(|o| &o["uid"] == uid)))
        .filter_map(|set| Some((revision(&set)?, set)))
        .filter(|(number, _)| Some(*number) != current)
        .collect();
    earlier.sort_by_key(|(number, _)| *number);
    let known = || earlier.iter().map(|(n, _)| n.to_string()).collect::<Vec<_>>().join(", ");
    let (to, set) = match args.to_revision {
        Some(asked) => earlier.iter().find(|(n, _)| *n == asked).ok_or_else(|| {
            let kept = if earlier.is_empty() { "no earlier revision is kept".to_string() } else { format!("the revisions kept are {}", known()) };
            invalid(tool, format!("Deployment/{} has no revision {asked} to go back to — {kept}", args.name))
        })?,
        None => earlier.last().ok_or_else(|| invalid(tool, format!("Deployment/{} has no earlier revision to go back to", args.name)))?,
    };
    let mut wanted = set.pointer("/spec/template").cloned().unwrap_or(Value::Null);
    // The ReplicaSet's own mark on its pods, not part of the Deployment's template.
    if let Some(labels) = wanted.pointer_mut("/metadata/labels").and_then(Value::as_object_mut) {
        labels.remove("pod-template-hash");
    }
    let from = current.map_or("?".to_string(), |n| n.to_string());
    plan_template(cluster, kind, &args.name, object, &wanted, format!("revision {from} → {to}"))
}

fn generation(object: &Value) -> Option<i64> {
    object.pointer("/metadata/generation").and_then(Value::as_i64)
}

fn version(object: &Value) -> Option<String> {
    object.pointer("/metadata/resourceVersion").and_then(Value::as_str).map(str::to_string)
}

/// Whether the object is as the change `made` left it: by its spec's
/// generation — which a status update does not move — or, for a kind that
/// has none, by its resource version.
fn same_since(object: &Value, made: &KubeChange) -> bool {
    match (made.generation_after, &made.version_after) {
        (Some(left), _) => generation(object) == Some(left),
        (None, Some(left)) => version(object).as_ref() == Some(left),
        (None, None) => true,
    }
}

/// An undo the server has accepted as a dry run (`docs/21-kubernetes-mode.md`,
/// decision 9): what the backup of `id` puts back. The record decides, not
/// the model's memory: the change happened, here, and is the last of its
/// object; its backup is still kept; and nobody else has changed the object
/// since, which its spec's generation tells.
fn plan_undo(cluster: &PinnedCluster, id: &str) -> Result<Plan, ToolError> {
    if !cluster.writes {
        return Err(ToolError::KubeReadOnly);
    }
    let refused = |why: String| invalid("kubeUndo", why);
    let id = id.trim();
    let changes = cluster.changes.ok_or_else(|| refused("this chat keeps no record of changes".to_string()))?;
    let history = changes.history().map_err(refused)?;
    let made = undoable(&history, id).map_err(refused)?;
    if (made.kubeconfig.as_str(), made.context.as_str(), made.namespace.as_str()) != (cluster.kubeconfig, cluster.context, cluster.namespace) {
        return Err(refused(format!(
            "the change {id} was made in context {} · namespace {} of the kubeconfig {} — it is undone from a chat pinned there, \
             which the user can open",
            made.context, made.namespace, made.kubeconfig
        )));
    }
    if !can_be_undone(&made.tool) {
        return Err(refused(format!("{id} was a rollout restart, which cannot be undone — its pods are already replaced")));
    }
    let (_, was) = changes.load(id).map_err(refused)?;
    let kind = if made.group.is_empty() { made.kind.clone() } else { format!("{}.{}", made.kind, made.group) };
    let name = made.name.clone();
    let plan = match made.tool.as_str() {
        "kubeScale" => {
            let replicas = was.pointer("/spec/replicas").and_then(Value::as_u64).and_then(|n| u32::try_from(n).ok()).unwrap_or(1);
            plan_scale(cluster, &KubeScaleArgs { kind, name, replicas: Some(replicas) })?
        }
        "kubeSuspend" => {
            let suspend = was.pointer("/spec/suspend").and_then(Value::as_bool).unwrap_or(false);
            plan_suspend(cluster, &KubeSuspendArgs { kind, name, suspend: Some(suspend) })?
        }
        "kubeRolloutUndo" => {
            let (kind, object) = target(cluster, "kubeUndo", &kind, &name, &["Deployment"], |kind| format!("a {kind}'s pod template is not put back here"))?;
            let wanted = was.pointer("/spec/template").cloned().unwrap_or(Value::Null);
            plan_template(cluster, kind, &name, object, &wanted, format!("the pod template as it was before {id}"))?
        }
        // What an apply made is taken away; what it changed is put back whole.
        "kubeApply" => {
            let kind = self::kind(cluster, &kind)?;
            let object = found(cluster, &kind, &name)?;
            if object.is_null() {
                return Err(refused(format!("{}/{name} is no longer there — it was deleted after {id}, and there is nothing to undo", kind.kind)));
            }
            let plan = if was.is_null() {
                Plan { tool: "kubeDelete", kind, name, object, act: Act::Delete, summary: format!("delete — it did not exist before {id}"), diff: None, warnings: Vec::new() }
            } else {
                let Some(back) = merge_diff(&declared(object.clone()), &declared(was.clone())) else {
                    return Err(refused(format!("{}/{name} is already as it was before {id} — there is nothing to change", kind.kind)));
                };
                let diff = diff_stats(&shown(&kind, &object), &shown(&kind, &was));
                Plan { diff: Some(diff), ..Plan::patching("kubeApply", kind, &name, object, back, format!("as it was before {id}")) }
            };
            checked(cluster, plan)?
        }
        // What was deleted is made again from its backup.
        "kubeDelete" => {
            let kind = self::kind(cluster, &kind)?;
            if !found(cluster, &kind, &name)?.is_null() {
                return Err(ToolError::KubeChangedSince(format!("{}/{name} exists again — someone recreated it after {id}", kind.kind)));
            }
            let diff = diff_stats("", &shown(&kind, &was));
            let summary = format!("create again, from the backup of {id}");
            checked(cluster, Plan { tool: "kubeApply", kind, name, object: Value::Null, act: Act::Apply(declared(was)), summary, diff: Some(diff), warnings: Vec::new() })?
        }
        other => return Err(refused(format!("a {other} change cannot be undone"))),
    };
    if !plan.object.is_null() && !same_since(&plan.object, made) {
        return Err(ToolError::KubeChangedSince(format!(
            "{}/{} was changed by someone else after {id} — undoing it now would be: {}",
            plan.kind.kind, plan.name, plan.summary
        )));
    }
    Ok(plan)
}

/// The changes a call asks for, each accepted by the server as a dry run —
/// one, but for a manifest of several objects. With them, the change an undo
/// puts back.
fn planned<'c>(cluster: &PinnedCluster, call: &'c ToolCall) -> Result<(Vec<Plan>, Option<&'c str>), ToolError> {
    let (mut plans, undoes) = plans_of(cluster, call)?;
    for plan in &mut plans {
        let reverters = reverters(cluster, plan);
        plan.warnings.extend(reverters);
    }
    Ok((plans, undoes))
}

fn plans_of<'c>(cluster: &PinnedCluster, call: &'c ToolCall) -> Result<(Vec<Plan>, Option<&'c str>), ToolError> {
    Ok(match call {
        ToolCall::KubeScale(args) => (vec![plan_scale(cluster, args)?], None),
        ToolCall::KubeSuspend(args) => (vec![plan_suspend(cluster, args)?], None),
        ToolCall::KubeRolloutRestart(args) => (vec![plan_restart(cluster, args)?], None),
        ToolCall::KubeRolloutUndo(args) => (vec![plan_rollout_undo(cluster, args)?], None),
        ToolCall::KubeApply(args) => (plan_apply(cluster, args)?, None),
        ToolCall::KubeDelete(args) => (vec![plan_delete(cluster, args)?], None),
        ToolCall::KubeUndo(args) => (vec![plan_undo(cluster, &args.change_id)?], Some(args.change_id.trim())),
        _ => (Vec::new(), None),
    })
}

fn changes(call: &ToolCall) -> bool {
    matches!(
        call,
        ToolCall::KubeScale(_)
            | ToolCall::KubeSuspend(_)
            | ToolCall::KubeRolloutRestart(_)
            | ToolCall::KubeRolloutUndo(_)
            | ToolCall::KubeApply(_)
            | ToolCall::KubeDelete(_)
            | ToolCall::KubeUndo(_)
    )
}

/// A change that would be refused is refused before anyone is asked to
/// approve it: no card for a call that cannot run. Reads pass.
pub fn preflight(kube: Option<PinnedCluster>, call: &ToolCall) -> Result<(), ToolError> {
    if !changes(call) {
        return Ok(());
    }
    planned(&cluster(kube)?, call).map(|_| ())
}

/// A warning as the card and the answer give it: under the object's name
/// where there are several to tell apart.
fn named(plans: &[Plan], plan: &Plan, warning: &str) -> String {
    if plans.len() > 1 { format!("{}: {warning}", titled(plan)) } else { warning.to_string() }
}

/// What will put an object back by itself (`docs/21-kubernetes-mode.md`,
/// "Что отменит изменение само"), read from the object the plan already has
/// — and, for a scale, from the namespace's autoscalers. Not a refusal: a
/// scale to zero under an HPA for five minutes can be exactly what is wanted.
/// But it is the user's to decide seeing it, not the model's to pass over.
fn reverters(cluster: &PinnedCluster, plan: &Plan) -> Vec<String> {
    let object = &plan.object;
    let mut found = Vec::new();
    let keys = |section: &str| -> Vec<String> {
        let map = object.pointer(&format!("/metadata/{section}")).and_then(Value::as_object);
        map.into_iter().flat_map(|map| map.keys().cloned()).collect()
    };
    let marked = |prefix: &str| keys("labels").iter().chain(keys("annotations").iter()).any(|key| key.starts_with(prefix));
    if plan.tool == "kubeScale" {
        let scalers = kind(cluster, "horizontalpodautoscalers.autoscaling")
            .and_then(|scalers| Ok(cluster.api.list(&scalers, &ListQuery { namespace: Some(cluster.namespace.to_string()), ..Default::default() })?))
            .map(|page| page.items)
            .unwrap_or_default();
        for scaler in scalers {
            let target = &scaler["spec"]["scaleTargetRef"];
            if target["kind"] == plan.kind.kind.as_str() && target["name"] == plan.name.as_str() {
                let bound = |field: &str| scaler["spec"][field].as_i64().map_or("?".to_string(), |n| n.to_string());
                found.push(format!(
                    "HorizontalPodAutoscaler/{} sets its replicas ({}–{}): it will scale it back.",
                    scaler["metadata"]["name"].as_str().unwrap_or("?"),
                    bound("minReplicas"),
                    bound("maxReplicas")
                ));
            }
        }
    }
    let flux = marked("kustomize.toolkit.fluxcd.io/") || marked("helm.toolkit.fluxcd.io/");
    if marked("argocd.argoproj.io/") {
        found.push("Managed by Argo CD: with self-heal on, it puts back what Git says.".to_string());
    }
    if flux {
        found.push("Managed by Flux: its next reconcile puts back what Git says.".to_string());
    }
    // Flux's Helm releases carry Helm's label too; the one line says enough.
    if !flux && object.pointer("/metadata/labels/app.kubernetes.io~1managed-by").and_then(Value::as_str) == Some("Helm") {
        found.push("Installed by Helm: the next `helm upgrade` overwrites this.".to_string());
    }
    let owners = object.pointer("/metadata/ownerReferences").and_then(Value::as_array);
    for owner in owners.into_iter().flatten().filter(|owner| owner["controller"] == true) {
        found.push(format!(
            "Owned by {}/{}: its owner may put it back, or make another.",
            owner["kind"].as_str().unwrap_or("?"),
            owner["name"].as_str().unwrap_or("?")
        ));
    }
    found
}

fn titled(plan: &Plan) -> String {
    format!("{}/{}", plan.kind.kind, plan.name)
}

/// What a change would do, for its approval card: where — the first thing
/// to read before agreeing — what becomes of what, and whether it can be
/// undone.
pub fn preview(kube: Option<PinnedCluster>, call: &ToolCall) -> ToolPreview {
    if !changes(call) {
        return ToolPreview::Nothing;
    }
    let (cluster, plans, undoes) = match cluster(kube).and_then(|cluster| planned(&cluster, call).map(|(plans, undoes)| (cluster, plans, undoes))) {
        Ok(found) => found,
        Err(e) => return ToolPreview::Failed { reason: e.to_string() },
    };
    let what: Vec<String> = plans.iter().map(|plan| format!("{}: {}", titled(plan), plan.summary)).collect();
    let what = match what.as_slice() {
        [one] => one.clone(),
        many => format!("{} objects — {}", many.len(), many.join("; ")),
    };
    let way_back = match undoes {
        Some(_) => "Put back from the change's backup. An undo is a change too: it is backed up and can be undone.",
        None if plans.iter().all(|plan| can_be_undone(plan.tool)) => "Can be undone: the object is backed up first.",
        None => "Cannot be undone: the pods are replaced, and the old ones do not come back.",
    };
    let mut notes: Vec<String> = plans.iter().flat_map(|plan| plan.warnings.iter().map(|warning| named(&plans, plan, warning))).collect();
    notes.push(way_back.to_string());
    ToolPreview::Change {
        place: format!("context {} · namespace {}", cluster.context, cluster.namespace),
        summary: undoes.map_or(what.clone(), |id| format!("Undo {id} — {what}")),
        notes,
        production: cluster.production,
        diffs: plans.iter().filter_map(|plan| Some(ChangeDiff { title: titled(plan), diff: plan.diff.clone()? })).collect(),
    }
}

/// Makes a planned change: the backup — no backup, no change — the change,
/// the audit line. `undoes` is the change it puts back, when it is an undo.
/// Answers with what it did, in a sentence.
fn make(cluster: &PinnedCluster, plan: &Plan, undoes: Option<&str>) -> Result<String, ToolError> {
    let changes = cluster.changes.ok_or_else(|| ToolError::KubeBackup("this chat has nowhere to record changes".to_string()))?;
    let mut change = KubeChange {
        id: format!("kc-{}", &uuid::Uuid::new_v4().simple().to_string()[..8]),
        at: Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
        kubeconfig: cluster.kubeconfig.to_string(),
        context: cluster.context.to_string(),
        namespace: cluster.namespace.to_string(),
        group: plan.kind.group.clone(),
        version: plan.kind.version.clone(),
        kind: plan.kind.kind.clone(),
        plural: plan.kind.plural.clone(),
        name: plan.name.clone(),
        // What kind of change it is — and so how it is put back — whether
        // a tool or an undo made it.
        tool: plan.tool.to_string(),
        summary: plan.summary.clone(),
        generation_before: generation(&plan.object),
        generation_after: None,
        version_after: None,
        error: None,
        undoes: undoes.map(str::to_string),
    };
    // `Null` for what was not there: the way back from a creation is a deletion.
    changes.backup(&change, &plan.object).map_err(ToolError::KubeBackup)?;
    let done = plan.run(cluster, false);
    match &done {
        Ok(object) => {
            change.generation_after = generation(object);
            change.version_after = if change.generation_after.is_none() { version(object) } else { None };
        }
        Err(e) => change.error = Some(e.to_string()),
    }
    // The change is made or refused either way; an audit that could not be
    // written is said, not hidden behind the result.
    let audited = changes.audit(&change);
    done?;
    let what = format!("{} in namespace {}: {}", titled(plan), cluster.namespace, change.summary);
    let mut text = match undoes {
        Some(undone) => format!("Undid {undone} — {what}. Change id {} — undoing that makes the change again.", change.id),
        None if can_be_undone(plan.tool) => format!("{what}. Change id {} — the object as it was is backed up.", change.id),
        None => format!("{what}. Change id {} — on record, but a restart cannot be undone.", change.id),
    };
    if let Err(e) = audited {
        text.push_str(&format!("\nThe audit line could not be written: {e}"));
    }
    Ok(text)
}

/// A changing tool, whole (`docs/21-kubernetes-mode.md`, K-5): the plan —
/// dry run and all — then for each object the backup, the change, the audit
/// line. A manifest stops at the first object the server refuses, and says
/// what it had already changed.
pub fn change(kube: Option<PinnedCluster>, call: &ToolCall) -> Result<ToolResult, ToolError> {
    let cluster = cluster(kube)?;
    let (plans, undoes) = planned(&cluster, call)?;
    let mut said: Vec<String> = Vec::new();
    for plan in &plans {
        match make(&cluster, plan, undoes) {
            Ok(text) => {
                said.push(text);
                // The user read these on the card; the model says what they mean for the change.
                said.extend(plan.warnings.iter().map(|warning| format!("  Note — {}", named(&plans, plan, warning))));
            }
            Err(e) if said.is_empty() => return Err(e),
            Err(e) => {
                let stopped = format!("{}\nThen {} failed, and nothing after it was changed: {e}", said.join("\n"), titled(plan));
                return Err(ToolError::Kube(KubeError::Cluster(stopped)));
            }
        }
    }
    let summary = match plans.as_slice() {
        [] => return Err(invalid("kube", "not a change")),
        [one] => one.summary.clone(),
        many => format!("{} objects", many.len()),
    };
    Ok(ToolResult::Kube { text: said.join("\n"), summary })
}

/// What every changing tool says of itself.
const CHANGE: &str = "Works only when the user has switched the chat's tab to \"Changes\"; otherwise it answers that the chat \
    is read-only. The user approves it on a card; the object is backed up first. Returns what it was and is, and a change id.";

/// How every Kubernetes tool is told where it reads.
const WHERE: &str = "Reads the cluster, context and namespace this chat is pinned to — you cannot pick another cluster.";

pub(super) fn list_definition() -> LlmToolDefinition {
    LlmToolDefinition {
        name: "kubeList".to_string(),
        description: format!(
            "List objects of one kind as a table with the columns `kubectl get` shows (a Pod's readiness, status, restarts, node). \
             Any kind the cluster serves, CRDs too (VirtualService, Certificate). `fields` adds columns from each object — the way \
             to compare one field across many objects in one call instead of a kubeGet each: \
             fields: [\"metadata.annotations.networking\\\\.istio\\\\.io/exportTo\"]. A field an object lacks shows as —. \
             A Secret shows keys and sizes, never values. {WHERE}"
        ),
        parameters: serde_json::json!({
            "type": "object",
            "properties": {
                "kind": { "type": "string", "description": "Kind, plural or short name: Deployment, pods, svc. Add the group when two serve one name: Gateway.networking.istio.io." },
                "namespace": { "type": "string", "description": "Default: the chat's namespace. `*` lists every namespace. Ignored for cluster-scoped kinds." },
                "labelSelector": { "type": "string", "description": "e.g. app=orders,tier!=cache" },
                "fieldSelector": { "type": "string", "description": "e.g. status.phase!=Running, spec.nodeName=node-1" },
                "fields": { "type": "array", "items": { "type": "string" }, "description": "Paths shown as extra columns: dots between keys, `\\\\.` for a dot inside a key (or ['key.with.dots']), [*] for every item, [0] for one." },
                "limit": { "type": "integer", "description": "Objects per page, default 200, at most 500." },
                "continue": { "type": "string", "description": "The token the previous page ended with." }
            },
            "required": ["kind"]
        }),
    }
}

pub(super) fn get_definition() -> LlmToolDefinition {
    LlmToolDefinition {
        name: "kubeGet".to_string(),
        description: format!(
            "One object as YAML, without managedFields, the last-applied copy and server counters; annotations and labels \
             stay. Ask for `sections` to read only part — [\"spec\"] for the desired state, [\"status\"] for what the \
             cluster reports. A Secret shows keys and sizes, never values; env values named like credentials are \
             <redacted>. {WHERE}"
        ),
        parameters: serde_json::json!({
            "type": "object",
            "properties": {
                "kind": { "type": "string", "description": "Kind, plural or short name; CRDs too." },
                "name": { "type": "string" },
                "namespace": { "type": "string", "description": "Default: the chat's namespace." },
                "sections": { "type": "array", "items": { "type": "string" }, "description": "Top-level keys to keep: metadata, spec, status, data." }
            },
            "required": ["kind", "name"]
        }),
    }
}

pub(super) fn events_definition() -> LlmToolDefinition {
    LlmToolDefinition {
        name: "kubeEvents".to_string(),
        description: format!(
            "Events oldest first, each distinct one once with its count summed. Of one object with kind and name — a \
             Deployment's pods have their own events, under each Pod. The cluster keeps events about an hour. {WHERE}"
        ),
        parameters: serde_json::json!({
            "type": "object",
            "properties": {
                "namespace": { "type": "string", "description": "Default: the chat's namespace. `*` for every namespace." },
                "kind": { "type": "string", "description": "The object's kind, with name." },
                "name": { "type": "string", "description": "The object's name." },
                "warningsOnly": { "type": "boolean" }
            },
            "required": []
        }),
    }
}

pub(super) fn logs_definition() -> LlmToolDefinition {
    LlmToolDefinition {
        name: "kubeLogs".to_string(),
        description: format!(
            "Container logs, the last lines. Of one pod, of every pod of an owner (kind and name: a Deployment, \
             StatefulSet, DaemonSet, Job, Service), or of a labelSelector's pods — several pods are merged by time, each \
             line marked with its pod, since which pod served a request is unknown. Up to {LOG_PODS} pods. Repeated lines \
             are said once with ×N. The container is the pod's default unless named; a mesh sidecar is \
             container: \"istio-proxy\". `previous` reads the container before its last restart — the crash. `grep` \
             searches further back than the tail. {WHERE}"
        ),
        parameters: serde_json::json!({
            "type": "object",
            "properties": {
                "pod": { "type": "string" },
                "kind": { "type": "string", "description": "An owner's kind, with name." },
                "name": { "type": "string", "description": "An owner's name." },
                "labelSelector": { "type": "string" },
                "namespace": { "type": "string", "description": "Default: the chat's namespace." },
                "container": { "type": "string" },
                "previous": { "type": "boolean" },
                "tail": { "type": "integer", "description": "Lines in all, across the pods; default 200, at most 2000." },
                "since": { "type": "string", "description": "Only this recent: 30s, 10m, 2h, 1d." },
                "grep": { "type": "string", "description": "A regex lines must match, case-insensitive: a path, a request id, `error|timeout`." }
            },
            "required": []
        }),
    }
}

pub(super) fn top_definition() -> LlmToolDefinition {
    LlmToolDefinition {
        name: "kubeTop".to_string(),
        description: format!(
            "CPU and memory in use now, busiest first — from metrics-server, which keeps no history. Says so when the \
             cluster has none. {WHERE}"
        ),
        parameters: serde_json::json!({
            "type": "object",
            "properties": {
                "kind": { "type": "string", "enum": ["pods", "nodes"] },
                "namespace": { "type": "string", "description": "For pods. Default: the chat's namespace; `*` for every namespace." }
            },
            "required": ["kind"]
        }),
    }
}

pub(super) fn field_history_definition() -> LlmToolDefinition {
    LlmToolDefinition {
        name: "kubeFieldHistory".to_string(),
        description: format!(
            "Who set each field of an object and when, from its managedFields: field → manager (kubectl, helm, argocd, \
             ansible, a controller), operation, time. For questions of origin — was this deployed by X, since when is this \
             annotation here. The time is the manager's last write of any field. {WHERE}"
        ),
        parameters: serde_json::json!({
            "type": "object",
            "properties": {
                "kind": { "type": "string" },
                "name": { "type": "string" },
                "namespace": { "type": "string", "description": "Default: the chat's namespace." },
                "paths": { "type": "array", "items": { "type": "string" }, "description": "Only fields under these, e.g. metadata.annotations, spec.template.spec.containers." }
            },
            "required": ["kind", "name"]
        }),
    }
}

pub(super) fn diagnose_definition() -> LlmToolDefinition {
    LlmToolDefinition {
        name: "kubeDiagnose".to_string(),
        description: format!(
            "Why a workload or pod is unhealthy, in one call instead of five: the object's status and conditions, its \
             pods and what is wrong with each (waiting reasons, restarts, how the last run ended, why one is not \
             scheduled — requests, nodeSelector, unbound claims), the events of it, its ReplicaSets and pods, and the \
             log of the container in trouble — from before its last restart when it crashed. Facts, not a verdict: the \
             hypothesis is yours. For a Deployment, StatefulSet, DaemonSet, Job, ReplicaSet or Pod. {WHERE}"
        ),
        parameters: serde_json::json!({
            "type": "object",
            "properties": {
                "kind": { "type": "string", "description": "Deployment, StatefulSet, DaemonSet, Job, ReplicaSet or Pod." },
                "name": { "type": "string" },
                "namespace": { "type": "string", "description": "Default: the chat's namespace." }
            },
            "required": ["kind", "name"]
        }),
    }
}

pub(super) fn wait_rollout_definition() -> LlmToolDefinition {
    LlmToolDefinition {
        name: "kubeWaitRollout".to_string(),
        description: format!(
            "Waits for a rollout of a Deployment, StatefulSet or DaemonSet in the chat's namespace and answers how it \
             ended: done, with the numbers; stuck — the controller gave up (ProgressDeadlineExceeded); or not done in \
             time. When it is not done the answer carries the diagnosis too: pods and what is wrong with each, events, \
             the log of the container in trouble. Done is the controller's count of available pods: a container that \
             crashes a moment after starting was counted — kubeDiagnose when in doubt. One call after a restart, a rollback, an apply or a scale — instead \
             of reading the object round after round. {WHERE}"
        ),
        parameters: serde_json::json!({
            "type": "object",
            "properties": {
                "kind": { "type": "string", "description": "Deployment, StatefulSet or DaemonSet." },
                "name": { "type": "string" },
                "timeoutSeconds": { "type": "integer", "description": "How long to wait. Default 120, at most 600." }
            },
            "required": ["kind", "name"]
        }),
    }
}

pub(super) fn scale_definition() -> LlmToolDefinition {
    LlmToolDefinition {
        name: "kubeScale".to_string(),
        description: "Set the replicas of a Deployment, StatefulSet or ReplicaSet in the chat's own namespace — a change is \
            never made in another namespace or cluster. Works only when the user has switched the chat's tab to \"Changes\"; \
            otherwise it answers that the chat is read-only. The user approves it on a card showing the cluster and the \
            counts; the object is backed up first. Returns what it was and is, and a change id. To scale several, call it \
            for each in one round — they share one card. A DaemonSet, CronJob or Job has no replicas: it answers how those \
            are stopped."
            .to_string(),
        parameters: serde_json::json!({
            "type": "object",
            "properties": {
                "kind": { "type": "string", "description": "Deployment, StatefulSet or ReplicaSet." },
                "name": { "type": "string" },
                "replicas": { "type": "integer", "description": "The number to scale to; 0 stops it." }
            },
            "required": ["kind", "name", "replicas"]
        }),
    }
}

pub(super) fn suspend_definition() -> LlmToolDefinition {
    LlmToolDefinition {
        name: "kubeSuspend".to_string(),
        description: format!(
            "Suspend a CronJob or a Job, or let it run again, in the chat's own namespace: spec.suspend. A suspended CronJob \
             starts no new Jobs — the ones already running finish; a suspended Job's pods are removed until it is resumed. \
             This is how those are stopped: they have no replicas. {CHANGE}"
        ),
        parameters: serde_json::json!({
            "type": "object",
            "properties": {
                "kind": { "type": "string", "description": "CronJob or Job." },
                "name": { "type": "string" },
                "suspend": { "type": "boolean", "description": "true to stop it, false to let it run again." }
            },
            "required": ["kind", "name", "suspend"]
        }),
    }
}

pub(super) fn rollout_restart_definition() -> LlmToolDefinition {
    LlmToolDefinition {
        name: "kubeRolloutRestart".to_string(),
        description: format!(
            "Restart a Deployment, StatefulSet or DaemonSet in the chat's own namespace, as `kubectl rollout restart` does: \
             every pod is replaced, a few at a time. It CANNOT be undone — the card tells the user so. Use it when a restart \
             is what is wanted (a config or secret to pick up, a stuck process), not as a guess at a fix: a crash loop is \
             not cured by restarting. {CHANGE}"
        ),
        parameters: serde_json::json!({
            "type": "object",
            "properties": {
                "kind": { "type": "string", "description": "Deployment, StatefulSet or DaemonSet." },
                "name": { "type": "string" }
            },
            "required": ["kind", "name"]
        }),
    }
}

pub(super) fn rollout_undo_definition() -> LlmToolDefinition {
    LlmToolDefinition {
        name: "kubeRolloutUndo".to_string(),
        description: format!(
            "Roll a Deployment in the chat's own namespace back to an earlier revision, as `kubectl rollout undo` does: its \
             pod template becomes that revision's — the one before the current unless toRevision names another. The \
             revisions kept are the Deployment's ReplicaSets (kubeList ReplicaSet shows them). The card shows the revisions \
             and the image on either side. A Deployment managed by Argo CD, Flux or Helm may be put back by them — say so. {CHANGE}"
        ),
        parameters: serde_json::json!({
            "type": "object",
            "properties": {
                "kind": { "type": "string", "description": "Deployment." },
                "name": { "type": "string" },
                "toRevision": { "type": "integer", "description": "The revision to go back to; omit for the previous one." }
            },
            "required": ["kind", "name"]
        }),
    }
}

pub(super) fn apply_definition() -> LlmToolDefinition {
    LlmToolDefinition {
        name: "kubeApply".to_string(),
        description: format!(
            "Create or update objects in the chat's own namespace from a YAML manifest, as `kubectl apply` does — up to \
             {APPLY_MOST} objects, `---` between them, on one card with each object's diff. Send each object WHOLE, as it \
             should be: what the manifest leaves out of an object this tool applied before is removed. To change an \
             existing object, read it with kubeGet first and send it back with your change. metadata.namespace may be \
             omitted; another namespace, or a cluster-wide kind (Namespace, ClusterRole, a CRD), is refused. It takes \
             over fields other tools set — an object managed by Argo CD, Flux or Helm will be put back by them; say so \
             rather than apply. An object the manifest would not change is skipped. Each object changed answers with its \
             own change id. {CHANGE}"
        ),
        parameters: serde_json::json!({
            "type": "object",
            "properties": {
                "manifest": { "type": "string", "description": "YAML: one or more whole objects, `---` between them." }
            },
            "required": ["manifest"]
        }),
    }
}

pub(super) fn delete_definition() -> LlmToolDefinition {
    LlmToolDefinition {
        name: "kubeDelete".to_string(),
        description: format!(
            "Delete one object in the chat's own namespace. It is backed up whole, and kubeUndo creates it again from \
             that — the object, not what it held: a deleted Deployment's pods are new ones, and a PersistentVolumeClaim's \
             data is gone unless its volume is kept (the card says which). What an owner manages — a pod of a Deployment \
             — comes back by itself: delete the owner, or scale it. A Namespace and other cluster-wide kinds are refused. {CHANGE}"
        ),
        parameters: serde_json::json!({
            "type": "object",
            "properties": {
                "kind": { "type": "string" },
                "name": { "type": "string" }
            },
            "required": ["kind", "name"]
        }),
    }
}

pub(super) fn undo_definition() -> LlmToolDefinition {
    LlmToolDefinition {
        name: "kubeUndo".to_string(),
        description: "Put a change back by its change id — the id a changing tool answered with, or one the user names. The \
            app restores from the backup it took before the change, so it works after the conversation has forgotten the \
            details; use it rather than changing the object back yourself. Needs \"Changes\" on the chat's tab and the \
            user's approval on a card, like any change, and is one itself: it answers with a new change id, and undoing \
            that makes the change again. It refuses when the object has been changed since — by a later change of yours \
            (undo that first) or by someone else (then the user decides what to do) — when the change was made \
            in another cluster or namespace, and when its backup is older than 30 days."
            .to_string(),
        parameters: serde_json::json!({
            "type": "object",
            "properties": {
                "changeId": { "type": "string", "description": "The change to undo, e.g. kc-1a2b3c4d." }
            },
            "required": ["changeId"]
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::tools::KubeUndoArgs;
    use crate::domain::kube::{KubeApi, KubeChanges, KubeError, ListPage};
    use serde_json::json;
    use std::sync::Mutex;

    /// A cluster in memory: objects by kind, logs by pod, and what it was asked.
    #[derive(Default)]
    struct Fake {
        kinds: Vec<KubeKind>,
        objects: Vec<(String, Value)>,
        logs: Vec<(String, Result<String, String>)>,
        asked: Mutex<Vec<String>>,
        /// What the server says to every patch, when it refuses them.
        refuses: Option<String>,
        /// What a watched object becomes, change by change.
        later: Vec<Value>,
    }

    /// What was backed up and audited; `full` is a disk that takes no backup.
    #[derive(Default)]
    struct Recorded {
        backups: Mutex<Vec<(KubeChange, Value)>>,
        audits: Mutex<Vec<KubeChange>>,
        full: bool,
    }

    impl KubeChanges for Recorded {
        fn backup(&self, change: &KubeChange, object: &Value) -> Result<(), String> {
            if self.full {
                return Err("no space left".to_string());
            }
            self.backups.lock().unwrap().push((change.clone(), object.clone()));
            Ok(())
        }

        fn audit(&self, change: &KubeChange) -> Result<(), String> {
            self.audits.lock().unwrap().push(change.clone());
            Ok(())
        }

        fn load(&self, id: &str) -> Result<(KubeChange, Value), String> {
            let backups = self.backups.lock().unwrap();
            backups.iter().find(|(change, _)| change.id == id).cloned().ok_or_else(|| format!("there is no backup of the change {id}"))
        }

        fn history(&self) -> Result<Vec<KubeChange>, String> {
            Ok(self.audits.lock().unwrap().clone())
        }
    }

    impl Fake {
        fn with(kinds: &[(&str, &str, &str)]) -> Fake {
            let kinds = kinds
                .iter()
                .map(|(group, kind, plural)| KubeKind {
                    group: group.to_string(),
                    version: "v1".into(),
                    kind: kind.to_string(),
                    plural: plural.to_string(),
                    namespaced: !["Node", "NodeMetrics", "PersistentVolume"].contains(kind),
                })
                .collect();
            Fake { kinds, ..Default::default() }
        }

        fn object(mut self, plural: &str, object: Value) -> Fake {
            self.objects.push((plural.to_string(), object));
            self
        }

        fn log(mut self, pod: &str, text: Result<&str, &str>) -> Fake {
            self.logs.push((pod.to_string(), text.map(str::to_string).map_err(str::to_string)));
            self
        }

        fn asked(&self) -> Vec<String> {
            self.asked.lock().unwrap().clone()
        }
    }

    /// RFC 7386, as the server applies a merge patch.
    fn merge(object: &mut Value, patch: &Value) {
        let Some(patch) = patch.as_object() else { return *object = patch.clone() };
        if !object.is_object() {
            *object = json!({});
        }
        for (key, value) in patch {
            if value.is_null() {
                object.as_object_mut().unwrap().remove(key);
            } else {
                merge(&mut object[key], value);
            }
        }
    }

    /// The tools by their old names: each is `change` on its own call.
    fn kube_scale(kube: Option<PinnedCluster>, args: &KubeScaleArgs) -> Result<ToolResult, ToolError> {
        change(kube, &ToolCall::KubeScale(args.clone()))
    }

    fn kube_undo(kube: Option<PinnedCluster>, args: &KubeUndoArgs) -> Result<ToolResult, ToolError> {
        change(kube, &ToolCall::KubeUndo(args.clone()))
    }

    fn labelled(object: &Value, selector: &str) -> bool {
        selector.split(',').all(|term| {
            let (key, value) = term.split_once('=').unwrap_or((term, ""));
            object.pointer(&format!("/metadata/labels/{key}")).and_then(Value::as_str) == Some(value)
        })
    }

    impl KubeApi for Fake {
        fn kinds(&self) -> Result<Vec<KubeKind>, KubeError> {
            Ok(self.kinds.clone())
        }

        fn list(&self, kind: &KubeKind, query: &ListQuery) -> Result<ListPage, KubeError> {
            self.asked.lock().unwrap().push(format!("list {} {:?} {:?} {:?}", kind.plural, query.namespace, query.label_selector, query.field_selector));
            let items = self
                .objects
                .iter()
                .filter(|(plural, _)| plural == &kind.plural)
                .map(|(_, o)| o.clone())
                .filter(|o| query.namespace.as_ref().is_none_or(|ns| o.pointer("/metadata/namespace").and_then(Value::as_str) == Some(ns)))
                .filter(|o| query.label_selector.as_deref().is_none_or(|s| labelled(o, s)))
                .collect();
            Ok(ListPage { items, ..Default::default() })
        }

        fn get(&self, kind: &KubeKind, namespace: &str, name: &str) -> Result<Value, KubeError> {
            self.asked.lock().unwrap().push(format!("get {} {namespace} {name}", kind.plural));
            self.objects
                .iter()
                .find(|(plural, o)| plural == &kind.plural && o["metadata"]["name"] == name && (!kind.namespaced || o["metadata"]["namespace"] == namespace))
                .map(|(_, o)| o.clone())
                .ok_or_else(|| KubeError::NotFound(format!("{} \"{name}\" not found", kind.plural)))
        }

        fn logs(&self, namespace: &str, pod: &str, query: &LogQuery) -> Result<String, KubeError> {
            let previous = if query.previous { " previous" } else { "" };
            self.asked.lock().unwrap().push(format!("logs {namespace} {pod} {:?} {:?}{previous}", query.container, query.tail));
            let (_, text) = self.logs.iter().find(|(p, _)| p == pod).expect("a log for the pod");
            text.clone().map_err(KubeError::Cluster)
        }

        fn patch(&self, kind: &KubeKind, namespace: &str, name: &str, patch: &Value, dry_run: bool) -> Result<Value, KubeError> {
            let how = if dry_run { " dry" } else { "" };
            self.asked.lock().unwrap().push(format!("patch {} {namespace} {name} {patch}{how}", kind.plural));
            if let Some(why) = &self.refuses {
                return Err(KubeError::Cluster(why.clone()));
            }
            let mut object = self.get(kind, namespace, name)?;
            merge(&mut object, patch);
            object["metadata"]["generation"] = json!(object["metadata"]["generation"].as_i64().unwrap_or(0) + 1);
            Ok(object)
        }

        /// As the server applies: the manifest over what is there, or alone
        /// when nothing is. A kind with a `spec` has a generation; one
        /// without — a ConfigMap — has only its resource version.
        fn apply(&self, kind: &KubeKind, namespace: &str, name: &str, manifest: &Value, dry_run: bool) -> Result<Value, KubeError> {
            let how = if dry_run { " dry" } else { "" };
            self.asked.lock().unwrap().push(format!("apply {} {namespace} {name} {manifest}{how}", kind.plural));
            if let Some(why) = &self.refuses {
                return Err(KubeError::Cluster(why.clone()));
            }
            let mut object = self.get(kind, namespace, name).unwrap_or(json!({}));
            merge(&mut object, manifest);
            let bumped = |path: &str| json!(object.pointer(path).and_then(Value::as_i64).unwrap_or(0) + 1);
            if object.get("spec").is_some() {
                object["metadata"]["generation"] = bumped("/metadata/generation");
            } else {
                let next = object.pointer("/metadata/resourceVersion").and_then(Value::as_str).and_then(|v| v.parse::<i64>().ok()).unwrap_or(0) + 1;
                object["metadata"]["resourceVersion"] = json!(next.to_string());
            }
            Ok(object)
        }

        fn delete(&self, kind: &KubeKind, namespace: &str, name: &str, dry_run: bool) -> Result<(), KubeError> {
            let how = if dry_run { " dry" } else { "" };
            self.asked.lock().unwrap().push(format!("delete {} {namespace} {name}{how}", kind.plural));
            match &self.refuses {
                Some(why) => Err(KubeError::Cluster(why.clone())),
                None => self.get(kind, namespace, name).map(|_| ()),
            }
        }

        fn watch(&self, kind: &KubeKind, namespace: &str, name: &str, timeout: std::time::Duration, stop: &dyn Fn() -> bool, seen: &mut dyn FnMut(&Value) -> bool) -> Result<(), KubeError> {
            let first = self.get(kind, namespace, name)?;
            self.asked.lock().unwrap().push(format!("watch {} {namespace} {name} {}s", kind.plural, timeout.as_secs()));
            for object in std::iter::once(&first).chain(&self.later) {
                if seen(object) || stop() {
                    break;
                }
                self.asked.lock().unwrap().push("shown".to_string());
            }
            Ok(())
        }
    }

    fn pinned(fake: &Fake) -> Option<PinnedCluster<'_>> {
        Some(PinnedCluster { api: fake, namespace: "orders", kubeconfig: "prod", context: "eks", writes: false, production: false, changes: None })
    }

    /// The same chat with "Changes" switched on.
    fn writing<'a>(fake: &'a Fake, recorded: &'a Recorded) -> Option<PinnedCluster<'a>> {
        Some(PinnedCluster { writes: true, changes: Some(recorded), ..pinned(fake).unwrap() })
    }

    fn workload(plural: &str, replicas: i64) -> (String, Value) {
        let object = json!({"metadata": {"name": "api", "namespace": "orders", "generation": 4}, "spec": {"replicas": replicas}});
        (plural.to_string(), object)
    }

    fn scalable(replicas: i64) -> Fake {
        let (plural, object) = workload("deployments", replicas);
        Fake::with(&[("apps", "Deployment", "deployments"), ("apps", "DaemonSet", "daemonsets"), ("batch", "CronJob", "cronjobs")])
            .object(&plural, object)
    }

    fn scale(replicas: u32) -> KubeScaleArgs {
        KubeScaleArgs { kind: "deploy".into(), name: "api".into(), replicas: Some(replicas) }
    }

    /// The order that makes a change safe: the server's dry run, the backup
    /// of the object as it was, the change, the audit — and what it was before
    /// in the answer, for "put it back".
    #[test]
    fn a_scale_is_dry_run_backed_up_made_and_audited_in_that_order() {
        let (fake, recorded) = (scalable(3), Recorded::default());
        let ToolResult::Kube { text, summary } = kube_scale(writing(&fake, &recorded), &scale(0)).unwrap() else { panic!() };
        assert_eq!(summary, "3 → 0 replicas");
        assert!(text.starts_with("Deployment/api in namespace orders: 3 → 0 replicas. Change id kc-"), "{text}");
        assert!(text.ends_with("the object as it was is backed up."), "{text}");
        let patches: Vec<String> = fake.asked().into_iter().filter(|a| a.starts_with("patch")).collect();
        assert_eq!(
            patches,
            ["patch deployments orders api {\"spec\":{\"replicas\":0}} dry", "patch deployments orders api {\"spec\":{\"replicas\":0}}"]
        );
        let backups = recorded.backups.lock().unwrap();
        let (change, object) = &backups[0];
        assert_eq!(object["spec"]["replicas"], 3, "the backup is the object before the change");
        assert!(text.contains(&change.id));
        assert_eq!(change.undoes, None);
        assert_eq!((change.kubeconfig.as_str(), change.context.as_str(), change.namespace.as_str()), ("prod", "eks", "orders"));
        assert_eq!((change.plural.as_str(), change.group.as_str(), change.tool.as_str()), ("deployments", "apps", "kubeScale"));
        let audits = recorded.audits.lock().unwrap();
        assert_eq!((audits[0].generation_before, audits[0].generation_after, &audits[0].error), (Some(4), Some(5), &None));
        assert_eq!(audits[0].id, change.id);
    }

    /// The switch on the tab is the policy: off, nothing is even asked of the server.
    #[test]
    fn a_read_only_chat_changes_nothing_and_says_how_to_allow_it() {
        let fake = scalable(3);
        let recorded = Recorded::default();
        let read_only = PinnedCluster { writes: false, ..writing(&fake, &recorded).unwrap() };
        assert!(matches!(kube_scale(Some(read_only), &scale(0)), Err(ToolError::KubeReadOnly)));
        assert!(matches!(preflight(Some(read_only), &ToolCall::KubeScale(scale(0))), Err(ToolError::KubeReadOnly)));
        assert!(fake.asked().is_empty(), "{:?}", fake.asked());
        assert!(ToolError::KubeReadOnly.to_string().contains("\"Changes\""));
    }

    /// No backup, no change: a cluster is not a working tree.
    #[test]
    fn a_scale_that_cannot_be_backed_up_is_not_made() {
        let fake = scalable(3);
        let full = Recorded { full: true, ..Default::default() };
        assert!(matches!(kube_scale(writing(&fake, &full), &scale(0)), Err(ToolError::KubeBackup(why)) if why == "no space left"));
        assert!(fake.asked().iter().all(|a| !a.starts_with("patch") || a.ends_with(" dry")), "{:?}", fake.asked());
        let nowhere = PinnedCluster { changes: None, ..writing(&fake, &full).unwrap() };
        assert!(matches!(kube_scale(Some(nowhere), &scale(0)), Err(ToolError::KubeBackup(_))));
    }

    /// What the server refuses — RBAC, a webhook — is known before the card.
    #[test]
    fn a_refused_dry_run_stops_the_call_before_anyone_is_asked() {
        let fake = Fake { refuses: Some("deployments.apps \"api\" is forbidden".into()), ..scalable(3) };
        let recorded = Recorded::default();
        let call = ToolCall::KubeScale(scale(0));
        assert!(matches!(preflight(writing(&fake, &recorded), &call), Err(ToolError::Kube(KubeError::Cluster(m))) if m.contains("forbidden")));
        assert!(matches!(preview(writing(&fake, &recorded), &call), ToolPreview::Failed { reason } if reason.contains("forbidden")));
        assert!(recorded.backups.lock().unwrap().is_empty());
    }

    /// A change that failed after its backup is still on record, with why.
    #[test]
    fn a_change_the_server_refuses_after_the_dry_run_is_audited_as_failed() {
        struct LateRefusal(Fake);
        impl KubeApi for LateRefusal {
            fn kinds(&self) -> Result<Vec<KubeKind>, KubeError> { self.0.kinds() }
            fn list(&self, kind: &KubeKind, query: &ListQuery) -> Result<ListPage, KubeError> { self.0.list(kind, query) }
            fn get(&self, kind: &KubeKind, namespace: &str, name: &str) -> Result<Value, KubeError> { self.0.get(kind, namespace, name) }
            fn logs(&self, namespace: &str, pod: &str, query: &LogQuery) -> Result<String, KubeError> { self.0.logs(namespace, pod, query) }
            fn patch(&self, kind: &KubeKind, namespace: &str, name: &str, patch: &Value, dry_run: bool) -> Result<Value, KubeError> {
                if dry_run { self.0.patch(kind, namespace, name, patch, dry_run) } else { Err(KubeError::Cluster("conflict".into())) }
            }
            fn apply(&self, kind: &KubeKind, namespace: &str, name: &str, object: &Value, dry_run: bool) -> Result<Value, KubeError> {
                if dry_run || name == "first" { self.0.apply(kind, namespace, name, object, dry_run) } else { Err(KubeError::Cluster("conflict".into())) }
            }
            fn delete(&self, kind: &KubeKind, namespace: &str, name: &str, dry_run: bool) -> Result<(), KubeError> { self.0.delete(kind, namespace, name, dry_run) }
        }
        let (late, recorded) = (LateRefusal(scalable(3)), Recorded::default());
        let cluster = PinnedCluster { api: &late, namespace: "orders", kubeconfig: "prod", context: "eks", writes: true, production: false, changes: Some(&recorded) };
        assert!(matches!(kube_scale(Some(cluster), &scale(0)), Err(ToolError::Kube(_))));
        let audits = recorded.audits.lock().unwrap();
        assert_eq!((audits[0].error.as_deref(), audits[0].generation_after), (Some("conflict"), None));
    }

    #[test]
    fn the_card_says_where_and_from_what_to_what() {
        let (fake, recorded) = (scalable(3), Recorded::default());
        let shown = preview(writing(&fake, &recorded), &ToolCall::KubeScale(scale(5)));
        let ToolPreview::Change { place, summary, notes, .. } = shown else { panic!("{shown:?}") };
        assert_eq!((place.as_str(), summary.as_str()), ("context eks · namespace orders", "Deployment/api: 3 → 5 replicas"));
        assert_eq!(notes, ["Can be undone: the object is backed up first."]);
        assert!(fake.asked().iter().all(|a| !a.starts_with("patch") || a.ends_with(" dry")), "a preview changes nothing: {:?}", fake.asked());
        let read = ToolCall::KubeGet(KubeGetArgs { kind: "deploy".into(), name: "api".into(), ..Default::default() });
        assert_eq!(preview(writing(&fake, &recorded), &read), ToolPreview::Nothing);
        assert!(preflight(None, &read).is_ok(), "a read needs no plan");
    }

    /// What has no replicas says how it is stopped; nothing to do is said too.
    #[test]
    fn what_cannot_be_scaled_says_how_it_is_stopped() {
        let (fake, recorded) = (scalable(3), Recorded::default());
        let refused = |kind: &str, replicas: Option<u32>| {
            let args = KubeScaleArgs { kind: kind.into(), name: "api".into(), replicas };
            match kube_scale(writing(&fake, &recorded), &args) {
                Err(ToolError::InvalidArguments { reason, .. }) => reason,
                other => panic!("{kind}: {other:?}"),
            }
        };
        assert!(refused("ds", Some(0)).contains("runs a pod on every node"));
        assert!(refused("cronjob", Some(0)).contains("stopped with kubeSuspend"));
        assert!(refused("deploy", Some(3)).contains("already has 3 replicas"));
        assert!(refused("deploy", None).contains("replicas is required"));
        assert!(recorded.backups.lock().unwrap().is_empty());
    }

    /// A cluster scaled to `now` by the change kc-1, which found 3 replicas
    /// and left the spec at `left` — the fake's own generation is 4.
    fn scaled(now: i64, left: i64) -> (Fake, Recorded) {
        let (fake, recorded) = (scalable(3), Recorded::default());
        kube_scale(writing(&fake, &recorded), &scale(now as u32)).unwrap();
        recorded.backups.lock().unwrap()[0].0.id = "kc-1".into();
        for audit in recorded.audits.lock().unwrap().iter_mut() {
            audit.id = "kc-1".into();
            audit.generation_after = Some(left);
        }
        (scalable(now), recorded)
    }

    fn undo_of(id: &str) -> KubeUndoArgs {
        KubeUndoArgs { change_id: id.into() }
    }

    fn refusal(result: Result<ToolResult, ToolError>) -> String {
        match result {
            Err(ToolError::InvalidArguments { tool, reason }) if tool == "kubeUndo" => reason,
            other => panic!("{other:?}"),
        }
    }

    /// The backup decides what comes back, and the undo is on record as a
    /// change of its own — dry run, backup, change, audit — naming what it undid.
    #[test]
    fn an_undo_puts_the_backup_back_and_is_itself_a_change() {
        let (fake, recorded) = scaled(0, 4);
        let done = kube_undo(writing(&fake, &recorded), &undo_of(" kc-1 ")).unwrap();
        let ToolResult::Kube { text, summary } = done else { panic!() };
        let id = recorded.audits.lock().unwrap()[1].id.clone();
        assert_eq!(summary, "0 → 3 replicas");
        assert_eq!(text, format!("Undid kc-1 — Deployment/api in namespace orders: 0 → 3 replicas. Change id {id} — undoing that makes the change again."));
        let patches: Vec<String> = fake.asked().into_iter().filter(|a| a.starts_with("patch")).collect();
        assert_eq!(
            patches,
            ["patch deployments orders api {\"spec\":{\"replicas\":3}} dry", "patch deployments orders api {\"spec\":{\"replicas\":3}}"]
        );
        let backups = recorded.backups.lock().unwrap();
        assert_eq!((backups[1].0.id.as_str(), &backups[1].1["spec"]["replicas"]), (id.as_str(), &json!(0)), "what the undo found");
        let audits = recorded.audits.lock().unwrap();
        assert_eq!((audits[1].undoes.as_deref(), audits[1].tool.as_str(), audits[1].generation_after), (Some("kc-1"), "kubeScale", Some(5)));
    }

    /// Not over what the app did not write: a spec generation other than
    /// the one the change left is somebody else's change, and what to do
    /// about it is the user's to say.
    #[test]
    fn an_object_changed_by_someone_else_is_not_undone() {
        let (fake, recorded) = scaled(2, 9);
        let refused = kube_undo(writing(&fake, &recorded), &undo_of("kc-1"));
        let Err(ToolError::KubeChangedSince(why)) = &refused else { panic!("{refused:?}") };
        assert_eq!(why, "Deployment/api was changed by someone else after kc-1 — undoing it now would be: 2 → 3 replicas");
        assert!(refused.unwrap_err().to_string().contains("the user's decision"));
        assert!(matches!(preflight(writing(&fake, &recorded), &ToolCall::KubeUndo(undo_of("kc-1"))), Err(ToolError::KubeChangedSince(_))));
        assert!(fake.asked().iter().all(|a| !a.starts_with("patch") || a.ends_with(" dry")), "{:?}", fake.asked());
        assert_eq!(recorded.backups.lock().unwrap().len(), 1);
    }

    /// Everything else that stops an undo, each said in words the model can pass on.
    #[test]
    fn an_undo_is_refused_read_only_elsewhere_out_of_turn_or_without_its_backup() {
        let (fake, recorded) = scaled(0, 4);
        let here = writing(&fake, &recorded).unwrap();
        assert!(matches!(kube_undo(Some(PinnedCluster { writes: false, ..here }), &undo_of("kc-1")), Err(ToolError::KubeReadOnly)));
        assert!(refusal(kube_undo(Some(PinnedCluster { changes: None, ..here }), &undo_of("kc-1"))).contains("no record"));
        for elsewhere in [PinnedCluster { namespace: "payments", ..here }, PinnedCluster { context: "gke", ..here }, PinnedCluster { kubeconfig: "dev", ..here }] {
            let why = refusal(kube_undo(Some(elsewhere), &undo_of("kc-1")));
            assert!(why.contains("was made in context eks · namespace orders of the kubeconfig prod"), "{why}");
        }
        assert!(refusal(kube_undo(Some(here), &undo_of("kc-2"))).contains("not on record"));
        assert!(fake.asked().is_empty(), "nothing is asked of the cluster for these: {:?}", fake.asked());

        // Undone once, the change is no longer the last of its object.
        kube_undo(Some(here), &undo_of("kc-1")).unwrap();
        assert!(refusal(kube_undo(Some(here), &undo_of("kc-1"))).contains("already undone"));

        let (fake, recorded) = scaled(0, 4);
        recorded.audits.lock().unwrap()[0].tool = "kubeRolloutRestart".into();
        assert!(refusal(kube_undo(writing(&fake, &recorded), &undo_of("kc-1"))).contains("was a rollout restart, which cannot be undone"));
        recorded.audits.lock().unwrap()[0].tool = "kubeExec".into();
        assert!(refusal(kube_undo(writing(&fake, &recorded), &undo_of("kc-1"))).contains("a kubeExec change cannot be undone"));
        assert!(fake.asked().is_empty(), "{:?}", fake.asked());
        recorded.audits.lock().unwrap()[0].tool = "kubeScale".into();
        recorded.backups.lock().unwrap().clear();
        assert!(refusal(kube_undo(writing(&fake, &recorded), &undo_of("kc-1"))).contains("no backup of the change kc-1"));
    }

    /// A workload whose kind has a group is found again by both; a backup
    /// with no replicas written is the one replica that means.
    #[test]
    fn the_card_of_an_undo_names_the_change_and_what_comes_back() {
        let (fake, recorded) = scaled(0, 4);
        let shown = preview(writing(&fake, &recorded), &ToolCall::KubeUndo(undo_of("kc-1")));
        let ToolPreview::Change { place, summary, notes, .. } = shown else { panic!("{shown:?}") };
        assert_eq!((place.as_str(), summary.as_str()), ("context eks · namespace orders", "Undo kc-1 — Deployment/api: 0 → 3 replicas"));
        assert!(notes[0].starts_with("Put back from the change's backup"));
        assert!(fake.asked().iter().all(|a| !a.starts_with("patch") || a.ends_with(" dry")), "{:?}", fake.asked());
        assert!(fake.asked().contains(&"get deployments orders api".to_string()));

        recorded.backups.lock().unwrap()[0].1 = json!({"spec": {}});
        let shown = preview(writing(&fake, &recorded), &ToolCall::KubeUndo(undo_of("kc-1")));
        assert!(matches!(&shown, ToolPreview::Change { summary, .. } if summary.ends_with("0 → 1 replicas")), "{shown:?}");
        let gone = preview(writing(&fake, &recorded), &ToolCall::KubeUndo(undo_of("kc-7")));
        assert!(matches!(&gone, ToolPreview::Failed { reason } if reason.contains("not on record")), "{gone:?}");
    }

    fn last_patch(fake: &Fake) -> String {
        fake.asked().into_iter().rfind(|asked| asked.starts_with("patch")).unwrap()
    }

    fn invalid_reason(result: Result<ToolResult, ToolError>) -> String {
        match result {
            Err(ToolError::InvalidArguments { reason, .. }) => reason,
            other => panic!("{other:?}"),
        }
    }

    /// A CronJob and a Job, a Deployment to refuse, and the kinds they are of.
    fn jobs() -> Fake {
        let job = |suspend: Value| json!({"metadata": {"name": "report", "namespace": "orders", "generation": 2}, "spec": {"suspend": suspend}});
        Fake::with(&[("batch", "CronJob", "cronjobs"), ("batch", "Job", "jobs"), ("apps", "Deployment", "deployments")])
            .object("cronjobs", job(json!(false)))
            .object("jobs", json!({"metadata": {"name": "report", "namespace": "orders", "generation": 1}, "spec": {}}))
            .object("deployments", workload("deployments", 3).1)
    }

    fn suspend(kind: &str, suspend: Option<bool>) -> ToolCall {
        ToolCall::KubeSuspend(KubeSuspendArgs { kind: kind.into(), name: if kind == "deploy" { "api" } else { "report" }.into(), suspend })
    }

    /// What has no replicas is stopped by its own switch, with the same
    /// dry run, card, backup and audit as a scale.
    #[test]
    fn a_cronjob_or_job_is_suspended_and_resumed_like_any_change() {
        let (fake, recorded) = (jobs(), Recorded::default());
        let here = writing(&fake, &recorded);
        let shown = preview(here, &suspend("cj", Some(true)));
        assert!(matches!(&shown, ToolPreview::Change { summary, .. } if summary == "CronJob/report: active → suspended"), "{shown:?}");
        let said = text(change(here, &suspend("cj", Some(true))));
        assert!(said.starts_with("CronJob/report in namespace orders: active → suspended. Change id kc-"), "{said}");
        let patches: Vec<String> = fake.asked().into_iter().filter(|a| a.starts_with("patch")).collect();
        let patch = "patch cronjobs orders report {\"spec\":{\"suspend\":true}}";
        assert_eq!(patches, [format!("{patch} dry"), format!("{patch} dry"), patch.to_string()], "the card's dry run, the call's, the change");
        let made = recorded.audits.lock().unwrap()[0].clone();
        assert_eq!((made.tool.as_str(), made.kind.as_str(), made.generation_after), ("kubeSuspend", "CronJob", Some(3)));
        assert_eq!(recorded.backups.lock().unwrap()[0].1["spec"]["suspend"], false);

        // A Job that never said is running: there is only one way to change it.
        assert!(text(change(here, &suspend("job", Some(true)))).contains("Job/report in namespace orders: active → suspended"));
        assert!(invalid_reason(change(here, &suspend("job", Some(false)))).contains("Job/report is already active"));
        assert!(invalid_reason(change(here, &suspend("cj", None))).contains("suspend is required"));
        assert!(invalid_reason(change(here, &suspend("deploy", Some(true)))).contains("stopped with kubeScale to 0"));
        let read_only = PinnedCluster { writes: false, ..here.unwrap() };
        assert!(matches!(change(Some(read_only), &suspend("cj", None)), Err(ToolError::KubeReadOnly)), "the switch comes before the arguments");
    }

    /// The backup says whether it was running; a Job's, which had no
    /// `suspend` at all, says it was.
    #[test]
    fn an_undone_suspend_is_whatever_the_backup_had() {
        let (fake, recorded) = (jobs(), Recorded::default());
        change(writing(&fake, &recorded), &suspend("job", Some(true))).unwrap();
        let made = recorded.audits.lock().unwrap()[0].clone();
        // The cluster as the change left it: suspended, at the generation on record.
        let job = json!({"metadata": {"name": "report", "namespace": "orders", "generation": made.generation_after}, "spec": {"suspend": true}});
        let after = Fake::with(&[("batch", "Job", "jobs")]).object("jobs", job);
        let said = text(kube_undo(writing(&after, &recorded), &undo_of(&made.id)));
        assert!(said.starts_with(&format!("Undid {} — Job/report in namespace orders: suspended → active.", made.id)), "{said}");
        assert!(after.asked().contains(&"patch jobs orders report {\"spec\":{\"suspend\":false}}".to_string()), "{:?}", after.asked());
        let audits = recorded.audits.lock().unwrap();
        assert_eq!((audits[1].tool.as_str(), audits[1].undoes.as_deref()), ("kubeSuspend", Some(made.id.as_str())));
    }

    /// A restart is `restartedAt` on the pod template — and the one change
    /// with no way back, which the card and the answer both say.
    #[test]
    fn a_rollout_restart_says_before_and_after_that_it_cannot_be_undone() {
        let (plural, daemons) = workload("daemonsets", 1);
        let fake = scalable(3).object(&plural, daemons);
        let recorded = Recorded::default();
        let here = writing(&fake, &recorded);
        let restart = |kind: &str| ToolCall::KubeRolloutRestart(KubeRolloutRestartArgs { kind: kind.into(), name: "api".into() });
        let shown = preview(here, &restart("deploy"));
        let ToolPreview::Change { summary, notes, .. } = shown else { panic!("{shown:?}") };
        assert_eq!(summary, "Deployment/api: rollout restart — every pod is replaced");
        assert_eq!(notes, ["Cannot be undone: the pods are replaced, and the old ones do not come back."]);

        let said = text(change(here, &restart("deploy")));
        assert!(said.ends_with("on record, but a restart cannot be undone."), "{said}");
        let last = last_patch(&fake);
        assert!(last.starts_with("patch deployments orders api {\"spec\":{\"template\":{\"metadata\":{\"annotations\":{\"kubectl.kubernetes.io/restartedAt\":\"20"), "{last}");
        assert!(!last.ends_with(" dry"));
        let made = recorded.audits.lock().unwrap()[0].clone();
        assert_eq!(made.tool, "kubeRolloutRestart");
        assert!(refusal(kube_undo(here, &undo_of(&made.id))).contains("was a rollout restart, which cannot be undone"));

        assert!(text(change(here, &restart("ds"))).starts_with("DaemonSet/api in namespace orders: rollout restart"));
        assert!(invalid_reason(change(here, &restart("cronjob"))).contains("only a Deployment, StatefulSet or DaemonSet is restarted"));
        assert!(matches!(change(Some(PinnedCluster { writes: false, ..here.unwrap() }), &restart("deploy")), Err(ToolError::KubeReadOnly)));
    }

    /// A Deployment at revision 3 with its three ReplicaSets, one more that
    /// is another Deployment's, and one with no revision written.
    fn rolled_out() -> Fake {
        let template = |image: &str, extra: Value| {
            let mut template = json!({"metadata": {"labels": {"app": "api"}}, "spec": {"containers": [{"name": "api", "image": image}]}});
            merge(&mut template, &extra);
            template
        };
        let set = |name: &str, revision: Option<&str>, owner: &str, image: &str| {
            let mut template = template(image, json!({}));
            template["metadata"]["labels"]["pod-template-hash"] = json!(name);
            json!({
                "metadata": {"name": name, "namespace": "orders", "labels": {"app": "api"}, "annotations": {"deployment.kubernetes.io/revision": revision}, "ownerReferences": [{"uid": owner}]},
                "spec": {"template": template}
            })
        };
        let deployment = json!({
            "metadata": {"name": "api", "namespace": "orders", "uid": "u-api", "generation": 7, "annotations": {"deployment.kubernetes.io/revision": "3"}},
            "spec": {"selector": {"matchLabels": {"app": "api"}}, "template": template("api:3", json!({"spec": {"nodeSelector": {"gpu": "yes"}}}))}
        });
        Fake::with(&[("apps", "Deployment", "deployments"), ("apps", "ReplicaSet", "replicasets"), ("apps", "StatefulSet", "statefulsets")])
            .object("deployments", deployment)
            .object("replicasets", set("api-1", Some("1"), "u-api", "api:1"))
            .object("replicasets", set("api-2", Some("2"), "u-api", "api:2"))
            .object("replicasets", set("api-3", Some("3"), "u-api", "api:3"))
            .object("replicasets", set("other-9", Some("9"), "u-other", "other:9"))
            .object("replicasets", set("api-x", None, "u-api", "api:x"))
    }

    fn rollback(kind: &str, to_revision: Option<u32>) -> ToolCall {
        ToolCall::KubeRolloutUndo(KubeRolloutUndoArgs { kind: kind.into(), name: "api".into(), to_revision })
    }

    /// The previous revision is the newest ReplicaSet of this Deployment
    /// that is not the current one; its template replaces the Deployment's
    /// whole — what the newer one added goes — without the ReplicaSet's own label.
    #[test]
    fn a_rollback_takes_the_previous_revisions_template_or_the_one_named() {
        let (fake, recorded) = (rolled_out(), Recorded::default());
        let here = writing(&fake, &recorded);
        let shown = preview(here, &rollback("deploy", None));
        let ToolPreview::Change { summary, notes, .. } = shown else { panic!("{shown:?}") };
        assert_eq!(summary, "Deployment/api: revision 3 → 2, image api:3 → api:2");
        assert_eq!(notes, ["Can be undone: the object is backed up first."]);
        assert!(fake.asked().contains(&"list replicasets Some(\"orders\") Some(\"app=api\") None".to_string()), "{:?}", fake.asked());

        let said = text(change(here, &rollback("deploy", None)));
        assert!(said.starts_with("Deployment/api in namespace orders: revision 3 → 2, image api:3 → api:2. Change id kc-"), "{said}");
        let last = last_patch(&fake);
        assert_eq!(
            last,
            "patch deployments orders api {\"spec\":{\"template\":{\"spec\":{\"containers\":[{\"image\":\"api:2\",\"name\":\"api\"}],\"nodeSelector\":null}}}}"
        );
        assert_eq!(recorded.audits.lock().unwrap()[0].tool, "kubeRolloutUndo");

        let named = preview(here, &rollback("deploy", Some(1)));
        assert!(matches!(&named, ToolPreview::Change { summary, .. } if summary.contains("revision 3 → 1, image api:3 → api:1")), "{named:?}");
        let refused = |call: ToolCall| invalid_reason(change(here, &call));
        assert!(refused(rollback("deploy", Some(9))).contains("no revision 9 to go back to — the revisions kept are 1, 2"));
        assert!(refused(rollback("deploy", Some(3))).contains("no revision 3"), "the current one is not gone back to");
        assert!(refused(rollback("sts", None)).contains("only a Deployment is rolled back here"));
    }

    #[test]
    fn a_deployment_with_nothing_earlier_is_not_rolled_back() {
        let mut fake = rolled_out();
        fake.objects.retain(|(plural, o)| plural != "replicasets" || o["metadata"]["name"] == "api-3");
        let recorded = Recorded::default();
        let here = writing(&fake, &recorded);
        assert!(invalid_reason(change(here, &rollback("deploy", None))).contains("no earlier revision to go back to"));
        assert!(invalid_reason(change(here, &rollback("deploy", Some(2)))).contains("no revision 2 to go back to — no earlier revision is kept"));
        assert!(recorded.backups.lock().unwrap().is_empty());
    }

    /// The backup has the template the rollback replaced: undoing puts that
    /// one back whole, node selector and all.
    #[test]
    fn an_undone_rollback_is_the_template_from_the_backup() {
        let (fake, recorded) = (rolled_out(), Recorded::default());
        change(writing(&fake, &recorded), &rollback("deploy", None)).unwrap();
        let made = recorded.audits.lock().unwrap()[0].clone();
        // Somebody put the template back already: there is nothing to undo to.
        let same = rolled_out();
        assert!(invalid_reason(kube_undo(writing(&same, &recorded), &undo_of(&made.id))).contains("already has that pod template"));
        assert!(same.asked().iter().all(|asked| !asked.starts_with("patch")), "{:?}", same.asked());

        let mut rolled = rolled_out();
        let deployment = &mut rolled.objects[0].1;
        deployment["metadata"]["generation"] = json!(made.generation_after);
        deployment["spec"]["template"] = json!({"metadata": {"labels": {"app": "api"}}, "spec": {"containers": [{"name": "api", "image": "api:2"}]}});

        let shown = preview(writing(&rolled, &recorded), &ToolCall::KubeUndo(undo_of(&made.id)));
        let wanted = format!("Undo {} — Deployment/api: the pod template as it was before {}, image api:2 → api:3", made.id, made.id);
        assert!(matches!(&shown, ToolPreview::Change { summary, .. } if summary == &wanted), "{shown:?}");
        kube_undo(writing(&rolled, &recorded), &undo_of(&made.id)).unwrap();
        assert_eq!(
            last_patch(&rolled),
            "patch deployments orders api {\"spec\":{\"template\":{\"spec\":{\"containers\":[{\"image\":\"api:3\",\"name\":\"api\"}],\"nodeSelector\":{\"gpu\":\"yes\"}}}}}"
        );
    }

    /// A namespace with a Deployment, a ConfigMap and a Service — and a
    /// cluster that also serves a cluster-wide kind.
    fn namespace_of_three() -> Fake {
        Fake::with(&[("apps", "Deployment", "deployments"), ("", "ConfigMap", "configmaps"), ("", "Service", "services"), ("", "Secret", "secrets"), ("", "Node", "nodes")])
            .object("deployments", json!({"apiVersion": "apps/v1", "kind": "Deployment", "metadata": {"name": "api", "namespace": "orders", "generation": 4, "uid": "u-1"}, "spec": {"replicas": 3}, "status": {"readyReplicas": 3}}))
            .object("services", json!({"apiVersion": "v1", "kind": "Service", "metadata": {"name": "api", "namespace": "orders", "resourceVersion": "70"}, "spec": {"ports": [{"port": 80}]}}))
    }

    fn apply(manifest: &str) -> ToolCall {
        ToolCall::KubeApply(KubeApplyArgs { manifest: manifest.into() })
    }

    const THREE: &str = "\
apiVersion: v1
kind: ConfigMap
metadata: {name: flags}
data: {beta: 'on'}
---
apiVersion: apps/v1
kind: Deployment
metadata: {name: api, namespace: orders}
spec: {replicas: 5}
---
apiVersion: v1
kind: Service
metadata: {name: api}
spec: {ports: [{port: 80}]}
";

    /// A manifest is one card: each object that would change, as it is
    /// against what it would be; one that would not change is not in it.
    #[test]
    fn a_manifests_card_shows_each_object_that_changes_and_its_diff() {
        let (fake, recorded) = (namespace_of_three(), Recorded::default());
        let shown = preview(writing(&fake, &recorded), &apply(THREE));
        let ToolPreview::Change { place, summary, notes, diffs, .. } = shown else { panic!("{shown:?}") };
        assert_eq!(place, "context eks · namespace orders");
        assert_eq!(summary, "2 objects — ConfigMap/flags: create; Deployment/api: update (+1 −1 lines)");
        assert_eq!(notes, ["Can be undone: the object is backed up first."]);
        let titles: Vec<&str> = diffs.iter().map(|d| d.title.as_str()).collect();
        assert_eq!(titles, ["ConfigMap/flags", "Deployment/api"]);
        assert!(diffs[0].diff.unified_diff.contains("+  beta: on"), "{}", diffs[0].diff.unified_diff);
        let changed = &diffs[1].diff.unified_diff;
        assert!(changed.contains("-  replicas: 3") && changed.contains("+  replicas: 5"), "{changed}");
        assert!(!changed.contains("uid") && !changed.contains("readyReplicas"), "what the server writes is not the manifest's: {changed}");
        assert!(fake.asked().iter().all(|a| !a.starts_with("apply") || a.ends_with(" dry")), "{:?}", fake.asked());
        // Sent to the chat's own namespace, whether or not the manifest named it.
        assert!(fake.asked().iter().any(|a| a.starts_with("apply configmaps orders flags {") && a.contains("\"namespace\":\"orders\"")), "{:?}", fake.asked());
    }

    /// Each object is its own change: a backup — nothing, for what was not
    /// there — an audit line and an id. A kind with no generation is marked
    /// by its resource version.
    #[test]
    fn an_applied_manifest_is_a_change_per_object() {
        let (fake, recorded) = (namespace_of_three(), Recorded::default());
        let ToolResult::Kube { text, summary } = change(writing(&fake, &recorded), &apply(THREE)).unwrap() else { panic!() };
        assert_eq!(summary, "2 objects");
        let audits = recorded.audits.lock().unwrap().clone();
        assert_eq!(
            text,
            format!(
                "ConfigMap/flags in namespace orders: create. Change id {} — the object as it was is backed up.\n\
                 Deployment/api in namespace orders: update (+1 −1 lines). Change id {} — the object as it was is backed up.",
                audits[0].id, audits[1].id
            )
        );
        assert_eq!((audits[0].tool.as_str(), audits[0].generation_after, audits[0].version_after.as_deref()), ("kubeApply", None, Some("1")));
        assert_eq!((audits[1].generation_before, audits[1].generation_after, &audits[1].version_after), (Some(4), Some(5), &None));
        let backups = recorded.backups.lock().unwrap().clone();
        assert!(backups[0].1.is_null(), "nothing was there");
        assert_eq!(backups[1].1["spec"]["replicas"], 3);
        let applied: Vec<String> = fake.asked().into_iter().filter(|a| a.starts_with("apply") && !a.ends_with(" dry")).collect();
        assert_eq!(applied.len(), 2, "{applied:?}");

        let one = "apiVersion: v1\nkind: ConfigMap\nmetadata: {name: flags}\n";
        let ToolResult::Kube { summary, .. } = change(writing(&fake, &recorded), &apply(one)).unwrap() else { panic!() };
        assert_eq!(summary, "create");
    }

    /// Everything a manifest can be refused for before the server is asked
    /// to change anything — and a Secret's values never reach the card.
    #[test]
    fn a_manifest_is_refused_for_what_it_is_or_where_it_points() {
        let (fake, recorded) = (namespace_of_three(), Recorded::default());
        let here = writing(&fake, &recorded);
        let refused = |manifest: &str| invalid_reason(change(here, &apply(manifest)));
        assert!(refused("kind: [").contains("the manifest is not YAML"));
        assert!(refused("").contains("this one has 0"));
        assert!(refused(&"---\napiVersion: v1\nkind: ConfigMap\nmetadata: {name: a}\n".repeat(21)).contains("this one has 21"));
        assert!(refused("apiVersion: v1\nkind: ConfigMap\n").contains("needs apiVersion, kind and metadata.name"));
        assert!(refused("apiVersion: v1\nkind: ConfigMap\nmetadata: {name: a, namespace: payments}\n").contains("names namespace payments; this chat changes only orders"));
        assert!(refused("apiVersion: v1\nkind: Node\nmetadata: {name: n1}\n").contains("a Node is cluster-wide"));
        assert!(refused("apiVersion: apps/v1beta1\nkind: Deployment\nmetadata: {name: api}\n").contains("serves Deployment as apiVersion apps/v1, not apps/v1beta1"));
        assert!(refused("apiVersion: v1\nkind: Deployment\nmetadata: {name: api}\n").contains("serves Deployment as apiVersion apps/v1, not v1"));
        assert!(refused("apiVersion: v1\nkind: Service\nmetadata: {name: api}\nspec: {ports: [{port: 80}]}\n").contains("exactly as the manifest says"));
        assert!(matches!(change(here, &apply("apiVersion: v2\nkind: Widget\nmetadata: {name: a}\n")), Err(ToolError::Kube(KubeError::UnknownKind(_)))));
        assert!(matches!(change(Some(PinnedCluster { writes: false, ..here.unwrap() }), &apply("kind: [")), Err(ToolError::KubeReadOnly)));
        assert!(recorded.backups.lock().unwrap().is_empty());

        let secret = "apiVersion: v1\nkind: Secret\nmetadata: {name: db}\nstringData: {password: hunter2}\n";
        let ToolPreview::Change { diffs, .. } = preview(here, &apply(secret)) else { panic!() };
        assert!(diffs[0].diff.unified_diff.contains("password") && !diffs[0].diff.unified_diff.contains("hunter2"), "{}", diffs[0].diff.unified_diff);
    }

    /// A manifest the server stops halfway says what it had already changed
    /// — those changes are made, on record, and undoable.
    #[test]
    fn a_manifest_stopped_halfway_says_what_was_already_changed() {
        struct SecondRefused(Fake);
        impl KubeApi for SecondRefused {
            fn kinds(&self) -> Result<Vec<KubeKind>, KubeError> { self.0.kinds() }
            fn list(&self, kind: &KubeKind, query: &ListQuery) -> Result<ListPage, KubeError> { self.0.list(kind, query) }
            fn get(&self, kind: &KubeKind, namespace: &str, name: &str) -> Result<Value, KubeError> { self.0.get(kind, namespace, name) }
            fn logs(&self, namespace: &str, pod: &str, query: &LogQuery) -> Result<String, KubeError> { self.0.logs(namespace, pod, query) }
            fn patch(&self, kind: &KubeKind, namespace: &str, name: &str, patch: &Value, dry_run: bool) -> Result<Value, KubeError> { self.0.patch(kind, namespace, name, patch, dry_run) }
            fn apply(&self, kind: &KubeKind, namespace: &str, name: &str, object: &Value, dry_run: bool) -> Result<Value, KubeError> {
                if dry_run || name == "first" { self.0.apply(kind, namespace, name, object, dry_run) } else { Err(KubeError::Cluster("quota exceeded".into())) }
            }
            fn delete(&self, kind: &KubeKind, namespace: &str, name: &str, dry_run: bool) -> Result<(), KubeError> { self.0.delete(kind, namespace, name, dry_run) }
        }
        let (stops, recorded) = (SecondRefused(namespace_of_three()), Recorded::default());
        let cluster = PinnedCluster { api: &stops, namespace: "orders", kubeconfig: "prod", context: "eks", writes: true, production: false, changes: Some(&recorded) };
        let names = ["first", "second", "third"].map(|name| format!("apiVersion: v1\nkind: ConfigMap\nmetadata: {{name: {name}}}\n")).join("---\n");
        let stopped = change(Some(cluster), &apply(&names)).unwrap_err().to_string();
        let audits = recorded.audits.lock().unwrap();
        assert!(stopped.starts_with(&format!("ConfigMap/first in namespace orders: create. Change id {}", audits[0].id)), "{stopped}");
        assert!(stopped.ends_with("Then ConfigMap/second failed, and nothing after it was changed: quota exceeded"), "{stopped}");
        assert_eq!(audits.len(), 2);
        assert_eq!((audits[1].name.as_str(), audits[1].error.as_deref()), ("second", Some("quota exceeded")));
    }

    fn delete(kind: &str, name: &str) -> ToolCall {
        ToolCall::KubeDelete(KubeDeleteArgs { kind: kind.into(), name: name.into() })
    }

    /// Deleted after a dry run, with the object whole in the backup; and what
    /// cannot be deleted from a chat at all.
    #[test]
    fn a_delete_backs_the_object_up_whole_and_stays_in_its_namespace() {
        let (fake, recorded) = (namespace_of_three(), Recorded::default());
        let here = writing(&fake, &recorded);
        let shown = preview(here, &delete("deploy", "api"));
        let ToolPreview::Change { summary, notes, diffs, .. } = shown else { panic!("{shown:?}") };
        assert_eq!((summary.as_str(), diffs.len()), ("Deployment/api: delete", 0));
        assert_eq!(notes, ["Can be undone: the object is backed up first."]);

        let said = text(change(here, &delete("deploy", "api")));
        assert!(said.starts_with("Deployment/api in namespace orders: delete. Change id kc-"), "{said}");
        let deletes: Vec<String> = fake.asked().into_iter().filter(|a| a.starts_with("delete")).collect();
        assert_eq!(deletes, ["delete deployments orders api dry", "delete deployments orders api dry", "delete deployments orders api"]);
        let made = recorded.audits.lock().unwrap()[0].clone();
        assert_eq!((made.tool.as_str(), made.generation_before, made.generation_after, made.version_after), ("kubeDelete", Some(4), None, None));
        assert_eq!(recorded.backups.lock().unwrap()[0].1["status"]["readyReplicas"], 3, "the backup is the object whole");

        assert!(invalid_reason(change(here, &delete("node", "n1"))).contains("a Node is cluster-wide"));
        assert!(matches!(change(here, &delete("cm", "absent")), Err(ToolError::Kube(KubeError::NotFound(_)))));
        assert!(matches!(change(Some(PinnedCluster { writes: false, ..here.unwrap() }), &delete("deploy", "api")), Err(ToolError::KubeReadOnly)));
    }

    /// A claim's data goes with its volume unless the volume is kept — said
    /// on the card, before the way back, which does not cover it.
    #[test]
    fn a_claims_card_says_when_its_data_is_lost() {
        let claim = |name: &str, volume: &str| json!({"metadata": {"name": name, "namespace": "orders"}, "spec": {"volumeName": volume}});
        let volume = |name: &str, policy: &str| json!({"metadata": {"name": name}, "spec": {"persistentVolumeReclaimPolicy": policy}});
        let fake = Fake::with(&[("", "PersistentVolumeClaim", "persistentvolumeclaims"), ("", "PersistentVolume", "persistentvolumes")])
            .object("persistentvolumeclaims", claim("data", "pv-1"))
            .object("persistentvolumeclaims", claim("kept", "pv-2"))
            .object("persistentvolumeclaims", claim("unknown", "pv-3"))
            .object("persistentvolumeclaims", json!({"metadata": {"name": "unbound", "namespace": "orders"}, "spec": {}}))
            .object("persistentvolumes", volume("pv-1", "Delete"))
            .object("persistentvolumes", volume("pv-2", "Retain"));
        let recorded = Recorded::default();
        let notes = |name: &str| match preview(writing(&fake, &recorded), &delete("pvc", name)) {
            ToolPreview::Change { notes, .. } => notes,
            other => panic!("{other:?}"),
        };
        assert_eq!(
            notes("data"),
            ["Its volume pv-1 is deleted with it: the DATA IS LOST, and the backup does not bring it back.", "Can be undone: the object is backed up first."]
        );
        assert_eq!(notes("kept").len(), 1);
        assert_eq!(notes("unbound").len(), 1);
        assert!(notes("unknown")[0].contains("Whether its volume pv-3 is kept could not be read"));
    }

    /// The way back from a delete is the backup applied again, without what
    /// the server wrote on the old object — and not over a new one of that name.
    #[test]
    fn an_undone_delete_creates_the_object_again_from_its_backup() {
        let (fake, recorded) = (namespace_of_three(), Recorded::default());
        change(writing(&fake, &recorded), &delete("deploy", "api")).unwrap();
        let id = recorded.audits.lock().unwrap()[0].id.clone();
        let refused = kube_undo(writing(&fake, &recorded), &undo_of(&id));
        assert!(matches!(&refused, Err(ToolError::KubeChangedSince(why)) if why.contains("Deployment/api exists again")), "{refused:?}");

        let mut gone = namespace_of_three();
        gone.objects.retain(|(plural, _)| plural != "deployments");
        let shown = preview(writing(&gone, &recorded), &ToolCall::KubeUndo(undo_of(&id)));
        let ToolPreview::Change { summary, diffs, .. } = shown else { panic!("{shown:?}") };
        assert_eq!(summary, format!("Undo {id} — Deployment/api: create again, from the backup of {id}"));
        assert!(diffs[0].diff.unified_diff.contains("+  replicas: 3"));
        kube_undo(writing(&gone, &recorded), &undo_of(&id)).unwrap();
        let applied = gone.asked().into_iter().rfind(|a| a.starts_with("apply")).unwrap();
        assert_eq!(
            applied,
            "apply deployments orders api {\"apiVersion\":\"apps/v1\",\"kind\":\"Deployment\",\"metadata\":{\"name\":\"api\",\"namespace\":\"orders\"},\"spec\":{\"replicas\":3}}"
        );
        let audits = recorded.audits.lock().unwrap();
        assert_eq!((audits[1].tool.as_str(), audits[1].undoes.as_deref()), ("kubeApply", Some(id.as_str())), "undoing the undo deletes it again");
        assert!(recorded.backups.lock().unwrap()[1].1.is_null());
    }

    /// What an apply created is deleted; what it changed is put back whole.
    /// Either only while the object is as the apply left it — for a kind with
    /// no generation, by its resource version.
    #[test]
    fn an_undone_apply_deletes_what_it_created_and_restores_what_it_changed() {
        let (fake, recorded) = (namespace_of_three(), Recorded::default());
        change(writing(&fake, &recorded), &apply(THREE)).unwrap();
        let (created, updated) = {
            let audits = recorded.audits.lock().unwrap();
            (audits[0].id.clone(), audits[1].id.clone())
        };
        // Neither is there as the apply left it: the fake keeps nothing.
        assert!(refusal(kube_undo(writing(&fake, &recorded), &undo_of(&created))).contains("ConfigMap/flags is no longer there"));
        assert!(refusal(kube_undo(writing(&fake, &recorded), &undo_of(&updated))).contains("Deployment/api is already as it was before"));

        let flags = |version: &str| json!({"apiVersion": "v1", "kind": "ConfigMap", "metadata": {"name": "flags", "namespace": "orders", "resourceVersion": version}, "data": {"beta": "on"}});
        let after = |version: &str, generation: i64, labels: Value| {
            Fake::with(&[("apps", "Deployment", "deployments"), ("", "ConfigMap", "configmaps")])
                .object("configmaps", flags(version))
                .object("deployments", json!({"apiVersion": "apps/v1", "kind": "Deployment", "metadata": {"name": "api", "namespace": "orders", "generation": generation, "labels": labels}, "spec": {"replicas": 5}}))
        };
        let moved = after("1", 9, json!({}));
        assert!(matches!(kube_undo(writing(&moved, &recorded), &undo_of(&updated)), Err(ToolError::KubeChangedSince(_))), "someone changed the Deployment");
        let edited = after("2", 5, json!({}));
        assert!(matches!(kube_undo(writing(&edited, &recorded), &undo_of(&created)), Err(ToolError::KubeChangedSince(_))), "someone edited the ConfigMap");

        let left = after("1", 5, json!({"added": "by-apply"}));
        let said = text(kube_undo(writing(&left, &recorded), &undo_of(&created)));
        assert!(said.contains(&format!("ConfigMap/flags in namespace orders: delete — it did not exist before {created}")), "{said}");
        assert_eq!(left.asked().pop().unwrap(), "get configmaps orders flags", "the fake's own look before it deletes");
        assert!(left.asked().contains(&"delete configmaps orders flags".to_string()));

        let shown = preview(writing(&left, &recorded), &ToolCall::KubeUndo(undo_of(&updated)));
        let ToolPreview::Change { summary, diffs, .. } = shown else { panic!("{shown:?}") };
        assert_eq!(summary, format!("Undo {updated} — Deployment/api: as it was before {updated}"));
        assert!(diffs[0].diff.unified_diff.contains("+  replicas: 3"), "{}", diffs[0].diff.unified_diff);
        kube_undo(writing(&left, &recorded), &undo_of(&updated)).unwrap();
        assert_eq!(last_patch(&left), "patch deployments orders api {\"metadata\":{\"labels\":null},\"spec\":{\"replicas\":3}}");
        let audits = recorded.audits.lock().unwrap();
        let tools: Vec<&str> = audits.iter().skip(2).map(|a| a.tool.as_str()).collect();
        assert_eq!(tools, ["kubeDelete", "kubeApply"], "each undo is on record as what it did");
    }

    /// What will put a change back by itself is on the card before anyone
    /// agrees, and in the answer for the model to say — a warning, not a refusal.
    #[test]
    fn the_card_names_what_will_put_the_change_back() {
        let managed = |name: &str, metadata: Value| {
            let mut object = json!({"apiVersion": "apps/v1", "kind": "Deployment", "metadata": {"name": name, "namespace": "orders", "generation": 1}, "spec": {"replicas": 3}});
            merge(&mut object["metadata"], &metadata);
            object
        };
        let scaler = |name: &str, target: &str| json!({"metadata": {"name": name, "namespace": "orders"}, "spec": {"minReplicas": 2, "maxReplicas": 9, "scaleTargetRef": {"kind": "Deployment", "name": target}}});
        let fake = Fake::with(&[("apps", "Deployment", "deployments"), ("autoscaling", "HorizontalPodAutoscaler", "horizontalpodautoscalers")])
            .object("deployments", managed("plain", json!({"labels": {"app.kubernetes.io/managed-by": "kustomize"}})))
            .object("deployments", managed("argo", json!({"annotations": {"argocd.argoproj.io/tracking-id": "x"}, "labels": {"app.kubernetes.io/managed-by": "Helm"}})))
            .object("deployments", managed("flux", json!({"labels": {"helm.toolkit.fluxcd.io/name": "x", "app.kubernetes.io/managed-by": "Helm"}})))
            .object("deployments", managed("owned", json!({"ownerReferences": [{"kind": "Rollout", "name": "api", "controller": true}, {"kind": "Thing", "name": "t"}]})))
            .object("horizontalpodautoscalers", scaler("plain-hpa", "plain"))
            .object("horizontalpodautoscalers", scaler("other-hpa", "other"));
        let recorded = Recorded::default();
        let here = writing(&fake, &recorded);
        let scale = |name: &str| ToolCall::KubeScale(KubeScaleArgs { kind: "deploy".into(), name: name.into(), replicas: Some(0) });
        let notes = |call: &ToolCall| match preview(here, call) {
            ToolPreview::Change { notes, .. } => notes,
            other => panic!("{other:?}"),
        };
        let way_back = "Can be undone: the object is backed up first.";
        assert_eq!(notes(&scale("plain")), ["HorizontalPodAutoscaler/plain-hpa sets its replicas (2–9): it will scale it back.", way_back]);
        assert_eq!(
            notes(&scale("argo")),
            ["Managed by Argo CD: with self-heal on, it puts back what Git says.", "Installed by Helm: the next `helm upgrade` overwrites this.", way_back]
        );
        assert_eq!(notes(&scale("flux")), ["Managed by Flux: its next reconcile puts back what Git says.", way_back]);
        assert_eq!(notes(&scale("owned")), ["Owned by Rollout/api: its owner may put it back, or make another.", way_back]);

        // An autoscaler is a scale's business only: a delete does not ask for them.
        let asked = fake.asked().len();
        assert_eq!(notes(&ToolCall::KubeDelete(KubeDeleteArgs { kind: "deploy".into(), name: "plain".into() })), [way_back]);
        assert!(fake.asked()[asked..].iter().all(|a| !a.starts_with("list")), "{:?}", fake.asked());

        let said = text(change(here, &scale("plain")));
        assert!(said.ends_with("\n  Note — HorizontalPodAutoscaler/plain-hpa sets its replicas (2–9): it will scale it back."), "{said}");

        // Several objects on one card: each warning under its object's name.
        let two = "apiVersion: apps/v1\nkind: Deployment\nmetadata: {name: flux}\nspec: {replicas: 1}\n---\napiVersion: apps/v1\nkind: Deployment\nmetadata: {name: plain}\nspec: {replicas: 1}\n";
        assert_eq!(notes(&ToolCall::KubeApply(KubeApplyArgs { manifest: two.into() })), ["Deployment/flux: Managed by Flux: its next reconcile puts back what Git says.", way_back]);
        assert!(matches!(preview(Some(PinnedCluster { production: true, ..here.unwrap() }), &scale("plain")), ToolPreview::Change { production: true, .. }));
        assert!(matches!(preview(here, &scale("plain")), ToolPreview::Change { production: false, .. }));
    }

    /// A Deployment of three, `updated` and `available` of them so far.
    fn rolling(updated: i64, available: i64) -> Value {
        json!({"metadata": {"name": "api", "namespace": "orders", "generation": 4},
               "spec": {"replicas": 3, "selector": {"matchLabels": {"app": "api"}}},
               "status": {"observedGeneration": 4, "replicas": 3, "updatedReplicas": updated, "availableReplicas": available}})
    }

    fn rolling_out(later: Vec<Value>) -> Fake {
        let kinds = [("apps", "Deployment", "deployments"), ("batch", "CronJob", "cronjobs"), ("", "Pod", "pods")];
        let waiting = json!({"metadata": {"name": "api-1", "namespace": "orders", "labels": {"app": "api"}},
            "status": {"phase": "Pending", "containerStatuses": [{"name": "app", "state": {"waiting": {"reason": "ImagePullBackOff"}}}]}});
        Fake { later, ..Fake::with(&kinds).object("deployments", rolling(1, 1)).object("pods", waiting).log("api-1", Ok("")) }
    }

    fn wait(kind: &str, seconds: Option<u64>) -> KubeWaitRolloutArgs {
        KubeWaitRolloutArgs { kind: kind.into(), name: "api".into(), timeout_seconds: seconds }
    }

    /// A read: it works in a read-only chat, in the chat's namespace, and
    /// stops at the first state that is an end.
    #[test]
    fn a_rollout_is_waited_for_until_it_is_done_and_no_longer() {
        let fake = rolling_out(vec![rolling(2, 1), rolling(3, 3), rolling(0, 0)]);
        let done = kube_wait_rollout(pinned(&fake), None, &wait("deploy", None)).unwrap();
        assert_eq!(done, ToolResult::Kube { text: "Rollout of Deployment/api is done: 3 of 3 pods up to date and available.".into(), summary: "done".into() });
        assert_eq!(fake.asked(), ["get deployments orders api", "watch deployments orders api 120s", "shown", "shown"]);

        let bounded = rolling_out(Vec::new());
        kube_wait_rollout(pinned(&bounded), None, &wait("deploy", Some(9_999))).unwrap();
        kube_wait_rollout(pinned(&bounded), None, &wait("deploy", Some(0))).unwrap();
        let watches: Vec<String> = bounded.asked().into_iter().filter(|a| a.starts_with("watch")).collect();
        assert_eq!(watches, ["watch deployments orders api 600s", "watch deployments orders api 1s"]);

        let reason = invalid_reason(kube_wait_rollout(pinned(&fake), None, &wait("cronjob", None)));
        assert!(reason.starts_with("CronJob/api has no rollout to wait for"), "{reason}");
        assert!(matches!(kube_wait_rollout(None, None, &wait("deploy", None)), Err(ToolError::NoCluster)));
    }

    /// Whatever is not done comes with why: the model asks that next.
    #[test]
    fn a_rollout_that_is_not_done_says_how_it_stands_and_is_diagnosed() {
        let late = text(kube_wait_rollout(pinned(&rolling_out(vec![rolling(2, 1)])), None, &wait("deploy", Some(30))));
        assert!(late.starts_with("Rollout of Deployment/api is not done after 30s: 2 of 3 pods up to date. It may still finish"), "{late}");
        assert!(late.contains("Pods: 1, 1 with problems.") && late.contains("ImagePullBackOff"), "{late}");

        let mut gave_up = rolling(1, 1);
        gave_up["status"]["conditions"] = json!([{"type": "Progressing", "status": "False", "reason": "ProgressDeadlineExceeded"}]);
        let fake = rolling_out(vec![gave_up, rolling(3, 3)]);
        let stuck = kube_wait_rollout(pinned(&fake), None, &wait("deploy", None)).unwrap();
        let ToolResult::Kube { text, summary } = stuck else { panic!("{stuck:?}") };
        assert!(text.starts_with("Rollout of Deployment/api is stuck: no progress within its deadline — Progressing=False (ProgressDeadlineExceeded).\n\nDeployment/api in namespace orders"), "{text}");
        assert_eq!(summary, "stuck");

        let fake = rolling_out(vec![rolling(2, 2), rolling(3, 3)]);
        let stopped = kube_wait_rollout(pinned(&fake), Some(&|| true), &wait("deploy", None)).unwrap();
        let ToolResult::Kube { text, summary } = stopped else { panic!("{stopped:?}") };
        assert!(text.starts_with("Stopped waiting for Deployment/api — the user stopped the turn: 1 of 3 pods up to date."), "{text}");
        assert_eq!(summary, "stopped");
    }

    fn text(result: Result<ToolResult, ToolError>) -> String {
        match result {
            Ok(ToolResult::Kube { text, .. }) => text,
            other => panic!("not a Kubernetes result: {other:?}"),
        }
    }

    fn pod(name: &str, app: &str, containers: &[&str]) -> Value {
        let containers: Vec<Value> = containers.iter().map(|c| json!({"name": c})).collect();
        json!({"metadata": {"name": name, "namespace": "orders", "labels": {"app": app}}, "spec": {"containers": containers}})
    }

    #[test]
    fn with_no_cluster_every_tool_says_so() {
        assert!(matches!(kube_list(None, &KubeListArgs { kind: "pods".into(), ..Default::default() }), Err(ToolError::NoCluster)));
        assert!(matches!(kube_logs(None, &KubeLogsArgs::default()), Err(ToolError::NoCluster)));
    }

    /// A list reads the chat's namespace unless told otherwise, and a Secret's
    /// value asked for as a field is its size.
    #[test]
    fn a_list_is_the_pinned_namespaces_unless_it_asks_for_all() {
        let secret = |ns: &str| json!({"metadata": {"name": "db", "namespace": ns}, "type": "Opaque", "data": {"password": "aHVudGVyMg=="}});
        let fake = Fake::with(&[("", "Secret", "secrets")]).object("secrets", secret("orders")).object("secrets", secret("billing"));
        let args = KubeListArgs { kind: "secret".into(), fields: Some(vec!["data.password".into()]), ..Default::default() };
        let shown = text(kube_list(pinned(&fake), &args));
        assert!(shown.starts_with("1 secrets in namespace orders:"), "{shown}");
        assert!(shown.contains("<7 bytes>") && !shown.contains("aHVudGVy"), "{shown}");
        let everywhere = text(kube_list(pinned(&fake), &KubeListArgs { namespace: Some("*".into()), ..args.clone() }));
        assert!(everywhere.starts_with("2 secrets in all namespaces:") && everywhere.contains("NAMESPACE"), "{everywhere}");
        let none = text(kube_list(pinned(&fake), &KubeListArgs { label_selector: Some("app=x".into()), ..args }));
        assert_eq!(none, "No secrets in namespace orders, labelSelector app=x.");
    }

    #[test]
    fn a_bad_field_path_or_kind_goes_back_to_the_model() {
        let fake = Fake::with(&[("", "Pod", "pods")]);
        let bad = kube_list(pinned(&fake), &KubeListArgs { kind: "pods".into(), fields: Some(vec!["a[x]".into()]), ..Default::default() });
        assert!(matches!(bad, Err(ToolError::InvalidArguments { .. })), "{bad:?}");
        let unknown = kube_list(pinned(&fake), &KubeListArgs { kind: "VirtualService".into(), ..Default::default() });
        assert!(matches!(unknown, Err(ToolError::Kube(KubeError::UnknownKind(_)))), "{unknown:?}");
    }

    #[test]
    fn get_redacts_and_tidies_before_the_model_reads_it() {
        let fake = Fake::with(&[("", "Secret", "secrets")]).object(
            "secrets",
            json!({"metadata": {"name": "db", "namespace": "orders", "managedFields": [{}]}, "data": {"password": "aHVudGVyMg=="}}),
        );
        let shown = text(kube_get(pinned(&fake), &KubeGetArgs { kind: "Secret".into(), name: "db".into(), ..Default::default() }));
        assert!(shown.contains("password: <7 bytes>") && !shown.contains("managedFields"), "{shown}");
        let star = kube_get(pinned(&fake), &KubeGetArgs { kind: "Secret".into(), name: "db".into(), namespace: Some("*".into()), ..Default::default() });
        assert!(matches!(star, Err(ToolError::InvalidArguments { .. })));
    }

    /// The owner's selector finds its pods; each is read from its default
    /// container, and a pod that could not be read is named, not dropped.
    #[test]
    fn logs_of_an_owner_read_every_pod_it_selects() {
        let deployment = json!({"metadata": {"name": "api", "namespace": "orders"},
                                "spec": {"selector": {"matchLabels": {"app": "api"}}}});
        let mut sidecar = pod("api-b", "api", &["istio-proxy", "app"]);
        sidecar["metadata"]["annotations"] = json!({"kubectl.kubernetes.io/default-container": "app"});
        let fake = Fake::with(&[("apps", "Deployment", "deployments"), ("", "Pod", "pods")])
            .object("deployments", deployment)
            .object("pods", pod("api-a", "api", &["app", "istio-proxy"]))
            .object("pods", sidecar)
            .object("pods", pod("web-a", "web", &["app"]))
            .log("api-a", Ok("2026-09-29T10:00:01Z started\n"))
            .log("api-b", Err("container \"app\" is waiting to start"));
        let args = KubeLogsArgs { kind: Some("deploy".into()), name: Some("api".into()), ..Default::default() };
        let shown = text(kube_logs(pinned(&fake), &args));
        assert!(shown.starts_with("Logs of Deployment/api: 2 pods, container app"), "{shown}");
        assert!(shown.contains("10:00:01 [api-a] started"), "{shown}");
        assert!(shown.contains("Not read: api-b: container \"app\" is waiting"), "{shown}");
        let asked = fake.asked();
        assert!(asked.contains(&"list pods Some(\"orders\") Some(\"app=api\") None".to_string()), "{asked:?}");
        assert!(asked.contains(&"logs orders api-b Some(\"app\") Some(200)".to_string()), "the pod's default container: {asked:?}");
    }

    #[test]
    fn one_pod_that_cannot_be_read_is_an_error_and_grep_reads_further_back() {
        let fake = Fake::with(&[("", "Pod", "pods")])
            .object("pods", pod("api-a", "api", &["app"]))
            .log("api-a", Err("previous terminated container \"app\" not found"));
        let failed = kube_logs(pinned(&fake), &KubeLogsArgs { pod: Some("api-a".into()), previous: Some(true), ..Default::default() });
        assert!(matches!(&failed, Err(ToolError::Kube(KubeError::Cluster(m))) if m.contains("not found")), "{failed:?}");

        let fake = Fake::with(&[("", "Pod", "pods")])
            .object("pods", pod("api-a", "api", &["app"]))
            .log("api-a", Ok("2026-09-29T10:00:01Z ok\n2026-09-29T10:00:02Z Timeout calling db\n"));
        let args = KubeLogsArgs { label_selector: Some("app=api".into()), grep: Some("timeout".into()), ..Default::default() };
        let shown = text(kube_logs(pinned(&fake), &args));
        assert!(shown.ends_with("last 1 lines, UTC:\n10:00:02 Timeout calling db"), "{shown}");
        assert!(fake.asked().iter().any(|a| a.ends_with(&format!("Some({LOG_GREP_WINDOW})"))), "{:?}", fake.asked());
        let neither = kube_logs(pinned(&fake), &KubeLogsArgs::default());
        assert!(matches!(neither, Err(ToolError::InvalidArguments { .. })));
    }

    /// The crash: a Deployment one of whose pods restarts. Its status, the
    /// pod's finding, the ReplicaSet's event and the crash's own log — and
    /// nothing about the healthy pod or a neighbour's events.
    #[test]
    fn diagnose_puts_status_pods_events_and_the_crash_log_in_one_report() {
        let deployment = json!({"metadata": {"name": "api", "namespace": "orders"},
            "spec": {"replicas": 2, "selector": {"matchLabels": {"app": "api"}}},
            "status": {"readyReplicas": 1, "conditions": [{"type": "Available", "status": "False", "reason": "MinimumReplicasUnavailable"}]}});
        let mut crashing = pod("api-a", "api", &["app"]);
        crashing["status"] = json!({"phase": "Running", "containerStatuses": [{"name": "app", "ready": false, "restartCount": 4,
            "state": {"waiting": {"reason": "CrashLoopBackOff"}},
            "lastState": {"terminated": {"reason": "Error", "exitCode": 1, "finishedAt": "2026-09-29T10:00:00Z"}}}]});
        let mut healthy = pod("api-b", "api", &["app"]);
        healthy["status"] = json!({"phase": "Running", "containerStatuses": [{"name": "app", "ready": true, "restartCount": 0, "state": {"running": {}}}]});
        let event = |kind: &str, name: &str, reason: &str| json!({"metadata": {"namespace": "orders"},
            "involvedObject": {"kind": kind, "name": name}, "type": "Warning", "reason": reason, "message": reason, "count": 1,
            "lastTimestamp": "2026-09-29T10:00:00Z"});
        let fake = Fake::with(&[("apps", "Deployment", "deployments"), ("", "Pod", "pods")])
            .object("deployments", deployment)
            .object("replicasets", json!({"metadata": {"name": "api-7d9", "namespace": "orders", "labels": {"app": "api"}}}))
            .object("pods", crashing)
            .object("pods", healthy)
            .object("events", event("ReplicaSet", "api-7d9", "FailedCreate"))
            .object("events", event("Pod", "api-a", "BackOff"))
            .object("events", event("Pod", "web-a", "Unrelated"))
            .log("api-a", Ok("2026-09-29T09:59:59Z panic: config key DB_URL missing\n"));
        let args = KubeDiagnoseArgs { kind: "deploy".into(), name: "api".into(), namespace: None };
        let shown = text(kube_diagnose(pinned(&fake), &args));
        for said in [
            "Deployment/api in namespace orders",
            "Replicas: 2 desired, 1 ready",
            "Available=False (MinimumReplicasUnavailable)",
            "Pods: 2, 1 with problems.",
            "api-a: container app: waiting CrashLoopBackOff; 4 restarts; last ended Error, exit code 1",
            "FailedCreate",
            "BackOff",
            "Log of api-a, container app, before its last restart: last 1 lines, UTC:\n09:59:59 panic: config key DB_URL missing",
            "Not visible here: events older than about an hour",
        ] {
            assert!(shown.contains(said), "{said:?} not in:\n{shown}");
        }
        assert!(!shown.contains("Unrelated") && !shown.contains("api-b "), "{shown}");
        assert!(fake.asked().contains(&"logs orders api-a Some(\"app\") Some(40) previous".to_string()), "{:?}", fake.asked());
    }

    /// Pending: no container to read, so the report is why it was not placed.
    #[test]
    fn diagnose_of_a_pending_pod_says_what_it_asked_for_and_which_claim_waits() {
        let mut pending = pod("db-0", "db", &["postgres"]);
        pending["spec"]["containers"][0]["resources"] = json!({"requests": {"cpu": "4", "memory": "8Gi"}});
        pending["spec"]["volumes"] = json!([{"name": "data", "persistentVolumeClaim": {"claimName": "data-db-0"}},
                                            {"name": "logs", "persistentVolumeClaim": {"claimName": "logs-db-0"}}]);
        pending["status"] = json!({"phase": "Pending", "conditions": [{"type": "PodScheduled", "status": "False",
            "reason": "Unschedulable", "message": "0/3 nodes are available: 3 Insufficient cpu."}]});
        let fake = Fake::with(&[("", "Pod", "pods")])
            .object("pods", pending)
            .object("persistentvolumeclaims", json!({"metadata": {"name": "data-db-0", "namespace": "orders"}, "status": {"phase": "Pending"}}))
            .object("persistentvolumeclaims", json!({"metadata": {"name": "logs-db-0", "namespace": "orders"}, "status": {"phase": "Bound"}}));
        let shown = text(kube_diagnose(pinned(&fake), &KubeDiagnoseArgs { kind: "pod".into(), name: "db-0".into(), namespace: None }));
        for said in ["3 Insufficient cpu", "db-0: requests cpu 4000m, memory 8192Mi", "db-0: claim data-db-0 is Pending", "No events of it or its pods."] {
            assert!(shown.contains(said), "{said:?} not in:\n{shown}");
        }
        assert!(!shown.contains("Log of"), "no container ran: {shown}");
        assert!(!shown.contains("logs-db-0"), "a bound claim is no finding: {shown}");
    }

    #[test]
    fn diagnose_of_what_runs_no_pods_itself_says_where_to_look() {
        let fake = Fake::with(&[("batch", "CronJob", "cronjobs")])
            .object("cronjobs", json!({"metadata": {"name": "nightly", "namespace": "orders"}, "spec": {"schedule": "0 3 * * *"}}));
        let refused = kube_diagnose(pinned(&fake), &KubeDiagnoseArgs { kind: "cronjob".into(), name: "nightly".into(), namespace: None });
        assert!(matches!(&refused, Err(ToolError::InvalidArguments { reason, .. }) if reason.contains("latest Job")), "{refused:?}");
    }

    #[test]
    fn a_selector_is_read_from_labels_expressions_or_a_services_map() {
        let deployment = json!({"spec": {"selector": {"matchLabels": {"app": "api"}, "matchExpressions": [
            {"key": "tier", "operator": "In", "values": ["a", "b"]}, {"key": "canary", "operator": "DoesNotExist"}]}}});
        assert_eq!(selector_of(&deployment).as_deref(), Some("app=api,tier in (a,b),!canary"));
        assert_eq!(selector_of(&json!({"spec": {"selector": {"app": "api"}}})).as_deref(), Some("app=api"));
        assert_eq!(selector_of(&json!({"spec": {}})), None);
    }

    #[test]
    fn events_name_the_kind_as_the_server_spells_it() {
        let fake = Fake::with(&[("apps", "Deployment", "deployments"), ("", "Event", "events")]);
        let args = KubeEventsArgs { kind: Some("deploy".into()), name: Some("api".into()), warnings_only: Some(true), ..Default::default() };
        let shown = text(kube_events(pinned(&fake), &args));
        assert!(shown.starts_with("No events for deploy/api in namespace orders"), "{shown}");
        assert_eq!(
            fake.asked(),
            ["list events Some(\"orders\") None Some(\"involvedObject.kind=Deployment,involvedObject.name=api,type=Warning\")"]
        );
    }

    #[test]
    fn top_without_metrics_server_says_so_instead_of_failing() {
        let fake = Fake::with(&[("", "Pod", "pods")]);
        let shown = text(kube_top(pinned(&fake), &KubeTopArgs { kind: "pods".into(), namespace: None }));
        assert!(shown.contains("metrics-server is not installed"), "{shown}");
        let fake = Fake::with(&[("metrics.k8s.io", "NodeMetrics", "nodes")])
            .object("nodes", json!({"metadata": {"name": "n1"}, "usage": {"cpu": "250m", "memory": "1Gi"}}));
        let nodes = text(kube_top(pinned(&fake), &KubeTopArgs { kind: "nodes".into(), namespace: None }));
        assert!(nodes.contains("n1    250m  1024Mi"), "{nodes}");
        assert_eq!(fake.asked(), ["list nodes None None None"], "nodes are not in a namespace");
    }

    #[test]
    fn field_history_is_read_from_the_whole_object() {
        let fake = Fake::with(&[("", "Service", "services")]).object(
            "services",
            json!({"metadata": {"name": "api", "namespace": "orders", "managedFields": [
                {"manager": "ansible", "operation": "Apply", "time": "2026-09-27T14:32:00Z",
                 "fieldsV1": {"f:metadata": {"f:annotations": {"f:networking.istio.io/exportTo": {}}}}}]}}),
        );
        let args = KubeFieldHistoryArgs { kind: "svc".into(), name: "api".into(), ..Default::default() };
        let shown = text(kube_field_history(pinned(&fake), &args));
        assert!(shown.contains("metadata.annotations.networking.istio.io/exportTo  ansible  Apply"), "{shown}");
    }
}
