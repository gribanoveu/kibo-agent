//! What the knowledge base says of the open repository, for the prompt
//! (`docs/40-knowledge-base.md`, decision 5), and which documents may be out
//! of date.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fs;
use std::path::{Path, PathBuf};

use crate::domain::knowledge::{self, DomainCount, Header, Listed, Overview, Since, GENERAL_DOMAIN};
use crate::infra::knowledge_store::{KnowledgeStore, KnowledgeStoreError};
use crate::infra::repository_identity::changed_since;

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
    let mut staleness = Staleness::default();
    let mut documents = Vec::new();
    let mut counts: BTreeMap<String, (usize, BTreeSet<String>)> = BTreeMap::new();
    for relative in store.documents()? {
        let Some((domain, id)) = knowledge::place_of(&relative) else {
            continue;
        };
        let owner = id.split_once('/').map_or("", |(owner, _)| owner);
        let count = counts.entry(domain.to_string()).or_default();
        count.0 += 1;
        count.1.insert(owner.to_string());
        if owner != repo {
            continue;
        }
        // A file edited past reading is left out, as search leaves it out.
        let text = fs::read_to_string(store.dir().join(&relative)).unwrap_or_default();
        let Some((header, _)) = knowledge::read_header(&text) else {
            continue;
        };
        let stale = staleness.of(root, &header);
        let cited = !header.evidence.is_empty();
        documents.push(Listed { id, domain: domain.to_string(), title: header.title, stale, cited });
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
    Ok(Overview { repo, documents, domains })
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
