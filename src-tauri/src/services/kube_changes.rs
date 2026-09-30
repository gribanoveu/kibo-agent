//! The user's own view of what the app changed in their clusters
//! (`docs/21-kubernetes-mode.md`, K-5b): the list in Settings → Kubernetes.
//! Read only — a change is undone by asking the model, `kubeUndo`.

use crate::domain::kube::KubeChange;
use crate::infra::kube_changes;

/// The changes that can still be undone, newest first: those whose backup
/// is kept. One past its thirty days is gone from here with it.
pub fn list() -> Result<Vec<KubeChange>, String> {
    let history = kube_changes::audit_log()?;
    Ok(history.into_iter().rev().filter(|change| kube_changes::has_backup(&change.id)).collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::kube::{KubeApi, KubeChanges, PinnedCluster};
    use crate::domain::tools::{KubeScaleArgs, KubeUndoArgs, ToolCall, ToolError};
    use crate::infra::kube_changes::ChangeStore;
    use crate::infra::kube_client::{ClusterApi, Clusters};
    use crate::services::ai_tools::tools::cluster;
    use crate::testing::with_app_dir;
    use serde_json::json;
    use std::sync::Arc;

    fn change(id: &str) -> KubeChange {
        KubeChange {
            id: id.into(),
            at: "2026-09-30T10:00:00Z".into(),
            kubeconfig: "prod".into(),
            context: "eks".into(),
            namespace: "orders".into(),
            group: "apps".into(),
            version: "v1".into(),
            kind: "Deployment".into(),
            plural: "deployments".into(),
            name: "api".into(),
            tool: "kubeScale".into(),
            summary: "3 → 0 replicas".into(),
            generation_before: Some(4),
            generation_after: Some(5),
            version_after: None,
            error: None,
            undoes: None,
        }
    }

    #[test]
    fn the_list_is_the_changes_still_backed_up_newest_first() {
        with_app_dir("kube-changes-list", || {
            assert_eq!(list().unwrap(), []);
            for (id, backed_up) in [("kc-1", true), ("kc-2", false), ("kc-3", true)] {
                if backed_up {
                    ChangeStore.backup(&change(id), &json!({})).unwrap();
                }
                ChangeStore.audit(&change(id)).unwrap();
            }
            assert_eq!(list().unwrap(), [change("kc-3"), change("kc-1")]);
        });
    }

    /// The whole way round on the local cluster, through the real store: a
    /// scale, its undo, and an undo refused over somebody else's change.
    /// Needs a Deployment of one's own, left as it was found:
    /// `KIBO_TEST_DEPLOYMENT=kibo-test/web cargo test live_cluster -- --ignored`
    #[test]
    #[ignore]
    fn live_cluster_undoes_a_scale_and_not_over_a_foreign_change() {
        let Ok(target) = std::env::var("KIBO_TEST_DEPLOYMENT") else { return };
        let (namespace, name) = target.split_once('/').expect("namespace/name");
        let path = dirs::home_dir().unwrap().join(".kube/config");
        with_app_dir("kube-changes-live", || {
            let api = ClusterApi::new(Arc::new(Clusters::default()), &path, "orbstack");
            let place = PinnedCluster { api: &api, namespace, kubeconfig: "local", context: "orbstack", writes: true, production: false, changes: Some(&ChangeStore) };
            let kind = crate::domain::kube::resolve_kind(&api.kinds().unwrap(), "deploy").unwrap().clone();
            let replicas = || api.get(&kind, namespace, name).unwrap()["spec"]["replicas"].as_u64().unwrap() as u32;
            let set = |to: u32| api.patch(&kind, namespace, name, &json!({"spec": {"replicas": to}}), false).unwrap();
            let scale = |to: u32| {
                cluster::change(Some(place), &ToolCall::KubeScale(KubeScaleArgs { kind: "deploy".into(), name: name.into(), replicas: Some(to) })).unwrap();
                list().unwrap()[0].id.clone()
            };
            let undo = |id: &str| cluster::change(Some(place), &ToolCall::KubeUndo(KubeUndoArgs { change_id: id.into() }));
            let was = replicas();

            let first = scale(was + 1);
            undo(&first).unwrap();
            assert_eq!(replicas(), was);
            assert!(undo(&first).unwrap_err().to_string().contains("already undone"));

            let second = scale(was + 1);
            set(was + 2);
            assert!(matches!(undo(&second), Err(ToolError::KubeChangedSince(_))));
            assert_eq!(replicas(), was + 2, "a refused undo changed the object");
            set(was);
            assert_eq!(list().unwrap().len(), 3, "two scales and an undo");
        });
    }

    /// The simple changes on the local cluster (K-5c), each left as found:
    /// a CronJob `report` suspended and resumed, a Deployment `rollme` with
    /// two revisions rolled back and put forward again, then restarted.
    /// `KIBO_TEST_DEPLOYMENT=kibo-test/web cargo test live_cluster -- --ignored`
    #[test]
    #[ignore]
    fn live_cluster_suspends_rolls_back_and_restarts() {
        use crate::domain::tools::{KubeRolloutRestartArgs, KubeRolloutUndoArgs, KubeSuspendArgs};
        let Ok(target) = std::env::var("KIBO_TEST_DEPLOYMENT") else { return };
        let (namespace, _) = target.split_once('/').expect("namespace/name");
        let path = dirs::home_dir().unwrap().join(".kube/config");
        with_app_dir("kube-changes-live-simple", || {
            let api = ClusterApi::new(Arc::new(Clusters::default()), &path, "orbstack");
            let place = PinnedCluster { api: &api, namespace, kubeconfig: "local", context: "orbstack", writes: true, production: false, changes: Some(&ChangeStore) };
            let kinds = api.kinds().unwrap();
            let read = |kind: &str, name: &str| api.get(crate::domain::kube::resolve_kind(&kinds, kind).unwrap(), namespace, name).unwrap();
            let run = |call: ToolCall| {
                cluster::change(Some(place), &call).unwrap();
                list().unwrap()[0].id.clone()
            };
            let undo = |id: String| cluster::change(Some(place), &ToolCall::KubeUndo(KubeUndoArgs { change_id: id }));

            let suspended = run(ToolCall::KubeSuspend(KubeSuspendArgs { kind: "cj".into(), name: "report".into(), suspend: Some(true) }));
            assert_eq!(read("cj", "report")["spec"]["suspend"], true);
            undo(suspended).unwrap();
            assert_eq!(read("cj", "report")["spec"]["suspend"], false);

            let before = read("deploy", "rollme")["spec"]["template"].clone();
            let rolled = run(ToolCall::KubeRolloutUndo(KubeRolloutUndoArgs { kind: "deploy".into(), name: "rollme".into(), to_revision: None }));
            assert_ne!(read("deploy", "rollme")["spec"]["template"], before);
            undo(rolled).unwrap();
            assert_eq!(read("deploy", "rollme")["spec"]["template"], before, "the template is back whole");

            let restarted = run(ToolCall::KubeRolloutRestart(KubeRolloutRestartArgs { kind: "deploy".into(), name: "rollme".into() }));
            let stamp = &read("deploy", "rollme")["spec"]["template"]["metadata"]["annotations"]["kubectl.kubernetes.io/restartedAt"];
            assert!(stamp.is_string(), "{stamp}");
            assert!(undo(restarted).unwrap_err().to_string().contains("cannot be undone"));
        });
    }

    /// A manifest applied, changed, deleted and each put back, on the local
    /// cluster (K-5d). Its own objects — a ConfigMap and a Deployment of no
    /// replicas, both `kibo-live` — are gone again when it ends.
    /// `KIBO_TEST_DEPLOYMENT=kibo-test/web cargo test live_cluster -- --ignored`
    #[test]
    #[ignore]
    fn live_cluster_applies_deletes_and_undoes_both() {
        use crate::domain::kube::KubeError;
        use crate::domain::tools::{KubeApplyArgs, KubeDeleteArgs, ToolPreview};
        let Ok(target) = std::env::var("KIBO_TEST_DEPLOYMENT") else { return };
        let (namespace, _) = target.split_once('/').expect("namespace/name");
        let path = dirs::home_dir().unwrap().join(".kube/config");
        let manifest = |beta: &str| {
            format!(
                "apiVersion: v1\nkind: ConfigMap\nmetadata: {{name: kibo-live}}\ndata: {{beta: '{beta}'}}\n---\n\
                 apiVersion: apps/v1\nkind: Deployment\nmetadata: {{name: kibo-live}}\n\
                 spec:\n  replicas: 0\n  selector: {{matchLabels: {{app: kibo-live}}}}\n  template:\n    metadata: {{labels: {{app: kibo-live}}}}\n    \
                 spec: {{containers: [{{name: main, image: 'busybox:stable'}}]}}\n"
            )
        };
        with_app_dir("kube-changes-live-apply", || {
            let api = ClusterApi::new(Arc::new(Clusters::default()), &path, "orbstack");
            let place = PinnedCluster { api: &api, namespace, kubeconfig: "local", context: "orbstack", writes: true, production: false, changes: Some(&ChangeStore) };
            let kinds = api.kinds().unwrap();
            let read = |kind: &str| api.get(crate::domain::kube::resolve_kind(&kinds, kind).unwrap(), namespace, "kibo-live");
            let run = |call: ToolCall| cluster::change(Some(place), &call);
            let apply = |beta: &str| ToolCall::KubeApply(KubeApplyArgs { manifest: manifest(beta) });
            let undo = |id: &str| run(ToolCall::KubeUndo(KubeUndoArgs { change_id: id.into() }));
            let id_of = |name_kind: &str, nth: usize| list().unwrap().into_iter().filter(|c| c.kind == name_kind).nth(nth).unwrap().id;

            // Both created; applied again, neither changes — the server's own defaults are not a difference.
            run(apply("on")).unwrap();
            assert_eq!(read("cm").unwrap()["data"]["beta"], "on");
            // Its controller writes the revision a moment after it is created: a change, but not ours.
            for _ in 0..50 {
                if !read("deploy").unwrap()["metadata"]["annotations"].is_null() {
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(100));
            }
            assert!(run(apply("on")).unwrap_err().to_string().contains("exactly as the manifest says"));

            // Only the ConfigMap changes, and its card shows that one line.
            let ToolPreview::Change { summary, diffs, .. } = cluster::preview(Some(place), &apply("off")) else { panic!() };
            assert!(summary.starts_with("ConfigMap/kibo-live: update"), "{summary}");
            assert_eq!(diffs.len(), 1);
            run(apply("off")).unwrap();
            undo(&id_of("ConfigMap", 0)).unwrap();
            assert_eq!(read("cm").unwrap()["data"]["beta"], "on", "the update is put back");

            // A delete, and the object made again from its backup.
            run(ToolCall::KubeDelete(KubeDeleteArgs { kind: "deploy".into(), name: "kibo-live".into() })).unwrap();
            assert!(matches!(read("deploy"), Err(KubeError::NotFound(_))));
            undo(&id_of("Deployment", 0)).unwrap();
            assert_eq!(read("deploy").unwrap()["spec"]["template"]["spec"]["containers"][0]["image"], "busybox:stable");

            // Undoing that undo deletes it again; the ConfigMap goes by a delete of its own.
            undo(&id_of("Deployment", 0)).unwrap();
            assert!(matches!(read("deploy"), Err(KubeError::NotFound(_))));
            run(ToolCall::KubeDelete(KubeDeleteArgs { kind: "cm".into(), name: "kibo-live".into() })).unwrap();
            assert!(matches!(read("cm"), Err(KubeError::NotFound(_))));
        });
    }

    /// An autoscaler the cluster really has is named on a scale's card (K-5e).
    /// Makes an HPA for `rollme` and takes it away again.
    /// `KIBO_TEST_DEPLOYMENT=kibo-test/web cargo test live_cluster -- --ignored`
    #[test]
    #[ignore]
    fn live_cluster_names_the_autoscaler_on_a_scales_card() {
        use crate::domain::tools::{KubeApplyArgs, KubeDeleteArgs, ToolPreview};
        let Ok(target) = std::env::var("KIBO_TEST_DEPLOYMENT") else { return };
        let (namespace, _) = target.split_once('/').expect("namespace/name");
        let path = dirs::home_dir().unwrap().join(".kube/config");
        let scaler = "apiVersion: autoscaling/v2\nkind: HorizontalPodAutoscaler\nmetadata: {name: kibo-live}\n\
                      spec:\n  minReplicas: 1\n  maxReplicas: 3\n  scaleTargetRef: {apiVersion: apps/v1, kind: Deployment, name: rollme}\n  \
                      metrics: [{type: Resource, resource: {name: cpu, target: {type: Utilization, averageUtilization: 80}}}]\n";
        with_app_dir("kube-changes-live-hpa", || {
            let api = ClusterApi::new(Arc::new(Clusters::default()), &path, "orbstack");
            let place = PinnedCluster { api: &api, namespace, kubeconfig: "local", context: "orbstack", writes: true, production: false, changes: Some(&ChangeStore) };
            cluster::change(Some(place), &ToolCall::KubeApply(KubeApplyArgs { manifest: scaler.into() })).unwrap();
            let scale = ToolCall::KubeScale(KubeScaleArgs { kind: "deploy".into(), name: "rollme".into(), replicas: Some(5) });
            let shown = cluster::preview(Some(place), &scale);
            cluster::change(Some(place), &ToolCall::KubeDelete(KubeDeleteArgs { kind: "hpa".into(), name: "kibo-live".into() })).unwrap();
            let ToolPreview::Change { notes, .. } = shown else { panic!("{shown:?}") };
            assert_eq!(notes[0], "HorizontalPodAutoscaler/kibo-live sets its replicas (1–3): it will scale it back.");
        });
    }
}
