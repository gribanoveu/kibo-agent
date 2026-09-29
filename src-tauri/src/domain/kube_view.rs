//! What the Kubernetes role's read tools make of the cluster's JSON
//! (`docs/21-kubernetes-mode.md`, K-3): noise cut, Secrets kept in the app,
//! lists as tables, logs merged and squeezed — decisions 5 and 6 of the plan.
//! Pure: `services::ai_tools::tools::kube` fetches, this shapes.

use chrono::{DateTime, Utc};
use serde_json::{Map, Value};

/// A result is cut at this many characters, with a note saying how much
/// there was and how to narrow it.
pub const RESULT_CAP: usize = 8_000;
/// A table cell or a log line longer than this is cut: one stack trace or
/// JSON blob must not be the whole answer.
const CELL_CAP: usize = 200;
const LINE_CAP: usize = 1_000;
/// Stands for a field an object does not have — absence is an answer.
pub const ABSENT: &str = "—";

/// Cuts `text` at [`RESULT_CAP`] on a line boundary and says so.
pub fn cap(text: String, hint: &str) -> String {
    let total = text.chars().count();
    let Some((at, _)) = text.char_indices().nth(RESULT_CAP) else { return text };
    let cut = text[..at].rfind('\n').unwrap_or(at);
    format!("{}\n… cut at {cut} of {total} chars — {hint}", &text[..cut])
}

fn clip(text: &str, limit: usize) -> String {
    match text.char_indices().nth(limit) {
        Some((at, _)) => format!("{}…", &text[..at]),
        None => text.to_string(),
    }
}

/// `a.b.c` in `value`, keys only.
fn at<'v>(value: &'v Value, path: &str) -> Option<&'v Value> {
    path.split('.').try_fold(value, |v, key| v.get(key))
}

fn text_at(value: &Value, path: &str) -> String {
    at(value, path).map_or_else(|| ABSENT.to_string(), cell)
}

fn int_at(value: &Value, path: &str) -> i64 {
    at(value, path).and_then(Value::as_i64).unwrap_or(0)
}

/// One value as a table cell: text as itself, the rest as compact JSON.
fn cell(value: &Value) -> String {
    let text = match value {
        Value::Null => ABSENT.to_string(),
        Value::String(s) if s.is_empty() => "\"\"".to_string(),
        Value::String(s) => s.clone(),
        other => other.to_string(),
    };
    clip(&text.replace('\n', "\\n"), CELL_CAP)
}

/// Drops what the model never needs and pays for on every object: the
/// managed-fields bookkeeping (it has `kubeFieldHistory`), the last applied
/// copy of the whole object, and the server's counters.
pub fn tidy(object: &mut Value) {
    let Some(metadata) = object.get_mut("metadata").and_then(Value::as_object_mut) else { return };
    for noise in ["managedFields", "uid", "resourceVersion", "generation", "selfLink"] {
        metadata.remove(noise);
    }
    if let Some(annotations) = metadata.get_mut("annotations").and_then(Value::as_object_mut) {
        annotations.remove("kubectl.kubernetes.io/last-applied-configuration");
        if annotations.is_empty() {
            metadata.remove("annotations");
        }
    }
}

/// Words in an environment variable's name that say its literal value is a
/// credential. A heuristic, and a wide one: a `KEYCLOAK_URL` goes too.
const SECRET_WORDS: &[&str] = &["PASSWORD", "PASSWD", "TOKEN", "SECRET", "KEY", "CREDENTIAL", "PRIVATE"];

pub const REDACTED: &str = "<redacted>";

/// Keeps secret values in the app: a `Secret`'s data becomes each key's
/// size, and a literal `value` of an env var named like a credential becomes
/// [`REDACTED`], wherever in the object it sits. Run on every object a tool
/// reads, before anything else sees it — including the last-applied copy,
/// which is how a Secret's data would otherwise leave through an annotation.
pub fn redact(kind: &str, object: &mut Value) {
    if kind == "Secret" {
        for (field, encoded) in [("data", true), ("stringData", false)] {
            if let Some(data) = object.get_mut(field).and_then(Value::as_object_mut) {
                for value in data.values_mut() {
                    let text = value.as_str().unwrap_or_default();
                    let bytes = if encoded { decoded_len(text) } else { text.len() };
                    *value = Value::String(format!("<{bytes} bytes>"));
                }
            }
        }
        if let Some(annotations) = object.pointer_mut("/metadata/annotations").and_then(Value::as_object_mut) {
            annotations.remove("kubectl.kubernetes.io/last-applied-configuration");
        }
    }
    redact_env(object);
}

fn decoded_len(base64: &str) -> usize {
    let padding = base64.chars().rev().take_while(|&c| c == '=').count();
    (base64.len() / 4 * 3).saturating_sub(padding)
}

fn redact_env(value: &mut Value) {
    match value {
        Value::Object(map) => {
            for (key, child) in map.iter_mut() {
                if key == "env" {
                    for var in child.as_array_mut().into_iter().flatten() {
                        let name = var.get("name").and_then(Value::as_str).unwrap_or_default().to_uppercase();
                        if SECRET_WORDS.iter().any(|word| name.contains(word)) && var.get("value").is_some() {
                            var["value"] = Value::String(REDACTED.to_string());
                        }
                    }
                } else {
                    redact_env(child);
                }
            }
        }
        Value::Array(items) => items.iter_mut().for_each(redact_env),
        _ => {}
    }
}

