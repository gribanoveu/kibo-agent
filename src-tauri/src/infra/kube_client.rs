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
use std::sync::Mutex;
use std::time::{Duration, SystemTime};

use k8s_openapi::api::authorization::v1::{SelfSubjectRulesReview, SelfSubjectRulesReviewSpec};
use k8s_openapi::api::core::v1::Namespace;
use kube::api::{Api, ListParams, PostParams};
use kube::config::{KubeConfigOptions, Kubeconfig};
use kube::{Client, Config};

use crate::domain::kube::{access_of, KubeContext, KubeContexts, KubeError, Reach, Rule};
use crate::infra::login_path;

/// A cluster that has not connected in this long is not coming.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
const READ_TIMEOUT: Duration = Duration::from_secs(20);
/// The whole of a probe, a login plugin's run included: `aws eks get-token`
/// waiting on an SSO login nobody will finish must not hold the turn.
const PROBE_TIMEOUT: Duration = Duration::from_secs(10);

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

/// The clients the app has made, one per kubeconfig and context, kept: a new
/// client is a new login — an `aws eks get-token` run — on every call.
#[derive(Default)]
pub struct Clusters {
    clients: Mutex<HashMap<Key, Client>>,
}

/// A client is for one file as it was: rewritten by a fresh login
/// (`aws eks update-kubeconfig`), the file makes a new one.
type Key = (PathBuf, String, Option<SystemTime>);

impl Clusters {
    /// The server's version and what this identity may do in `namespace` —
    /// or why the cluster could not be asked.
    pub fn probe(&self, path: &Path, context: &str, namespace: &str) -> Reach {
        let asked = self.within(path, context, |client| async move {
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
        self.within(path, context, |client| async move {
            let listed = Api::<Namespace>::all(client).list(&ListParams::default()).await.map_err(cluster_error)?;
            let mut names: Vec<String> = listed.items.into_iter().filter_map(|ns| ns.metadata.name).collect();
            names.sort();
            Ok(names)
        })
    }

    /// Runs `ask` with this kubeconfig's client, bounded by [`PROBE_TIMEOUT`].
    fn within<T, F, Fut>(&self, path: &Path, context: &str, ask: F) -> Result<T, KubeError>
    where
        F: FnOnce(Client) -> Fut,
        Fut: std::future::Future<Output = Result<T, KubeError>>,
    {
        tauri::async_runtime::block_on(async {
            let client = self.client(path, context).await?;
            match tokio::time::timeout(PROBE_TIMEOUT, ask(client)).await {
                Ok(result) => result,
                Err(_) => {
                    // A login that hangs today may be done tomorrow: start over then.
                    self.forget(path, context);
                    Err(KubeError::Timeout(PROBE_TIMEOUT.as_secs()))
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
}

fn modified(path: &Path) -> Option<SystemTime> {
    std::fs::metadata(path).and_then(|m| m.modified()).ok()
}

/// What the cluster said, cut to what fits in a chip's hint and a prompt.
fn cluster_error(error: kube::Error) -> KubeError {
    let text = error.to_string();
    let line = text.lines().next().unwrap_or_default();
    KubeError::Cluster(line.chars().take(240).collect())
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

    /// Nothing listens on port 1: the probe says so instead of failing the turn.
    #[test]
    fn a_cluster_that_is_not_there_is_unreachable_not_an_error() {
        let path = write("kube-probe", WITH_PLUGINS);
        let reach = Clusters::default().probe(&path, "stg", "default");
        assert!(matches!(reach, Reach::Unreachable(_)), "{reach:?}");
    }
}
