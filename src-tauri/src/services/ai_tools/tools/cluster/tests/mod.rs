use super::*;
use crate::domain::tools::KubeUndoArgs;
use crate::domain::kube::{KubeApi, KubeChanges, KubeError, ListPage};
use serde_json::json;
use std::sync::Mutex;

/// A cluster in memory: objects by kind, logs by pod, and what it was asked.
#[derive(Default)]
struct Fake {
    kinds: Vec<KubeKind>,
    objects: Vec<(String, Value)>,
    logs: Vec<(String, Result<String, String>)>,
    asked: Mutex<Vec<String>>,
    /// What the server says to every patch, when it refuses them.
    refuses: Option<String>,
    /// What a watched object becomes, change by change.
    later: Vec<Value>,
    /// What each program in the image answers; one not here is not in it.
    programs: Vec<(String, crate::domain::kube::ExecOutput)>,
}

/// What was backed up and audited; `full` is a disk that takes no backup.
#[derive(Default)]
struct Recorded {
    backups: Mutex<Vec<(KubeChange, Value)>>,
    audits: Mutex<Vec<KubeChange>>,
    full: bool,
}

impl KubeChanges for Recorded {
    fn backup(&self, change: &KubeChange, object: &Value) -> Result<(), String> {
        if self.full {
            return Err("no space left".to_string());
        }
        self.backups.lock().unwrap().push((change.clone(), object.clone()));
        Ok(())
    }

    fn audit(&self, change: &KubeChange) -> Result<(), String> {
        self.audits.lock().unwrap().push(change.clone());
        Ok(())
    }

    fn load(&self, id: &str) -> Result<(KubeChange, Value), String> {
        let backups = self.backups.lock().unwrap();
        backups.iter().find(|(change, _)| change.id == id).cloned().ok_or_else(|| format!("there is no backup of the change {id}"))
    }

    fn history(&self) -> Result<Vec<KubeChange>, String> {
        Ok(self.audits.lock().unwrap().clone())
    }
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
                namespaced: !["Node", "NodeMetrics", "PersistentVolume"].contains(kind),
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

/// RFC 7386, as the server applies a merge patch.
fn merge(object: &mut Value, patch: &Value) {
    let Some(patch) = patch.as_object() else { return *object = patch.clone() };
    if !object.is_object() {
        *object = json!({});
    }
    for (key, value) in patch {
        if value.is_null() {
            object.as_object_mut().unwrap().shift_remove(key);
        } else {
            merge(&mut object[key], value);
        }
    }
}

/// The tools by their old names: each is `change` on its own call.
fn kube_scale(kube: Option<PinnedCluster>, args: &KubeScaleArgs) -> Result<ToolResult, ToolError> {
    change(kube, &ToolCall::KubeScale(args.clone()))
}

fn kube_undo(kube: Option<PinnedCluster>, args: &KubeUndoArgs) -> Result<ToolResult, ToolError> {
    change(kube, &ToolCall::KubeUndo(args.clone()))
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
            .find(|(plural, o)| plural == &kind.plural && o["metadata"]["name"] == name && (!kind.namespaced || o["metadata"]["namespace"] == namespace))
            .map(|(_, o)| o.clone())
            .ok_or_else(|| KubeError::NotFound(format!("{} \"{name}\" not found", kind.plural)))
    }

    fn logs(&self, namespace: &str, pod: &str, query: &LogQuery) -> Result<String, KubeError> {
        let previous = if query.previous { " previous" } else { "" };
        self.asked.lock().unwrap().push(format!("logs {namespace} {pod} {:?} {:?}{previous}", query.container, query.tail));
        let (_, text) = self.logs.iter().find(|(p, _)| p == pod).expect("a log for the pod");
        text.clone().map_err(KubeError::Cluster)
    }

    fn patch(&self, kind: &KubeKind, namespace: &str, name: &str, patch: &Value, dry_run: bool) -> Result<Value, KubeError> {
        let how = if dry_run { " dry" } else { "" };
        self.asked.lock().unwrap().push(format!("patch {} {namespace} {name} {patch}{how}", kind.plural));
        if let Some(why) = &self.refuses {
            return Err(KubeError::Cluster(why.clone()));
        }
        let mut object = self.get(kind, namespace, name)?;
        merge(&mut object, patch);
        object["metadata"]["generation"] = json!(object["metadata"]["generation"].as_i64().unwrap_or(0) + 1);
        Ok(object)
    }