/// How long ago `timestamp` was, as `kubectl` shows age: the largest unit.
pub fn age(timestamp: Option<&str>, now: DateTime<Utc>) -> String {
    let Some(then) = timestamp.and_then(|t| DateTime::parse_from_rfc3339(t).ok()) else { return ABSENT.to_string() };
    let seconds = (now - then.with_timezone(&Utc)).num_seconds().max(0);
    match seconds {
        s if s >= 86_400 => format!("{}d", s / 86_400),
        s if s >= 3_600 => format!("{}h", s / 3_600),
        s if s >= 60 => format!("{}m", s / 60),
        s => format!("{s}s"),
    }
}

fn created(object: &Value, now: DateTime<Utc>) -> String {
    age(at(object, "metadata.creationTimestamp").and_then(Value::as_str), now)
}

/// One step of a field path.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Step {
    Key(String),
    Index(usize),
    Every,
}

/// A column the model asked for: `metadata.annotations.networking\.istio\.io/exportTo`,
/// or `metadata.annotations['networking.istio.io/exportTo']`, `spec.ports[*].port`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FieldPath {
    header: String,
    steps: Vec<Step>,
}

impl FieldPath {
    pub fn parse(path: &str) -> Result<FieldPath, String> {
        let trimmed = path.trim().trim_start_matches(['{', '$']).trim_end_matches('}').trim_start_matches('.');
        let mut steps = Vec::new();
        let mut key = String::new();
        let mut chars = trimmed.chars().peekable();
        let bad = |why: &str| format!("field path `{path}`: {why}");
        let end_key = |key: &mut String, steps: &mut Vec<Step>| {
            if !key.is_empty() {
                steps.push(Step::Key(std::mem::take(key)));
            }
        };
        while let Some(c) = chars.next() {
            match c {
                '\\' => key.push(chars.next().ok_or_else(|| bad("ends in a backslash"))?),
                '.' => end_key(&mut key, &mut steps),
                '[' => {
                    end_key(&mut key, &mut steps);
                    let inside: String = chars.by_ref().take_while(|&c| c != ']').collect();
                    let quoted = inside.trim_matches(|c| c == '\'' || c == '"');
                    steps.push(match inside.as_str() {
                        "*" => Step::Every,
                        _ if quoted.len() + 2 == inside.len() => Step::Key(quoted.to_string()),
                        _ => Step::Index(inside.parse().map_err(|_| bad("an index is a number, `*` or a quoted key"))?),
                    });
                }
                c => key.push(c),
            }
        }
        end_key(&mut key, &mut steps);
        let header = steps
            .iter()
            .rev()
            .find_map(|s| match s {
                Step::Key(k) => Some(k.rsplit('/').next().unwrap_or(k).to_uppercase()),
                _ => None,
            })
            .ok_or_else(|| bad("names no field"))?;
        Ok(FieldPath { header, steps })
    }

    /// The field in `object`: [`ABSENT`] when it is not there, the values
    /// joined when the path runs through `[*]`.
    fn read(&self, object: &Value) -> String {
        let mut found = vec![object];
        for step in &self.steps {
            found = found
                .into_iter()
                .flat_map(|v| -> Vec<&Value> {
                    match step {
                        Step::Key(k) => v.get(k).into_iter().collect(),
                        Step::Index(i) => v.get(i).into_iter().collect(),
                        Step::Every => v.as_array().map(|a| a.iter().collect()).unwrap_or_default(),
                    }
                })
                .collect();
        }
        if found.is_empty() {
            return ABSENT.to_string();
        }
        clip(&found.into_iter().map(cell).collect::<Vec<_>>().join(","), CELL_CAP)
    }
}

