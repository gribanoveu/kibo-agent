//! The one place the app talks to a Kubernetes cluster — the Kubernetes role's
//! (`docs/21-kubernetes-mode.md`, K-2). Only ever the cluster of a kubeconfig
//! the user added in Settings, under the context the chat is pinned to; listed
//! in `data_policy::NETWORK_ALLOWED`, and `docs/08-data-policy.md` says what
//! goes there.
//!
//! `kube-rs` is async and the app's callers are not: each call here blocks on
//! Tauri's runtime from the thread it is called on, which is a blocking one.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

use k8s_openapi::api::authorization::v1::{SelfSubjectRulesReview, SelfSubjectRulesReviewSpec};
use k8s_openapi::api::core::v1::{Namespace, Pod};
use futures_util::StreamExt;
use kube::api::{Api, ApiResource, DeleteParams, DynamicObject, ListParams, LogParams, Patch, PatchParams, PostParams, WatchEvent, WatchParams};
use kube::config::{KubeConfigOptions, Kubeconfig};
use kube::core::discovery::{verbs, Scope};
use kube::{Client, Config, Discovery};

use crate::domain::kube::{
    access_of, KubeApi, KubeContext, KubeContexts, KubeError, KubeKind, ListPage, ListQuery, LogQuery, Reach, Rule,
};
use crate::infra::login_path;

/// A cluster that has not connected in this long is not coming.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
const READ_TIMEOUT: Duration = Duration::from_secs(20);
/// The whole of a probe, a login plugin's run included: `aws eks get-token`
/// waiting on an SSO login nobody will finish must not hold the turn.
const PROBE_TIMEOUT: Duration = Duration::from_secs(10);
/// One read of a tool: a discovery of every API group, a page of a list, a
/// pod's log.
const READ_CALL_TIMEOUT: Duration = Duration::from_secs(30);
/// How often a watch with nothing to show looks at whether it was stopped.
const STOP_CHECK: Duration = Duration::from_millis(500);

/// A kubeconfig's contexts, read from the file alone — no cluster is asked.
pub fn contexts(path: &Path) -> Result<KubeContexts, KubeError> {
    let file = Kubeconfig::read_from(path).map_err(|e| KubeError::Kubeconfig(e.to_string()))?;
    Ok(KubeContexts {
        contexts: file
            .contexts
            .into_iter()
            .map(|named| {
                let context = named.context.unwrap_or_default();
                KubeContext { name: named.name, cluster: context.cluster, namespace: context.namespace }
            })
            .collect(),
        current: file.current_context,
    })
}

/// The manager the app's changes are recorded under in `managedFields`.
const FIELD_MANAGER: &str = "kibo";

/// The clients the app has made, one per kubeconfig and context, kept: a new
/// client is a new login — an `aws eks get-token` run — on every call.
#[derive(Default)]
pub struct Clusters {
    clients: Mutex<HashMap<Key, Client>>,
    /// What each served, from discovery: a dozen requests on a cluster with
    /// many CRDs, asked once per client rather than per call.
    kinds: Mutex<HashMap<Key, Vec<KubeKind>>>,
}

/// A client is for one file as it was: rewritten by a fresh login
/// (`aws eks update-kubeconfig`), the file makes a new one.
type Key = (PathBuf, String, Option<SystemTime>);

impl Clusters {
    /// The server's version and what this identity may do in `namespace` —
    /// or why the cluster could not be asked.
    pub fn probe(&self, path: &Path, context: &str, namespace: &str) -> Reach {
        let asked = self.within(path, context, PROBE_TIMEOUT, |client| async move {
            let version = client.apiserver_version().await.map_err(cluster_error)?.git_version;
            let review = SelfSubjectRulesReview {
                spec: SelfSubjectRulesReviewSpec { namespace: Some(namespace.to_string()) },
                ..Default::default()
            };
            let reviewed = Api::<SelfSubjectRulesReview>::all(client)
                .create(&PostParams::default(), &review)
                .await
                .map_err(cluster_error)?;
            let rules: Vec<Rule> = reviewed
                .status
                .map(|status| status.resource_rules)
                .unwrap_or_default()
                .into_iter()
                .map(|rule| Rule {
                    verbs: rule.verbs,
                    api_groups: rule.api_groups.unwrap_or_default(),
                    resources: rule.resources.unwrap_or_default(),
                })
                .collect();
            Ok(Reach::Answered { version, access: access_of(&rules) })
        });
        asked.unwrap_or_else(|e| Reach::Unreachable(e.to_string()))
    }

