use super::*;

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
    assert!(ToolError::KubeReadOnly.to_string().contains("\"Allow changes\""));
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

/// A StatefulSet that deletes its pods' claims when scaled down says so
/// before anyone agrees — and only when it is being scaled down.
#[test]
fn a_scale_down_that_deletes_claims_says_the_data_is_lost() {
    let kinds = [("apps", "StatefulSet", "statefulsets")];
    let queue = |policy: &str, claims: Value| {
        let object = json!({"metadata": {"name": "api", "namespace": "orders", "generation": 1},
            "spec": {"replicas": 2, "persistentVolumeClaimRetentionPolicy": {"whenScaled": policy}, "volumeClaimTemplates": claims}});
        Fake::with(&kinds).object("statefulsets", object)
    };
    let notes = |fake: &Fake, replicas: u32| {
        let recorded = Recorded::default();
        let call = ToolCall::KubeScale(KubeScaleArgs { kind: "sts".into(), name: "api".into(), replicas: Some(replicas), confirm_data_loss: Some(true) });
        let ToolPreview::Change { notes, .. } = preview(writing(fake, &recorded), &call) else { panic!() };
        notes.join(" | ")
    };
    let unasked = |fake: &Fake, replicas: u32, said: Option<bool>| {
        let recorded = Recorded::default();
        let call = ToolCall::KubeScale(KubeScaleArgs { kind: "sts".into(), name: "api".into(), replicas: Some(replicas), confirm_data_loss: said });
        let done = change(writing(fake, &recorded), &call);
        let backups = recorded.backups.lock().unwrap().len();
        (done, backups)
    };
    let claims = json!([{"metadata": {"name": "data"}}]);
    let lost = notes(&queue("Delete", claims.clone()), 0);
    assert!(lost.starts_with("It deletes the claims of the pods it removes") && lost.contains("DATA IS LOST"), "{lost}");
    assert!(notes(&queue("Delete", claims.clone()), 1).contains("DATA IS LOST"));
    assert!(!notes(&queue("Delete", claims.clone()), 3).contains("DATA IS LOST"), "scaling up deletes nothing");
    assert!(!notes(&queue("Retain", claims), 0).contains("DATA IS LOST"));
    assert!(!notes(&queue("Delete", json!([])), 0).contains("DATA IS LOST"), "no claims, nothing to lose");

    // Until the call says the user accepted the loss, nothing is changed — `false` is not acceptance.
    for said in [None, Some(false)] {
        let fake = queue("Delete", json!([{"metadata": {"name": "data"}}]));
        let (refused, backups) = unasked(&fake, 0, said);
        let reason = invalid_reason(refused);
        assert!(reason.starts_with("not done — scaling StatefulSet/api down: It deletes the claims") && reason.contains("confirmDataLoss"), "{reason}");
        assert!(backups == 0 && fake.asked().iter().all(|asked| !asked.starts_with("patch")), "{:?}", fake.asked());
    }
    let fake = queue("Delete", json!([{"metadata": {"name": "data"}}]));
    assert!(unasked(&fake, 0, Some(true)).0.is_ok() && unasked(&fake, 3, None).0.is_ok(), "accepted, or nothing to lose");
    assert!(unasked(&queue("Retain", json!([{"metadata": {"name": "data"}}])), 0, None).0.is_ok());

    // A StatefulSet deleted whole, when it takes its claims with it.
    let gone = |policy: &str| {
        let object = json!({"metadata": {"name": "api", "namespace": "orders"},
            "spec": {"persistentVolumeClaimRetentionPolicy": {"whenDeleted": policy}, "volumeClaimTemplates": [{"metadata": {"name": "data"}}]}});
        let (fake, recorded) = (Fake::with(&kinds).object("statefulsets", object), Recorded::default());
        change(writing(&fake, &recorded), &ToolCall::KubeDelete(KubeDeleteArgs { kind: "sts".into(), name: "api".into(), ..Default::default() }))
    };
    assert!(invalid_reason(gone("Delete")).contains("whenDeleted: Delete"));
    assert!(gone("Retain").is_ok());
}