type Column = (&'static str, fn(&Value, DateTime<Utc>) -> String);

/// A kind's columns after NAME, as `kubectl get` would show them; AGE for
/// a kind it has none for.
fn columns(kind: &str) -> Vec<Column> {
    let aged: Column = ("AGE", created);
    match kind {
        "Pod" => vec![
            ("READY", |o, _| {
                let statuses = at(o, "status.containerStatuses").and_then(Value::as_array);
                let ready = statuses.map_or(0, |s| s.iter().filter(|c| c["ready"] == true).count());
                let all = at(o, "spec.containers").and_then(Value::as_array).map_or(0, Vec::len);
                format!("{ready}/{all}")
            }),
            ("STATUS", |o, _| pod_status(o)),
            ("RESTARTS", |o, _| {
                let statuses = at(o, "status.containerStatuses").and_then(Value::as_array);
                statuses.map_or(0, |s| s.iter().map(|c| int_at(c, "restartCount")).sum::<i64>()).to_string()
            }),
            aged,
            ("NODE", |o, _| text_at(o, "spec.nodeName")),
        ],
        "Deployment" => vec![
            ("READY", |o, _| format!("{}/{}", int_at(o, "status.readyReplicas"), int_at(o, "spec.replicas"))),
            ("UP-TO-DATE", |o, _| int_at(o, "status.updatedReplicas").to_string()),
            ("AVAILABLE", |o, _| int_at(o, "status.availableReplicas").to_string()),
            aged,
        ],
        "StatefulSet" => {
            vec![("READY", |o, _| format!("{}/{}", int_at(o, "status.readyReplicas"), int_at(o, "spec.replicas"))), aged]
        }
        "ReplicaSet" => vec![
            ("DESIRED", |o, _| int_at(o, "spec.replicas").to_string()),
            ("READY", |o, _| int_at(o, "status.readyReplicas").to_string()),
            aged,
        ],
        "DaemonSet" => vec![
            ("DESIRED", |o, _| int_at(o, "status.desiredNumberScheduled").to_string()),
            ("READY", |o, _| int_at(o, "status.numberReady").to_string()),
            ("AVAILABLE", |o, _| int_at(o, "status.numberAvailable").to_string()),
            aged,
        ],
        "Job" => vec![
            ("COMPLETIONS", |o, _| format!("{}/{}", int_at(o, "status.succeeded"), at(o, "spec.completions").and_then(Value::as_i64).unwrap_or(1))),
            ("STATUS", |o, _| {
                let conditions = at(o, "status.conditions").and_then(Value::as_array).cloned().unwrap_or_default();
                let holds = |kind: &str| conditions.iter().any(|c| c["type"] == kind && c["status"] == "True");
                let status = if holds("Complete") {
                    "Complete"
                } else if holds("Failed") {
                    "Failed"
                } else if holds("Suspended") {
                    "Suspended"
                } else {
                    "Running"
                };
                status.to_string()
            }),
            aged,
        ],
        "CronJob" => vec![
            ("SCHEDULE", |o, _| text_at(o, "spec.schedule")),
            ("SUSPEND", |o, _| (at(o, "spec.suspend") == Some(&Value::Bool(true))).to_string()),
            ("LAST-RUN", |o, now| age(at(o, "status.lastScheduleTime").and_then(Value::as_str), now)),
            aged,
        ],
        "Service" => vec![
            ("TYPE", |o, _| text_at(o, "spec.type")),
            ("CLUSTER-IP", |o, _| text_at(o, "spec.clusterIP")),
            ("PORTS", |o, _| {
                let ports = at(o, "spec.ports").and_then(Value::as_array).cloned().unwrap_or_default();
                let port = |p: &Value| match p.get("nodePort") {
                    Some(node) => format!("{}:{}/{}", cell(&p["port"]), cell(node), text_at(p, "protocol")),
                    None => format!("{}/{}", cell(&p["port"]), text_at(p, "protocol")),
                };
                ports.iter().map(port).collect::<Vec<_>>().join(",")
            }),
            aged,
        ],
        "Ingress" => vec![
            ("CLASS", |o, _| text_at(o, "spec.ingressClassName")),
            ("HOSTS", |o, _| {
                let rules = at(o, "spec.rules").and_then(Value::as_array).cloned().unwrap_or_default();
                rules.iter().map(|r| text_at(r, "host")).collect::<Vec<_>>().join(",")
            }),
            aged,
        ],
        "Node" => vec![
            ("STATUS", |o, _| {
                let conditions = at(o, "status.conditions").and_then(Value::as_array).cloned().unwrap_or_default();
                let ready = conditions.iter().any(|c| c["type"] == "Ready" && c["status"] == "True");
                let cordoned = at(o, "spec.unschedulable") == Some(&Value::Bool(true));
                format!("{}{}", if ready { "Ready" } else { "NotReady" }, if cordoned { ",SchedulingDisabled" } else { "" })
            }),
            ("ROLES", |o, _| {
                let labels = at(o, "metadata.labels").and_then(Value::as_object).cloned().unwrap_or_default();
                let roles: Vec<&str> = labels.keys().filter_map(|k| k.strip_prefix("node-role.kubernetes.io/")).collect();
                if roles.is_empty() { ABSENT.to_string() } else { roles.join(",") }
            }),
            ("VERSION", |o, _| text_at(o, "status.nodeInfo.kubeletVersion")),
            aged,
        ],
        "Namespace" => vec![("STATUS", |o, _| text_at(o, "status.phase")), aged],
        "Secret" => vec![
            ("TYPE", |o, _| text_at(o, "type")),
            ("KEYS", |o, _| at(o, "data").and_then(Value::as_object).map_or(0, Map::len).to_string()),
            aged,
        ],
        "ConfigMap" => vec![
            ("KEYS", |o, _| {
                let count = |field| at(o, field).and_then(Value::as_object).map_or(0, Map::len);
                (count("data") + count("binaryData")).to_string()
            }),
            aged,
        ],
        "PersistentVolumeClaim" => vec![
            ("STATUS", |o, _| text_at(o, "status.phase")),
            ("VOLUME", |o, _| text_at(o, "spec.volumeName")),
            ("CAPACITY", |o, _| text_at(o, "status.capacity.storage")),
            aged,
        ],
        "HorizontalPodAutoscaler" => vec![
            ("REFERENCE", |o, _| format!("{}/{}", text_at(o, "spec.scaleTargetRef.kind"), text_at(o, "spec.scaleTargetRef.name"))),
            ("MIN", |o, _| at(o, "spec.minReplicas").and_then(Value::as_i64).unwrap_or(1).to_string()),
            ("MAX", |o, _| int_at(o, "spec.maxReplicas").to_string()),
            ("REPLICAS", |o, _| int_at(o, "status.currentReplicas").to_string()),
            aged,
        ],
        _ => vec![aged],
    }
}

/// A pod's state as `kubectl get pods` sums it up: the reason a container
/// is stuck — its init containers' first — before the phase.
fn pod_status(pod: &Value) -> String {
    if at(pod, "metadata.deletionTimestamp").is_some() {
        return "Terminating".to_string();
    }
    let stuck = |field: &str| -> Option<String> {
        at(pod, field)?.as_array()?.iter().find_map(|c| {
            let state = c.get("state")?;
            let waiting = state.pointer("/waiting/reason").and_then(Value::as_str);
            let failed = state.pointer("/terminated/reason").and_then(Value::as_str).filter(|&r| r != "Completed");
            waiting.or(failed).map(str::to_string)
        })
    };
    if let Some(reason) = stuck("status.initContainerStatuses") {
        return format!("Init:{reason}");
    }
    stuck("status.containerStatuses")
        .or_else(|| at(pod, "status.reason").and_then(Value::as_str).map(str::to_string))
        .unwrap_or_else(|| text_at(pod, "status.phase"))
}

/// Rows as aligned columns, two spaces apart.
fn aligned(rows: &[Vec<String>]) -> String {
    let widths: Vec<usize> = (0..rows.first().map_or(0, Vec::len))
        .map(|i| rows.iter().map(|r| r[i].chars().count()).max().unwrap_or(0))
        .collect();
    rows.iter()
        .map(|row| {
            let last = row.len() - 1;
            row.iter()
                .enumerate()
                .map(|(i, c)| if i == last { c.clone() } else { format!("{c:<w$}", w = widths[i]) })
                .collect::<Vec<_>>()
                .join("  ")
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// A list as a table: NAMESPACE when it spans several, NAME, the kind's
/// columns, then the fields asked for.
pub fn table(kind: &str, items: &[Value], all_namespaces: bool, fields: &[FieldPath], now: DateTime<Utc>) -> String {
    let columns = columns(kind);
    let mut header: Vec<String> = Vec::new();
    if all_namespaces {
        header.push("NAMESPACE".into());
    }
    header.push("NAME".into());
    header.extend(columns.iter().map(|(name, _)| name.to_string()));
    header.extend(fields.iter().map(|f| f.header.clone()));
    let mut rows = vec![header];
    for item in items {
        let mut row = Vec::new();
        if all_namespaces {
            row.push(text_at(item, "metadata.namespace"));
        }
        row.push(text_at(item, "metadata.name"));
        row.extend(columns.iter().map(|(_, read)| read(item, now)));
        row.extend(fields.iter().map(|f| f.read(item)));
        rows.push(row);
    }
    aligned(&rows)
}

/// A single object as YAML, cut to `sections` of its top level when given —
/// `["spec"]` for a question about the desired state alone.
pub fn object_yaml(mut object: Value, sections: &[String]) -> String {
    tidy(&mut object);
    if let Value::Object(map) = &mut object {
        map.remove("apiVersion");
        map.remove("kind");
        if !sections.is_empty() {
            map.retain(|key, _| sections.iter().any(|s| s == key));
        }
    }
    yaml_serde::to_string(&object).unwrap_or_else(|e| format!("(could not render: {e})"))
}

/// Events oldest first, each distinct one once, with its count summed —
/// a probe failing every ten seconds is one line with ×360, not 360.
pub fn events(items: &[Value], now: DateTime<Utc>, limit: usize) -> String {
    struct Seen {
        last: String,
        kind: String,
        reason: String,
        object: String,
        count: i64,
        message: String,
    }
    let mut seen: Vec<Seen> = Vec::new();
    for event in items {
        let object = format!("{}/{}", text_at(event, "involvedObject.kind"), text_at(event, "involvedObject.name"));
        let last = ["lastTimestamp", "eventTime", "series.lastObservedTime", "metadata.creationTimestamp"]
            .iter()
            .find_map(|f| at(event, f).and_then(Value::as_str))
            .unwrap_or_default()
            .to_string();
        let count = at(event, "count").or(at(event, "series.count")).and_then(Value::as_i64).unwrap_or(1);
        let (kind, reason) = (text_at(event, "type"), text_at(event, "reason"));
        let message = clip(text_at(event, "message").trim(), 300);
        match seen.iter_mut().find(|s| s.object == object && s.reason == reason && s.message == message && s.kind == kind) {
            Some(same) => {
                same.count += count;
                same.last = same.last.clone().max(last);
            }
            None => seen.push(Seen { last, kind, reason, object, count, message }),
        }
    }
    seen.sort_by(|a, b| a.last.cmp(&b.last));
    let dropped = seen.len().saturating_sub(limit);
    let mut rows = vec![["LAST", "TYPE", "REASON", "OBJECT", "COUNT", "MESSAGE"].map(String::from).to_vec()];
    for s in seen.into_iter().skip(dropped) {
        rows.push(vec![age(Some(&s.last), now), s.kind, s.reason, s.object, format!("×{}", s.count), s.message]);
    }
    let table = aligned(&rows);
    if dropped > 0 { format!("{dropped} older events left out.\n{table}") } else { table }
}

/// `10m`, `2h`, `30s`, `1d` as seconds.
pub fn parse_since(since: &str) -> Result<i64, String> {
    let since = since.trim();
    let (number, unit) = since.split_at(since.find(|c: char| !c.is_ascii_digit()).unwrap_or(since.len()));
    let number: i64 = number.parse().map_err(|_| format!("`since` is a number and a unit, like 10m or 2h — not `{since}`"))?;
    let unit = match unit {
        "" | "s" => 1,
        "m" => 60,
        "h" => 3_600,
        "d" => 86_400,
        _ => return Err(format!("`since` takes s, m, h or d — not `{since}`")),
    };
    Ok(number * unit)
}

/// One pod's log, as the server sent it with timestamps.
pub struct PodLog {
    pub pod: String,
    pub text: String,
}

/// Several pods' logs as one: lines merged by time, the pod's name before
/// each when `name_pods` — more than one was asked about — colour codes gone, a line repeated in a
/// row said once with ×N, and the last `tail` of what `grep` kept.
pub fn merge_logs(logs: &[PodLog], tail: usize, grep: Option<&regex::Regex>, name_pods: bool) -> Vec<String> {
    let ansi = regex::Regex::new(r"\x1b\[[0-9;?]*[ -/]*[@-~]").expect("a valid pattern");
    let mut lines: Vec<(Option<DateTime<Utc>>, &str, String)> = Vec::new();
    for log in logs {
        let mut stamp = None;
        for raw in log.text.lines() {
            // A line without a stamp continues the one before (a stack trace).
            let parsed = raw.split_once(' ').and_then(|(time, rest)| Some((DateTime::parse_from_rfc3339(time).ok()?, rest)));
            let message = match parsed {
                Some((time, rest)) => {
                    stamp = Some(time.with_timezone(&Utc));
                    rest
                }
                None => raw,
            };
            let message = ansi.replace_all(message, "").trim_end().to_string();
            if grep.is_none_or(|g| g.is_match(&message)) {
                lines.push((stamp, &log.pod, message));
            }
        }
    }
    // Stable: a pod's own lines keep their order under one timestamp.
    lines.sort_by(|a, b| a.0.cmp(&b.0));
    let kept = &lines[lines.len().saturating_sub(tail)..];
    let mut out: Vec<(String, usize)> = Vec::new();
    let mut previous: Option<(&str, &str)> = None;
    for (stamp, pod, message) in kept {
        if previous == Some((pod, message.as_str())) {
            if let Some(last) = out.last_mut() {
                last.1 += 1;
            }
            continue;
        }
        previous = Some((pod, message.as_str()));
        let time = stamp.map_or_else(|| "--:--:--".to_string(), |t| t.format("%H:%M:%S").to_string());
        let prefix = if name_pods { format!("{time} [{pod}] ") } else { format!("{time} ") };
        out.push((format!("{prefix}{}", clip(message, LINE_CAP)), 1));
    }
    out.into_iter().map(|(line, n)| if n > 1 { format!("{line}  ×{n}") } else { line }).collect()
}

/// A CPU quantity in millicores: `250m`, `1`, `12345678n`.
pub fn cpu_millis(quantity: &str) -> f64 {
    let (number, scale) = match quantity.chars().last() {
        Some('n') => (&quantity[..quantity.len() - 1], 1e-6),
        Some('u') => (&quantity[..quantity.len() - 1], 1e-3),
        Some('m') => (&quantity[..quantity.len() - 1], 1.0),
        _ => (quantity, 1_000.0),
    };
    number.parse::<f64>().unwrap_or(0.0) * scale
}

/// A memory quantity in MiB: `131072Ki`, `128Mi`, `1G`, bytes.
pub fn memory_mib(quantity: &str) -> f64 {
    const UNITS: &[(&str, f64)] = &[
        ("Ki", 1024.0),
        ("Mi", 1_048_576.0),
        ("Gi", 1_073_741_824.0),
        ("Ti", 1_099_511_627_776.0),
        ("k", 1e3),
        ("M", 1e6),
        ("G", 1e9),
        ("T", 1e12),
    ];
    let (number, bytes) = UNITS
        .iter()
        .find_map(|(unit, bytes)| quantity.strip_suffix(unit).map(|n| (n, *bytes)))
        .unwrap_or((quantity, 1.0));
    number.parse::<f64>().unwrap_or(0.0) * bytes / 1_048_576.0
}

/// `PodMetrics` or `NodeMetrics` as a table, busiest CPU first.
pub fn top(items: &[Value], all_namespaces: bool) -> String {
    let usage = |item: &Value| -> (f64, f64) {
        let sum = |containers: Vec<&Value>| {
            containers.iter().fold((0.0, 0.0), |(cpu, mem), u| {
                (cpu + cpu_millis(u["cpu"].as_str().unwrap_or("0")), mem + memory_mib(u["memory"].as_str().unwrap_or("0")))
            })
        };
        match item.get("containers").and_then(Value::as_array) {
            Some(containers) => sum(containers.iter().map(|c| &c["usage"]).collect()),
            None => sum(vec![&item["usage"]]),
        }
    };
    let mut measured: Vec<(&Value, (f64, f64))> = items.iter().map(|i| (i, usage(i))).collect();
    measured.sort_by(|a, b| b.1 .0.total_cmp(&a.1 .0));
    let mut header = vec!["NAME".to_string(), "CPU".into(), "MEMORY".into()];
    if all_namespaces {
        header.insert(0, "NAMESPACE".into());
    }
    let mut rows = vec![header];
    for (item, (cpu, memory)) in measured {
        let mut row = vec![text_at(item, "metadata.name"), format!("{cpu:.0}m"), format!("{memory:.0}Mi")];
        if all_namespaces {
            row.insert(0, text_at(item, "metadata.namespace"));
        }
        rows.push(row);
    }
    aligned(&rows)
}

/// Who set which field of `object`, from its `managedFields`: one row per
/// field and manager, cut to the `paths` asked about when there are any.
pub fn field_history(object: &Value, paths: &[String]) -> Option<String> {
    let entries = at(object, "metadata.managedFields")?.as_array()?;
    let mut rows = Vec::new();
    for entry in entries {
        let mut fields = Vec::new();
        expand_fields(&entry["fieldsV1"], "", &mut fields);
        let manager = match entry.get("subresource").and_then(Value::as_str) {
            Some(sub) => format!("{} ({sub})", text_at(entry, "manager")),
            None => text_at(entry, "manager"),
        };
        let time = text_at(entry, "time").replacen('T', " ", 1).chars().take(16).collect::<String>();
        for field in fields {
            if paths.is_empty() || paths.iter().any(|p| field.starts_with(p.trim_start_matches('.'))) {
                rows.push(vec![field, manager.clone(), text_at(entry, "operation"), time.clone()]);
            }
        }
    }
    rows.sort();
    rows.insert(0, ["FIELD", "MANAGER", "OPERATION", "TIME"].map(String::from).to_vec());
    Some(aligned(&rows))
}

/// `FieldsV1` — `{"f:metadata":{"f:labels":{"f:app":{}}}}` — as the paths of
/// its leaves: `metadata.labels.app`. A list item named by its key is
/// `[name=app]`, by value `[=x]`, by position `[2]`.
fn expand_fields(node: &Value, prefix: &str, out: &mut Vec<String>) {
    let Some(map) = node.as_object() else { return };
    for (key, child) in map {
        let step = if let Some(name) = key.strip_prefix("f:") {
            if prefix.is_empty() { name.to_string() } else { format!("{prefix}.{name}") }
        } else if let Some(fields) = key.strip_prefix("k:") {
            let named = serde_json::from_str::<Map<String, Value>>(fields)
                .map(|m| m.iter().map(|(k, v)| format!("{k}={}", cell(v))).collect::<Vec<_>>().join(","))
                .unwrap_or_else(|_| fields.to_string());
            format!("{prefix}[{named}]")
        } else if let Some(value) = key.strip_prefix("v:") {
            format!("{prefix}[={value}]")
        } else if let Some(index) = key.strip_prefix("i:") {
            format!("{prefix}[{index}]")
        } else {
            continue;
        };
        let leaf = child.as_object().is_none_or(|m| m.keys().all(|k| k == "."));
        if leaf {
            out.push(step);
        } else {
            expand_fields(child, &step, out);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn now() -> DateTime<Utc> {
        DateTime::parse_from_rfc3339("2026-09-29T12:00:00Z").unwrap().with_timezone(&Utc)
    }

    #[test]
    fn a_long_result_is_cut_on_a_line_and_says_by_how_much() {
        let text: String = (0..2_000).map(|i| format!("line {i}\n")).collect();
        let cut = cap(text.clone(), "narrow with labelSelector");
        assert!(cut.len() < text.len());
        let note = cut.lines().last().unwrap();
        assert!(note.starts_with("… cut at ") && note.contains(&format!("of {} chars — narrow", text.len())), "{note}");
        assert!(cut.lines().nth_back(1).unwrap().starts_with("line "), "cut mid-line");
        assert_eq!(cap("short".into(), "x"), "short");
    }

    #[test]
    fn tidy_drops_the_bookkeeping_and_keeps_the_annotations_that_matter() {
        let mut object = json!({"metadata": {
            "name": "api", "uid": "u", "resourceVersion": "1", "generation": 3, "managedFields": [{}],
            "annotations": {"kubectl.kubernetes.io/last-applied-configuration": "{…}", "networking.istio.io/exportTo": "*"}
        }});
        tidy(&mut object);
        assert_eq!(object, json!({"metadata": {"name": "api", "annotations": {"networking.istio.io/exportTo": "*"}}}));
        let mut only_applied = json!({"metadata": {"annotations": {"kubectl.kubernetes.io/last-applied-configuration": "{}"}}});
        tidy(&mut only_applied);
        assert_eq!(only_applied, json!({"metadata": {}}));
    }

    /// Decision 6: a Secret's values never leave the app — not as data, not
    /// through the copy `kubectl apply` leaves in an annotation.
    #[test]
    fn a_secret_says_its_keys_and_sizes_and_nothing_else() {
        let mut secret = json!({
            "metadata": {"annotations": {"kubectl.kubernetes.io/last-applied-configuration": "{\"data\":{\"password\":\"aHVudGVyMg==\"}}"}},
            "data": {"password": "aHVudGVyMg==", "user": "YWRtaW4="},
            "stringData": {"token": "abc"}
        });
        redact("Secret", &mut secret);
        assert_eq!(secret["data"], json!({"password": "<7 bytes>", "user": "<5 bytes>"}));
        assert_eq!(secret["stringData"], json!({"token": "<3 bytes>"}));
        assert!(!secret.to_string().contains("aHVudGVyMg"), "{secret}");
        let mut config = json!({"data": {"password": "kept"}});
        redact("ConfigMap", &mut config);
        assert_eq!(config["data"]["password"], "kept", "only a Secret's data is secret");
    }

    #[test]
    fn a_credential_in_a_containers_env_is_redacted_wherever_the_pod_template_is() {
        let mut cronjob = json!({"spec": {"jobTemplate": {"spec": {"template": {"spec": {"containers": [{"env": [
            {"name": "DB_PASSWORD", "value": "hunter2"},
            {"name": "api_token", "value": "t0k"},
            {"name": "DB_HOST", "value": "db"},
            {"name": "SECRET_REF", "valueFrom": {"secretKeyRef": {"name": "db", "key": "p"}}}
        ]}]}}}}}});
        redact("CronJob", &mut cronjob);
        let env = &cronjob["spec"]["jobTemplate"]["spec"]["template"]["spec"]["containers"][0]["env"];
        assert_eq!(env[0]["value"], REDACTED);
        assert_eq!(env[1]["value"], REDACTED);
        assert_eq!(env[2]["value"], "db");
        assert!(env[3].get("value").is_none(), "a reference is not a value to redact");
    }

    #[test]
    fn age_is_the_largest_unit() {
        let ago = |t: &str| age(Some(t), now());
        assert_eq!(ago("2026-09-29T11:59:20Z"), "40s");
        assert_eq!(ago("2026-09-29T11:15:00Z"), "45m");
        assert_eq!(ago("2026-09-29T07:00:00Z"), "5h");
        assert_eq!(ago("2026-09-26T12:00:00Z"), "3d");
        assert_eq!(age(None, now()), ABSENT);
    }

    #[test]
    fn a_field_path_takes_escaped_dots_quoted_keys_and_every_item() {
        let service = json!({"metadata": {"annotations": {"networking.istio.io/exportTo": "*"}},
                             "spec": {"ports": [{"port": 80}, {"port": 443}]}});
        let escaped = FieldPath::parse(r"metadata.annotations.networking\.istio\.io/exportTo").unwrap();
        assert_eq!((escaped.header.as_str(), escaped.read(&service).as_str()), ("EXPORTTO", "*"));
        let quoted = FieldPath::parse("{.metadata.annotations['networking.istio.io/exportTo']}").unwrap();
        assert_eq!(quoted.read(&service), "*");
        assert_eq!(FieldPath::parse("spec.ports[*].port").unwrap().read(&service), "80,443");
        assert_eq!(FieldPath::parse("spec.ports[1].port").unwrap().read(&service), "443");
        assert_eq!(FieldPath::parse("metadata.labels.app").unwrap().read(&service), ABSENT);
        assert!(FieldPath::parse("spec.ports[x]").is_err());
        assert!(FieldPath::parse("  ").is_err());
    }

    /// The `exportTo` question in one call: a row per Service, and the one
    /// without the annotation says so rather than dropping out.
    #[test]
    fn a_table_has_the_kinds_columns_then_the_fields_asked_for() {
        let service = |name: &str, export: Option<&str>| {
            let annotations = export.map_or(json!({}), |e| json!({"networking.istio.io/exportTo": e}));
            json!({"metadata": {"name": name, "namespace": "orders", "creationTimestamp": "2026-09-28T12:00:00Z", "annotations": annotations},
                   "spec": {"type": "ClusterIP", "clusterIP": "10.0.0.1", "ports": [{"port": 80, "protocol": "TCP"}]}})
        };
        let fields = [FieldPath::parse(r"metadata.annotations.networking\.istio\.io/exportTo").unwrap()];
        let shown = table("Service", &[service("orders-api", Some("*")), service("orders-metrics", None)], false, &fields, now());
        assert_eq!(
            shown,
            "NAME            TYPE       CLUSTER-IP  PORTS   AGE  EXPORTTO\n\
             orders-api      ClusterIP  10.0.0.1    80/TCP  1d   *\n\
             orders-metrics  ClusterIP  10.0.0.1    80/TCP  1d   —"
        );
        let everywhere = table("Service", &[service("orders-api", None)], true, &[], now());
        assert!(everywhere.starts_with("NAMESPACE  NAME") && everywhere.contains("\norders     orders-api"), "{everywhere}");
    }

    #[test]
    fn a_pods_status_is_why_it_is_stuck_before_its_phase() {
        let pod = |status: Value| json!({"metadata": {"name": "p"}, "spec": {"containers": [{}, {}]}, "status": status});
        let crashing = pod(json!({"phase": "Running", "containerStatuses": [
            {"ready": true, "restartCount": 0, "state": {"running": {}}},
            {"ready": false, "restartCount": 7, "state": {"waiting": {"reason": "CrashLoopBackOff"}}}
        ]}));
        assert_eq!(table("Pod", &[crashing], false, &[], now()).lines().nth(1).unwrap().split_whitespace().collect::<Vec<_>>(),
                   ["p", "1/2", "CrashLoopBackOff", "7", "—", "—"]);
        let init = pod(json!({"phase": "Pending", "initContainerStatuses": [{"state": {"waiting": {"reason": "ImagePullBackOff"}}}]}));
        assert_eq!(pod_status(&init), "Init:ImagePullBackOff");
        let oom = pod(json!({"phase": "Running", "containerStatuses": [{"state": {"terminated": {"reason": "OOMKilled"}}}]}));
        assert_eq!(pod_status(&oom), "OOMKilled");
        let done = pod(json!({"phase": "Succeeded", "containerStatuses": [{"state": {"terminated": {"reason": "Completed"}}}]}));
        assert_eq!(pod_status(&done), "Succeeded");
        assert_eq!(pod_status(&json!({"metadata": {"deletionTimestamp": "x"}, "status": {"phase": "Running"}})), "Terminating");
    }

    #[test]
    fn an_object_is_yaml_without_the_noise_and_cut_to_its_sections() {
        let deployment = json!({"apiVersion": "apps/v1", "kind": "Deployment",
            "metadata": {"name": "api", "managedFields": [{}]}, "spec": {"replicas": 2}, "status": {"readyReplicas": 1}});
        assert_eq!(object_yaml(deployment.clone(), &[]), "metadata:\n  name: api\nspec:\n  replicas: 2\nstatus:\n  readyReplicas: 1\n");
        assert_eq!(object_yaml(deployment, &["spec".into()]), "spec:\n  replicas: 2\n");
    }

    #[test]
    fn the_same_event_again_is_one_row_with_its_counts_summed() {
        let event = |reason: &str, count: i64, last: &str| {
            json!({"involvedObject": {"kind": "Pod", "name": "api-1"}, "type": "Warning", "reason": reason,
                   "message": "Readiness probe failed", "count": count, "lastTimestamp": last})
        };
        let shown = events(
            &[event("Unhealthy", 30, "2026-09-29T11:50:00Z"), event("BackOff", 1, "2026-09-29T11:40:00Z"), event("Unhealthy", 12, "2026-09-29T11:58:00Z")],
            now(),
            100,
        );
        let rows: Vec<&str> = shown.lines().collect();
        assert_eq!(rows.len(), 3, "{shown}");
        assert!(rows[1].contains("BackOff") && rows[2].starts_with("2m ") && rows[2].contains("×42"), "{shown}");
        let newest = events(&[event("A", 1, "2026-09-29T11:00:00Z"), event("B", 1, "2026-09-29T11:30:00Z")], now(), 1);
        assert!(newest.starts_with("1 older events left out.") && newest.contains("B") && !newest.contains(" A "), "{newest}");
    }

    #[test]
    fn since_is_a_number_and_a_unit() {
        assert_eq!(parse_since("10m"), Ok(600));
        assert_eq!(parse_since("2h"), Ok(7_200));
        assert_eq!(parse_since("1d"), Ok(86_400));
        assert_eq!(parse_since("45"), Ok(45));
        assert!(parse_since("10 minutes").is_err());
        assert!(parse_since("m").is_err());
    }

    /// Two pods of one Deployment: which served the request is unknown, so
    /// both are read and merged by time — the tail is for the whole.
    #[test]
    fn logs_of_several_pods_are_merged_by_time_squeezed_and_tailed_together() {
        let logs = [
            PodLog { pod: "api-a".into(), text: "2026-09-29T10:00:01.5Z GET /health 200\n2026-09-29T10:00:03Z \u{1b}[31mERROR\u{1b}[0m db timeout\n  at pool.rs:12\n".into() },
            PodLog { pod: "api-b".into(), text: "2026-09-29T10:00:02Z GET /orders 500\n2026-09-29T10:00:04Z retry\n2026-09-29T10:00:05Z retry\n2026-09-29T10:00:06Z retry\n".into() },
        ];
        assert_eq!(
            merge_logs(&logs, 100, None, true),
            [
                "10:00:01 [api-a] GET /health 200",
                "10:00:02 [api-b] GET /orders 500",
                "10:00:03 [api-a] ERROR db timeout",
                "10:00:03 [api-a]   at pool.rs:12",
                "10:00:04 [api-b] retry  ×3",
            ]
        );
        assert_eq!(merge_logs(&logs, 2, None, true), ["10:00:05 [api-b] retry  ×2"], "the tail is of lines, before squeezing");
        let errors = regex::Regex::new("(?i)error|500").unwrap();
        assert_eq!(merge_logs(&logs, 100, Some(&errors), true), ["10:00:02 [api-b] GET /orders 500", "10:00:03 [api-a] ERROR db timeout"]);
        assert_eq!(merge_logs(&logs[..1], 1, None, false), ["10:00:03   at pool.rs:12"], "one pod needs no name");
    }

    #[test]
    fn quantities_read_as_millicores_and_mebibytes() {
        assert_eq!(cpu_millis("250m"), 250.0);
        assert_eq!(cpu_millis("2"), 2_000.0);
        assert_eq!(cpu_millis("12500000n"), 12.5);
        assert_eq!(memory_mib("131072Ki"), 128.0);
        assert_eq!(memory_mib("2Gi"), 2_048.0);
        assert_eq!(memory_mib("1048576"), 1.0);
    }

    #[test]
    fn top_sums_a_pods_containers_busiest_first() {
        let pod = |name: &str, cpu: &str| json!({"metadata": {"name": name}, "containers": [
            {"usage": {"cpu": cpu, "memory": "64Mi"}}, {"usage": {"cpu": "10m", "memory": "32Mi"}}]});
        assert_eq!(top(&[pod("idle", "0"), pod("busy", "500m")], false), "NAME  CPU   MEMORY\nbusy  510m  96Mi\nidle  10m   96Mi");
        let node = json!({"metadata": {"name": "n1"}, "usage": {"cpu": "1500000000n", "memory": "4Gi"}});
        assert_eq!(top(&[node], false), "NAME  CPU    MEMORY\nn1    1500m  4096Mi");
    }

    #[test]
    fn field_history_says_who_set_each_field_and_when() {
        let service = json!({"metadata": {"managedFields": [
            {"manager": "ansible", "operation": "Apply", "time": "2026-09-27T14:32:10Z", "fieldsType": "FieldsV1",
             "fieldsV1": {"f:metadata": {"f:annotations": {".": {}, "f:networking.istio.io/exportTo": {}}},
                          "f:spec": {"f:ports": {"k:{\"port\":80,\"protocol\":\"TCP\"}": {".": {}, "f:port": {}}}}}},
            {"manager": "kube-controller-manager", "operation": "Update", "subresource": "status", "time": "2026-09-28T09:00:00Z",
             "fieldsV1": {"f:status": {"f:loadBalancer": {}}}}
        ]}});
        assert_eq!(
            field_history(&service, &[]).unwrap(),
            "FIELD                                              MANAGER                           OPERATION  TIME\n\
             metadata.annotations.networking.istio.io/exportTo  ansible                           Apply      2026-09-27 14:32\n\
             spec.ports[port=80,protocol=TCP].port              ansible                           Apply      2026-09-27 14:32\n\
             status.loadBalancer                                kube-controller-manager (status)  Update     2026-09-28 09:00"
        );
        let asked = field_history(&service, &["metadata.annotations".into()]).unwrap();
        assert_eq!(asked.lines().count(), 2, "{asked}");
        // An item owned whole, with no field of it named: `{".": {}}` is a leaf.
        let whole = json!({"metadata": {"managedFields": [{"manager": "helm", "operation": "Update", "time": "2026-09-27T14:32:00Z",
            "fieldsV1": {"f:spec": {"f:ports": {"k:{\"port\":80}": {".": {}}}}}}]}});
        assert!(field_history(&whole, &[]).unwrap().contains("spec.ports[port=80]  helm"), "an item owned whole is lost");
        assert_eq!(field_history(&json!({"metadata": {}}), &[]), None);
    }
}
