//! What the model is told about each Kubernetes tool.

use super::*;

/// What every changing tool says of itself.
pub(crate) const CHANGE: &str = "Needs \"Allow changes\" on for the chat and the user's approval on a card; the object is backed up first. \
    Returns what it was and is, and a change id for each object changed.";

/// How every Kubernetes tool is told where it reads. The namespace is each
/// tool's own to say: "this chat's namespace" here read as a ban on the
/// `namespace` its schema offers.
pub(crate) const WHERE: &str = "Only the cluster this chat is pinned to.";

pub(crate) fn list_definition() -> LlmToolDefinition {
    LlmToolDefinition {
        name: "kubeList".to_string(),
        description: format!(
            "List objects of one kind as a table with the columns `kubectl get` shows (a Pod's readiness, status, restarts, node). \
             Any kind the cluster serves, CRDs too (VirtualService, Certificate). `fields` adds columns from each object — the way \
             to compare one field across many objects in one call instead of a kubeGet each: \
             fields: [\"metadata.annotations.networking\\\\.istio\\\\.io/exportTo\"]. A field an object lacks shows as —. \
             A Secret shows keys and sizes, never values. One kind a call: there is no `all`, and no list of kinds. {WHERE}"
        ),
        parameters: serde_json::json!({
            "type": "object",
            "properties": {
                "kind": { "type": "string", "description": "Kind, plural or short name: Deployment, pods, svc. Add the group when two serve one name: Gateway.networking.istio.io." },
                "namespace": { "type": "string", "description": "Default: the chat's namespace. `*` lists every namespace. Ignored for cluster-scoped kinds." },
                "labelSelector": { "type": "string", "description": "e.g. app=orders,tier!=cache" },
                "fieldSelector": { "type": "string", "description": "e.g. status.phase!=Running, spec.nodeName=node-1" },
                "fields": { "type": "array", "items": { "type": "string" }, "description": "Paths shown as extra columns: dots between keys, `\\\\.` for a dot inside a key (or ['key.with.dots']), [*] for every item, [0] for one." },
                "limit": { "type": "integer", "description": "Objects per page, default 200, at most 500." },
                "continue": { "type": "string", "description": "The token a previous page of this list ended with, as it was given — never one of your own. Omit for the first page." }
            },
            "required": ["kind"]
        }),
    }
}

pub(crate) fn get_definition() -> LlmToolDefinition {
    LlmToolDefinition {
        name: "kubeGet".to_string(),
        description: format!(
            "One object as YAML, without managedFields, the last-applied copy and server counters; annotations and labels \
             stay. Ask for `sections` to read only part — [\"spec\"] for the desired state, [\"status\"] for what the \
             cluster reports. A Secret shows keys and sizes, never values; env values and a ConfigMap's settings named \
             like credentials are <redacted> — a value you cannot read and must not send back. {WHERE}"
        ),
        parameters: serde_json::json!({
            "type": "object",
            "properties": {
                "kind": { "type": "string", "description": "Kind, plural or short name; CRDs too." },
                "name": { "type": "string" },
                "namespace": { "type": "string", "description": "Default: the chat's namespace." },
                "sections": { "type": "array", "items": { "type": "string" }, "description": "Top-level keys to keep: metadata, spec, status, data." }
            },
            "required": ["kind", "name"]
        }),
    }
}

pub(crate) fn events_definition() -> LlmToolDefinition {
    LlmToolDefinition {
        name: "kubeEvents".to_string(),
        description: format!(
            "Events oldest first, each distinct one once with its count summed. Of one object with kind and name, or without them of the whole namespace — a \
             Deployment's pods have their own events, under each Pod. The cluster keeps events about an hour. {WHERE}"
        ),
        parameters: serde_json::json!({
            "type": "object",
            "properties": {
                "namespace": { "type": "string", "description": "Default: the chat's namespace. `*` for every namespace." },
                "kind": { "type": "string", "description": "The object's kind, with name." },
                "name": { "type": "string", "description": "The object's name." },
                "warningsOnly": { "type": "boolean" }
            },
            "required": []
        }),
    }
}

