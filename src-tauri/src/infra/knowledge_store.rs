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

use crate::domain::knowledge::{is_domain_id, is_repo_name, place_of, Domain, GENERAL_DOMAIN};
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
    #[error("there is no domain `{0}` in the base")]
    NoDomain(String),
    /// Another repository took the name between the call's reading and its
    /// writing: the next call is given another.
    #[error("`{0}` was just given to another repository — send the call again")]
    NameTaken(String),
    #[error("`{0}` is not a repository's name in the base")]
    NotARepo(String),
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
                    if place_of(&relative).is_some() {
                        found.push(relative);
                    }
                }
            }
        }
        found.sort();
        Ok(found)
    }

    /// Where the document `<repo>/<slug>` is kept, in whichever domain — the
    /// first domain by name, when a copy by hand left it in two.
    pub fn find(&self, repo: &str, slug: &str) -> Result<Option<PathBuf>, KnowledgeStoreError> {
        Ok(self.copies(repo, slug)?.into_iter().next())
    }

    /// Every file of the document `<repo>/<slug>`, in the folders
    /// [`Self::documents`] reads, by domain.
    fn copies(&self, repo: &str, slug: &str) -> Result<Vec<PathBuf>, KnowledgeStoreError> {
        let mut domains: Vec<String> = children(&self.dir)?.into_iter().filter(|d| is_domain_id(d)).collect();
        domains.sort();
        let file = format!("{slug}.md");
        Ok(domains.into_iter().map(|d| self.dir.join(d).join(repo).join(&file)).filter(|p| p.is_file()).collect())
    }

    pub fn apply(&self, change: Change) -> Result<(), KnowledgeStoreError> {
        let _writing = WRITING.lock().unwrap_or_else(PoisonError::into_inner);
        app_dir::ensure().map_err(KnowledgeStoreError::NoDir)?;

        // What the call was checked against may have changed while the model
        // looked at it: the name `repo` gave, a domain the page removed.
        let repos_path = self.dir.join(REPOS_FILE);
        let mut repos: Vec<RepoRecord> = read_list(&repos_path)?;
        let repo = &change.repo.name;
        if repos.iter().any(|r| r.name == *repo && r.repository_id != change.repo.repository_id) {
            return Err(KnowledgeStoreError::NameTaken(repo.clone()));
        }
        let domains_path = self.dir.join(DOMAINS_FILE);
        let mut domains: Vec<Domain> = read_list(&domains_path)?;
        let before = domains.len();
        for domain in change.new_domains {
            if !domains.iter().any(|d| d.id == domain.id) {
                domains.push(domain.clone());
            }
        }
        if let Some((missing, _, _)) = change.write.iter().find(|(domain, _, _)| !known(&domains, domain)) {
            return Err(KnowledgeStoreError::NoDomain(missing.clone()));
        }
        if domains.len() != before {
            write_json(&domains_path, &domains)?;
        }

        for (domain, slug, text) in &change.write {
            // Written before the other copies go: a write that fails leaves the old one.
            let target = self.dir.join(domain).join(repo).join(format!("{slug}.md"));
            app_dir::write_private(&target, text.as_bytes()).map_err(KnowledgeStoreError::Write)?;
            for kept in self.copies(repo, slug)?.into_iter().filter(|kept| *kept != target) {
                remove(&kept)?;
            }
        }
        for slug in &change.delete {
            for kept in self.copies(repo, slug)? {
                remove(&kept)?;
            }
        }

        repos.retain(|r| r.repository_id != change.repo.repository_id);
        repos.push(change.repo);
        write_json(&repos_path, &repos)
    }

    /// Kept files filed under other domains: each `(file, domain, text)` is
    /// written as `<domain>/<its repo>/<its name>` and the old file removed.
    /// Every domain is checked before the first file moves.
    pub fn refile(&self, moves: &[(PathBuf, String, String)]) -> Result<(), KnowledgeStoreError> {
        let _writing = WRITING.lock().unwrap_or_else(PoisonError::into_inner);
        self.refile_held(moves)
    }

    /// [`Self::refile`], by a caller holding `WRITING`.
    fn refile_held(&self, moves: &[(PathBuf, String, String)]) -> Result<(), KnowledgeStoreError> {
        let domains: Vec<Domain> = read_list(&self.dir.join(DOMAINS_FILE))?;
        if let Some((_, missing, _)) = moves.iter().find(|(_, domain, _)| !known(&domains, domain)) {
            return Err(KnowledgeStoreError::NoDomain(missing.clone()));
        }
        for (kept, domain, text) in moves {
            let (Some(file), Some(repo)) = (kept.file_name(), kept.parent().and_then(Path::file_name)) else {
                continue;
            };
            let target = self.dir.join(domain).join(repo).join(file);
            app_dir::write_private(&target, text.as_bytes()).map_err(KnowledgeStoreError::Write)?;
            if target != *kept {
                remove(kept)?;
            }
        }
        Ok(())
    }
}

