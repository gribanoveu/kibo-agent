//! `knowledgeWrite` — documents about the open repository, kept in the
//! knowledge base for agents working in other repositories
//! (`docs/40-knowledge-base.md`).
//!
//! All or nothing, as `kb`'s `publish_docs` was: every document is parsed and
//! its evidence found in the tree first, and every reason goes back at once,
//! so one round fixes them all. Then the model checks the lot
//! (`KNOWLEDGE_CHECK_PROMPT`), as it checks a `remember` note, and only then
//! is anything written. No approval card: one `/analyze` writes dozens.

use std::fs;

use crate::domain::knowledge::{self, Document, GENERAL_DOMAIN, KNOWLEDGE_CHECK_PROMPT, MAX_BODY_CHARS};
use crate::domain::llm::LlmToolDefinition;
use crate::domain::tools::{KnowledgeWriteArgs, ToolDeps, ToolError, ToolResult, ToolScope};
use crate::infra::knowledge_store::{Change, KnowledgeStore};
use crate::infra::repository_identity::head_commit;
use crate::services::ai_tools::resolve::resolve_existing;

pub fn knowledge_write(scope: &ToolScope, args: &KnowledgeWriteArgs, deps: &ToolDeps) -> Result<ToolResult, ToolError> {
    let documents = args.documents.as_deref().unwrap_or_default();
    let delete = args.delete.as_deref().unwrap_or_default();
    let new_domains = &args.new_domains;
    if documents.is_empty() && delete.is_empty() && new_domains.is_empty() {
        return Err(fail("nothing to do — pass `documents`, `delete` or `newDomains`"));
    }
    let store = KnowledgeStore::open().map_err(fail)?;
    let mut repo = store.repo(scope.root()).map_err(fail)?;
    let kept = store.domains().map_err(fail)?;
    let mut problems = Vec::new();

    for (i, domain) in new_domains.iter().enumerate() {
        let taken = domain.id == GENERAL_DOMAIN
            || kept.iter().any(|d| d.id == domain.id)
            || new_domains[..i].iter().any(|d| d.id == domain.id);
        match domain.check() {
            Err(e) => problems.push(format!("newDomains[{i}]: {e}")),
            Ok(()) if taken => problems.push(format!("newDomains[{i}]: `{}` is already a domain — use it", domain.id)),
            Ok(()) => {}
        }
    }
    let known = |id: &str| id == GENERAL_DOMAIN || kept.iter().any(|d| d.id == id) || new_domains.iter().any(|d| d.id == id);

    let mut parsed: Vec<Document> = Vec::new();
    for (i, text) in documents.iter().enumerate() {
        let doc = match knowledge::parse_document(text, &repo.name) {
            Ok(doc) => doc,
            Err(e) => {
                problems.push(format!("documents[{i}]: {e}"));
                continue;
            }
        };
        let mut wrong = Vec::new();
        if !known(&doc.domain) {
            let ids: Vec<String> = kept.iter().map(|d| format!("`{}` ({})", d.id, d.name)).collect();
            wrong.push(format!(
                "domain `{}` is not in the base — use one of {}, `{GENERAL_DOMAIN}`, or add it in `newDomains`",
                doc.domain,
                if ids.is_empty() { "none yet".to_string() } else { ids.join(", ") },
            ));
        }
        wrong.extend(doc.evidence.iter().filter_map(|evidence| evidence_problem(scope, evidence)));
        if parsed.iter().any(|d| d.slug == doc.slug) {
            wrong.push("this id is in the call twice".to_string());
        }
        if wrong.is_empty() {
            parsed.push(doc);
        } else {
            problems.extend(wrong.into_iter().map(|why| format!("documents[{i}] (`{}`): {why}", doc.id())));
        }
    }

    let mut deleted = Vec::new();
    for id in delete {
        match knowledge::own_slug(id, &repo.name) {
            Err(e) => problems.push(format!("delete `{id}`: {e}")),
            Ok(slug) if parsed.iter().any(|d| d.slug == slug) => {
                problems.push(format!("delete `{id}`: it is written in the same call"))
            }
            Ok(slug) => match store.find(&repo.name, slug).map_err(fail)? {
                Some(_) => deleted.push(slug.to_string()),
                None => problems.push(format!("delete `{id}`: there is no such document")),
            },
        }
    }

    if !problems.is_empty() {
        return Err(fail(format!(
            "nothing was written; fix these and send the whole call again:\n- {}",
            problems.join("\n- ")
        )));
    }

    let commit = head_commit(scope.root());
    let rendered: Vec<String> = parsed.iter().map(|d| d.render(commit.as_deref())).collect();
    if !rendered.is_empty() || !new_domains.is_empty() {
        let check = deps.check.ok_or_else(|| fail("not available here"))?;
        check(KNOWLEDGE_CHECK_PROMPT, &knowledge::check_text(&rendered, new_domains)).map_err(|why| {
            fail(format!(
                "not saved: {why}. Do not reword it to get past the check; tell the user what you wanted to save."
            ))
        })?;
    }

    if !parsed.is_empty() {
        repo.commit = commit;
    }
    let name = repo.name.clone();
    let written = parsed.iter().map(Document::id).collect();
    store
        .apply(Change {
            repo,
            new_domains,
            write: parsed.into_iter().zip(rendered).map(|(d, text)| (d.domain, d.slug, text)).collect(),
            delete: deleted.clone(),
        })
        .map_err(fail)?;
    Ok(ToolResult::KnowledgeWritten {
        written,
        deleted: deleted.iter().map(|slug| format!("{name}/{slug}")).collect(),
        new_domains: new_domains.iter().map(|d| d.id.clone()).collect(),
        repo: name,
    })
}

