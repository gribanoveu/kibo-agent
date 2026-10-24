//! The knowledge base on disk (`docs/40-knowledge-base.md`): a folder per
//! domain, a folder per repository in it, a file per document —
//! `<app dir>/knowledge/<domain>/<repo>/<slug>.md` — beside `domains.json`
//! and `repos.json`. A domain id has no `.`, so no domain's folder is either
//! file.
//!
//! What a document may say is `domain::knowledge`; this only keeps files.

use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, PoisonError};

use serde::{Deserialize, Serialize};
use serde::de::DeserializeOwned;
use thiserror::Error;

use crate::domain::knowledge::Domain;
use crate::infra::app_dir;
use crate::infra::repository_identity::{repository_id, repository_name};

const DOMAINS_FILE: &str = "domains.json";
const REPOS_FILE: &str = "repos.json";

/// One change at a time: `domains.json` and `repos.json` are read, changed
/// and written whole.
///
/// ponytail: one lock for the base, and per process — a second app instance
/// writing at the same moment can lose a line of either file. A lock file
/// when two instances on one base turn out to be real.
static WRITING: Mutex<()> = Mutex::new(());

#[derive(Debug, Error)]
pub enum KnowledgeStoreError {
    #[error("there is no app directory to keep it in: {0}")]
    NoDir(String),
    #[error("could not read {path}: {source}")]
    Read { path: PathBuf, source: std::io::Error },
    /// Not overwritten: what is in it would be lost.
    #[error("{path} does not parse ({reason}) — fix or remove it")]
    Corrupt { path: PathBuf, reason: String },
    #[error("could not write: {0}")]
    Write(String),
}

/// A repository the base has documents of.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RepoRecord {
    /// The folder's name in every domain, and the first part of document ids.
    pub name: String,
    /// Who it is: two clones share it, two repositories of one name do not.
    pub repository_id: String,
    /// Where it was open last.
    pub path: String,
    /// The commit its documents were written at last.
    #[serde(default)]
    pub commit: Option<String>,
}

/// One call's change. Validated before it gets here — this writes what it is given.
pub struct Change<'a> {
    pub repo: RepoRecord,
    pub new_domains: &'a [Domain],
    /// Domain, slug, the file's text. A document already kept in another
    /// domain moves.
    pub write: Vec<(String, String, String)>,
    /// Slugs of this repository's documents to remove.
    pub delete: Vec<String>,
}

pub struct KnowledgeStore {
    dir: PathBuf,
}

impl KnowledgeStore {
    pub fn open() -> Result<Self, KnowledgeStoreError> {
        let dir = app_dir::dir().map_err(|e| KnowledgeStoreError::NoDir(e.to_string()))?;
        Ok(Self { dir: dir.join("knowledge") })
    }

    /// The base itself — what its search indexes. Not created until a change is applied.
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    pub fn domains(&self) -> Result<Vec<Domain>, KnowledgeStoreError> {
        read_list(&self.dir.join(DOMAINS_FILE))
    }

    /// The record of the repository at `root`: the one kept, with where it is
    /// now, or a new one under a name nobody else has — `tax-api`, then
    /// `tax-api-2`. Not saved until a change is applied.
    pub fn repo(&self, root: &Path) -> Result<RepoRecord, KnowledgeStoreError> {
        let id = repository_id(root);
        let path = root.to_string_lossy().into_owned();
        let repos: Vec<RepoRecord> = read_list(&self.dir.join(REPOS_FILE))?;
        if let Some(kept) = repos.iter().find(|r| r.repository_id == id) {
            return Ok(RepoRecord { path, ..kept.clone() });
        }
        let base = repository_name(root);
        let taken = |name: &str| repos.iter().any(|r| r.name == name);
        let name = std::iter::once(base.clone())
            .chain((2..).map(|n| format!("{base}-{n}")))
            .find(|name| !taken(name))
            .unwrap_or(base);
        Ok(RepoRecord { name, repository_id: id, path, commit: None })
    }

    pub fn repos(&self) -> Result<Vec<RepoRecord>, KnowledgeStoreError> {
        read_list(&self.dir.join(REPOS_FILE))
    }

    /// Every file kept as a document, by its path from the base:
    /// `<domain>/<repo>/<slug>.md`. Anything else under it is left out.
    pub fn documents(&self) -> Result<Vec<String>, KnowledgeStoreError> {
        let mut found = Vec::new();
        for domain in children(&self.dir)? {
            for repo in children(&self.dir.join(&domain))? {
                for file in children(&self.dir.join(&domain).join(&repo))? {
                    let relative = format!("{domain}/{repo}/{file}");
                    if crate::domain::knowledge::place_of(&relative).is_some() {
                        found.push(relative);
                    }
                }
            }
        }
        found.sort();
        Ok(found)
    }

