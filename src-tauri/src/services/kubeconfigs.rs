//! The kubeconfig files Chat mode's Kubernetes role is pointed at: kept in
//! `settings.json` by path, and read again — the file, then the cluster — as
//! each turn starts, for the chat's own pin (`docs/21-kubernetes-mode.md`, K-2).

use std::path::Path;

use serde::Serialize;

use crate::domain::kube::{KubeContexts, KubePin, KubeSetup, KubeTarget};
use crate::domain::settings::{KubeSettings, Kubeconfig, SettingsError};
use crate::infra::kube_client::{self, Clusters};
use crate::infra::settings_store;

pub fn list() -> Result<KubeSettings, SettingsError> {
    Ok(settings_store::load()?.kube)
}

/// Adds `config`, or replaces the one of its name.
pub fn save(config: Kubeconfig) -> Result<(), SettingsError> {
    update(|kube| kube.upsert(config))
}

pub fn remove(name: &str) -> Result<(), SettingsError> {
    update(|kube| kube.remove(name))
}

/// Where a new chat starts; `None` goes back to the first.
pub fn pick(name: Option<String>) -> Result<(), SettingsError> {
    update(|kube| kube.active = name)
}

/// A namespace typed on a chat's tab, offered again for that kubeconfig.
pub fn remember_namespace(kubeconfig: &str, namespace: &str) -> Result<(), SettingsError> {
    update(|kube| kube.remember_namespace(kubeconfig, namespace))
}

/// What a kubeconfig's context menu lists, from the file alone.
pub fn contexts(kubeconfig: &str) -> Result<KubeContexts, String> {
    let config = configured(kubeconfig).map_err(|e| e.to_string())?.ok_or_else(|| format!("no kubeconfig named {kubeconfig}"))?;
    kube_client::contexts(Path::new(&config.path)).map_err(|e| e.to_string())
}

/// The namespaces a chat's menu offers, by where each came from.
#[derive(Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Namespaces {
    /// Named by the kubeconfig's contexts: there without asking anyone.
    pub kubeconfig: Vec<String>,
    /// Typed on the tab before, newest first.
    pub typed: Vec<String>,
    /// The cluster's own list, when this identity may list it.
    pub cluster: Vec<String>,
    /// Why the cluster's list is missing — often RBAC, which is no fault.
    pub cluster_error: Option<String>,
}

pub fn namespaces(pin: &KubePin, clusters: &Clusters) -> Result<Namespaces, String> {
    let settings = list().map_err(|e| e.to_string())?;
    let config = settings.configs.iter().find(|c| c.name == pin.kubeconfig).ok_or("no such kubeconfig")?;
    let path = Path::new(&config.path);
    let contexts = kube_client::contexts(path).map_err(|e| e.to_string())?;
    let mut kubeconfig: Vec<String> = contexts.contexts.iter().filter_map(|c| c.namespace.clone()).collect();
    kubeconfig.sort();
    kubeconfig.dedup();
    let typed = settings.typed_namespaces.get(&pin.kubeconfig).cloned().unwrap_or_default();
    let (cluster, cluster_error) = match contexts.chosen(pin) {
        Some(context) => match clusters.namespaces(path, &context.name) {
            Ok(listed) => (listed, None),
            Err(e) => (Vec::new(), Some(e.to_string())),
        },
        None => (Vec::new(), Some("the chat's context is not in the kubeconfig".to_string())),
    };
    Ok(Namespaces { kubeconfig, typed, cluster, cluster_error })
}

/// What the role is told for `pin`: the file checked now rather than trusted
/// from when it was saved, and the cluster asked what it runs and what this
/// identity may do there.
pub fn setup(pin: Option<&KubePin>, clusters: &Clusters) -> Result<KubeSetup, SettingsError> {
    let Some(pin) = pin else { return Ok(KubeSetup::NotSet) };
    // A kubeconfig removed from Settings since the chat was pinned to it.
    let Some(config) = configured(&pin.kubeconfig)? else { return Ok(KubeSetup::NotSet) };
    let path = Path::new(&config.path);
    if !path.is_file() {
        return Ok(KubeSetup::Missing(config));
    }
    let contexts = match kube_client::contexts(path) {
        Ok(contexts) => contexts,
        Err(e) => return Ok(KubeSetup::Unreadable { config, reason: e.to_string() }),
    };
    let Some(context) = contexts.chosen(pin) else {
        let context = pin.context.clone().unwrap_or_else(|| "its current context".to_string());
        return Ok(KubeSetup::NoContext { config, context });
    };
    let namespace = context.namespace_for(pin);
    let reach = clusters.probe(path, &context.name, &namespace);
    Ok(KubeSetup::Pinned(KubeTarget {
        config: config.clone(),
        context: context.name.clone(),
        cluster: context.cluster.clone(),
        namespace,
        reach,
    }))
}

