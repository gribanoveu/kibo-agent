//! The read tools: list, get, events, logs, top, field history, diagnose,
//! waiting on a rollout and probing from a pod.

use super::*;

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
pub(super) fn selector_of(owner: &Value) -> Option<String> {
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
pub(super) fn container_of(pod: &Value, asked: Option<&str>) -> (Option<String>, Vec<String>) {
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
pub(super) const WAIT_SECONDS: u64 = 120;
pub(super) const WAIT_MOST: u64 = 600;

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

/// `kubeProbe`: whether a pod reaches a name, a port or a URL, asked from
/// inside the pod with a command this app wrote — `domain::kube_probe` has
/// them. The next command is tried only when the image lacks the one before.
pub fn kube_probe(kube: Option<PinnedCluster>, args: &KubeProbeArgs) -> Result<ToolResult, ToolError> {
    let cluster = cluster(kube)?;
    let target = kube_probe::target(&args.target).map_err(|why| invalid("kubeProbe", why))?;
    let kind = kind(&cluster, &args.kind)?;
    let owner = cluster.api.get(&kind, cluster.namespace, &args.name)?;
    let pod = if kind.kind == "Pod" {
        args.name.clone()
    } else {
        let what = format!("{}/{}", kind.kind, args.name);
        let selector = selector_of(&owner).ok_or_else(|| invalid("kubeProbe", format!("{what} runs no pods to probe from — name a pod or a workload")))?;
        let query = ListQuery { namespace: Some(cluster.namespace.to_string()), label_selector: Some(selector), ..Default::default() };
        let pods = cluster.api.list(&KubeKind::core("Pod", "pods"), &query)?.items;
        let running = pods.iter().find(|pod| pod.pointer("/status/phase").and_then(Value::as_str) == Some("Running"));
        let name = running.and_then(|pod| pod.pointer("/metadata/name")).and_then(Value::as_str);
        name.ok_or_else(|| invalid("kubeProbe", format!("{what} has no running pod to probe from")))?.to_string()
    };
    let container = args.container.as_deref().map(str::trim).filter(|container| !container.is_empty());
    let from = container.map_or_else(|| format!("pod {pod}"), |container| format!("pod {pod}, container {container}"));

    let mut lacking = Vec::new();
    for command in kube_probe::attempts(&target) {
        let output = cluster.api.exec(cluster.namespace, &pod, container, &command)?;
        if kube_probe::missing(&output) {
            lacking.push(command[0].clone());
            continue;
        }
        let (word, sentence) = kube_probe::verdict(&target, &command, &output);
        let mut out = vec![format!("From {from} to {}: {sentence}", args.target.trim()), format!("Ran: {}", command.join(" "))];
        // Dropped packets are what a policy looks like from inside.
        if matches!(word.as_str(), "timed out" | "no connection") {
            let query = ListQuery { namespace: Some(cluster.namespace.to_string()), ..Default::default() };
            let policies = resolve_kind(&cluster.api.kinds()?, "networkpolicies").ok().and_then(|policies| cluster.api.list(policies, &query).ok());
            let names: Vec<&str> = policies.iter().flat_map(|page| &page.items).filter_map(|policy| policy.pointer("/metadata/name")?.as_str()).collect();
            if !names.is_empty() {
                out.push(format!(
                    "NetworkPolicies in namespace {}: {} — one may be dropping this (kubeGet reads them); the target's namespace may have its own.",
                    cluster.namespace,
                    names.join(", ")
                ));
            }
        }
        return result(out.join("\n"), word, "");
    }
    result(
        format!(
            "Not probed: the image of {from} has none of {} — probe from a pod whose image has one, or from another container of this pod (`container`).",
            lacking.join(", ")
        ),
        "no tool in the image".to_string(),
        "",
    )
}