    /// Where the document `<repo>/<slug>` is kept, in whichever domain.
    pub fn find(&self, repo: &str, slug: &str) -> Result<Option<PathBuf>, KnowledgeStoreError> {
        let entries = match fs::read_dir(&self.dir) {
            Ok(entries) => entries,
            Err(e) if e.kind() == ErrorKind::NotFound => return Ok(None),
            Err(source) => return Err(KnowledgeStoreError::Read { path: self.dir.clone(), source }),
        };
        let file = format!("{slug}.md");
        Ok(entries
            .filter_map(Result::ok)
            .map(|entry| entry.path().join(repo).join(&file))
            .find(|path| path.is_file()))
    }

    pub fn apply(&self, change: Change) -> Result<(), KnowledgeStoreError> {
        let _writing = WRITING.lock().unwrap_or_else(PoisonError::into_inner);
        app_dir::ensure().map_err(KnowledgeStoreError::NoDir)?;

        if !change.new_domains.is_empty() {
            let path = self.dir.join(DOMAINS_FILE);
            let mut domains: Vec<Domain> = read_list(&path)?;
            for domain in change.new_domains {
                if !domains.iter().any(|d| d.id == domain.id) {
                    domains.push(domain.clone());
                }
            }
            write_json(&path, &domains)?;
        }

        let repo = &change.repo.name;
        for (domain, slug, text) in &change.write {
            let target = self.dir.join(domain).join(repo).join(format!("{slug}.md"));
            if let Some(kept) = self.find(repo, slug)?.filter(|kept| *kept != target) {
                remove(&kept)?;
            }
            app_dir::write_private(&target, text.as_bytes()).map_err(KnowledgeStoreError::Write)?;
        }
        for slug in &change.delete {
            if let Some(kept) = self.find(repo, slug)? {
                remove(&kept)?;
            }
        }

        let path = self.dir.join(REPOS_FILE);
        let mut repos: Vec<RepoRecord> = read_list(&path)?;
        repos.retain(|r| r.repository_id != change.repo.repository_id);
        repos.push(change.repo);
        write_json(&path, &repos)
    }
}

#[cfg(test)]
impl KnowledgeStore {
    pub fn at(dir: PathBuf) -> Self {
        Self { dir }
    }
}

/// The names in a folder; none when it is not one.
fn children(dir: &Path) -> Result<Vec<String>, KnowledgeStoreError> {
    match fs::read_dir(dir) {
        Ok(entries) => Ok(entries.filter_map(Result::ok).map(|e| e.file_name().to_string_lossy().into_owned()).collect()),
        Err(e) if matches!(e.kind(), ErrorKind::NotFound | ErrorKind::NotADirectory) => Ok(Vec::new()),
        Err(source) => Err(KnowledgeStoreError::Read { path: dir.to_path_buf(), source }),
    }
}

fn read_list<T: DeserializeOwned>(path: &Path) -> Result<Vec<T>, KnowledgeStoreError> {
    match fs::read_to_string(path) {
        Ok(text) => serde_json::from_str(&text)
            .map_err(|e| KnowledgeStoreError::Corrupt { path: path.to_path_buf(), reason: e.to_string() }),
        Err(e) if e.kind() == ErrorKind::NotFound => Ok(Vec::new()),
        Err(source) => Err(KnowledgeStoreError::Read { path: path.to_path_buf(), source }),
    }
}

fn write_json<T: Serialize>(path: &Path, value: &T) -> Result<(), KnowledgeStoreError> {
    let text = serde_json::to_string_pretty(value).map_err(|e| KnowledgeStoreError::Write(e.to_string()))?;
    app_dir::write_private(path, text.as_bytes()).map_err(KnowledgeStoreError::Write)
}