pub(crate) fn logs_definition() -> LlmToolDefinition {
    LlmToolDefinition {
        name: "kubeLogs".to_string(),
        description: format!(
            "Container logs, the last lines. Of one pod, of every pod of an owner (kind and name: a Deployment, \
             StatefulSet, DaemonSet, Job, Service), or of a labelSelector's pods — several pods are merged by time, each \
             line marked with its pod, since which pod served a request is unknown. Up to {LOG_PODS} pods. Repeated lines \
             are said once with ×N. The container is the pod's default unless named; a mesh sidecar is \
             container: \"istio-proxy\". `previous` reads the container before its last restart — the crash. `grep` \
             searches further back than the tail. {WHERE}"
        ),
        parameters: serde_json::json!({
            "type": "object",
            "properties": {
                "pod": { "type": "string" },
                "kind": { "type": "string", "description": "An owner's kind, with name." },
                "name": { "type": "string", "description": "An owner's name." },
                "labelSelector": { "type": "string" },
                "namespace": { "type": "string", "description": "Default: the chat's namespace." },
                "container": { "type": "string" },
                "previous": { "type": "boolean" },
                "tail": { "type": "integer", "description": "Lines in all, across the pods; default 200, at most 2000." },
                "since": { "type": "string", "description": "Only this recent: 30s, 10m, 2h, 1d." },
                "grep": { "type": "string", "description": "A regex lines must match, case-insensitive: a path, a request id, `error|timeout`." }
            },
            "required": []
        }),
    }
}

pub(crate) fn top_definition() -> LlmToolDefinition {
    LlmToolDefinition {
        name: "kubeTop".to_string(),
        description: format!(
            "CPU and memory in use now, busiest first — from metrics-server, which keeps no history. Says so when the \
             cluster has none. {WHERE}"
        ),
        parameters: serde_json::json!({
            "type": "object",
            "properties": {
                "kind": { "type": "string", "enum": ["pods", "nodes"] },
                "namespace": { "type": "string", "description": "For pods. Default: the chat's namespace; `*` for every namespace." }
            },
            "required": ["kind"]
        }),
    }
}

pub(crate) fn field_history_definition() -> LlmToolDefinition {
    LlmToolDefinition {
        name: "kubeFieldHistory".to_string(),
        description: format!(
            "Who set each field of an object and when, from its managedFields: field → manager (kubectl, helm, argocd, \
             ansible, a controller), operation, time. For questions of origin — was this deployed by X, since when is this \
             annotation here. The time is the manager's last write of any field. {WHERE}"
        ),
        parameters: serde_json::json!({
            "type": "object",
            "properties": {
                "kind": { "type": "string" },
                "name": { "type": "string" },
                "namespace": { "type": "string", "description": "Default: the chat's namespace." },
                "paths": { "type": "array", "items": { "type": "string" }, "description": "Only fields under these, e.g. metadata.annotations, spec.template.spec.containers." }
            },
            "required": ["kind", "name"]
        }),
    }
}

pub(crate) fn diagnose_definition() -> LlmToolDefinition {
    LlmToolDefinition {
        name: "kubeDiagnose".to_string(),
        description: format!(
            "Why a workload or pod is unhealthy, in one call instead of five: the object's status and conditions, its \
             pods and what is wrong with each (waiting reasons, restarts, how the last run ended, why one is not \
             scheduled — requests, nodeSelector, unbound claims), the events of it, its ReplicaSets and pods, and the \
             log of the container in trouble — from before its last restart when it crashed. Facts, not a verdict: the \
             hypothesis is yours. For a Deployment, StatefulSet, DaemonSet, Job, ReplicaSet or Pod. {WHERE}"
        ),
        parameters: serde_json::json!({
            "type": "object",
            "properties": {
                "kind": { "type": "string", "description": "Deployment, StatefulSet, DaemonSet, Job, ReplicaSet or Pod." },
                "name": { "type": "string" },
                "namespace": { "type": "string", "description": "Default: the chat's namespace." }
            },
            "required": ["kind", "name"]
        }),
    }
}

pub(crate) fn wait_rollout_definition() -> LlmToolDefinition {
    LlmToolDefinition {
        name: "kubeWaitRollout".to_string(),
        description: format!(
            "Waits for a rollout of a Deployment, StatefulSet or DaemonSet in the chat's namespace and answers how it \
             ended: done, with the numbers; stuck — the controller gave up (ProgressDeadlineExceeded); or not done in \
             time. When it is not done the answer carries the diagnosis too: pods and what is wrong with each, events, \
             the log of the container in trouble. Done is the controller's count of available pods: a container that \
             crashes a moment after starting was counted — kubeDiagnose when in doubt. One call after a restart, a rollback, an apply or a scale — instead \
             of reading the object round after round. {WHERE}"
        ),
        parameters: serde_json::json!({
            "type": "object",
            "properties": {
                "kind": { "type": "string", "description": "Deployment, StatefulSet or DaemonSet." },
                "name": { "type": "string" },
                "timeoutSeconds": { "type": "integer", "description": "How long to wait. Default 120, at most 600." }
            },
            "required": ["kind", "name"]
        }),
    }
}

