//! The knowledge base's tools (`docs/40-knowledge-base.md`): `knowledgeWrite`
//! keeps documents about the open repository for agents working in other
//! repositories; `knowledgeSearch` and `knowledgeRead` find and read what
//! any repository's agent kept.
//!
//! All or nothing, as `kb`'s `publish_docs` was: every document is parsed and
//! its evidence found in the tree first, and every reason goes back at once,
//! so one round fixes them all. Then the model checks the lot
//! (`KNOWLEDGE_CHECK_PROMPT`), as it checks a `remember` note, and only then
//! is anything written. No approval card: one `/analyze` writes dozens.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::PathBuf;

use crate::domain::code_search::SearchFilter;
use crate::domain::knowledge::{
    self, Checks, Document, Entity, Evidence, Header, KnowledgeError, KnowledgeHit, Relation, GENERAL_DOMAIN, KNOWLEDGE_CHECK_PROMPT,
    MAX_BODY_CHARS,
};
use crate::domain::llm::LlmToolDefinition;
use crate::domain::search_query::PathFilter;
use crate::domain::skills::split_frontmatter;
use crate::domain::tools::{
    KnowledgeReadArgs, KnowledgeRelationsArgs, KnowledgeSearchArgs, KnowledgeWriteArgs, ToolDeps, ToolError, ToolResult, ToolScope,
};
use crate::infra::knowledge_store::{Change, KnowledgeStore};
use crate::infra::repository_identity::head_commit;
use crate::services::ai_tools::resolve::{relative_to_root, resolve_existing};
use crate::services::ai_tools::tools::semantic_search::shorten;
use crate::services::knowledge::Staleness;