fn remove(path: &Path) -> Result<(), KnowledgeStoreError> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == ErrorKind::NotFound => Ok(()),
        Err(e) => Err(KnowledgeStoreError::Write(format!("could not remove {}: {e}", path.display()))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{temp_dir, with_app_dir};

    fn store(label: &str) -> KnowledgeStore {
        KnowledgeStore::at(temp_dir(label).join("knowledge"))
    }

    fn repo_at(label: &str, remote: &str) -> PathBuf {
        let dir = temp_dir(label);
        git2::Repository::init(&dir).unwrap().remote("origin", remote).unwrap();
        dir
    }

    fn domain(id: &str) -> Domain {
        Domain { id: id.into(), name: id.to_uppercase(), description: format!("about {id}") }
    }

    fn write(repo: RepoRecord, docs: &[(&str, &str, &str)]) -> Change<'static> {
        Change {
            repo,
            new_domains: &[],
            write: docs.iter().map(|(d, s, t)| (d.to_string(), s.to_string(), t.to_string())).collect(),
            delete: Vec::new(),
        }
    }

    #[test]
    fn a_repository_keeps_its_record_and_a_namesake_gets_a_number() {
        with_app_dir("knowledge-names", || {
            let store = store("knowledge-names-base");
            let first = repo_at("knowledge-names-a", "git@github.com:org/tax-api.git");
            let clone = repo_at("knowledge-names-b", "https://github.com/org/tax-api");
            let namesake = repo_at("knowledge-names-c", "git@gitlab.com:other/tax-api.git");

            let record = store.repo(&first).unwrap();
            assert_eq!((record.name.as_str(), record.commit.as_deref()), ("tax-api", None));
            store.apply(write(RepoRecord { commit: Some("c1".into()), ..record }, &[])).unwrap();

            let again = store.repo(&clone).unwrap();
            assert_eq!(again.name, "tax-api", "a clone is the same repository");
            assert_eq!(again.commit.as_deref(), Some("c1"));
            assert_eq!(again.path, clone.to_string_lossy());

            let other = store.repo(&namesake).unwrap();
            assert_eq!(other.name, "tax-api-2");
            store.apply(write(other, &[])).unwrap();
            assert_eq!(store.repo(&repo_at("knowledge-names-d", "git@x.org:y/tax-api.git")).unwrap().name, "tax-api-3");
        });
    }

    #[test]
    fn a_document_is_kept_under_its_domain_and_moves_with_it() {
        with_app_dir("knowledge-move", || {
            let store = store("knowledge-move-base");
            let repo = store.repo(&repo_at("knowledge-move-repo", "git@h:o/usn-api.git")).unwrap();
            let new = [domain("usn")];
            store
                .apply(Change { new_domains: &new, ..write(repo.clone(), &[("usn", "tariff", "one"), ("general", "service", "s")]) })
                .unwrap();
            let kept = store.find("usn-api", "tariff").unwrap().unwrap();
            assert_eq!(kept, store.dir.join("usn").join("usn-api").join("tariff.md"));
            assert_eq!(fs::read_to_string(&kept).unwrap(), "one");
            assert_eq!(store.domains().unwrap(), new.to_vec());

            store.apply(write(repo.clone(), &[("general", "tariff", "two")])).unwrap();
            assert!(!kept.exists(), "the old domain's copy is gone");
            assert_eq!(fs::read_to_string(store.find("usn-api", "tariff").unwrap().unwrap()).unwrap(), "two");

            store.apply(Change { delete: vec!["tariff".into(), "never-was".into()], ..write(repo, &[]) }).unwrap();
            assert_eq!(store.find("usn-api", "tariff").unwrap(), None);
            assert!(store.find("usn-api", "service").unwrap().is_some(), "only what was named is deleted");
        });
    }

    #[test]
    fn the_documents_are_the_files_where_documents_go() {
        with_app_dir("knowledge-list", || {
            let store = store("knowledge-list-base");
            assert_eq!(store.documents().unwrap(), Vec::<String>::new(), "no base yet");
            let repo = store.repo(&repo_at("knowledge-list-repo", "git@h:o/usn-api.git")).unwrap();
            store.apply(write(repo, &[("usn", "tariff", "t"), ("general", "service", "s")])).unwrap();
            fs::write(store.dir.join("usn").join("usn-api").join("notes.txt"), "x").unwrap();
            fs::write(store.dir.join("usn").join("stray.md"), "x").unwrap();
            assert_eq!(store.documents().unwrap(), ["general/usn-api/service.md", "usn/usn-api/tariff.md"]);
            assert_eq!(store.repos().unwrap().iter().map(|r| r.name.as_str()).collect::<Vec<_>>(), ["usn-api"]);
        });
    }

    #[test]
    fn a_domain_is_added_once() {
        with_app_dir("knowledge-domains", || {
            let store = store("knowledge-domains-base");
            assert_eq!(store.domains().unwrap(), Vec::<Domain>::new());
            let repo = store.repo(&repo_at("knowledge-domains-repo", "git@h:o/r.git")).unwrap();
            let first = [domain("usn"), domain("ausn")];
            store.apply(Change { new_domains: &first, ..write(repo.clone(), &[]) }).unwrap();
            let again = [Domain { name: "renamed".into(), ..domain("usn") }, domain("fns")];
            store.apply(Change { new_domains: &again, ..write(repo, &[]) }).unwrap();
            let ids: Vec<_> = store.domains().unwrap().into_iter().map(|d| (d.id, d.name)).collect();
            assert_eq!(ids, [("usn".into(), "USN".into()), ("ausn".into(), "AUSN".into()), ("fns".into(), "FNS".into())]);
        });
    }

    /// A file that does not parse is refused, not replaced by one that has
    /// only this call's line in it.
    #[test]
    fn a_broken_list_is_not_written_over() {
        with_app_dir("knowledge-corrupt", || {
            let store = store("knowledge-corrupt-base");
            fs::create_dir_all(&store.dir).unwrap();
            fs::write(store.dir.join(REPOS_FILE), "{ not json").unwrap();
            let repo = repo_at("knowledge-corrupt-repo", "git@h:o/r.git");
            assert!(matches!(store.repo(&repo), Err(KnowledgeStoreError::Corrupt { .. })));
            let record = RepoRecord { name: "r".into(), repository_id: "x".into(), path: String::new(), commit: None };
            assert!(matches!(store.apply(write(record, &[])), Err(KnowledgeStoreError::Corrupt { .. })));
            assert_eq!(fs::read_to_string(store.dir.join(REPOS_FILE)).unwrap(), "{ not json");
        });
    }
}
