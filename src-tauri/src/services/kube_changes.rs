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
    use crate::domain::tools::{KubeScaleArgs, KubeUndoArgs, ToolError};
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
            let place = PinnedCluster { api: &api, namespace, kubeconfig: "local", context: "orbstack", writes: true, changes: Some(&ChangeStore) };
            let kind = crate::domain::kube::resolve_kind(&api.kinds().unwrap(), "deploy").unwrap().clone();
            let replicas = || api.get(&kind, namespace, name).unwrap()["spec"]["replicas"].as_u64().unwrap() as u32;
            let set = |to: u32| api.patch(&kind, namespace, name, &json!({"spec": {"replicas": to}}), false).unwrap();
            let scale = |to: u32| {
                cluster::kube_scale(Some(place), &KubeScaleArgs { kind: "deploy".into(), name: name.into(), replicas: Some(to) }).unwrap();
                list().unwrap()[0].id.clone()
            };
            let undo = |id: &str| cluster::kube_undo(Some(place), &KubeUndoArgs { change_id: id.into() });
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
}