const DEFAULT_HITS: usize = 8;
const MAX_HITS: usize = 20;
/// Per section found: enough for a fact and its context, not a document.
const HIT_CHARS: usize = 1_500;

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
    let known_domain = |id: &str| id == GENERAL_DOMAIN || kept.iter().any(|d| d.id == id) || new_domains.iter().any(|d| d.id == id);

    let ids: Vec<String> = kept.iter().map(|d| format!("`{}` ({})", d.id, d.name)).collect();
    let unknown_domain = |id: &str| {
        let known = if ids.is_empty() { "none yet".to_string() } else { ids.join(", ") };
        (!known_domain(id)).then(|| KnowledgeError::UnknownDomain { id: id.to_string(), known })
    };
    let cite = |evidence: &Evidence| evidence_path(scope, evidence).map(|path| Evidence { path, ..evidence.clone() });
    let checks = Checks { domain: &unknown_domain, evidence: &cite };

    let mut parsed: Vec<Document> = Vec::new();
    for (i, text) in documents.iter().enumerate() {
        match knowledge::parse_document(text, &repo.name, &checks) {
            Err(found) => problems.extend(found.iter().map(|why| format!("{}: {why}", shown(i, text, &repo.name)))),
            Ok(doc) if parsed.iter().any(|d| d.slug == doc.slug) => {
                problems.push(format!("{}: this id is in the call twice", shown(i, text, &repo.name)))
            }
            Ok(doc) => parsed.push(doc),
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

/// The sections nearest the question, across the base or within a domain or
/// repository, each with what its document is. The same ranking as
/// `semanticSearch`, over the base's documents only.
pub fn knowledge_search(args: &KnowledgeSearchArgs, deps: &ToolDeps) -> Result<ToolResult, ToolError> {
    let fail = |why: String| ToolError::KnowledgeRead(format!("knowledgeSearch: {why}"));
    let search = deps.knowledge.as_ref().ok_or_else(|| fail("the knowledge base is not available here".into()))?;
    if args.query.trim().is_empty() {
        return Err(fail("`query` is empty — say what to find".into()));
    }
    let domain = args.domain.as_deref().map(str::trim).filter(|d| !d.is_empty());
    let repo = args.repo.as_deref().map(str::trim).filter(|r| !r.is_empty());
    if let Some(domain) = domain.filter(|d| !knowledge::is_domain_id(d)) {
        return Err(fail(format!("`{domain}` is not a domain id — lowercase Latin letters, digits and -")));
    }
    if let Some(repo) = repo.filter(|r| !knowledge::is_repo_name(r)) {
        return Err(fail(format!("`{repo}` is not a repository name — as it is in document ids, before the `/`")));
    }
    // Only the documents: `domains.json` and `repos.json` are in the base too.
    let glob = format!("{}/{}/*.md", domain.unwrap_or("*"), repo.unwrap_or("*"));
    let filter = SearchFilter { include_docs: true, paths: PathFilter::new(Some(&glob), None).map_err(fail)? };
    let limit = args.limit.unwrap_or(DEFAULT_HITS).clamp(1, MAX_HITS);
    // A kind is known only from each document's header: ask for more, keep the kind.
    let asked = if args.kind.is_some() { limit * 3 } else { limit };
    let found = search(&[args.query.as_str()], None, asked, &filter).map_err(fail)?;

    let store = KnowledgeStore::open().map_err(|e| fail(e.to_string()))?;
    let roots = repository_roots(&store);
    let mut staleness = Staleness::default();
    let mut headers: HashMap<String, Option<Header>> = HashMap::new();
    let mut hits = Vec::new();
    for found in found.matches {
        let Some((domain, id)) = knowledge::place_of(&found.path) else {
            continue;
        };
        let header = headers.entry(found.path.clone()).or_insert_with(|| {
            let text = fs::read_to_string(store.dir().join(&found.path)).ok()?;
            knowledge::read_header(&text).map(|(header, _)| header)
        });
        // A file gone since the sync, or edited past reading.
        let Some(header) = header.as_ref() else {
            continue;
        };
        if args.kind.is_some_and(|kind| kind != header.kind) {
            continue;
        }
        // The first section carries the frontmatter, which the header already says.
        let text = split_frontmatter(&found.text).map_or(found.text.as_str(), |(_, body)| body);
        let stale = stale_in(&roots, &mut staleness, &id, header);
        hits.push(KnowledgeHit {
            id,
            domain: domain.to_string(),
            title: header.title.clone(),
            kind: header.kind,
            confidence: header.confidence,
            section: found.name,
            text: shorten(text.trim(), HIT_CHARS),
            evidence: header.evidence.clone(),
            stale,
        });
        if hits.len() == limit {
            break;
        }
    }
    Ok(ToolResult::KnowledgeFound { hits, weak: found.meta.weak })
}

/// A document of any repository by its id, whole or one section of it.
pub fn knowledge_read(args: &KnowledgeReadArgs) -> Result<ToolResult, ToolError> {
    let fail = |why: String| ToolError::KnowledgeRead(format!("knowledgeRead: {why}"));
    let (repo, slug) = knowledge::parse_id(&args.id).map_err(|e| fail(e.to_string()))?;
    let store = KnowledgeStore::open().map_err(|e| fail(e.to_string()))?;
    let path = store
        .find(repo, slug)
        .map_err(|e| fail(e.to_string()))?
        .ok_or_else(|| fail(format!("there is no document `{repo}/{slug}` — find it with knowledgeSearch")))?;
    let text = fs::read_to_string(&path).map_err(|e| fail(format!("could not read it: {e}")))?;
    let domain = path.parent().and_then(|p| p.parent()).and_then(|p| p.file_name()).map(|n| n.to_string_lossy().into_owned());
    let section = args.section.as_deref().map(str::trim).filter(|s| !s.is_empty());
    let id = format!("{repo}/{slug}");
    let stale = knowledge::read_header(&text)
        .is_some_and(|(header, _)| stale_in(&repository_roots(&store), &mut Staleness::default(), &id, &header));
    let text = match section {
        None => text,
        Some(wanted) => {
            let body = split_frontmatter(&text).map_or(text.as_str(), |(_, body)| body);
            knowledge::section(body, wanted)
                .map_err(|headings| fail(format!("no section matches `{wanted}` — its sections are: {}", headings.join("; "))))?
                .to_string()
        }
    };
    Ok(ToolResult::KnowledgeDocument { id, domain: domain.unwrap_or_default(), section: section.map(str::to_string), text, stale })
}

/// Who provides and who consumes the contract `entity` names — or every
/// contract of `repo`'s documents — across every domain of the base.
pub fn knowledge_relations(args: &KnowledgeRelationsArgs) -> Result<ToolResult, ToolError> {
    let fail = |why: String| ToolError::KnowledgeRead(format!("knowledgeRelations: {why}"));
    let entity = args.entity.as_deref().map(str::trim).filter(|e| !e.is_empty());
    let repo = args.repo.as_deref().map(str::trim).filter(|r| !r.is_empty());
    let wanted = entity.map(Entity::parse).transpose().map_err(|e| fail(e.to_string()))?;
    if let Some(repo) = repo.filter(|r| !knowledge::is_repo_name(r)) {
        return Err(fail(format!("`{repo}` is not a repository name — as it is in document ids, before the `/`")));
    }
    let asked = match (entity, repo) {
        (Some(entity), None) => format!("`{entity}`"),
        (None, Some(repo)) => format!("anything `{repo}`'s documents name"),
        (Some(entity), Some(repo)) => format!("`{entity}` among what `{repo}`'s documents name"),
        (None, None) => {
            return Err(fail("say what to look up: `entity`, a contract such as `topic: tax.events.v1`, or `repo`".into()));
        }
    };

    // ponytail: every header read on each call, no index of its own — hundreds
    // of files take milliseconds. An index when the base has thousands.
    let store = KnowledgeStore::open().map_err(|e| fail(e.to_string()))?;
    let mut all = Vec::new();
    for relative in store.documents().map_err(|e| fail(e.to_string()))? {
        let Some((domain, id)) = knowledge::place_of(&relative) else {
            continue;
        };
        let text = fs::read_to_string(store.dir().join(&relative)).unwrap_or_default();
        if let Some((header, _)) = knowledge::read_header(&text) {
            all.extend(knowledge::relations_of(domain, &id, &header));
        }
    }
    let of_repo: Option<HashSet<_>> =
        repo.map(|repo| all.iter().filter(|r| r.id.split_once('/').is_some_and(|(owner, _)| owner == repo)).map(Relation::key).collect());
    let mut relations: Vec<Relation> = all
        .into_iter()
        .filter(|r| wanted.as_ref().is_none_or(|wanted| wanted.names(r)))
        .filter(|r| of_repo.as_ref().is_none_or(|keys| keys.contains(&r.key())))
        .collect();
    relations.sort_by(|a, b| (a.key(), a.side, &a.domain, &a.id).cmp(&(b.key(), b.side, &b.domain, &b.id)));
    relations.dedup();
    Ok(ToolResult::KnowledgeRelations { asked, relations })
}

/// Where each repository of the base was open last. A list that cannot be
/// read leaves every document unmarked rather than failing the call.
fn repository_roots(store: &KnowledgeStore) -> HashMap<String, PathBuf> {
    store.repos().unwrap_or_default().into_iter().map(|r| (r.name, PathBuf::from(r.path))).collect()
}

/// Whether document `id` is stale, in its repository as it is on this machine.
fn stale_in(roots: &HashMap<String, PathBuf>, staleness: &mut Staleness, id: &str, header: &Header) -> bool {
    let owner = id.split_once('/').map_or(id, |(owner, _)| owner);
    roots.get(owner).is_some_and(|root| staleness.of(root, header))
}

/// `evidence`'s file as git names it — from the root, `/`-separated, no `./`
/// or `..` — which is what staleness compares with the history. `Err` says
/// why it cannot be cited: not a file of the open folder, or not as many
/// lines as it names.
fn evidence_path(scope: &ToolScope, evidence: &Evidence) -> Result<String, KnowledgeError> {
    let uncited = |why: String| KnowledgeError::Uncited { path: evidence.path.clone(), why };
    let path = resolve_existing(scope, &evidence.path).map_err(|_| uncited("no such file in the repository".into()))?;
    if !path.is_file() {
        return Err(uncited("not a file".into()));
    }
    let bytes = fs::read(&path).map_err(|e| uncited(format!("could not be read ({e})")))?;
    let newlines = bytes.iter().filter(|b| **b == b'\n').count();
    let total = newlines + usize::from(!bytes.is_empty() && !bytes.ends_with(b"\n"));
    if !evidence.fits(u32::try_from(total).unwrap_or(u32::MAX)) {
        return Err(uncited(format!("the file has {total} lines")));
    }
    relative_to_root(scope, &path).map_err(|e| uncited(e.to_string()))
}

/// `documents[i]`, with the document's id when its frontmatter names one of
/// this repository — what the model finds it by among a call's documents.
fn shown(i: usize, text: &str, repo: &str) -> String {
    let id = split_frontmatter(text)
        .and_then(|(yaml, _)| yaml_serde::from_str::<yaml_serde::Mapping>(yaml).ok())
        .and_then(|fields| fields.get("id")?.as_str().map(str::to_string))
        .and_then(|id| knowledge::own_slug(&id, repo).ok().map(|slug| format!("{repo}/{slug}")));
    match id {
        Some(id) => format!("documents[{i}] (`{id}`)"),
        None => format!("documents[{i}]"),
    }
}

fn fail(why: impl ToString) -> ToolError {
    ToolError::Knowledge(why.to_string())
}

pub(super) fn search_definition() -> LlmToolDefinition {
    LlmToolDefinition {
        name: "knowledgeSearch".to_string(),
        description: format!(
            "Search the knowledge base: documents agents wrote about the repositories they worked in — services, their endpoints, \
topics and queues, tables, algorithms, how-tos — filed by domain (a subject area such as usn, ausn, fns-exchange). \
Use it before reading another repository's code or guessing how it behaves: when this code calls another service, reads its topic or table, \
or the user asks how something works elsewhere. Write the query the way code names things — identifiers, topic and table names, config keys — \
plus a few domain words, in Russian or English; if nothing fits, reword or try the other language. \
Each hit is one section, with its document's id, domain, kind, confidence and evidence; read more with knowledgeRead. \
The same words can mean different things in different domains: narrow with `domain` when the conversation says which. \
Returns at most {MAX_HITS} sections, {DEFAULT_HITS} by default."
        ),
        parameters: serde_json::json!({
            "type": "object",
            "properties": {
                "query": { "type": "string", "description": "Identifiers and a few words: `RetrySubmissionJob повтор отправки декларации`." },
                "domain": { "type": "string", "description": "Only this domain's documents." },
                "repo": { "type": "string", "description": "Only this repository's documents, by the name in their ids." },
                "kind": { "type": "string", "enum": ["service", "api", "event", "datastore", "concept", "algorithm", "howto"] },
                "limit": { "type": "integer", "description": "How many sections, at most 20." }
            },
            "required": ["query"]
        }),
    }
}

pub(super) fn relations_definition() -> LlmToolDefinition {
    LlmToolDefinition {
        name: "knowledgeRelations".to_string(),
        description: "Who provides and who consumes a contract, across every repository and domain of the knowledge base, from the \
`provides` and `consumes` of their documents: a topic or queue, an endpoint, a table, a storage bucket, a library, an external system. \
Use it when this code calls another service's API or reads a topic, table or bucket it does not own — before guessing who is on the other side. \
`entity` is the contract with its exact name, as code and configuration spell it: `topic: tax.events.v1`, `api: tax-api POST /api/v1/declarations`, \
`db: tax.declarations`, `storage: <bucket>/<prefix>`, `lib: <package>`, `external: <system>`; without the kind, a name of any kind. \
`repo` alone follows every contract that repository's documents name. Returns documents by id; read one with knowledgeRead."
            .to_string(),
        parameters: serde_json::json!({
            "type": "object",
            "properties": {
                "entity": { "type": "string", "description": "`topic: tax.events.v1`" },
                "repo": { "type": "string", "description": "A repository's name, as in document ids: `tax-api`." }
            },
            "required": []
        }),
    }
}

pub(super) fn read_definition() -> LlmToolDefinition {
    LlmToolDefinition {
        name: "knowledgeRead".to_string(),
        description: "Read a knowledge base document by its id (`<repo>/<slug>`, from knowledgeSearch): the whole document with its frontmatter, \
or with `section` only the part under the heading that contains it, subheadings included. Its evidence names files of that repository, \
which may not be the open one."
            .to_string(),
        parameters: serde_json::json!({
            "type": "object",
            "properties": {
                "id": { "type": "string", "description": "`tax-api/tax-events`" },
                "section": { "type": "string", "description": "Words of a heading: `Формат события`." }
            },
            "required": ["id"]
        }),
    }
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
                    // As a model wrote one: a field of its own, no title line, lines past the end.
                    doc("thrift", "general", "src/Api.java#L2-L9").replace("kind:", "slugs: [thrift]\nkind:").replace("# Декларации API\n\n", ""),
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
                "documents[8] (`tax-api/thrift`): `slugs` is not a field",
                "documents[8] (`tax-api/thrift`): the body must start with `# <title>`",
                "documents[8] (`tax-api/thrift`): evidence `src/Api.java`: the file has 3 lines",
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

    const TARIFF: &str = "---\nid: tariff\nkind: algorithm\ntitle: Расчёт тарифа\nevidence:\n  - src/Api.java#L2\nconfidence: inferred\n---\n\
# Расчёт тарифа\n\nКак выбирается тариф.\n\n## Входы\n\nсумма, регион\n\n## Пример\n\n100 → 6\n";

    /// Two documents of `tax-api`: an API in `usn`, an algorithm in `general`.
    fn two_documents(label: &str) {
        let (scope, _) = repo(label);
        let pass = |_: &str, _: &str| Ok(());
        let deps = ToolDeps { check: Some(&pass), ..ToolDeps::default() };
        let call = args(&[doc("declarations-api", "usn", "src/Api.java"), TARIFF.to_string()], &[], &[usn()]);
        knowledge_write(&scope, &call, &deps).unwrap();
    }

    fn found(path: &str, name: Option<&str>, text: &str) -> crate::domain::code_search::CodeMatch {
        crate::domain::code_search::CodeMatch {
            path: path.into(),
            start_line: 1,
            end_line: 9,
            name: name.map(str::to_string),
            text: text.into(),
            source: crate::domain::code_search::MatchSource::Lexical,
        }
    }

    /// A search that answers `matches` and remembers what it was asked.
    fn answering(
        matches: Vec<crate::domain::code_search::CodeMatch>,
    ) -> (crate::domain::tools::CodeSearchFn, std::sync::Arc<std::sync::Mutex<Vec<(usize, SearchFilter)>>>) {
        let asked = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let seen = std::sync::Arc::clone(&asked);
        let search: crate::domain::tools::CodeSearchFn = std::sync::Arc::new(move |_, _, top_k, filter: &SearchFilter| {
            seen.lock().unwrap().push((top_k, filter.clone()));
            let meta = crate::domain::code_search::SearchMeta { tiers_used: vec![], weak: false, hint: None };
            Ok(crate::domain::code_search::CodeSearchResult { matches: matches.clone(), meta })
        });
        (search, asked)
    }

    fn search(args: KnowledgeSearchArgs, with: &crate::domain::tools::CodeSearchFn) -> Result<Vec<KnowledgeHit>, String> {
        let deps = ToolDeps { knowledge: Some(with.clone()), ..ToolDeps::default() };
        match knowledge_search(&args, &deps) {
            Ok(ToolResult::KnowledgeFound { hits, .. }) => Ok(hits),
            Ok(other) => panic!("{other:?}"),
            Err(e) => Err(e.to_string()),
        }
    }

    fn query(text: &str) -> KnowledgeSearchArgs {
        KnowledgeSearchArgs { query: text.into(), ..KnowledgeSearchArgs::default() }
    }

    #[test]
    fn a_hit_is_a_section_with_what_its_document_is() {
        with_app_dir("knowledge-search-hits", || {
            two_documents("knowledge-search-hits-repo");
            let api = kept("tax-api", "declarations-api").unwrap();
            let (with, asked) = answering(vec![
                found("usn/tax-api/declarations-api.md", Some("Декларации API"), &api),
                found("domains.json", None, "[]"),
                found("general/tax-api/gone.md", None, "# Gone"),
                found("general/tax-api/tariff.md", Some("Расчёт тарифа > Входы"), &format!("## Входы\n\n{}", "я".repeat(HIT_CHARS + 10))),
            ]);
            let hits = search(query("декларации"), &with).unwrap();
            assert_eq!(hits.iter().map(|h| h.id.as_str()).collect::<Vec<_>>(), ["tax-api/declarations-api", "tax-api/tariff"]);
            let first = &hits[0];
            assert_eq!((first.domain.as_str(), first.title.as_str(), first.kind), ("usn", "Декларации API", knowledge::Kind::Api));
            assert_eq!(first.confidence, knowledge::Confidence::Confirmed);
            assert_eq!(first.section.as_deref(), Some("Декларации API"));
            assert!(first.text.starts_with("# Декларации API"), "the frontmatter is left out: {}", first.text);
            assert_eq!(first.evidence, ["src/Api.java"]);
            assert_eq!(hits[1].domain, "general");
            assert_eq!(hits[1].text.chars().count(), HIT_CHARS + 1, "cut, with an ellipsis");

            let (top_k, filter) = asked.lock().unwrap()[0].clone();
            assert_eq!(top_k, DEFAULT_HITS);
            assert!(filter.include_docs);
            assert!(filter.paths.allows("usn/tax-api/declarations-api.md") && !filter.paths.allows("domains.json"));
        });
    }

    #[test]
    fn a_search_narrows_by_domain_repository_kind_and_count() {
        with_app_dir("knowledge-search-narrow", || {
            two_documents("knowledge-search-narrow-repo");
            let both = || {
                vec![
                    found("usn/tax-api/declarations-api.md", None, "# Декларации API"),
                    found("general/tax-api/tariff.md", None, "# Расчёт тарифа"),
                ]
            };
            let (with, asked) = answering(both());
            let narrowed = KnowledgeSearchArgs { domain: Some("usn".into()), repo: Some("tax-api".into()), ..query("x") };
            search(narrowed, &with).unwrap();
            let (_, filter) = asked.lock().unwrap().pop().unwrap();
            assert!(filter.paths.allows("usn/tax-api/a.md"));
            assert!(!filter.paths.allows("general/tax-api/a.md") && !filter.paths.allows("usn/usn-api/a.md"));
            search(KnowledgeSearchArgs { repo: Some("usn-api".into()), ..query("x") }, &with).unwrap();
            let (_, filter) = asked.lock().unwrap().pop().unwrap();
            assert!(filter.paths.allows("ausn/usn-api/a.md") && !filter.paths.allows("ausn/tax-api/a.md"));

            let algorithms = KnowledgeSearchArgs { kind: Some(knowledge::Kind::Algorithm), limit: Some(2), ..query("x") };
            let hits = search(algorithms, &with).unwrap();
            assert_eq!(hits.iter().map(|h| h.id.as_str()).collect::<Vec<_>>(), ["tax-api/tariff"]);
            assert_eq!(asked.lock().unwrap().pop().unwrap().0, 6, "more asked for, as the kind drops some");

            let one = search(KnowledgeSearchArgs { limit: Some(1), ..query("x") }, &with).unwrap();
            assert_eq!(one.len(), 1);
            assert_eq!(asked.lock().unwrap().pop().unwrap().0, 1);
            search(KnowledgeSearchArgs { limit: Some(500), ..query("x") }, &with).unwrap();
            assert_eq!(asked.lock().unwrap().pop().unwrap().0, MAX_HITS);
            search(KnowledgeSearchArgs { limit: Some(0), ..query("x") }, &with).unwrap();
            assert_eq!(asked.lock().unwrap().pop().unwrap().0, 1, "at least one");

            // A search the index calls weak says so to the model.
            let weak: crate::domain::tools::CodeSearchFn = std::sync::Arc::new(|_, _, _, _| {
                let meta = crate::domain::code_search::SearchMeta { tiers_used: vec![], weak: true, hint: None };
                Ok(crate::domain::code_search::CodeSearchResult { matches: vec![], meta })
            });
            let deps = ToolDeps { knowledge: Some(weak), ..ToolDeps::default() };
            assert!(matches!(knowledge_search(&query("x"), &deps), Ok(ToolResult::KnowledgeFound { weak: true, .. })));
        });
    }

    #[test]
    fn a_search_that_cannot_be_done_says_why() {
        with_app_dir("knowledge-search-refused", || {
            let (with, asked) = answering(vec![]);
            assert!(search(query(" "), &with).unwrap_err().contains("`query` is empty"));
            assert!(search(KnowledgeSearchArgs { domain: Some("*".into()), ..query("x") }, &with).unwrap_err().contains("not a domain id"));
            assert!(search(KnowledgeSearchArgs { repo: Some("a/b".into()), ..query("x") }, &with).unwrap_err().contains("not a repository name"));
            assert!(asked.lock().unwrap().is_empty(), "nothing searched for a call that is wrong");
            assert_eq!(search(query("x"), &with), Ok(vec![]));
            let none = knowledge_search(&query("x"), &ToolDeps::default()).unwrap_err().to_string();
            assert!(none.contains("not available here"), "{none}");
        });
    }

    #[test]
    fn a_document_is_read_whole_or_by_section() {
        with_app_dir("knowledge-read", || {
            two_documents("knowledge-read-repo");
            let read = |id: &str, section: Option<&str>| {
                knowledge_read(&KnowledgeReadArgs { id: id.into(), section: section.map(str::to_string) }).map_err(|e| e.to_string())
            };
            let ToolResult::KnowledgeDocument { id, domain, section, text, stale } = read("tax-api/tariff", None).unwrap() else {
                panic!()
            };
            assert_eq!((id.as_str(), domain.as_str(), section, stale), ("tax-api/tariff", "general", None, false));
            assert!(text.starts_with("---\nid: tax-api/tariff") && text.contains("## Пример"), "{text}");

            let ToolResult::KnowledgeDocument { domain, section, text, .. } = read(" tax-api/tariff ", Some("входы")).unwrap() else {
                panic!()
            };
            assert_eq!((domain.as_str(), section.as_deref(), text.as_str()), ("general", Some("входы"), "## Входы\n\nсумма, регион"));
            let ToolResult::KnowledgeDocument { domain, .. } = read("tax-api/declarations-api", Some(" ")).unwrap() else { panic!() };
            assert_eq!(domain, "usn", "a blank section is the whole document");

            let missing = read("tax-api/tariff", Some("Выходы")).unwrap_err();
            assert!(missing.contains("no section matches `Выходы` — its sections are: Расчёт тарифа; Входы; Пример"), "{missing}");
            assert!(read("tax-api/nothing", None).unwrap_err().contains("there is no document `tax-api/nothing`"));
            assert!(read("tariff", None).unwrap_err().contains("not a document id"));
            assert!(read("../tariff", None).unwrap_err().contains("not a document id"));
        });
    }

    /// A document put in the base by hand, as another repository's agent left it.
    fn kept_by_hand(relative: &str, title: &str, contracts: &str) {
        let path = base().join(relative);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, format!("---\nkind: event\ntitle: {title}\nconfidence: inferred\n{contracts}---\n# {title}\n")).unwrap();
    }

    fn relations(entity: Option<&str>, repo: Option<&str>) -> Result<(String, Vec<String>), String> {
        let args = KnowledgeRelationsArgs { entity: entity.map(str::to_string), repo: repo.map(str::to_string) };
        match knowledge_relations(&args) {
            Ok(ToolResult::KnowledgeRelations { asked, relations }) => {
                let rows = relations
                    .iter()
                    .map(|r| format!("{}: {} | {:?} {}/{} {}", r.kind.key(), r.name, r.side, r.domain, r.id, r.title))
                    .collect();
                Ok((asked, rows))
            }
            Ok(other) => panic!("{other:?}"),
            Err(e) => Err(e.to_string()),
        }
    }

    #[test]
    fn relations_are_who_provides_and_who_consumes_a_contract_in_every_domain() {
        with_app_dir("knowledge-relations", || {
            two_documents("knowledge-relations-repo");
            kept_by_hand("usn/tax-api/events.md", "События", "provides:\n  - topic: tax.events.v1\nconsumes:\n  - db: tax.declarations\n");
            let ledger = "consumes:\n  - topic: tax.events.v1\n  - api: tax-api  POST /api/v1/declarations\n";
            kept_by_hand("ausn/ledger/listener.md", "Слушатель", ledger);
            kept_by_hand("fns-exchange/gateway/fns.md", "Шлюз", "provides:\n  - external: fns\n  - topic: fns.answers\n");
            fs::write(base().join("ausn/ledger/broken.md"), "consumes: [topic: tax.events.v1]").unwrap();

            let topic = ["topic: tax.events.v1 | Provides usn/tax-api/events События", "topic: tax.events.v1 | Consumes ausn/ledger/listener Слушатель"];
            assert_eq!(relations(Some("topic: tax.events.v1"), None), Ok(("`topic: tax.events.v1`".into(), topic.map(String::from).to_vec())));
            assert_eq!(relations(Some(" tax.events.v1 "), None).unwrap().1, topic, "a name alone, of any kind");

            let (asked, rows) = relations(None, Some("tax-api")).unwrap();
            assert_eq!(asked, "anything `tax-api`'s documents name");
            assert_eq!(
                rows,
                [
                    topic[0],
                    topic[1],
                    "api: tax-api POST /api/v1/declarations | Provides usn/tax-api/declarations-api Декларации API",
                    "api: tax-api  POST /api/v1/declarations | Consumes ausn/ledger/listener Слушатель",
                    "db: tax.declarations | Consumes usn/tax-api/events События",
                ],
                "by contract, the provider first; another repository's side by the same name, spaces aside"
            );
            let (_, both) = relations(Some("api: tax-api POST /api/v1/declarations"), Some("ledger")).unwrap();
            assert_eq!(both.len(), 2);
            assert_eq!(relations(Some("topic: fns.answers"), Some("tax-api")).unwrap().1, Vec::<String>::new(), "not one of tax-api's");

            let (asked, none) = relations(Some("topic: nothing"), None).unwrap();
            assert_eq!((asked.as_str(), none.len()), ("`topic: nothing`", 0));
            assert!(relations(None, Some(" ")).unwrap_err().contains("say what to look up"));
            assert!(relations(Some("topic: "), None).unwrap_err().contains("names nothing to look up"));
            assert!(relations(None, Some("a/b")).unwrap_err().contains("not a repository name"));
        });
    }

    /// Commits the folder's files as they are now.
    fn commit_all(dir: &Path) {
        let git = git2::Repository::open(dir).unwrap();
        let mut index = git.index().unwrap();
        index.add_all(["*"], git2::IndexAddOption::DEFAULT, None).unwrap();
        let tree = git.find_tree(index.write_tree().unwrap()).unwrap();
        let me = git2::Signature::now("t", "t@example.com").unwrap();
        let parent = git.head().ok().and_then(|h| h.peel_to_commit().ok());
        git.commit(Some("HEAD"), &me, &me, "c", &tree, &parent.iter().collect::<Vec<_>>()).unwrap();
    }

    #[test]
    fn a_document_whose_evidence_changed_since_is_stale_where_it_is_found_and_read() {
        with_app_dir("knowledge-stale", || {
            let (scope, dir) = repo("knowledge-stale-repo");
            fs::write(dir.join("src/Job.java"), "j\n").unwrap();
            commit_all(&dir);
            let pass = |_: &str, _: &str| Ok(());
            let deps = ToolDeps { check: Some(&pass), ..ToolDeps::default() };
            let call = args(&[doc("declarations-api", "usn", "src/Job.java"), TARIFF.to_string()], &[], &[usn()]);
            knowledge_write(&scope, &call, &deps).unwrap();
            fs::write(dir.join("src/Api.java"), "changed\n").unwrap();
            commit_all(&dir);

            let (with, _) = answering(vec![
                found("usn/tax-api/declarations-api.md", None, "# Декларации API"),
                found("general/tax-api/tariff.md", None, "# Расчёт тарифа"),
            ]);
            let hits = search(query("x"), &with).unwrap();
            assert_eq!(hits.iter().map(|h| (h.id.as_str(), h.stale)).collect::<Vec<_>>(), [("tax-api/declarations-api", false), ("tax-api/tariff", true)]);
            let read = |id: &str, section: Option<&str>| {
                match knowledge_read(&KnowledgeReadArgs { id: id.into(), section: section.map(str::to_string) }) {
                    Ok(ToolResult::KnowledgeDocument { stale, .. }) => stale,
                    other => panic!("{other:?}"),
                }
            };
            assert!(read("tax-api/tariff", Some("Входы")), "a section of a stale document is stale");
            assert!(!read("tax-api/declarations-api", None));
        });
    }

    /// A file without a final newline still counts its last line.
    #[test]
    fn the_last_line_counts_without_a_newline() {
        let (scope, dir): (ToolScope, PathBuf) = repo("knowledge-tool-lines");
        let at = |text: &str, cited: &str| {
            fs::write(Path::new(&dir).join("src/F.java"), text).unwrap();
            evidence_path(&scope, &knowledge::Evidence::parse(cited).unwrap()).err().map(|e| e.to_string())
        };
        assert_eq!(at("a\nb", "src/F.java#L2"), None);
        assert_eq!(at("a\nb", "src/F.java#L3").as_deref(), Some("evidence `src/F.java`: the file has 2 lines"));
        assert_eq!(at("a\nb\n", "src/F.java#L2"), None);
        assert_eq!(at("a\nb\n", "src/F.java#L3").as_deref(), Some("evidence `src/F.java`: the file has 2 lines"));
        assert_eq!(at("", "src/F.java"), None);
        assert!(at("", "src/F.java#L1").is_some());
    }

    /// Evidence is kept as git names the file, whatever spelling found it:
    /// staleness compares it with the paths a commit changed.
    #[test]
    fn evidence_is_kept_as_the_history_names_it() {
        let (scope, _) = repo("knowledge-tool-spelling");
        let path = |cited: &str| evidence_path(&scope, &knowledge::Evidence::parse(cited).unwrap());
        for cited in ["src/Api.java", "./src/Api.java", "src//Api.java", "./src/./Api.java"] {
            assert_eq!(path(cited).as_deref(), Ok("src/Api.java"), "{cited}");
        }
        assert!(path("../outside.java").is_err());

        with_app_dir("knowledge-tool-spelling-kept", || {
            let check = |_: &str, _: &str| Ok(());
            let deps = ToolDeps { check: Some(&check), ..ToolDeps::default() };
            knowledge_write(&scope, &args(&[doc("api", "general", "./src/Api.java#L1-L3")], &[], &[]), &deps).unwrap();
            let text = kept("tax-api", "api").unwrap();
            assert!(text.contains("- src/Api.java#L1-L3") && !text.contains("./src"), "{text}");
        });
    }
}
