//! Where the app's changes to clusters are kept (`docs/21-kubernetes-mode.md`,
//! decisions 6 and 9): the object as it was, sealed, one file per change under
//! `<app dir>/kube-backups/`, and one line per change in `kube-audit.jsonl`.
//!
//! Sealed because a Secret's backup holds the secrets themselves — and all of
//! them are, so that nothing has to decide which are. The audit holds no
//! object, only where and what; unlike the tool-call log it is not a setting.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, SystemTime};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use zeroize::Zeroizing;

use crate::domain::kube::{KubeChange, KubeChanges};
use crate::infra::secret_store::{self, SecretPurpose};
use crate::infra::{app_dir, master_key};

const BACKUPS: &str = "kube-backups";
const AUDIT: &str = "kube-audit.jsonl";
/// How long a backup is kept, and so how long a change can be undone — the
/// file history's term.
pub const RETENTION_DAYS: u64 = crate::infra::file_history::RETENTION_DAYS;
const RETENTION: Duration = Duration::from_secs(RETENTION_DAYS * 24 * 60 * 60);

/// What one backup file holds, before sealing.
#[derive(Serialize, Deserialize)]
struct Backup {
    change: KubeChange,
    object: Value,
}

/// The app directory's store; the only implementation outside tests.
pub struct ChangeStore;

fn backup_path(id: &str) -> Result<PathBuf, String> {
    // An id names a file: the app's own are `kc-` and hex, and anything else
    // is not one of them.
    if id.is_empty() || !id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
        return Err(format!("not a change id: {id}"));
    }
    Ok(app_dir::dir()?.join(BACKUPS).join(format!("{id}.bin")))
}

impl KubeChanges for ChangeStore {
    fn backup(&self, change: &KubeChange, object: &Value) -> Result<(), String> {
        let backup = Backup { change: change.clone(), object: object.clone() };
        let plain = Zeroizing::new(serde_json::to_vec(&backup).map_err(|e| format!("could not serialize the backup: {e}"))?);
        let key = master_key::resolve()?;
        let blob = secret_store::seal(&key, SecretPurpose::KubeBackup, &plain).map_err(|e| e.to_string())?;
        let path = backup_path(&change.id)?;
        // Once a run, as the file history clears its copies.
        static PRUNED: AtomicBool = AtomicBool::new(false);
        if let (false, Some(dir)) = (PRUNED.swap(true, Ordering::Relaxed), path.parent()) {
            prune(dir);
        }
        app_dir::write_private(&path, &blob)
    }

