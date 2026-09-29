//! The Kubernetes role's read tools (`docs/21-kubernetes-mode.md`, K-3):
//! `kubeList`, `kubeGet`, `kubeEvents`, `kubeLogs`, `kubeTop`,
//! `kubeFieldHistory`. Each reads the cluster the chat is pinned to — none
//! takes a kubeconfig or a context — through `domain::kube::KubeApi`, and
//! shapes what it read with `domain::kube_view` before the model sees it.

use chrono::Utc;
use serde_json::Value;

use crate::domain::kube::{resolve_kind, KubeKind, ListQuery, LogQuery, PinnedCluster};
use crate::domain::kube_view::{self, cap, FieldPath, PodLog};
use crate::domain::llm::LlmToolDefinition;
use crate::domain::tools::{
    KubeDiagnoseArgs, KubeEventsArgs, KubeFieldHistoryArgs, KubeGetArgs, KubeListArgs, KubeLogsArgs, KubeTopArgs, ToolError, ToolResult,
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::kube::{KubeApi, KubeError, ListPage};
    use serde_json::json;
    use std::sync::Mutex;

    /// A cluster in memory: objects by kind, logs by pod, and what it was asked.
    #[derive(Default)]
    struct Fake {
        kinds: Vec<KubeKind>,
        objects: Vec<(String, Value)>,
        logs: Vec<(String, Result<String, String>)>,
        asked: Mutex<Vec<String>>,
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
                    namespaced: *kind != "Node" && *kind != "NodeMetrics",
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
                .find(|(plural, o)| plural == &kind.plural && o["metadata"]["name"] == name && o["metadata"]["namespace"] == namespace)
                .map(|(_, o)| o.clone())
                .ok_or_else(|| KubeError::Cluster(format!("{} \"{name}\" not found", kind.plural)))
        }

        fn logs(&self, namespace: &str, pod: &str, query: &LogQuery) -> Result<String, KubeError> {
            let previous = if query.previous { " previous" } else { "" };
            self.asked.lock().unwrap().push(format!("logs {namespace} {pod} {:?} {:?}{previous}", query.container, query.tail));
            let (_, text) = self.logs.iter().find(|(p, _)| p == pod).expect("a log for the pod");
            text.clone().map_err(KubeError::Cluster)
        }
    }

    fn pinned(fake: &Fake) -> Option<PinnedCluster<'_>> {
        Some(PinnedCluster { api: fake, namespace: "orders" })
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