/// What is wrong with citing `evidence`: not a file of the open folder, or
/// not as many lines as it names. `None` when it holds.
fn evidence_problem(scope: &ToolScope, evidence: &knowledge::Evidence) -> Option<String> {
    let shown = &evidence.path;
    let Ok(path) = resolve_existing(scope, shown) else {
        return Some(format!("evidence `{shown}`: no such file in the repository"));
    };
    if !path.is_file() {
        return Some(format!("evidence `{shown}`: not a file"));
    }
    let bytes = match fs::read(&path) {
        Ok(bytes) => bytes,
        Err(e) => return Some(format!("evidence `{shown}`: could not be read ({e})")),
    };
    let newlines = bytes.iter().filter(|b| **b == b'\n').count();
    let total = newlines + usize::from(!bytes.is_empty() && !bytes.ends_with(b"\n"));
    (!evidence.fits(u32::try_from(total).unwrap_or(u32::MAX))).then(|| format!("evidence `{shown}`: the file has {total} lines"))
}

fn fail(why: impl ToString) -> ToolError {
    ToolError::Knowledge(why.to_string())
}

pub(super) fn definition() -> LlmToolDefinition {
    LlmToolDefinition {
        name: "knowledgeWrite".to_string(),
        description: format!(
            "Save documents about this repository to the knowledge base, where agents working in other repositories find them. \
Write facts with their sources, not a retelling of the code: what the service is for, its endpoints, the topics and queues it \
publishes or reads, the tables it owns, how a calculation goes (inputs, steps, edge cases, a worked example), how to do a common task. \
Save when you learned something about the code that took many reads and that another repository's agent would need; \
facts about the user or their machine go to `remember` instead.\n\n\
Each document is Markdown with YAML frontmatter:\n\
---\n\
id: <slug>              # lowercase, digits, - _ . — or <repo>/<slug>\n\
domain: <domain id>     # the subject area, e.g. usn, ausn, fns-exchange; `{GENERAL_DOMAIN}` when none fits\n\
kind: service | api | event | datastore | concept | algorithm | howto\n\
title: Short title\n\
aliases: [what people call it]\n\
provides:               # contracts this repository owns or serves\n\
  - topic: tax.events.v1          # also api: <repo> METHOD /path, db: <db>.<table>, storage: <bucket>/<prefix>, lib: <package>, external: <system>\n\
consumes:               # the same, used from others\n\
  - db: tax.declarations\n\
evidence:               # files from the repository root, with lines\n\
  - src/main/java/TaxEventProducer.java#L40-L60\n\
related: [<repo>/<slug>]\n\
confidence: confirmed | inferred | unknown\n\
---\n\
# Title\n\
One or two sentences on what this is. Then short `##` sections, one question each.\n\n\
Rules: one topic per document, at most {MAX_BODY_CHARS} characters; exactly one `service` overview per repository, written last. \
`confirmed` and any `provides`/`consumes` need `evidence` you saw; otherwise `inferred`, or `unknown` saying what is not known — never guess. \
Contract names are the real ones from the configuration, no `${{…}}` placeholders. No secrets: write `<token>` or `${{DB_PASSWORD}}`. \
An unknown domain is refused: pass it in `newDomains` with a name and a description — but use an existing one when it fits. \
Writing an id again replaces that document. The call is all or nothing: on errors nothing is written, and each document's reasons come back — fix them and send the whole call again. \
The documents are checked before they are kept; refused ones are not saved — do not reword them to get past the check."
        ),
        parameters: serde_json::json!({
            "type": "object",
            "properties": {
                "documents": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "Whole documents, frontmatter first. A few per call, as each topic is written."
                },
                "delete": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "Ids of this repository's documents about things that no longer exist."
                },
                "newDomains": {
                    "type": "array",
                    "items": {
                        "type": "object",
                        "properties": {
                            "id": { "type": "string", "description": "Lowercase Latin letters, digits and -: usn, fns-exchange." },
                            "name": { "type": "string", "description": "As people say it: «Бухгалтерия УСН»." },
                            "description": { "type": "string", "description": "One or two sentences: what belongs here, so later documents are filed right." }
                        },
                        "required": ["id", "name", "description"]
                    },
                    "description": "Domains to add, when no existing one fits these documents."
                }
            },
            "required": []
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::knowledge::Domain;
    use crate::testing::{temp_dir, with_app_dir};
    use std::cell::RefCell;
    use std::path::{Path, PathBuf};

    fn doc(id: &str, domain: &str, evidence: &str) -> String {
        format!(
            "---\nid: {id}\ndomain: {domain}\nkind: api\ntitle: Декларации API\nprovides:\n  - api: tax-api POST /api/v1/declarations\n\
evidence:\n  - {evidence}\nconfidence: confirmed\n---\n# Декларации API\n\nПринимает декларации.\n"
        )
    }

    /// A repository named `tax-api` by its remote, with one committed file of three lines.
    fn repo(label: &str) -> (ToolScope, PathBuf) {
        let dir = temp_dir(label);
        let git = git2::Repository::init(&dir).unwrap();
        git.remote("origin", "git@github.com:org/tax-api.git").unwrap();
        fs::create_dir_all(dir.join("src")).unwrap();
        fs::write(dir.join("src/Api.java"), "a\nb\nc\n").unwrap();
        (ToolScope::new(&dir).unwrap(), dir)
    }

    fn args(documents: &[String], delete: &[&str], new_domains: &[Domain]) -> KnowledgeWriteArgs {
        KnowledgeWriteArgs {
            documents: Some(documents.to_vec()),
            delete: Some(delete.iter().map(|s| s.to_string()).collect()),
            new_domains: new_domains.to_vec(),
        }
    }

    fn kept(repo: &str, slug: &str) -> Option<String> {
        let path = KnowledgeStore::open().unwrap().find(repo, slug).unwrap()?;
        Some(fs::read_to_string(path).unwrap())
    }

    fn base() -> PathBuf {
        crate::infra::app_dir::dir().unwrap().join("knowledge")
    }

    fn usn() -> Domain {
        Domain { id: "usn".into(), name: "Бухгалтерия УСН".into(), description: "Упрощённая система".into() }
    }

    #[test]
    fn checked_documents_are_kept_under_their_domain_with_the_commit() {
        with_app_dir("knowledge-tool-saved", || {
            let (scope, _) = repo("knowledge-tool-saved-repo");
            let asked = RefCell::new(Vec::new());
            let check = |prompt: &str, text: &str| {
                assert_eq!(prompt, KNOWLEDGE_CHECK_PROMPT);
                asked.borrow_mut().push(text.to_string());
                Ok(())
            };
            let deps = ToolDeps { check: Some(&check), ..ToolDeps::default() };

            let call = args(&[doc("declarations-api", "usn", "src/Api.java#L1-L3")], &[], &[usn()]);
            assert_eq!(
                knowledge_write(&scope, &call, &deps).unwrap(),
                ToolResult::KnowledgeWritten {
                    repo: "tax-api".into(),
                    written: vec!["tax-api/declarations-api".into()],
                    deleted: vec![],
                    new_domains: vec!["usn".into()],
                }
            );
            assert!(base().join("usn/tax-api/declarations-api.md").is_file());
            let text = kept("tax-api", "declarations-api").unwrap();
            assert!(text.contains("id: tax-api/declarations-api") && !text.contains("commit:"), "no commit yet: {text}");
            assert_eq!(asked.borrow().len(), 1);
            assert!(asked.borrow()[0].contains("New domain `usn`") && asked.borrow()[0].contains("# Декларации API"));

            // Once there is a commit, the document says which, and so does the record.
            let git = git2::Repository::open(scope.root()).unwrap();
            let tree = git.treebuilder(None).unwrap().write().unwrap();
            let me = git2::Signature::now("t", "t@example.com").unwrap();
            let head = git.commit(Some("HEAD"), &me, &me, "c", &git.find_tree(tree).unwrap(), &[]).unwrap().to_string();
            knowledge_write(&scope, &args(&[doc("declarations-api", "usn", "src/Api.java")], &[], &[]), &deps).unwrap();
            assert!(kept("tax-api", "declarations-api").unwrap().contains(&format!("commit: {head}")));
            assert_eq!(KnowledgeStore::open().unwrap().repo(scope.root()).unwrap().commit, Some(head.clone()));

            // Deleting writes nothing new: the commit stays the one the documents were written at.
            let parent = git.find_commit(git2::Oid::from_str(&head).unwrap()).unwrap();
            git.commit(Some("HEAD"), &me, &me, "c2", &git.find_tree(tree).unwrap(), &[&parent]).unwrap();
            knowledge_write(&scope, &args(&[], &["declarations-api"], &[]), &deps).unwrap();
            assert_eq!(KnowledgeStore::open().unwrap().repo(scope.root()).unwrap().commit, Some(head));
        });
    }

    #[test]
    fn every_reason_comes_back_and_nothing_is_written() {
        with_app_dir("knowledge-tool-refused", || {
            let (scope, _) = repo("knowledge-tool-refused-repo");
            let check = |_: &str, _: &str| -> Result<(), String> { panic!("not asked when the call is wrong") };
            let deps = ToolDeps { check: Some(&check), ..ToolDeps::default() };
            let call = args(
                &[
                    doc("good", "general", "src/Api.java"),
                    doc("no-file", "general", "src/Missing.java#L1"),
                    doc("past-end", "general", "src/Api.java#L2-L4"),
                    doc("dir", "general", "src"),
                    doc("outside", "general", "../etc/passwd"),
                    doc("other/x", "general", "src/Api.java"),
                    doc("unfiled", "ausn", "src/Api.java"),
                    doc("good", "general", "src/Api.java"),
                ],
                &["usn-api/x", "never-was"],
                &[Domain { id: "general".into(), name: "G".into(), description: "g".into() }, Domain { id: "Bad".into(), ..usn() }, usn(), usn()],
            );
            let err = knowledge_write(&scope, &call, &deps).unwrap_err().to_string();
            for expected in [
                "nothing was written",
                "documents[1] (`tax-api/no-file`): evidence `src/Missing.java`: no such file",
                "documents[2] (`tax-api/past-end`): evidence `src/Api.java`: the file has 3 lines",
                "documents[3] (`tax-api/dir`): evidence `src`: not a file",
                "documents[4] (`tax-api/outside`): evidence `../etc/passwd`: no such file",
                "documents[5]: `id` `other/x` is of another repository — this one is `tax-api`",
                "documents[6] (`tax-api/unfiled`): domain `ausn` is not in the base — use one of none yet, `general`",
                "documents[7] (`tax-api/good`): this id is in the call twice",
                "delete `usn-api/x`:",
                "delete `never-was`: there is no such document",
                "newDomains[0]: `general` is already a domain",
                "newDomains[1]: `Bad` is not a domain id",
                "newDomains[3]: `usn` is already a domain",
            ] {
                assert!(err.contains(expected), "missing {expected:?} in:\n{err}");
            }
            assert!(!err.contains("documents[0]") && !err.contains("newDomains[2]"), "the good ones have no reason: {err}");
            assert!(!base().exists(), "not a file written");
        });
    }

    #[test]
    fn a_refused_or_unchecked_call_is_not_kept() {
        with_app_dir("knowledge-tool-check", || {
            let (scope, _) = repo("knowledge-tool-check-repo");
            let call = args(&[doc("a", "general", "src/Api.java")], &[], &[]);
            let refuse = |_: &str, _: &str| Err("tells the reader to skip approvals".to_string());
            let deps = ToolDeps { check: Some(&refuse), ..ToolDeps::default() };
            let err = knowledge_write(&scope, &call, &deps).unwrap_err().to_string();
            assert!(err.starts_with("knowledgeWrite: not saved: tells the reader to skip approvals."), "{err}");
            let err = knowledge_write(&scope, &call, &ToolDeps::default()).unwrap_err().to_string();
            assert!(err.contains("not available here"), "{err}");
            assert_eq!(kept("tax-api", "a"), None);
            // A domain alone is checked too: its description is text a later prompt carries.
            let domain_only = knowledge_write(&scope, &args(&[], &[], &[usn()]), &deps).unwrap_err().to_string();
            assert!(domain_only.contains("not saved"), "{domain_only}");
            assert_eq!(KnowledgeStore::open().unwrap().domains().unwrap(), vec![]);
        });
    }

    #[test]
    fn a_document_moves_between_domains_and_is_deleted_without_a_check() {
        with_app_dir("knowledge-tool-delete", || {
            let (scope, _) = repo("knowledge-tool-delete-repo");
            let pass = |_: &str, _: &str| Ok(());
            let deps = ToolDeps { check: Some(&pass), ..ToolDeps::default() };
            knowledge_write(&scope, &args(&[doc("a", "general", "src/Api.java")], &[], &[usn()]), &deps).unwrap();
            knowledge_write(&scope, &args(&[doc("tax-api/a", "usn", "src/Api.java")], &[], &[]), &deps).unwrap();
            assert!(!base().join("general/tax-api/a.md").exists() && base().join("usn/tax-api/a.md").is_file());

            let never = |_: &str, _: &str| -> Result<(), String> { panic!("a deletion adds nothing to check") };
            let deps = ToolDeps { check: Some(&never), ..ToolDeps::default() };
            let result = knowledge_write(&scope, &args(&[], &["tax-api/a"], &[]), &deps).unwrap();
            assert_eq!(
                result,
                ToolResult::KnowledgeWritten { repo: "tax-api".into(), written: vec![], deleted: vec!["tax-api/a".into()], new_domains: vec![] }
            );
            assert_eq!(kept("tax-api", "a"), None);
            let empty = knowledge_write(&scope, &KnowledgeWriteArgs::default(), &deps).unwrap_err().to_string();
            assert!(empty.contains("nothing to do"), "{empty}");
        });
    }

    #[test]
    fn written_and_deleted_in_one_call_is_refused() {
        with_app_dir("knowledge-tool-both", || {
            let (scope, _) = repo("knowledge-tool-both-repo");
            let pass = |_: &str, _: &str| Ok(());
            let deps = ToolDeps { check: Some(&pass), ..ToolDeps::default() };
            knowledge_write(&scope, &args(&[doc("a", "general", "src/Api.java")], &[], &[]), &deps).unwrap();
            let err = knowledge_write(&scope, &args(&[doc("a", "general", "src/Api.java")], &["a"], &[]), &deps).unwrap_err();
            assert!(err.to_string().contains("delete `a`: it is written in the same call"), "{err}");
        });
    }

    /// A file without a final newline still counts its last line.
    #[test]
    fn the_last_line_counts_without_a_newline() {
        let (scope, dir): (ToolScope, PathBuf) = repo("knowledge-tool-lines");
        let at = |text: &str, cited: &str| {
            fs::write(Path::new(&dir).join("src/F.java"), text).unwrap();
            evidence_problem(&scope, &knowledge::Evidence::parse(cited).unwrap())
        };
        assert_eq!(at("a\nb", "src/F.java#L2"), None);
        assert_eq!(at("a\nb", "src/F.java#L3").as_deref(), Some("evidence `src/F.java`: the file has 2 lines"));
        assert_eq!(at("a\nb\n", "src/F.java#L2"), None);
        assert_eq!(at("a\nb\n", "src/F.java#L3").as_deref(), Some("evidence `src/F.java`: the file has 2 lines"));
        assert_eq!(at("", "src/F.java"), None);
        assert!(at("", "src/F.java#L1").is_some());
    }
}