    /// Every namespace, when this identity may list them — often it may not.
    pub fn namespaces(&self, path: &Path, context: &str) -> Result<Vec<String>, KubeError> {
        self.within(path, context, PROBE_TIMEOUT, |client| async move {
            let listed = Api::<Namespace>::all(client).list(&ListParams::default()).await.map_err(cluster_error)?;
            let mut names: Vec<String> = listed.items.into_iter().filter_map(|ns| ns.metadata.name).collect();
            names.sort();
            Ok(names)
        })
    }

    /// Runs `ask` with this kubeconfig's client, bounded by `timeout`.
    fn within<T, F, Fut>(&self, path: &Path, context: &str, timeout: Duration, ask: F) -> Result<T, KubeError>
    where
        F: FnOnce(Client) -> Fut,
        Fut: std::future::Future<Output = Result<T, KubeError>>,
    {
        tauri::async_runtime::block_on(async {
            let client = self.client(path, context).await?;
            match tokio::time::timeout(timeout, ask(client)).await {
                Ok(result) => result,
                Err(_) => {
                    // A login that hangs today may be done tomorrow: start over then.
                    self.forget(path, context);
                    Err(KubeError::Timeout(timeout.as_secs()))
                }
            }
        })
    }

    async fn client(&self, path: &Path, context: &str) -> Result<Client, KubeError> {
        let key = (path.to_path_buf(), context.to_string(), modified(path));
        if let Some(client) = self.clients.lock().ok().and_then(|clients| clients.get(&key).cloned()) {
            return Ok(client);
        }
        let mut file = Kubeconfig::read_from(path).map_err(|e| KubeError::Kubeconfig(e.to_string()))?;
        with_login_path(&mut file, login_path::path());
        let options = KubeConfigOptions { context: Some(context.to_string()), ..Default::default() };
        let mut config =
            Config::from_custom_kubeconfig(file, &options).await.map_err(|e| KubeError::Kubeconfig(e.to_string()))?;
        config.connect_timeout = Some(CONNECT_TIMEOUT);
        config.read_timeout = Some(READ_TIMEOUT);
        let client = Client::try_from(config).map_err(cluster_error)?;
        if let Ok(mut clients) = self.clients.lock() {
            clients.retain(|(p, c, _), _| !(p == path && c == context));
            clients.insert(key, client.clone());
        }
        Ok(client)
    }

    fn forget(&self, path: &Path, context: &str) {
        if let Ok(mut clients) = self.clients.lock() {
            clients.retain(|(p, c, _), _| !(p == path && c == context));
        }
    }

    fn kinds(&self, path: &Path, context: &str) -> Result<Vec<KubeKind>, KubeError> {
        let key = (path.to_path_buf(), context.to_string(), modified(path));
        if let Some(kinds) = self.kinds.lock().ok().and_then(|kinds| kinds.get(&key).cloned()) {
            return Ok(kinds);
        }
        let kinds = self.within(path, context, READ_CALL_TIMEOUT, |client| async move {
            // Two requests where the server has aggregated discovery (1.30+),
            // one per API group where it has not.
            let discovery = match Discovery::new(client.clone()).run_aggregated().await {
                Ok(discovery) => discovery,
                Err(_) => Discovery::new(client).run().await.map_err(cluster_error)?,
            };
            Ok(discovery
                .groups()
                .flat_map(|group| group.recommended_resources())
                .filter(|(_, caps)| caps.supports_operation(verbs::LIST))
                .map(|(resource, caps)| KubeKind {
                    group: resource.group,
                    version: resource.version,
                    kind: resource.kind,
                    plural: resource.plural,
                    namespaced: caps.scope == Scope::Namespaced,
                })
                .collect::<Vec<_>>())
        })?;
        if let Ok(mut cached) = self.kinds.lock() {
            cached.retain(|(p, c, _), _| !(p == path && c == context));
            cached.insert(key, kinds.clone());
        }
        Ok(kinds)
    }
}