pub(crate) fn probe_definition() -> LlmToolDefinition {
    LlmToolDefinition {
        name: "kubeProbe".to_string(),
        description: format!(
            "Whether a pod reaches something, checked from inside it: a URL (`http://orders:8080/health` — does the \
             server answer, and with which status), a port (`db:5432` — is it open), or a name (`orders` — does it \
             resolve, and to what). Answers reached, name not resolved, refused, or timed out — and names the \
             namespace's NetworkPolicies when packets are dropped. It runs one fixed command of this app's in the \
             container (curl or wget, nc, getent or nslookup — whichever the image has) and reads no response body; \
             it runs nothing else there. The source is a pod or a workload (one of its running pods) in the chat's \
             namespace; the target may be anywhere. {WHERE}"
        ),
        parameters: serde_json::json!({
            "type": "object",
            "properties": {
                "kind": { "type": "string", "description": "What to probe from: Pod, Deployment, StatefulSet, DaemonSet, Job." },
                "name": { "type": "string" },
                "container": { "type": "string", "description": "Default: the pod's default container." },
                "target": { "type": "string", "description": "`http(s)://host[:port][/path]`, `host:port`, or `host`." }
            },
            "required": ["kind", "name", "target"]
        }),
    }
}

pub(crate) fn scale_definition() -> LlmToolDefinition {
    LlmToolDefinition {
        name: "kubeScale".to_string(),
        description: "Set the replicas of a Deployment, StatefulSet or ReplicaSet in the chat's own namespace — a change is \
            never made in another namespace or cluster. Works only when the user has turned on \"Allow changes\" for the chat; \
            otherwise it answers that the chat is read-only. The user approves it on a card showing the cluster and the \
            counts; the object is backed up first. Returns what it was and is, and a change id. To scale several, call it \
            for each in one round — they share one card. A DaemonSet, CronJob or Job has no replicas: it answers how those \
            are stopped. Scaling down a StatefulSet that deletes its pods' claims destroys their data: it is refused until \
            the user has accepted that (`confirmDataLoss`)."
            .to_string(),
        parameters: serde_json::json!({
            "type": "object",
            "properties": {
                "kind": { "type": "string", "description": "Deployment, StatefulSet or ReplicaSet." },
                "name": { "type": "string" },
                "replicas": { "type": "integer", "description": "The number to scale to; 0 stops it." },
                "confirmDataLoss": { "type": "boolean", "description": "Only after the tool refused because data would be destroyed, and the user then said in this conversation that losing it is acceptable." }
            },
            "required": ["kind", "name", "replicas"]
        }),
    }
}

pub(crate) fn suspend_definition() -> LlmToolDefinition {
    LlmToolDefinition {
        name: "kubeSuspend".to_string(),
        description: format!(
            "Suspend a CronJob or a Job, or let it run again, in the chat's own namespace: spec.suspend. A suspended CronJob \
             starts no new Jobs — the ones already running finish; a suspended Job's pods are removed until it is resumed. \
             This is how those are stopped: they have no replicas. {CHANGE}"
        ),
        parameters: serde_json::json!({
            "type": "object",
            "properties": {
                "kind": { "type": "string", "description": "CronJob or Job." },
                "name": { "type": "string" },
                "suspend": { "type": "boolean", "description": "true to stop it, false to let it run again." }
            },
            "required": ["kind", "name", "suspend"]
        }),
    }
}

pub(crate) fn rollout_restart_definition() -> LlmToolDefinition {
    LlmToolDefinition {
        name: "kubeRolloutRestart".to_string(),
        description: format!(
            "Restart a Deployment, StatefulSet or DaemonSet in the chat's own namespace, as `kubectl rollout restart` does: \
             every pod is replaced, a few at a time. It CANNOT be undone — the card tells the user so. Use it when a restart \
             is what is wanted (a config or secret to pick up, a stuck process), not as a guess at a fix: a crash loop is \
             not cured by restarting. {CHANGE}"
        ),
        parameters: serde_json::json!({
            "type": "object",
            "properties": {
                "kind": { "type": "string", "description": "Deployment, StatefulSet or DaemonSet." },
                "name": { "type": "string" }
            },
            "required": ["kind", "name"]
        }),
    }
}