/// Whether documents may be filed under `id`: one of `domains`, or `general`.
fn known(domains: &[Domain], id: &str) -> bool {
    id == GENERAL_DOMAIN || domains.iter().any(|d| d.id == id)
}

/// What the Knowledge page changes by hand (`docs/40-knowledge-base.md`, decision 9).
impl KnowledgeStore {
    /// A domain's name and description, as the user put them.
    pub fn edit_domain(&self, domain: &Domain) -> Result<(), KnowledgeStoreError> {
        let _writing = WRITING.lock().unwrap_or_else(PoisonError::into_inner);
        let path = self.dir.join(DOMAINS_FILE);
        let mut domains: Vec<Domain> = read_list(&path)?;
        let kept = domains.iter_mut().find(|d| d.id == domain.id).ok_or_else(|| KnowledgeStoreError::NoDomain(domain.id.clone()))?;
        *kept = domain.clone();
        write_json(&path, &domains)
    }

    /// Every document filed under `from` moved into `into`, and `from` off the
    /// list — under one lock: a document written between the two would stay in
    /// the folder of a domain that is gone. `rewrite` gives a file's text as
    /// filed under `into`, from its path in the base and on disk; every file is
    /// rewritten before the first one moves, so one it refuses leaves the merge
    /// undone, not half done.
    pub fn merge_domains<E: From<KnowledgeStoreError>>(
        &self,
        from: &str,
        into: &str,
        rewrite: impl Fn(&str, &Path) -> Result<String, E>,
    ) -> Result<(), E> {
        let _writing = WRITING.lock().unwrap_or_else(PoisonError::into_inner);
        let domains: Vec<Domain> = read_list(&self.dir.join(DOMAINS_FILE))?;
        if let Some(missing) = [from, into].into_iter().find(|id| !known(&domains, id)) {
            return Err(KnowledgeStoreError::NoDomain(missing.to_string()).into());
        }
        if from == into {
            return Ok(());
        }
        let mut moves = Vec::new();
        for relative in self.documents()? {
            if place_of(&relative).is_some_and(|(domain, _)| domain == from) {
                let path = self.dir.join(&relative);
                let text = rewrite(&relative, &path)?;
                moves.push((path, into.to_string(), text));
            }
        }
        self.refile_held(&moves)?;
        Ok(self.drop_domain(from)?)
    }

    /// A domain off the list, its folder with it once nothing is left in it.
    /// The caller holds `WRITING`.
    fn drop_domain(&self, id: &str) -> Result<(), KnowledgeStoreError> {
        let path = self.dir.join(DOMAINS_FILE);
        let mut domains: Vec<Domain> = read_list(&path)?;
        domains.retain(|d| d.id != id);
        write_json(&path, &domains)?;
        remove_empty(&self.dir.join(id))
    }

    /// Every document of the repository `name`, in every domain, and its record.
    /// Only domain folders are looked in, as [`Self::documents`] reads them: a
    /// folder of the user's own beside them is not the base's to remove.
    pub fn remove_repo(&self, name: &str) -> Result<(), KnowledgeStoreError> {
        // A folder under every domain is removed whole: `..` or `` would be the base or a domain.
        if !is_repo_name(name) {
            return Err(KnowledgeStoreError::NotARepo(name.to_string()));
        }
        let _writing = WRITING.lock().unwrap_or_else(PoisonError::into_inner);
        // Read before anything goes: a list that does not parse stops the removal, not its last step.
        let path = self.dir.join(REPOS_FILE);
        let mut repos: Vec<RepoRecord> = read_list(&path)?;
        for domain in children(&self.dir)?.into_iter().filter(|d| is_domain_id(d)) {
            let folder = self.dir.join(&domain).join(name);
            if folder.is_dir() {
                fs::remove_dir_all(&folder).map_err(|e| KnowledgeStoreError::Write(format!("could not remove {}: {e}", folder.display())))?;
            }
        }
        repos.retain(|r| r.name != name);
        write_json(&path, &repos)
    }
}