    /// As the server applies: the manifest over what is there, or alone
    /// when nothing is. A kind with a `spec` has a generation; one
    /// without — a ConfigMap — has only its resource version.
    fn apply(&self, kind: &KubeKind, namespace: &str, name: &str, manifest: &Value, dry_run: bool) -> Result<Value, KubeError> {
        let how = if dry_run { " dry" } else { "" };
        self.asked.lock().unwrap().push(format!("apply {} {namespace} {name} {manifest}{how}", kind.plural));
        if let Some(why) = &self.refuses {
            return Err(KubeError::Cluster(why.clone()));
        }
        let mut object = self.get(kind, namespace, name).unwrap_or(json!({}));
        merge(&mut object, manifest);
        let bumped = |path: &str| json!(object.pointer(path).and_then(Value::as_i64).unwrap_or(0) + 1);
        if object.get("spec").is_some() {
            object["metadata"]["generation"] = bumped("/metadata/generation");
        } else {
            let next = object.pointer("/metadata/resourceVersion").and_then(Value::as_str).and_then(|v| v.parse::<i64>().ok()).unwrap_or(0) + 1;
            object["metadata"]["resourceVersion"] = json!(next.to_string());
        }
        Ok(object)
    }

    fn delete(&self, kind: &KubeKind, namespace: &str, name: &str, dry_run: bool) -> Result<(), KubeError> {
        let how = if dry_run { " dry" } else { "" };
        self.asked.lock().unwrap().push(format!("delete {} {namespace} {name}{how}", kind.plural));
        match &self.refuses {
            Some(why) => Err(KubeError::Cluster(why.clone())),
            None => self.get(kind, namespace, name).map(|_| ()),
        }
    }

    fn exec(&self, namespace: &str, pod: &str, container: Option<&str>, command: &[String]) -> Result<crate::domain::kube::ExecOutput, KubeError> {
        self.asked.lock().unwrap().push(format!("exec {namespace} {pod} {container:?} {}", command.join(" ")));
        let absent = crate::domain::kube::ExecOutput { code: 127, ..Default::default() };
        Ok(self.programs.iter().find(|(program, _)| program == &command[0]).map_or(absent, |(_, output)| output.clone()))
    }

    fn watch(&self, kind: &KubeKind, namespace: &str, name: &str, timeout: std::time::Duration, stop: &dyn Fn() -> bool, seen: &mut dyn FnMut(&Value) -> bool) -> Result<(), KubeError> {
        let first = self.get(kind, namespace, name)?;
        self.asked.lock().unwrap().push(format!("watch {} {namespace} {name} {}s", kind.plural, timeout.as_secs()));
        for object in std::iter::once(&first).chain(&self.later) {
            if seen(object) || stop() {
                break;
            }
            self.asked.lock().unwrap().push("shown".to_string());
        }
        Ok(())
    }
}

fn pinned(fake: &Fake) -> Option<PinnedCluster<'_>> {
    Some(PinnedCluster { api: fake, namespace: "orders", kubeconfig: "prod", context: "eks", writes: false, production: false, changes: None })
}

/// The same chat with "Allow changes" on.
fn writing<'a>(fake: &'a Fake, recorded: &'a Recorded) -> Option<PinnedCluster<'a>> {
    Some(PinnedCluster { writes: true, changes: Some(recorded), ..pinned(fake).unwrap() })
}

fn workload(plural: &str, replicas: i64) -> (String, Value) {
    let object = json!({"metadata": {"name": "api", "namespace": "orders", "generation": 4}, "spec": {"replicas": replicas}});
    (plural.to_string(), object)
}

fn scalable(replicas: i64) -> Fake {
    let (plural, object) = workload("deployments", replicas);
    Fake::with(&[("apps", "Deployment", "deployments"), ("apps", "DaemonSet", "daemonsets"), ("batch", "CronJob", "cronjobs")])
        .object(&plural, object)
}

fn scale(replicas: u32) -> KubeScaleArgs {
    KubeScaleArgs { kind: "deploy".into(), name: "api".into(), replicas: Some(replicas), ..Default::default() }
}

mod change;
mod read;

use change::*;
use read::*;