pub(crate) fn rollout_undo_definition() -> LlmToolDefinition {
    LlmToolDefinition {
        name: "kubeRolloutUndo".to_string(),
        description: format!(
            "Roll a Deployment in the chat's own namespace back to an earlier revision, as `kubectl rollout undo` does: its \
             pod template becomes that revision's — the one before the current unless toRevision names another. The \
             revisions kept are the Deployment's ReplicaSets (kubeList ReplicaSet shows them). The card shows the revisions \
             and the image on either side. A Deployment managed by Argo CD, Flux or Helm may be put back by them — say so. {CHANGE}"
        ),
        parameters: serde_json::json!({
            "type": "object",
            "properties": {
                "kind": { "type": "string", "description": "Deployment." },
                "name": { "type": "string" },
                "toRevision": { "type": "integer", "description": "The revision to go back to; omit for the previous one." }
            },
            "required": ["kind", "name"]
        }),
    }
}

pub(crate) fn apply_definition() -> LlmToolDefinition {
    LlmToolDefinition {
        name: "kubeApply".to_string(),
        description: format!(
            "Create or update objects in the chat's own namespace from a YAML manifest, as `kubectl apply` does — up to \
             {APPLY_MOST} objects, `---` between them, on one card with each object's diff. Send each object WHOLE, as it \
             should be: what the manifest leaves out of an object this tool applied before is removed. To change an \
             existing object, read it with kubeGet first and send it back with your change. metadata.namespace may be \
             omitted; another namespace, or a cluster-wide kind (Namespace, ClusterRole, a CRD), is refused. It takes \
             over fields other tools set — an object managed by Argo CD, Flux or Helm will be put back by them; say so \
             rather than apply. An object the manifest would not change is skipped. Each object changed answers with its \
             own change id. {CHANGE}"
        ),
        parameters: serde_json::json!({
            "type": "object",
            "properties": {
                "manifest": { "type": "string", "description": "YAML: one or more whole objects, `---` between them." }
            },
            "required": ["manifest"]
        }),
    }
}

pub(crate) fn delete_definition() -> LlmToolDefinition {
    LlmToolDefinition {
        name: "kubeDelete".to_string(),
        description: format!(
            "Delete one object in the chat's own namespace. It is backed up whole, and kubeUndo creates it again from \
             that — the object, not what it held: a deleted Deployment's pods are new ones, and a PersistentVolumeClaim's \
             data is gone unless its volume is kept — such a delete is refused until the user has accepted the loss (`confirmDataLoss`). What an owner manages — a pod of a Deployment \
             — comes back by itself: delete the owner, or scale it. A Namespace and other cluster-wide kinds are refused. {CHANGE}"
        ),
        parameters: serde_json::json!({
            "type": "object",
            "properties": {
                "kind": { "type": "string" },
                "name": { "type": "string" },
                "confirmDataLoss": { "type": "boolean", "description": "Only after the tool refused because data would be destroyed, and the user then said in this conversation that losing it is acceptable." }
            },
            "required": ["kind", "name"]
        }),
    }
}

pub(crate) fn undo_definition() -> LlmToolDefinition {
    LlmToolDefinition {
        name: "kubeUndo".to_string(),
        description: "Put a change back by its change id — the id a changing tool answered with, or one the user names. The \
            app restores from the backup it took before the change, so it works after the conversation has forgotten the \
            details; use it rather than changing the object back yourself. Needs \"Allow changes\" on for the chat and the \
            user's approval on a card, like any change, and is one itself: it answers with a new change id, and undoing \
            that makes the change again. It refuses when the object has been changed since — by a later change of yours \
            (undo that first) or by someone else (then the user decides what to do) — when the change was made \
            in another cluster or namespace, and when its backup is older than 30 days."
            .to_string(),
        parameters: serde_json::json!({
            "type": "object",
            "properties": {
                "changeId": { "type": "string", "description": "The change to undo, e.g. kc-1a2b3c4d." }
            },
            "required": ["changeId"]
        }),
    }
}