/// The cluster a chat is pinned to, for its read tools: the kubeconfig and
/// context are fixed here, so no call can reach another.
pub struct ClusterApi {
    clusters: Arc<Clusters>,
    path: PathBuf,
    context: String,
}

impl ClusterApi {
    pub fn new(clusters: Arc<Clusters>, path: &Path, context: &str) -> Self {
        ClusterApi { clusters, path: path.to_path_buf(), context: context.to_string() }
    }

    fn within<T, F, Fut>(&self, ask: F) -> Result<T, KubeError>
    where
        F: FnOnce(Client) -> Fut,
        Fut: std::future::Future<Output = Result<T, KubeError>>,
    {
        self.clusters.within(&self.path, &self.context, READ_CALL_TIMEOUT, ask)
    }
}

fn resource(kind: &KubeKind) -> ApiResource {
    let api_version = if kind.group.is_empty() { kind.version.clone() } else { format!("{}/{}", kind.group, kind.version) };
    ApiResource {
        group: kind.group.clone(),
        version: kind.version.clone(),
        api_version,
        kind: kind.kind.clone(),
        plural: kind.plural.clone(),
    }
}

fn dynamic(client: Client, kind: &KubeKind, namespace: Option<&str>) -> Api<DynamicObject> {
    match namespace {
        Some(namespace) if kind.namespaced => Api::namespaced_with(client, namespace, &resource(kind)),
        _ => Api::all_with(client, &resource(kind)),
    }
}

fn json(object: DynamicObject) -> Result<serde_json::Value, KubeError> {
    serde_json::to_value(object).map_err(|e| KubeError::Cluster(e.to_string()))
}

impl KubeApi for ClusterApi {
    fn kinds(&self) -> Result<Vec<KubeKind>, KubeError> {
        self.clusters.kinds(&self.path, &self.context)
    }

    fn list(&self, kind: &KubeKind, query: &ListQuery) -> Result<ListPage, KubeError> {
        self.within(|client| async move {
            let params = ListParams {
                label_selector: query.label_selector.clone(),
                field_selector: query.field_selector.clone(),
                limit: query.limit,
                continue_token: query.continue_token.clone(),
                ..Default::default()
            };
            let listed = dynamic(client, kind, query.namespace.as_deref()).list(&params).await.map_err(cluster_error)?;
            Ok(ListPage {
                items: listed.items.into_iter().map(json).collect::<Result<_, _>>()?,
                continue_token: listed.metadata.continue_.filter(|token| !token.is_empty()),
                remaining: listed.metadata.remaining_item_count,
            })
        })
    }

    fn get(&self, kind: &KubeKind, namespace: &str, name: &str) -> Result<serde_json::Value, KubeError> {
        self.within(|client| async move { json(dynamic(client, kind, Some(namespace)).get(name).await.map_err(cluster_error)?) })
    }

    fn logs(&self, namespace: &str, pod: &str, query: &LogQuery) -> Result<String, KubeError> {
        self.within(|client| async move {
            let params = LogParams {
                container: query.container.clone(),
                previous: query.previous,
                tail_lines: query.tail,
                since_seconds: query.since_seconds,
                timestamps: true,
                ..Default::default()
            };
            Api::<Pod>::namespaced(client, namespace).logs(pod, &params).await.map_err(cluster_error)
        })
    }

    fn patch(
        &self,
        kind: &KubeKind,
        namespace: &str,
        name: &str,
        patch: &serde_json::Value,
        dry_run: bool,
    ) -> Result<serde_json::Value, KubeError> {
        self.within(|client| async move {
            // Named, so `kubeFieldHistory` shows what this app set.
            let params = PatchParams { dry_run, field_manager: Some(FIELD_MANAGER.to_string()), ..Default::default() };
            let api = dynamic(client, kind, Some(namespace));
            json(api.patch(name, &params, &Patch::Merge(patch)).await.map_err(cluster_error)?)
        })
    }