    fn audit(&self, change: &KubeChange) -> Result<(), String> {
        let path = app_dir::ensure()?.join(AUDIT);
        let line = serde_json::to_string(change).map_err(|e| e.to_string())?;
        let mut options = std::fs::OpenOptions::new();
        options.create(true).append(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(&path).map_err(|e| format!("could not open {}: {e}", path.display()))?;
        writeln!(file, "{line}").map_err(|e| format!("could not write {}: {e}", path.display()))
    }

    fn load(&self, id: &str) -> Result<(KubeChange, Value), String> {
        load_backup(id)
    }

    fn history(&self) -> Result<Vec<KubeChange>, String> {
        audit_log()
    }
}

/// Whether the change's backup is still kept: past [`RETENTION_DAYS`] it is not.
pub fn has_backup(id: &str) -> bool {
    backup_path(id).is_ok_and(|path| path.is_file())
}

fn prune(dir: &Path) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    let now = SystemTime::now();
    for entry in entries.flatten() {
        let old = entry.metadata().and_then(|m| m.modified()).is_ok_and(|at| now.duration_since(at).is_ok_and(|age| age > RETENTION));
        if old {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

/// The change and the object as it was before it, by the change's id.
pub fn load_backup(id: &str) -> Result<(KubeChange, Value), String> {
    let path = backup_path(id)?;
    let blob = std::fs::read(&path).map_err(|_| format!("there is no backup of the change {id}"))?;
    let key = master_key::resolve()?;
    let plain = Zeroizing::new(secret_store::open(&key, SecretPurpose::KubeBackup, &blob).map_err(|e| e.to_string())?);
    let backup: Backup = serde_json::from_slice(&plain).map_err(|e| format!("the backup of {id} is damaged: {e}"))?;
    Ok((backup.change, backup.object))
}

/// Every change on record, oldest first. A line that does not parse is
/// skipped: one torn write must not hide the rest.
pub fn audit_log() -> Result<Vec<KubeChange>, String> {
    let path = app_dir::dir()?.join(AUDIT);
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(format!("could not read {}: {e}", path.display())),
    };
    Ok(text.lines().filter_map(|line| serde_json::from_str(line).ok()).collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::with_app_dir;
    use serde_json::json;

    fn change(id: &str) -> KubeChange {
        KubeChange {
            id: id.to_string(),
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
            generation_after: None,
            error: None,
            undoes: None,
        }
    }

    /// A Secret's backup is the Secret: on disk it is sealed, and it opens
    /// only as a backup.
    #[test]
    fn a_backup_is_sealed_on_disk_and_comes_back_whole() {
        with_app_dir("kube-changes-backup", || {
            let secret = json!({"kind": "Secret", "data": {"password": "aHVudGVyMg=="}});
            ChangeStore.backup(&change("kc-1"), &secret).unwrap();
            let raw = std::fs::read(backup_path("kc-1").unwrap()).unwrap();
            assert!(!String::from_utf8_lossy(&raw).contains("aHVudGVyMg"), "the backup is readable on disk");
            let key = master_key::resolve().unwrap();
            assert!(secret_store::open(&key, SecretPurpose::ProviderApiKey, &raw).is_err(), "it opens as something else");
            assert_eq!(load_backup("kc-1").unwrap(), (change("kc-1"), secret));
            assert!(load_backup("kc-2").unwrap_err().contains("no backup"));
        });
    }

    /// A change can be undone for thirty days: then its backup goes.
    #[test]
    fn a_backup_past_retention_is_cleared_and_a_fresh_one_stays() {
        with_app_dir("kube-changes-prune", || {
            ChangeStore.backup(&change("kc-old"), &json!({})).unwrap();
            ChangeStore.backup(&change("kc-new"), &json!({})).unwrap();
            let old = backup_path("kc-old").unwrap();
            let past = SystemTime::now() - RETENTION - Duration::from_secs(60);
            std::fs::File::options().write(true).open(&old).unwrap().set_modified(past).unwrap();
            prune(old.parent().unwrap());
            assert!(!has_backup("kc-old") && has_backup("kc-new"));
            assert!(!has_backup("../settings"));
            assert_eq!(ChangeStore.load("kc-new").unwrap().0, change("kc-new"));
            assert_eq!(ChangeStore.history().unwrap(), []);
        });
    }

    /// An id is a file name, and nothing the app did not make is one.
    #[test]
    fn an_id_that_is_a_path_is_refused() {
        with_app_dir("kube-changes-id", || {
            for id in ["../settings", "a/b", "", "kc 1"] {
                assert!(load_backup(id).unwrap_err().contains("not a change id"), "{id}");
                assert!(ChangeStore.backup(&change(id), &json!({})).is_err(), "{id}");
            }
        });
    }

    #[test]
    fn the_audit_keeps_every_change_in_order_past_a_torn_line() {
        with_app_dir("kube-changes-audit", || {
            assert_eq!(audit_log().unwrap(), []);
            ChangeStore.audit(&change("kc-1")).unwrap();
            let path = app_dir::dir().unwrap().join(AUDIT);
            std::fs::OpenOptions::new().append(true).open(&path).unwrap().write_all(b"{\"id\": \"torn\n").unwrap();
            let failed = KubeChange { error: Some("conflict".into()), ..change("kc-2") };
            ChangeStore.audit(&failed).unwrap();
            assert_eq!(audit_log().unwrap(), [change("kc-1"), failed]);
        });
    }
}
