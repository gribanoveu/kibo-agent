use super::*;

/// A Deployment of three, `updated` and `available` of them so far.
pub(super) fn rolling(updated: i64, available: i64) -> Value {
    json!({"metadata": {"name": "api", "namespace": "orders", "generation": 4},
           "spec": {"replicas": 3, "selector": {"matchLabels": {"app": "api"}}},
           "status": {"observedGeneration": 4, "replicas": 3, "updatedReplicas": updated, "availableReplicas": available}})
}

pub(super) fn rolling_out(later: Vec<Value>) -> Fake {
    let kinds = [("apps", "Deployment", "deployments"), ("batch", "CronJob", "cronjobs"), ("", "Pod", "pods")];
    let waiting = json!({"metadata": {"name": "api-1", "namespace": "orders", "labels": {"app": "api"}},
        "status": {"phase": "Pending", "containerStatuses": [{"name": "app", "state": {"waiting": {"reason": "ImagePullBackOff"}}}]}});
    Fake { later, ..Fake::with(&kinds).object("deployments", rolling(1, 1)).object("pods", waiting).log("api-1", Ok("")) }
}

pub(super) fn wait(kind: &str, seconds: Option<u64>) -> KubeWaitRolloutArgs {
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

/// A Deployment with a pod still starting and one running, and an image
/// that has the `programs` given.
pub(super) fn probing(programs: &[(&str, i32, &str)]) -> Fake {
    let pod = |name: &str, phase: &str| json!({"metadata": {"name": name, "namespace": "orders", "labels": {"app": "api"}}, "status": {"phase": phase}});
    let kinds = [("apps", "Deployment", "deployments"), ("", "Pod", "pods"), ("", "ConfigMap", "configmaps"), ("networking.k8s.io", "NetworkPolicy", "networkpolicies")];
    let programs = programs.iter().map(|(program, code, stderr)| (program.to_string(), crate::domain::kube::ExecOutput { code: *code, stderr: stderr.to_string(), ..Default::default() }));
    let fake = Fake::with(&kinds)
        .object("deployments", rolling(3, 3))
        .object("pods", pod("api-new", "Pending"))
        .object("pods", json!({"metadata": {"name": "api-elsewhere", "namespace": "billing", "labels": {"app": "api"}}, "status": {"phase": "Running"}}))
        .object("pods", pod("api-1", "Running"))
        .object("configmaps", json!({"metadata": {"name": "flags", "namespace": "orders"}}))
        .object("networkpolicies", json!({"metadata": {"name": "deny-all", "namespace": "orders"}}));
    Fake { programs: programs.collect(), ..fake }
}

pub(super) fn probe(kind: &str, name: &str, container: Option<&str>, target: &str) -> KubeProbeArgs {
    KubeProbeArgs { kind: kind.into(), name: name.into(), container: container.map(str::to_string), target: target.into() }
}

/// A read: it works in a read-only chat. From a workload it is a running
/// pod of it; the next program is asked only when the image lacks one.
#[test]
fn a_probe_runs_in_a_running_pod_with_the_first_program_the_image_has() {
    let fake = probing(&[("nc", 0, "")]);
    let open = kube_probe(pinned(&fake), &probe("deploy", "api", None, "db:5432")).unwrap();
    assert_eq!(open, ToolResult::Kube { text: "From pod api-1 to db:5432: Port 5432 on db is open.\nRan: nc -z -w 5 db 5432".into(), summary: "open".into() });
    assert_eq!(fake.asked().last().unwrap(), "exec orders api-1 None nc -z -w 5 db 5432");

    let fake = probing(&[("wget", 1, "  HTTP/1.1 503 Service Unavailable\n")]);
    let answered = text(kube_probe(pinned(&fake), &probe("pod", "api-new", Some(" sidecar "), "http://orders/health")));
    assert!(answered.starts_with("From pod api-new, container sidecar to http://orders/health: HTTP 503 — the server answered.\nRan: wget "), "{answered}");
    let execs: Vec<String> = fake.asked().into_iter().filter(|asked| asked.starts_with("exec")).map(|asked| asked.split(' ').take(5).collect::<Vec<_>>().join(" ")).collect();
    assert_eq!(execs, ["exec orders api-new Some(\"sidecar\") curl", "exec orders api-new Some(\"sidecar\") wget"]);

    let bare = probing(&[]);
    let none = kube_probe(pinned(&bare), &probe("deploy", "api", None, "db")).unwrap();
    let ToolResult::Kube { text, summary } = none else { panic!("{none:?}") };
    assert!(text.starts_with("Not probed: the image of pod api-1 has none of getent, nslookup — "), "{text}");
    assert_eq!(summary, "no tool in the image");
}

#[test]
fn a_probe_that_is_dropped_names_the_namespaces_policies_and_a_refused_one_does_not() {
    let dropped = text(kube_probe(pinned(&probing(&[("curl", 28, "curl: (28) Connection timed out")])), &probe("deploy", "api", None, "http://db/")));
    assert!(dropped.ends_with("\nNetworkPolicies in namespace orders: deny-all — one may be dropping this (kubeGet reads them); the target's namespace may have its own."), "{dropped}");
    let silent = text(kube_probe(pinned(&probing(&[("nc", 1, "")])), &probe("deploy", "api", None, "db:5432")));
    assert!(silent.contains("NetworkPolicies in namespace orders: deny-all"), "{silent}");
    let refused = text(kube_probe(pinned(&probing(&[("curl", 7, "curl: (7) Failed to connect")])), &probe("deploy", "api", None, "http://db/")));
    assert!(refused.contains("the connection was refused") && !refused.contains("NetworkPolicies"), "{refused}");
}

#[test]
fn a_probe_is_refused_for_its_target_or_for_a_source_with_no_running_pod() {
    let fake = probing(&[("nc", 0, "")]);
    let reason = |args: KubeProbeArgs| invalid_reason(kube_probe(pinned(&fake), &args));
    assert_eq!(reason(probe("deploy", "api", None, "-o/etc/passwd")), "`-o/etc/passwd` is not a host name or an IPv4 address");
    assert_eq!(reason(probe("configmap", "flags", None, "db:5432")), "ConfigMap/flags runs no pods to probe from — name a pod or a workload");
    assert!(fake.asked().iter().all(|asked| !asked.starts_with("exec")), "{:?}", fake.asked());

    let mut starting = probing(&[("nc", 0, "")]);
    starting.objects.retain(|(_, object)| object["metadata"]["name"] != "api-1");
    assert_eq!(invalid_reason(kube_probe(pinned(&starting), &probe("deploy", "api", None, "db:5432"))), "Deployment/api has no running pod to probe from");
    assert!(matches!(kube_probe(None, &probe("deploy", "api", None, "db:5432")), Err(ToolError::NoCluster)));
}

pub(super) fn text(result: Result<ToolResult, ToolError>) -> String {
    match result {
        Ok(ToolResult::Kube { text, .. }) => text,
        other => panic!("not a Kubernetes result: {other:?}"),
    }
}

pub(super) fn pod(name: &str, app: &str, containers: &[&str]) -> Value {
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