    fn apply(&self, kind: &KubeKind, namespace: &str, name: &str, object: &serde_json::Value, dry_run: bool) -> Result<serde_json::Value, KubeError> {
        self.within(|client| async move {
            let params = PatchParams { dry_run, force: true, field_manager: Some(FIELD_MANAGER.to_string()), ..Default::default() };
            let api = dynamic(client, kind, Some(namespace));
            json(api.patch(name, &params, &Patch::Apply(object)).await.map_err(cluster_error)?)
        })
    }

    fn delete(&self, kind: &KubeKind, namespace: &str, name: &str, dry_run: bool) -> Result<(), KubeError> {
        self.within(|client| async move {
            let params = DeleteParams { dry_run, ..Default::default() };
            dynamic(client, kind, Some(namespace)).delete(name, &params).await.map_err(cluster_error)?;
            Ok(())
        })
    }

    fn watch(
        &self,
        kind: &KubeKind,
        namespace: &str,
        name: &str,
        timeout: Duration,
        stop: &dyn Fn() -> bool,
        seen: &mut dyn FnMut(&serde_json::Value) -> bool,
    ) -> Result<(), KubeError> {
        // The call's own bound is a backstop: the watch ends itself at `timeout`.
        self.clusters.within(&self.path, &self.context, timeout + READ_CALL_TIMEOUT, |client| async move {
            let api = dynamic(client, kind, Some(namespace));
            let deadline = tokio::time::Instant::now() + timeout;
            let over = || stop() || tokio::time::Instant::now() >= deadline;
            let params = WatchParams::default().fields(&format!("metadata.name={name}"));
            loop {
                // Read, then watch from what was read. The server closes a
                // watch when it likes; one that ends starts over here.
                let object = api.get(name).await.map_err(cluster_error)?;
                let version = object.metadata.resource_version.clone().unwrap_or_default();
                if seen(&json(object)?) || over() {
                    return Ok(());
                }
                let mut events = std::pin::pin!(api.watch(&params, &version).await.map_err(cluster_error)?);
                loop {
                    // Wakes to see whether the turn was stopped — it asks the
                    // cluster nothing.
                    match tokio::time::timeout(STOP_CHECK, events.next()).await {
                        Ok(Some(Ok(WatchEvent::Added(object) | WatchEvent::Modified(object)))) => {
                            if seen(&json(object)?) {
                                return Ok(());
                            }
                        }
                        Ok(Some(Ok(WatchEvent::Deleted(_)))) => return Err(KubeError::NotFound(format!("{} \"{name}\" was deleted", kind.plural))),
                        Ok(Some(Ok(WatchEvent::Bookmark(_)))) | Err(_) => {}
                        Ok(Some(Ok(WatchEvent::Error(_)) | Err(_)) | None) => break,
                    }
                    if over() {
                        return Ok(());
                    }
                }
            }
        })
    }
}

fn modified(path: &Path) -> Option<SystemTime> {
    std::fs::metadata(path).and_then(|m| m.modified()).ok()
}

/// What the cluster said, cut to what fits in a chip's hint and a prompt —
/// for a refusal, the server's own sentence (`pods is forbidden: User …`).
fn cluster_error(error: kube::Error) -> KubeError {
    let missing = matches!(&error, kube::Error::Api(status) if status.code == 404);
    let text = match error {
        kube::Error::Api(status) if !status.message.is_empty() => status.message,
        other => other.to_string(),
    };
    let line: String = text.lines().next().unwrap_or_default().chars().take(400).collect();
    if missing { KubeError::NotFound(line) } else { KubeError::Cluster(line) }
}