/// Removes the folders under `dir` that hold no file, then `dir` itself if it is empty.
fn remove_empty(dir: &Path) -> Result<(), KnowledgeStoreError> {
    for child in children(dir)? {
        let _ = fs::remove_dir(dir.join(child));
    }
    match fs::remove_dir(dir) {
        Ok(()) => Ok(()),
        Err(e) if matches!(e.kind(), ErrorKind::NotFound | ErrorKind::DirectoryNotEmpty) => Ok(()),
        Err(e) => Err(KnowledgeStoreError::Write(format!("could not remove {}: {e}", dir.display()))),
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
            let new = [domain("usn")];
            store.apply(Change { new_domains: &new, ..write(repo, &[("usn", "tariff", "t"), ("general", "service", "s")]) }).unwrap();
            fs::write(store.dir.join("usn").join("usn-api").join("notes.txt"), "x").unwrap();
            fs::write(store.dir.join("usn").join("stray.md"), "x").unwrap();
            assert_eq!(store.documents().unwrap(), ["general/usn-api/service.md", "usn/usn-api/tariff.md"]);
            assert_eq!(store.repos().unwrap().iter().map(|r| r.name.as_str()).collect::<Vec<_>>(), ["usn-api"]);
        });
    }

    #[test]
    fn the_page_edits_a_domain_removes_one_and_removes_a_repository() {
        with_app_dir("knowledge-page-store", || {
            let store = store("knowledge-page-store-base");
            let repo = store.repo(&repo_at("knowledge-page-store-repo", "git@h:o/usn-api.git")).unwrap();
            let other = RepoRecord { name: "ledger".into(), repository_id: "other".into(), ..repo.clone() };
            let new = [domain("usn"), domain("ausn")];
            store.apply(Change { new_domains: &new, ..write(repo, &[("usn", "tariff", "t"), ("ausn", "rules", "r")]) }).unwrap();
            store.apply(write(other, &[("usn", "listener", "l")])).unwrap();

            let renamed = Domain { id: "usn".into(), name: "Бухгалтерия УСН".into(), description: "упрощёнка".into() };
            store.edit_domain(&renamed).unwrap();
            assert_eq!(store.domains().unwrap(), [renamed.clone(), domain("ausn")]);
            assert!(matches!(store.edit_domain(&domain("fns")), Err(KnowledgeStoreError::NoDomain(id)) if id == "fns"));

            store.remove_repo("usn-api").unwrap();
            assert_eq!(store.documents().unwrap(), ["usn/ledger/listener.md"]);
            assert_eq!(store.repos().unwrap().iter().map(|r| r.name.as_str()).collect::<Vec<_>>(), ["ledger"]);

            fs::create_dir_all(store.dir.join("ausn").join("left-empty")).unwrap();
            store.drop_domain("ausn").unwrap();
            assert_eq!(store.domains().unwrap(), [renamed]);
            assert!(!store.dir.join("ausn").exists(), "its folder, empty now, is gone");
            store.drop_domain("usn").unwrap();
            assert!(store.dir.join("usn").join("ledger").join("listener.md").exists(), "a folder with documents stays");
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

    /// What a call was checked against is checked again when it is written:
    /// a name given to another repository meanwhile, a domain removed.
    #[test]
    fn a_change_is_checked_again_when_it_is_written() {
        with_app_dir("knowledge-race", || {
            let store = store("knowledge-race-base");
            let first = store.repo(&repo_at("knowledge-race-a", "git@h:o/tax-api.git")).unwrap();
            let second = store.repo(&repo_at("knowledge-race-b", "git@other:x/tax-api.git")).unwrap();
            assert_eq!((first.name.as_str(), second.name.as_str()), ("tax-api", "tax-api"), "both asked before either wrote");
            store.apply(write(first, &[("general", "a", "a")])).unwrap();
            assert!(matches!(store.apply(write(second.clone(), &[("general", "b", "b")])), Err(KnowledgeStoreError::NameTaken(n)) if n == "tax-api"));
            assert_eq!(store.documents().unwrap(), ["general/tax-api/a.md"], "nothing of the second was written");
            assert_eq!(store.repo(&PathBuf::from(&second.path)).unwrap().name, "tax-api-2", "its next call gets a name of its own");

            let usn = [domain("usn")];
            let repo = store.repo(&repo_at("knowledge-race-c", "git@h:o/ledger.git")).unwrap();
            store.apply(Change { new_domains: &usn, ..write(repo.clone(), &[]) }).unwrap();
            store.drop_domain("usn").unwrap();
            assert!(matches!(store.apply(write(repo, &[("usn", "c", "c")])), Err(KnowledgeStoreError::NoDomain(d)) if d == "usn"));
            assert!(!store.dir.join("usn").exists());

            let kept = store.dir.join("general/ledger/d.md");
            store.apply(write(store.repo(&repo_at("knowledge-race-d", "git@h:o/ledger.git")).unwrap(), &[("general", "d", "d")])).unwrap();
            assert!(matches!(store.refile(&[(kept.clone(), "usn".into(), "d".into())]), Err(KnowledgeStoreError::NoDomain(d)) if d == "usn"));
            assert!(kept.exists() && !store.dir.join("usn").exists(), "nothing moved into a domain that is gone");
        });
    }

    /// Only domain folders hold documents; a copy left in a second domain is
    /// found first by domain name, and gone once the document is written.
    #[test]
    fn a_document_is_found_where_documents_go_and_kept_once() {
        with_app_dir("knowledge-copies", || {
            let store = store("knowledge-copies-base");
            let repo = store.repo(&repo_at("knowledge-copies-repo", "git@h:o/r.git")).unwrap();
            let new = [domain("ausn"), domain("usn")];
            store.apply(Change { new_domains: &new, ..write(repo.clone(), &[("usn", "a", "usn")]) }).unwrap();
            for folder in ["ausn", "Old Notes"] {
                fs::create_dir_all(store.dir.join(folder).join("r")).unwrap();
                fs::write(store.dir.join(folder).join("r/a.md"), folder).unwrap();
            }
            assert_eq!(store.find("r", "a").unwrap(), Some(store.dir.join("ausn/r/a.md")));
            fs::remove_file(store.dir.join("ausn/r/a.md")).unwrap();
            assert_eq!(store.find("r", "a").unwrap(), Some(store.dir.join("usn/r/a.md")), "not the folder that is no domain");

            fs::write(store.dir.join("ausn/r/a.md"), "copy").unwrap();
            store.apply(write(repo.clone(), &[("general", "a", "new")])).unwrap();
            assert_eq!(store.documents().unwrap(), ["general/r/a.md"], "every copy gone");
            assert!(matches!(store.remove_repo(".."), Err(KnowledgeStoreError::NotARepo(_))));
            assert!(store.dir.join("general/r/a.md").exists());
        });
    }

    /// A document moving to another domain is written before its old copy
    /// goes: a write that fails leaves it where it was.
    #[test]
    fn a_move_that_cannot_be_written_keeps_the_old_copy() {
        with_app_dir("knowledge-failed-move", || {
            let store = store("knowledge-failed-move-base");
            let repo = store.repo(&repo_at("knowledge-failed-move-repo", "git@h:o/r.git")).unwrap();
            let new = [domain("usn")];
            store.apply(Change { new_domains: &new, ..write(repo.clone(), &[("usn", "a", "one")]) }).unwrap();
            // A folder where the new file would go: nothing can be renamed onto it.
            fs::create_dir_all(store.dir.join("general/r/a.md")).unwrap();
            assert!(matches!(store.apply(write(repo, &[("general", "a", "two")])), Err(KnowledgeStoreError::Write(_))));
            assert_eq!(fs::read_to_string(store.dir.join("usn/r/a.md")).unwrap(), "one");
        });
    }

    /// A repository's documents go from the domains' folders only, and
    /// nothing goes while the list of repositories cannot be read.
    #[test]
    fn removing_a_repository_spares_other_folders_and_waits_for_a_readable_list() {
        with_app_dir("knowledge-remove-repo", || {
            let store = store("knowledge-remove-repo-base");
            let repo = store.repo(&repo_at("knowledge-remove-repo-repo", "git@h:o/r.git")).unwrap();
            store.apply(write(repo, &[("general", "a", "a")])).unwrap();
            fs::create_dir_all(store.dir.join("Old Notes/r")).unwrap();
            fs::write(store.dir.join("Old Notes/r/mine.md"), "mine").unwrap();

            let kept = fs::read_to_string(store.dir.join(REPOS_FILE)).unwrap();
            fs::write(store.dir.join(REPOS_FILE), "{ not json").unwrap();
            assert!(matches!(store.remove_repo("r"), Err(KnowledgeStoreError::Corrupt { .. })));
            assert!(store.dir.join("general/r/a.md").exists(), "nothing removed before the list was read");

            fs::write(store.dir.join(REPOS_FILE), kept).unwrap();
            store.remove_repo("r").unwrap();
            assert!(!store.dir.join("general/r").exists());
            assert!(store.repos().unwrap().is_empty());
            assert!(store.dir.join("Old Notes/r/mine.md").exists(), "not a domain's folder");
        });
    }

    /// A merge holds the lock from listing the documents to removing the
    /// domain: a write meanwhile waits, then finds the domain gone — no
    /// document is left in the folder of a domain that is off the list.
    #[test]
    fn a_write_during_a_merge_lands_after_it() {
        with_app_dir("knowledge-merge-race", || {
            let store = store("knowledge-merge-race-base");
            let repo = store.repo(&repo_at("knowledge-merge-race-repo", "git@h:o/r.git")).unwrap();
            let new = [domain("usn"), domain("ausn")];
            store.apply(Change { new_domains: &new, ..write(repo.clone(), &[("ausn", "a", "a")]) }).unwrap();

            let (started, merging) = std::sync::mpsc::channel();
            std::thread::scope(|s| {
                let (store, repo) = (&store, repo.clone());
                let writer = s.spawn(move || {
                    merging.recv().unwrap();
                    store.apply(write(repo, &[("ausn", "b", "b")]))
                });
                store
                    .merge_domains("ausn", "usn", |_, path| {
                        let _ = started.send(());
                        std::thread::sleep(std::time::Duration::from_millis(200));
                        Ok::<_, KnowledgeStoreError>(fs::read_to_string(path).unwrap())
                    })
                    .unwrap();
                assert!(matches!(writer.join().unwrap(), Err(KnowledgeStoreError::NoDomain(d)) if d == "ausn"));
            });
            assert_eq!(store.documents().unwrap(), ["usn/r/a.md"]);
            assert!(!store.dir.join("ausn").exists());
        });
    }

    /// A repository with a name past what an id holds still gets one that
    /// lists, finds and removes its documents — a namesake's too.
    #[test]
    fn a_long_name_still_makes_ids() {
        with_app_dir("knowledge-long-name", || {
            let store = store("knowledge-long-name-base");
            let long = "a-very-long-service-name-".repeat(5);
            let first = store.repo(&repo_at("knowledge-long-name-a", &format!("git@h:o/{long}.git"))).unwrap();
            store.apply(write(first.clone(), &[("general", "a", "a")])).unwrap();
            let namesake = store.repo(&repo_at("knowledge-long-name-b", &format!("git@other:x/{long}.git"))).unwrap();
            store.apply(write(namesake.clone(), &[("general", "a", "a")])).unwrap();

            assert_eq!(namesake.name, format!("{}-2", first.name));
            let (one, two) = (format!("general/{}/a.md", first.name), format!("general/{}/a.md", namesake.name));
            let mut ids = vec![one.clone(), two];
            ids.sort();
            assert_eq!(store.documents().unwrap(), ids);
            store.remove_repo(&namesake.name).unwrap();
            assert_eq!(store.documents().unwrap(), [one]);
        });
    }
}