fn configured(name: &str) -> Result<Option<Kubeconfig>, SettingsError> {
    Ok(list()?.configs.into_iter().find(|c| c.name == name))
}

fn update(change: impl FnOnce(&mut KubeSettings)) -> Result<(), SettingsError> {
    let mut settings = settings_store::load()?;
    change(&mut settings.kube);
    settings_store::save(&settings)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::kube::Reach;
    use crate::testing::with_app_dir;

    /// Nothing listens on port 1, so a probe says "unreachable" at once.
    const CONFIG: &str = r#"
apiVersion: v1
kind: Config
current-context: prod
clusters:
- name: eks
  cluster: { server: "https://127.0.0.1:1" }
contexts:
- name: prod
  context: { cluster: eks, user: me, namespace: payments }
- name: stg
  context: { cluster: eks, user: me }
users:
- name: me
  user: { token: "t" }
"#;

    fn added(label: &str) -> (Kubeconfig, std::path::PathBuf) {
        let file = crate::infra::app_dir::dir().unwrap().join(format!("{label}.yaml"));
        let config = Kubeconfig { name: "prod".to_string(), path: file.to_string_lossy().into_owned() };
        save(config.clone()).unwrap();
        (config, file)
    }

    fn pin(context: Option<&str>, namespace: Option<&str>) -> KubePin {
        KubePin { kubeconfig: "prod".into(), context: context.map(Into::into), namespace: namespace.map(Into::into) }
    }

    #[test]
    fn the_setup_follows_the_file_and_the_pin() {
        with_app_dir("kubeconfigs-setup", || {
            let clusters = Clusters::default();
            assert_eq!(setup(None, &clusters).unwrap(), KubeSetup::NotSet);
            assert_eq!(setup(Some(&pin(None, None)), &clusters).unwrap(), KubeSetup::NotSet, "no such kubeconfig");

            let (config, file) = added("setup");
            assert_eq!(setup(Some(&pin(None, None)), &clusters).unwrap(), KubeSetup::Missing(config.clone()));

            std::fs::write(&file, "not: [a kubeconfig").unwrap();
            assert!(matches!(setup(Some(&pin(None, None)), &clusters).unwrap(), KubeSetup::Unreadable { .. }));

            std::fs::write(&file, CONFIG).unwrap();
            let gone = setup(Some(&pin(Some("old"), None)), &clusters).unwrap();
            assert_eq!(gone, KubeSetup::NoContext { config: config.clone(), context: "old".into() });

            let KubeSetup::Pinned(target) = setup(Some(&pin(None, None)), &clusters).unwrap() else { panic!("not pinned") };
            assert_eq!((target.context.as_str(), target.namespace.as_str(), target.cluster.as_str()), ("prod", "payments", "eks"));
            assert!(matches!(target.reach, Reach::Unreachable(_)), "{:?}", target.reach);

            let KubeSetup::Pinned(chosen) = setup(Some(&pin(Some("stg"), Some("orders"))), &clusters).unwrap() else {
                panic!("not pinned")
            };
            assert_eq!((chosen.context.as_str(), chosen.namespace.as_str()), ("stg", "orders"));
        });
    }

    /// The file's namespaces and the typed ones are there whatever the
    /// cluster says; an unreachable cluster costs its list, not the menu.
    #[test]
    fn namespaces_come_from_the_file_the_user_and_the_cluster_when_it_answers() {
        with_app_dir("kubeconfigs-namespaces", || {
            let (_, file) = added("namespaces");
            std::fs::write(&file, CONFIG).unwrap();
            remember_namespace("prod", "orders").unwrap();
            let listed = namespaces(&pin(None, None), &Clusters::default()).unwrap();
            assert_eq!(listed.kubeconfig, ["payments"]);
            assert_eq!(listed.typed, ["orders"]);
            assert!(listed.cluster.is_empty());
            assert!(listed.cluster_error.is_some());
        });
    }

    #[test]
    fn the_last_pick_is_kept_and_cleared() {
        with_app_dir("kubeconfigs-pick", || {
            for name in ["prod", "staging"] {
                save(Kubeconfig { name: name.to_string(), path: format!("/k/{name}") }).unwrap();
            }
            pick(Some("staging".to_string())).unwrap();
            assert_eq!(list().unwrap().active().map(|c| c.name.clone()).as_deref(), Some("staging"));
            pick(None).unwrap();
            assert_eq!(list().unwrap().active().map(|c| c.name.clone()).as_deref(), Some("prod"));
        });
    }
}