/// Gives each login plugin (`exec`) the login shell's `PATH`.
///
/// `kube-rs` starts the plugin itself — `aws`, `gke-gcloud-auth-plugin`,
/// `kubelogin` — so `login_path::apply` never sees its command. Started from
/// the Dock the app has `/usr/bin:/bin:/usr/sbin:/sbin`, where Homebrew's `aws`
/// is not, and the cluster "does not answer" while `kubectl` works in a
/// terminal. `exec.env` is kubeconfig's own field, passed to the plugin, and
/// the lookup of its command honours a `PATH` set there. In memory only; a
/// `PATH` the kubeconfig sets itself is its author's choice and stays.
fn with_login_path(file: &mut Kubeconfig, path: Option<&str>) {
    let Some(path) = path else { return };
    let execs = file.auth_infos.iter_mut().filter_map(|named| named.auth_info.as_mut()?.exec.as_mut());
    for exec in execs {
        let env = exec.env.get_or_insert_with(Vec::new);
        if env.iter().any(|var| var.get("name").map(String::as_str) == Some("PATH")) {
            continue;
        }
        env.push(HashMap::from([("name".to_string(), "PATH".to_string()), ("value".to_string(), path.to_string())]));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::temp_dir;

    /// Two users with a login plugin — one leaving `PATH` alone, one setting its
    /// own — and one with a token.
    const WITH_PLUGINS: &str = r#"
apiVersion: v1
kind: Config
current-context: prod
clusters:
- name: eks
  cluster: { server: "https://127.0.0.1:1" }
contexts:
- name: prod
  context: { cluster: eks, user: aws, namespace: payments }
- name: stg
  context: { cluster: eks, user: token }
users:
- name: aws
  user:
    exec:
      apiVersion: client.authentication.k8s.io/v1beta1
      command: aws
      args: ["eks", "get-token", "--cluster-name", "prod"]
      env:
      - { name: AWS_PROFILE, value: work }
- name: own
  user:
    exec:
      apiVersion: client.authentication.k8s.io/v1beta1
      command: kubelogin
      env:
      - { name: PATH, value: /opt/own/bin }
- name: token
  user: { token: "t" }
"#;

    fn write(label: &str, text: &str) -> PathBuf {
        let path = temp_dir(label).join("config");
        std::fs::write(&path, text).unwrap();
        path
    }

    /// The plugin's `PATH`: `kube-rs` sets the entries in order, so a second
    /// one would win over the first — there has to be exactly one.
    fn path_of(file: &Kubeconfig, user: &str) -> Option<String> {
        let exec = file.auth_infos.iter().find(|a| a.name == user)?.auth_info.as_ref()?.exec.as_ref()?;
        let paths: Vec<&String> = exec
            .env
            .as_ref()?
            .iter()
            .filter(|v| v.get("name").map(String::as_str) == Some("PATH"))
            .filter_map(|v| v.get("value"))
            .collect();
        assert!(paths.len() <= 1, "{user} has more than one PATH: {paths:?}");
        paths.first().map(|p| p.to_string())
    }

    #[test]
    fn a_login_plugin_gets_the_login_shells_path_unless_it_sets_its_own() {
        let mut file = Kubeconfig::read_from(write("kube-path", WITH_PLUGINS)).unwrap();
        with_login_path(&mut file, Some("/opt/homebrew/bin:/usr/bin"));
        assert_eq!(path_of(&file, "aws").as_deref(), Some("/opt/homebrew/bin:/usr/bin"));
        assert_eq!(path_of(&file, "own").as_deref(), Some("/opt/own/bin"), "the kubeconfig's own PATH was overwritten");
        let aws = file.auth_infos.iter().find(|a| a.name == "aws").unwrap();
        let env = aws.auth_info.as_ref().unwrap().exec.as_ref().unwrap().env.as_ref().unwrap();
        assert!(env.iter().any(|v| v.get("name").map(String::as_str) == Some("AWS_PROFILE")), "the plugin's own env was lost");
    }

    /// Started from a terminal there is no login `PATH` to give: nothing changes.
    #[test]
    fn without_a_login_path_the_plugins_are_left_alone() {
        let mut file = Kubeconfig::read_from(write("kube-no-path", WITH_PLUGINS)).unwrap();
        with_login_path(&mut file, None);
        assert_eq!(path_of(&file, "aws"), None);
    }

    #[test]
    fn contexts_come_from_the_file_with_their_namespaces() {
        let read = contexts(&write("kube-contexts", WITH_PLUGINS)).unwrap();
        assert_eq!(read.current.as_deref(), Some("prod"));
        assert_eq!(
            read.contexts,
            vec![
                KubeContext { name: "prod".into(), cluster: "eks".into(), namespace: Some("payments".into()) },
                KubeContext { name: "stg".into(), cluster: "eks".into(), namespace: None },
            ]
        );
        assert!(matches!(contexts(Path::new("/nowhere/config")), Err(KubeError::Kubeconfig(_))));
    }

    /// The real client against a real cluster — the local one the bench uses
    /// (OrbStack's, context `orbstack` in `~/.kube/config`). Ignored: it needs
    /// that cluster running. `cargo test live_cluster -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn live_cluster_is_probed_discovered_listed_and_read() {
        let path = dirs::home_dir().unwrap().join(".kube/config");
        let clusters = Arc::new(Clusters::default());
        let reach = clusters.probe(&path, "orbstack", "kube-system");
        assert!(matches!(reach, Reach::Answered { .. }), "{reach:?}");

        let api = ClusterApi::new(clusters, &path, "orbstack");
        let kinds = api.kinds().unwrap();
        let find = |name: &str| crate::domain::kube::resolve_kind(&kinds, name).unwrap().clone();
        assert!(find("deploy").namespaced && !find("nodes").namespaced, "scope comes from discovery");

        let query = ListQuery { namespace: Some("kube-system".into()), ..Default::default() };
        let pods = api.list(&find("pods"), &query).unwrap().items;
        let name = pods[0]["metadata"]["name"].as_str().expect("a pod in kube-system").to_string();
        assert_eq!(api.get(&find("pods"), "kube-system", &name).unwrap()["metadata"]["name"], name.as_str());
        let log = api.logs("kube-system", &name, &LogQuery { tail: Some(3), ..Default::default() }).unwrap();
        println!("{} kinds, {} pods, log of {name}:\n{log}", kinds.len(), pods.len());
        assert!(log.lines().all(|l| chrono::DateTime::parse_from_rfc3339(l.split(' ').next().unwrap()).is_ok()), "lines carry timestamps");

        // The server's own sentence, not the client's wrapping of it.
        let gone = api.get(&find("pods"), "kube-system", "no-such-pod").unwrap_err().to_string();
        assert_eq!(gone, "pods \"no-such-pod\" not found");
        let paged = api.list(&find("pods"), &ListQuery { limit: Some(1), ..Default::default() }).unwrap();
        assert!(paged.items.len() == 1 && paged.continue_token.is_some(), "a page ends with a token when there is more");
    }

    /// A dry run is checked by the server and kept by nobody; a patch is kept,
    /// moves the spec's generation, and is recorded under the app's name.
    /// Dry-runs CoreDNS, which every OrbStack cluster has. The real patch needs
    /// a Deployment of one's own to scale up and back:
    /// `KIBO_TEST_DEPLOYMENT=kibo-test/web cargo test live_cluster -- --ignored`
    #[test]
    #[ignore]
    fn live_cluster_dry_runs_without_changing_and_patches_for_real() {
        let path = dirs::home_dir().unwrap().join(".kube/config");
        let api = ClusterApi::new(Arc::new(Clusters::default()), &path, "orbstack");
        let deployments = crate::domain::kube::resolve_kind(&api.kinds().unwrap(), "deploy").unwrap().clone();
        let replicas = |object: &serde_json::Value| object["spec"]["replicas"].as_i64().unwrap();

        let before = api.get(&deployments, "kube-system", "coredns").unwrap();
        let patch = serde_json::json!({"spec": {"replicas": replicas(&before) + 1}});
        let dry = api.patch(&deployments, "kube-system", "coredns", &patch, true).unwrap();
        assert_eq!(replicas(&dry), replicas(&before) + 1, "the server answers with what it would be");
        let after = api.get(&deployments, "kube-system", "coredns").unwrap();
        assert_eq!(after["metadata"]["generation"], before["metadata"]["generation"], "a dry run changed the object");

        let Ok(target) = std::env::var("KIBO_TEST_DEPLOYMENT") else { return };
        let (namespace, name) = target.split_once('/').expect("namespace/name");
        let was = api.get(&deployments, namespace, name).unwrap();
        let up = serde_json::json!({"spec": {"replicas": replicas(&was) + 1}});
        let scaled = api.patch(&deployments, namespace, name, &up, false).unwrap();
        assert_eq!(replicas(&scaled), replicas(&was) + 1);
        assert!(scaled["metadata"]["generation"].as_i64() > was["metadata"]["generation"].as_i64());
        let managers: Vec<&str> = scaled["metadata"]["managedFields"].as_array().unwrap().iter().filter_map(|m| m["manager"].as_str()).collect();
        assert!(managers.contains(&FIELD_MANAGER), "{managers:?}");
        let back = serde_json::json!({"spec": {"replicas": replicas(&was)}});
        assert_eq!(replicas(&api.patch(&deployments, namespace, name, &back, false).unwrap()), replicas(&was));
    }

    /// A watch shows the object now and after each change, and ends when told
    /// it has seen enough, when stopped, and when its time is out. Scales a
    /// Deployment of one's own up by one and back:
    /// `KIBO_TEST_DEPLOYMENT=kibo-test/web cargo test live_cluster -- --ignored`
    #[test]
    #[ignore]
    fn live_cluster_watches_a_rollout_to_its_end() {
        use crate::domain::kube_view::{rollout, Rollout};
        let Ok(target) = std::env::var("KIBO_TEST_DEPLOYMENT") else { return };
        let (namespace, name) = target.split_once('/').expect("namespace/name");
        let path = dirs::home_dir().unwrap().join(".kube/config");
        let api = ClusterApi::new(Arc::new(Clusters::default()), &path, "orbstack");
        let deployments = crate::domain::kube::resolve_kind(&api.kinds().unwrap(), "deploy").unwrap().clone();
        let was = api.get(&deployments, namespace, name).unwrap()["spec"]["replicas"].as_i64().unwrap();
        let minute = Duration::from_secs(60);

        for replicas in [was + 1, was] {
            api.patch(&deployments, namespace, name, &serde_json::json!({"spec": {"replicas": replicas}}), false).unwrap();
            let (mut shown, mut stands) = (0, Rollout::Going(String::new()));
            api.watch(&deployments, namespace, name, minute, &|| false, &mut |object| {
                shown += 1;
                stands = rollout("Deployment", object);
                println!("{replicas}: {stands:?}");
                !matches!(stands, Rollout::Going(_))
            })
            .unwrap();
            assert_eq!(stands, Rollout::Done(format!("{replicas} of {replicas} pods up to date and available")));
            assert!(replicas == was || shown > 1, "a pod does not start before the first read");
        }

        let started = std::time::Instant::now();
        let mut shown = 0;
        api.watch(&deployments, namespace, name, minute, &|| true, &mut |_| { shown += 1; false }).unwrap();
        assert!(shown == 1 && started.elapsed() < Duration::from_secs(5), "a stopped turn waits no longer");
        let started = std::time::Instant::now();
        api.watch(&deployments, namespace, name, Duration::from_secs(2), &|| false, &mut |_| false).unwrap();
        assert!((2..6).contains(&started.elapsed().as_secs()), "{:?}", started.elapsed());
        let gone = api.watch(&deployments, namespace, "no-such", minute, &|| false, &mut |_| false).unwrap_err();
        assert!(matches!(gone, KubeError::NotFound(_)), "{gone:?}");
    }

    /// Nothing listens on port 1: the probe says so instead of failing the turn.
    #[test]
    fn a_cluster_that_is_not_there_is_unreachable_not_an_error() {
        let path = write("kube-probe", WITH_PLUGINS);
        let reach = Clusters::default().probe(&path, "stg", "default");
        assert!(matches!(reach, Reach::Unreachable(_)), "{reach:?}");
    }
}
