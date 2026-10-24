//! What the knowledge base says of the open repository, for the prompt
//! (`docs/40-knowledge-base.md`, decision 5); the whole base, and the changes
//! made to it by hand, for the Knowledge page (decision 9); and which
//! documents may be out of date.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use thiserror::Error;

use crate::domain::knowledge::{
    self, Base, Domain, DomainCount, Header, KnowledgeError, Listed, Overview, RepoSummary, Since, GENERAL_DOMAIN,
};
use crate::infra::knowledge_store::{Change, KnowledgeStore, KnowledgeStoreError};
use crate::infra::repository_identity::changed_since;

#[derive(Debug, Error)]
pub enum KnowledgePageError {
    #[error(transparent)]
    Store(#[from] KnowledgeStoreError),
    #[error(transparent)]
    Document(#[from] KnowledgeError),
    #[error("there is no document `{0}`")]
    NoDocument(String),
    #[error("there is no domain `{0}`")]
    NoDomain(String),
    #[error("`{GENERAL_DOMAIN}` is the app's: it cannot be renamed")]
    General,
    #[error("the base has no record of `{0}` — its documents were put there by hand")]
    NoRepo(String),
}

/// Whether documents are stale, asking each repository's history once per
/// commit they were written at.
#[derive(Default)]
pub struct Staleness {
    seen: HashMap<(PathBuf, String), Since>,
}

impl Staleness {
    /// Of a document of the repository at `root`, as its header says.
    pub fn of(&mut self, root: &Path, header: &Header) -> bool {
        let Some(commit) = header.commit.as_deref() else {
            return false;
        };
        let since = self
            .seen
            .entry((root.to_path_buf(), commit.to_string()))
            .or_insert_with(|| changed_since(root, commit));
        knowledge::is_stale(&header.evidence, since)
    }
}

/// The open repository's documents and the base's domains.
pub fn overview(root: &Path) -> Result<Overview, KnowledgeStoreError> {
    let store = KnowledgeStore::open()?;
    let repo = store.repo(root)?.name;
    let roots = HashMap::from([(repo.clone(), root.to_path_buf())]);
    let (documents, domains) = walk(&store, &roots, Some(&repo))?;
    Ok(Overview { repo, documents, domains })
}

/// Every document, domain and repository of the base, documents of a
/// repository marked stale against where it was open last.
pub fn base() -> Result<Base, KnowledgeStoreError> {
    let store = KnowledgeStore::open()?;
    let records = store.repos()?;
    let roots = records.iter().map(|r| (r.name.clone(), PathBuf::from(&r.path))).collect();
    let (documents, domains) = walk(&store, &roots, None)?;
    let repos = records
        .into_iter()
        .map(|record| {
            let own: Vec<&Listed> = documents.iter().filter(|d| d.repo == record.name).collect();
            RepoSummary {
                documents: own.len(),
                stale: own.iter().filter(|d| d.stale).count(),
                written: own.iter().filter_map(|d| d.written).max(),
                name: record.name,
                path: record.path,
                commit: record.commit,
            }
        })
        .collect();
    Ok(Base { domains, repos, documents })
}

/// The documents — of `only`, or all — read from their headers, and every
/// domain with how many documents and repositories it has.
fn walk(
    store: &KnowledgeStore,
    roots: &HashMap<String, PathBuf>,
    only: Option<&str>,
) -> Result<(Vec<Listed>, Vec<DomainCount>), KnowledgeStoreError> {
    let mut staleness = Staleness::default();
    let mut documents = Vec::new();
    let mut counts: BTreeMap<String, (usize, BTreeSet<String>)> = BTreeMap::new();
    for relative in store.documents()? {
        let Some((domain, id)) = knowledge::place_of(&relative) else {
            continue;
        };
        let owner = id.split_once('/').map_or("", |(owner, _)| owner).to_string();
        let count = counts.entry(domain.to_string()).or_default();
        count.0 += 1;
        count.1.insert(owner.clone());
        if only.is_some_and(|only| only != owner) {
            continue;
        }
        // A file edited past reading is left out, as search leaves it out.
        let path = store.dir().join(&relative);
        let text = fs::read_to_string(&path).unwrap_or_default();
        let Some((header, _)) = knowledge::read_header(&text) else {
            continue;
        };
        let stale = roots.get(&owner).is_some_and(|root| staleness.of(root, &header));
        let written = fs::metadata(&path).and_then(|m| m.modified()).ok();
        documents.push(Listed {
            id,
            domain: domain.to_string(),
            repo: owner,
            kind: header.kind,
            cited: !header.evidence.is_empty(),
            title: header.title,
            confidence: header.confidence,
            stale,
            written: written.and_then(|t| t.duration_since(UNIX_EPOCH).ok()).map(|d| d.as_secs()),
        });
    }
    // `general` is never in `domains.json`: `knowledgeWrite` refuses it as a new domain.
    let domains = store
        .domains()?
        .into_iter()
        .chain(counts.contains_key(GENERAL_DOMAIN).then(knowledge::general_domain))
        .map(|domain| {
            let (documents, repos) = counts.get(&domain.id).map_or((0, 0), |(n, repos)| (*n, repos.len()));
            DomainCount { domain, documents, repos }
        })
        .collect();
    Ok((documents, domains))
}

/// A document's file as kept.
pub fn document(id: &str) -> Result<String, KnowledgePageError> {
    let store = KnowledgeStore::open()?;
    let path = kept(&store, id)?;
    fs::read_to_string(&path).map_err(|source| KnowledgeStoreError::Read { path, source }.into())
}

/// A document filed under another domain: its `domain` field and its folder.
pub fn move_document(id: &str, into: &str) -> Result<(), KnowledgePageError> {
    let store = KnowledgeStore::open()?;
    let (repo, slug) = knowledge::parse_id(id)?;
    if into != GENERAL_DOMAIN && !store.domains()?.iter().any(|d| d.id == into) {
        return Err(KnowledgePageError::NoDomain(into.to_string()));
    }
    let path = kept(&store, id)?;
    let text = fs::read_to_string(&path).map_err(|source| KnowledgeStoreError::Read { path, source })?;
    let commit = knowledge::read_header(&text).and_then(|(header, _)| header.commit);
    let mut doc = knowledge::parse_document(&text, repo)?;
    doc.domain = into.to_string();
    let record = record_of(&store, repo)?;
    let write = vec![(into.to_string(), slug.to_string(), doc.render(commit.as_deref()))];
    Ok(store.apply(Change { repo: record, new_domains: &[], write, delete: Vec::new() })?)
}

pub fn delete_document(id: &str) -> Result<(), KnowledgePageError> {
    let store = KnowledgeStore::open()?;
    let (repo, slug) = knowledge::parse_id(id)?;
    kept(&store, id)?;
    let record = record_of(&store, repo)?;
    Ok(store.apply(Change { repo: record, new_domains: &[], write: Vec::new(), delete: vec![slug.to_string()] })?)
}

pub fn delete_repo(name: &str) -> Result<(), KnowledgePageError> {
    Ok(KnowledgeStore::open()?.remove_repo(name)?)
}

/// A domain's name and description, checked as a new one is.
pub fn edit_domain(domain: &Domain) -> Result<(), KnowledgePageError> {
    if domain.id == GENERAL_DOMAIN {
        return Err(KnowledgePageError::General);
    }
    let domain = Domain { id: domain.id.clone(), name: domain.name.trim().to_string(), description: domain.description.trim().to_string() };
    domain.check()?;
    Ok(KnowledgeStore::open()?.edit_domain(&domain)?)
}

/// Every document of `from` moved into `into`, and `from` off the list.
pub fn merge_domains(from: &str, into: &str) -> Result<(), KnowledgePageError> {
    let store = KnowledgeStore::open()?;
    let known = |id: &str| -> Result<bool, KnowledgeStoreError> { Ok(id == GENERAL_DOMAIN || store.domains()?.iter().any(|d| d.id == id)) };
    for id in [from, into] {
        if !known(id)? {
            return Err(KnowledgePageError::NoDomain(id.to_string()));
        }
    }
    if from == into {
        return Ok(());
    }
    for relative in store.documents()? {
        if let Some((_, id)) = knowledge::place_of(&relative).filter(|(domain, _)| *domain == from) {
            move_document(&id, into)?;
        }
    }
    Ok(store.remove_domain(from)?)
}

fn kept(store: &KnowledgeStore, id: &str) -> Result<PathBuf, KnowledgePageError> {
    let (repo, slug) = knowledge::parse_id(id)?;
    store.find(repo, slug)?.ok_or_else(|| KnowledgePageError::NoDocument(id.to_string()))
}

fn record_of(store: &KnowledgeStore, repo: &str) -> Result<crate::infra::knowledge_store::RepoRecord, KnowledgePageError> {
    store.repos()?.into_iter().find(|r| r.name == repo).ok_or_else(|| KnowledgePageError::NoRepo(repo.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::knowledge::Domain;
    use crate::infra::knowledge_store::Change;
    use crate::testing::{temp_dir, with_app_dir};
    use git2::Repository;

    fn doc(title: &str, evidence: &str, commit: &str) -> String {
        format!("---\nid: x\nkind: api\ntitle: {title}\ncommit: {commit}\nevidence:\n  - {evidence}\nconfidence: confirmed\n---\n# {title}\n")
    }

    /// A repository with `src/Api.java` and `src/Job.java`, committed; the commit.
    fn repository(label: &str) -> (PathBuf, String) {
        let dir = temp_dir(label);
        let repo = Repository::init(&dir).unwrap();
        repo.remote("origin", "git@h:o/tax-api.git").unwrap();
        let commit = commit_files(&dir, &[("src/Api.java", "a"), ("src/Job.java", "j")]);
        (dir, commit)
    }

    fn commit_files(dir: &Path, files: &[(&str, &str)]) -> String {
        let repo = Repository::open(dir).unwrap();
        let mut index = repo.index().unwrap();
        for (path, text) in files {
            fs::create_dir_all(dir.join(path).parent().unwrap()).unwrap();
            fs::write(dir.join(path), text).unwrap();
            index.add_path(Path::new(path)).unwrap();
        }
        index.write().unwrap();
        let tree = repo.find_tree(index.write_tree().unwrap()).unwrap();
        let me = git2::Signature::now("t", "t@example.com").unwrap();
        let parent = repo.head().ok().and_then(|h| h.peel_to_commit().ok());
        repo.commit(Some("HEAD"), &me, &me, "c", &tree, &parent.iter().collect::<Vec<_>>()).unwrap().to_string()
    }

    #[test]
    fn the_overview_lists_this_repositorys_documents_and_every_domain() {
        with_app_dir("knowledge-overview", || {
            let (root, first) = repository("knowledge-overview-repo");
            let store = KnowledgeStore::open().unwrap();
            assert_eq!(overview(&root).unwrap(), Overview { repo: "tax-api".into(), ..Overview::default() }, "an empty base");

            let usn = Domain { id: "usn".into(), name: "УСН".into(), description: "упрощёнка".into() };
            let ausn = Domain { id: "ausn".into(), name: "АУСН".into(), description: "автоупрощёнка".into() };
            let record = store.repo(&root).unwrap();
            let other = crate::infra::knowledge_store::RepoRecord { name: "usn-api".into(), repository_id: "other".into(), ..record.clone() };
            let write = |repo, docs: &[(&str, &str, String)], new: &[Domain]| {
                let write = docs.iter().map(|(d, s, t)| (d.to_string(), s.to_string(), t.clone())).collect();
                store.apply(Change { repo, new_domains: new, write, delete: vec![] }).unwrap();
            };
            write(
                record,
                &[
                    ("usn", "api", doc("Декларации API", "src/Api.java#L1", &first)),
                    ("usn", "job", doc("Повтор отправки", "src/Job.java", &first)),
                    ("usn", "notes", format!("---\nid: x\nkind: concept\ntitle: Заметки\ncommit: {first}\nconfidence: unknown\n---\n# Заметки\n")),
                    ("general", "service", doc("Сервис", "src/Job.java", "0123456789012345678901234567890123456789")),
                    ("general", "broken", "no header".into()),
                ],
                &[usn.clone(), ausn.clone()],
            );
            write(other, &[("usn", "tariff", doc("Тариф", "src/T.java", "abc"))], &[]);
            commit_files(&root, &[("src/Api.java", "changed")]);

            let seen = overview(&root).unwrap();
            let listed: Vec<_> = seen.documents.iter().map(|d| (d.domain.as_str(), d.id.as_str(), d.title.as_str(), d.stale)).collect();
            assert_eq!(seen.documents.iter().map(|d| d.cited).collect::<Vec<_>>(), [true, true, true, false], "the notes cite nothing");
            assert_eq!(
                listed,
                [
                    ("general", "tax-api/service", "Сервис", true),
                    ("usn", "tax-api/api", "Декларации API", true),
                    ("usn", "tax-api/job", "Повтор отправки", false),
                    ("usn", "tax-api/notes", "Заметки", false),
                ],
                "by domain; a file it cites changed, or its commit is gone"
            );
            let domains: Vec<_> = seen.domains.iter().map(|d| (d.domain.id.as_str(), d.documents, d.repos)).collect();
            assert_eq!(domains, [("usn", 4, 2), ("ausn", 0, 0), ("general", 2, 1)]);
        });
    }

    fn file(slug: &str, domain: &str, title: &str, commit: &str) -> String {
        format!("---\nid: {slug}\ndomain: {domain}\nkind: api\ntitle: {title}\ncommit: {commit}\nevidence:\n  - src/Api.java\nconfidence: confirmed\n---\n# {title}\n\nО чём.\n")
    }

    #[test]
    fn the_page_sees_the_whole_base_and_changes_it_by_hand() {
        with_app_dir("knowledge-page", || {
            let (root, first) = repository("knowledge-page-repo");
            let store = KnowledgeStore::open().unwrap();
            assert_eq!(base().unwrap(), Base::default(), "an empty base");
            let usn = Domain { id: "usn".into(), name: "УСН".into(), description: "упрощёнка".into() };
            let ausn = Domain { id: "ausn".into(), name: "АУСН".into(), description: "автоупрощёнка".into() };
            let record = store.repo(&root).unwrap();
            let other = crate::infra::knowledge_store::RepoRecord { name: "ledger".into(), repository_id: "other".into(), path: "/nowhere".into(), commit: None };
            let write = |repo, docs: &[(&str, &str, String)], new: &[Domain]| {
                let write = docs.iter().map(|(d, s, t)| (d.to_string(), s.to_string(), t.clone())).collect();
                store.apply(Change { repo, new_domains: new, write, delete: vec![] }).unwrap();
            };
            let new = [usn.clone(), ausn.clone()];
            write(record, &[("usn", "api", file("api", "usn", "API", &first)), ("usn", "job", file("job", "usn", "Job", &first))], &new);
            write(other, &[("ausn", "listener", file("listener", "ausn", "Слушатель", "abc"))], &[]);
            commit_files(&root, &[("src/Api.java", "changed")]);
            let touch = |path: &str, secs: u64| {
                let file = fs::File::options().write(true).open(store.dir().join(path)).unwrap();
                file.set_modified(UNIX_EPOCH + std::time::Duration::from_secs(secs)).unwrap();
            };
            touch("usn/tax-api/api.md", 1_000_000_000);
            touch("usn/tax-api/job.md", 2_000_000_000);

            let seen = base().unwrap();
            let ids: Vec<_> = seen.documents.iter().map(|d| (d.domain.as_str(), d.id.as_str(), d.repo.as_str(), d.stale)).collect();
            assert_eq!(ids, [("ausn", "ledger/listener", "ledger", false), ("usn", "tax-api/api", "tax-api", true), ("usn", "tax-api/job", "tax-api", true)]);
            let repos: Vec<_> = seen.repos.iter().map(|r| (r.name.as_str(), r.documents, r.stale, r.written.is_some())).collect();
            assert_eq!(repos, [("tax-api", 2, 2, true), ("ledger", 1, 0, true)]);
            assert_eq!(seen.repos[0].written, Some(2_000_000_000), "the repository was written when its latest document was");
            assert_eq!(seen.domains.iter().map(|d| (d.domain.id.as_str(), d.documents)).collect::<Vec<_>>(), [("usn", 2), ("ausn", 1)]);
            assert!(document("tax-api/api").unwrap().contains("title: API"));
            assert!(matches!(document("tax-api/none"), Err(KnowledgePageError::NoDocument(_))));

            move_document("tax-api/job", "ausn").unwrap();
            let moved = document("tax-api/job").unwrap();
            assert!(store.dir().join("ausn/tax-api/job.md").exists() && !store.dir().join("usn/tax-api/job.md").exists());
            assert!(moved.contains("domain: ausn") && moved.contains(&format!("commit: {first}")), "{moved}");
            assert!(matches!(move_document("tax-api/api", "fns"), Err(KnowledgePageError::NoDomain(d)) if d == "fns"));
            move_document("tax-api/api", "general").unwrap();

            let renamed = Domain { name: " АУСН-2 ".into(), description: "  автоматическая упрощёнка ".into(), ..ausn.clone() };
            edit_domain(&renamed).unwrap();
            let trimmed = Domain { name: "АУСН-2".into(), description: "автоматическая упрощёнка".into(), ..ausn.clone() };
            assert_eq!(store.domains().unwrap(), [usn.clone(), trimmed]);
            assert!(matches!(edit_domain(&Domain { name: " ".into(), ..ausn.clone() }), Err(KnowledgePageError::Document(_))));
            assert!(matches!(edit_domain(&knowledge::general_domain()), Err(KnowledgePageError::General)));

            merge_domains("usn", "usn").unwrap();
            assert_eq!(store.domains().unwrap().len(), 2, "a domain merged into itself stays");
            merge_domains("ausn", "usn").unwrap();
            let after: Vec<_> = base().unwrap().documents.iter().map(|d| format!("{}/{}", d.domain, d.id)).collect();
            assert_eq!(after, ["general/tax-api/api", "usn/ledger/listener", "usn/tax-api/job"]);
            assert_eq!(store.domains().unwrap(), [usn.clone()]);
            assert!(document("ledger/listener").unwrap().contains("domain: usn"));
            assert!(matches!(merge_domains("fns", "usn"), Err(KnowledgePageError::NoDomain(_))));

            delete_document("tax-api/api").unwrap();
            assert!(matches!(delete_document("tax-api/api"), Err(KnowledgePageError::NoDocument(_))));
            delete_repo("ledger").unwrap();
            let left = base().unwrap();
            assert_eq!(left.documents.iter().map(|d| d.id.as_str()).collect::<Vec<_>>(), ["tax-api/job"]);
            assert_eq!(left.repos.iter().map(|r| r.name.as_str()).collect::<Vec<_>>(), ["tax-api"]);
        });
    }

    #[test]
    fn a_history_is_asked_once_per_commit() {
        let (root, first) = repository("knowledge-staleness-once");
        let header = |evidence: &str, commit: Option<&str>| Header {
            domain: "usn".into(),
            kind: knowledge::Kind::Api,
            title: "T".into(),
            confidence: knowledge::Confidence::Confirmed,
            evidence: vec![evidence.into()],
            commit: commit.map(str::to_string),
            provides: vec![],
            consumes: vec![],
        };
        let mut staleness = Staleness::default();
        assert!(!staleness.of(&root, &header("src/Api.java", Some(&first))));
        commit_files(&root, &[("src/Api.java", "changed")]);
        assert!(!staleness.of(&root, &header("src/Api.java", Some(&first))), "kept from the first ask");
        assert!(Staleness::default().of(&root, &header("src/Api.java", Some(&first))));
        assert!(!Staleness::default().of(&root, &header("src/Api.java", None)), "no commit, nothing to compare");
    }
}