/// What has no replicas says how it is stopped; nothing to do is said too.
#[test]
fn what_cannot_be_scaled_says_how_it_is_stopped() {
    let (fake, recorded) = (scalable(3), Recorded::default());
    let refused = |kind: &str, replicas: Option<u32>| {
        let args = KubeScaleArgs { kind: kind.into(), name: "api".into(), replicas, ..Default::default() };
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
pub(super) fn scaled(now: i64, left: i64) -> (Fake, Recorded) {
    let (fake, recorded) = (scalable(3), Recorded::default());
    kube_scale(writing(&fake, &recorded), &scale(now as u32)).unwrap();
    recorded.backups.lock().unwrap()[0].0.id = "kc-1".into();
    for audit in recorded.audits.lock().unwrap().iter_mut() {
        audit.id = "kc-1".into();
        audit.generation_after = Some(left);
    }
    (scalable(now), recorded)
}

pub(super) fn undo_of(id: &str) -> KubeUndoArgs {
    KubeUndoArgs { change_id: id.into() }
}

pub(super) fn refusal(result: Result<ToolResult, ToolError>) -> String {
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

pub(super) fn last_patch(fake: &Fake) -> String {
    fake.asked().into_iter().rfind(|asked| asked.starts_with("patch")).unwrap()
}

pub(super) fn invalid_reason(result: Result<ToolResult, ToolError>) -> String {
    match result {
        Err(ToolError::InvalidArguments { reason, .. }) => reason,
        other => panic!("{other:?}"),
    }
}

/// A CronJob and a Job, a Deployment to refuse, and the kinds they are of.
pub(super) fn jobs() -> Fake {
    let job = |suspend: Value| json!({"metadata": {"name": "report", "namespace": "orders", "generation": 2}, "spec": {"suspend": suspend}});
    Fake::with(&[("batch", "CronJob", "cronjobs"), ("batch", "Job", "jobs"), ("apps", "Deployment", "deployments")])
        .object("cronjobs", job(json!(false)))
        .object("jobs", json!({"metadata": {"name": "report", "namespace": "orders", "generation": 1}, "spec": {}}))
        .object("deployments", workload("deployments", 3).1)
}

pub(super) fn suspend(kind: &str, suspend: Option<bool>) -> ToolCall {
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
pub(super) fn rolled_out() -> Fake {
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

pub(super) fn rollback(kind: &str, to_revision: Option<u32>) -> ToolCall {
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
        "patch deployments orders api {\"spec\":{\"template\":{\"spec\":{\"containers\":[{\"name\":\"api\",\"image\":\"api:2\"}],\"nodeSelector\":null}}}}"
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
        "patch deployments orders api {\"spec\":{\"template\":{\"spec\":{\"containers\":[{\"name\":\"api\",\"image\":\"api:3\"}],\"nodeSelector\":{\"gpu\":\"yes\"}}}}}"
    );
}

/// A namespace with a Deployment, a ConfigMap and a Service — and a
/// cluster that also serves a cluster-wide kind.
pub(super) fn namespace_of_three() -> Fake {
    Fake::with(&[("apps", "Deployment", "deployments"), ("", "ConfigMap", "configmaps"), ("", "Service", "services"), ("", "Secret", "secrets"), ("", "Node", "nodes")])
        .object("deployments", json!({"apiVersion": "apps/v1", "kind": "Deployment", "metadata": {"name": "api", "namespace": "orders", "generation": 4, "uid": "u-1"}, "spec": {"replicas": 3}, "status": {"readyReplicas": 3}}))
        .object("services", json!({"apiVersion": "v1", "kind": "Service", "metadata": {"name": "api", "namespace": "orders", "resourceVersion": "70"}, "spec": {"ports": [{"port": 80}]}}))
}

pub(super) fn apply(manifest: &str) -> ToolCall {
    ToolCall::KubeApply(KubeApplyArgs { manifest: manifest.into() })
}

pub(super) const THREE: &str = "\
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
    // A value the model was not shown is not one it may write.
    let hidden = "apiVersion: v1\nkind: ConfigMap\nmetadata: {name: a}\ndata: {DB_PASSWORD: <redacted>, MODE: new}\n";
    assert!(refused(hidden).contains("applying it would overwrite the real value"));
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

pub(super) fn delete(kind: &str, name: &str) -> ToolCall {
    ToolCall::KubeDelete(KubeDeleteArgs { kind: kind.into(), name: name.into(), ..Default::default() })
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
    let accepted = |name: &str| ToolCall::KubeDelete(KubeDeleteArgs { kind: "pvc".into(), name: name.into(), confirm_data_loss: Some(true) });
    let notes = |name: &str| match preview(writing(&fake, &recorded), &accepted(name)) {
        ToolPreview::Change { notes, .. } => notes,
        other => panic!("{other:?}"),
    };
    // Refused until the user has accepted the loss — before the server is asked, and with nothing backed up or deleted.
    for lost in ["data", "unknown"] {
        let reason = invalid_reason(change(writing(&fake, &recorded), &delete("pvc", lost)));
        assert!(reason.starts_with(&format!("not done — deleting PersistentVolumeClaim/{lost}: ")) && reason.contains("confirmDataLoss: true only after the user has said"), "{reason}");
    }
    assert!(recorded.backups.lock().unwrap().is_empty() && fake.asked().iter().all(|asked| !asked.starts_with("delete")), "{:?}", fake.asked());
    assert!(change(writing(&fake, &recorded), &delete("pvc", "kept")).is_ok(), "a kept volume loses nothing");
    assert!(change(writing(&fake, &recorded), &accepted("data")).is_ok());
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
    let scale = |name: &str| ToolCall::KubeScale(KubeScaleArgs { kind: "deploy".into(), name: name.into(), replicas: Some(0), ..Default::default() });
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
    assert_eq!(notes(&ToolCall::KubeDelete(KubeDeleteArgs { kind: "deploy".into(), name: "plain".into(), ..Default::default() })), [way_back]);
    assert!(fake.asked()[asked..].iter().all(|a| !a.starts_with("list")), "{:?}", fake.asked());

    let said = text(change(here, &scale("plain")));
    assert!(said.ends_with("\n  Note — HorizontalPodAutoscaler/plain-hpa sets its replicas (2–9): it will scale it back."), "{said}");

    // Several objects on one card: each warning under its object's name.
    let two = "apiVersion: apps/v1\nkind: Deployment\nmetadata: {name: flux}\nspec: {replicas: 1}\n---\napiVersion: apps/v1\nkind: Deployment\nmetadata: {name: plain}\nspec: {replicas: 1}\n";
    assert_eq!(notes(&ToolCall::KubeApply(KubeApplyArgs { manifest: two.into() })), ["Deployment/flux: Managed by Flux: its next reconcile puts back what Git says.", way_back]);
    assert!(matches!(preview(Some(PinnedCluster { production: true, ..here.unwrap() }), &scale("plain")), ToolPreview::Change { production: true, .. }));
    assert!(matches!(preview(here, &scale("plain")), ToolPreview::Change { production: false, .. }));
}
