import { useState } from "react";
import { ChevronRight, FileText, Folder, MoreHorizontal, Search, Trash2 } from "lucide-react";
import { Dropdown } from "./Dropdown";
import { Markdown } from "./Markdown";
import { Modal } from "./Modal";
import { Page, PageSheet } from "./Page";
import { useKnowledge } from "../hooks/useKnowledge";
import { splitDocument, type DocumentFacts, type DomainCount, type KnowledgeDocument, type KnowledgeDomain, type KnowledgeKind, type RepoSummary } from "../lib/knowledge";
import { matches } from "../lib/shortcuts";
import type { PageContext } from "./pages";
import "./KnowledgePage.css";

/** The kinds in the order a reader meets a service: what it is, then its surfaces, then the rest. */
const KINDS: KnowledgeKind[] = ["service", "api", "event", "datastore", "algorithm", "concept", "howto"];

const KIND_LABELS: Record<KnowledgeKind, string> = {
  service: "Service",
  api: "API",
  event: "Events",
  datastore: "Data stores",
  algorithm: "Flows",
  concept: "Concepts",
  howto: "How-to",
};

/** The frontmatter's lists, in the order they are shown under the text's bar. */
const FACTS: { key: keyof DocumentFacts; label: string }[] = [
  { key: "provides", label: "Provides" },
  { key: "consumes", label: "Consumes" },
  { key: "evidence", label: "Evidence" },
  { key: "aliases", label: "Also called" },
];

const GENERAL = "general";

const plural = (n: number, one: string, many: string) => `${n} ${n === 1 ? one : many}`;

const day = (secs: number) => new Date(secs * 1000).toLocaleDateString(undefined, { day: "numeric", month: "short", year: "numeric" });

type Confirm =
  | { what: "document"; doc: KnowledgeDocument }
  | { what: "repo"; repo: RepoSummary }
  | { what: "merge"; from: KnowledgeDomain; into: KnowledgeDomain };

/**
 * The knowledge base (docs/40-knowledge-base.md, decision 9), as folders: the
 * domains first, and in one its repositories' documents as cards, by kind. A
 * search looks through every domain. A document picked opens on the right,
 * where it can go to another domain or be deleted; a domain is renamed,
 * described again or merged into another; a repository's documents go all at once.
 */
export function KnowledgePage({ sideHandle }: PageContext) {
  const [open, setOpen] = useState<string | null>(null);
  const knowledge = useKnowledge(open);
  const { base } = knowledge;
  const [editing, setEditing] = useState<KnowledgeDomain | null>(null);
  const [confirm, setConfirm] = useState<Confirm | null>(null);
  const [query, setQuery] = useState("");
  const [folder, setFolder] = useState<string | null>(null);

  const doc = base.documents.find((d) => d.id === open);
  const read = splitDocument(knowledge.file);
  const words = query.trim().toLowerCase();
  const found = base.documents.filter((d) => `${d.title} ${d.lead} ${d.id} ${d.kind} ${KIND_LABELS[d.kind]}`.toLowerCase().includes(words));
  // A folder merged away or left empty is no place to stay in.
  const inside = base.domains.find((d) => d.domain.id === folder && (d.documents > 0 || d.domain.id !== GENERAL));
  const folders = base.domains.filter((d) => d.documents > 0 || d.domain.id !== GENERAL);
  const domainName = (id: string) => base.domains.find((d) => d.domain.id === id)?.domain.name ?? id;

  const confirmed = async () => {
    if (!confirm) return;
    const c = confirm;
    setConfirm(null);
    if (c.what === "document" && (await knowledge.remove(c.doc.id))) setOpen(null);
    if (c.what === "repo" && (await knowledge.removeRepo(c.repo.name)) && doc?.repo === c.repo.name) setOpen(null);
    if (c.what === "merge") await knowledge.mergeDomains(c.from.id, c.into.id);
  };
  const saveDomain = async () => {
    if (editing && (await knowledge.editDomain(editing))) setEditing(null);
  };

  return (
    <Page
      title="Knowledge"
      sideHandle={sideHandle}
      bar={
        <>
          <nav className="knowledge-crumbs" aria-label="Where you are">
            <button type="button" aria-current={inside ? undefined : "page"} onClick={() => setFolder(null)}>
              All domains
            </button>
            {inside && (
              <>
                <ChevronRight size={13} className="knowledge-crumb-sep" />
                <button type="button" aria-current="page">
                  {inside.domain.name}
                </button>
              </>
            )}
          </nav>
          {base.documents.length > 0 && (
            <label className="knowledge-search">
              <Search size={14} />
              <input type="search" value={query} onChange={(e) => setQuery(e.target.value)} placeholder="Search every domain" aria-label="Search the knowledge base" />
            </label>
          )}
        </>
      }
      sheet={
        doc ? (
          <PageSheet icon={FileText} title={doc.title} label={`Document ${doc.id}`} onClose={() => setOpen(null)}>
            <div className="knowledge-doc-bar">
              <span className="knowledge-doc-meta">
                {KIND_LABELS[doc.kind]} · {doc.confidence}
                {doc.written && ` · ${day(doc.written)}`}
                {doc.stale && <span className="knowledge-mark stale">stale</span>}
              </span>
              <Dropdown
                label={`Domain: ${domainName(doc.domain)}`}
                title="Move it to another domain"
                heading="Move to"
                options={base.domains.map((d) => ({ value: d.domain.id, label: d.domain.name, hint: d.domain.description }))}
                value={doc.domain}
                onPick={(to) => to !== doc.domain && void knowledge.move(doc.id, to)}
                below
                right
              />
              <button type="button" className="knowledge-icon-button" title="Delete document" aria-label="Delete document" onClick={() => setConfirm({ what: "document", doc })}>
                <Trash2 size={14} />
              </button>
            </div>
            {knowledge.fileError ? (
              <p className="knowledge-error">{knowledge.fileError}</p>
            ) : (
              <div className="knowledge-doc-text">
                {FACTS.filter(({ key }) => read.facts[key].length > 0).map(({ key, label }) => (
                  <details key={key} className="knowledge-facts">
                    <summary>
                      {label} <span className="knowledge-count">{read.facts[key].length}</span>
                    </summary>
                    <ul>
                      {read.facts[key].map((item) => (
                        <li key={item}>{item}</li>
                      ))}
                    </ul>
                  </details>
                ))}
                <Markdown text={read.body} streaming={false} />
                <p className="knowledge-doc-id">{doc.id}</p>
              </div>
            )}
          </PageSheet>
        ) : undefined
      }
    >
      {knowledge.error && <p className="knowledge-error">{knowledge.error}</p>}
      {!knowledge.error && base.documents.length === 0 && (
        <p className="knowledge-empty">
          Nothing written yet. Run /analyze in a repository, or let the agent write down what it works out — documents land here by domain.
        </p>
      )}
      {words ? (
        <>
          {found.length === 0 && <p className="knowledge-empty">Nothing matches “{query.trim()}”.</p>}
          {base.domains
            .filter((d) => found.some((doc) => doc.domain === d.domain.id))
            .map(({ domain }) => (
              <section key={domain.id} className="knowledge-domain" aria-label={domain.name}>
                <h2 className="knowledge-found-head">{domain.name}</h2>
                <Cards label={`Found in ${domain.name}`} documents={found.filter((d) => d.domain === domain.id)} open={open} onOpen={setOpen} withKind />
              </section>
            ))}
        </>
      ) : inside ? (
        <DomainView
          count={inside}
          others={base.domains.filter((d) => d.domain.id !== inside.domain.id).map((d) => d.domain)}
          repos={base.repos}
          documents={base.documents.filter((d) => d.domain === inside.domain.id)}
          open={open}
          onOpen={setOpen}
          onEdit={() => setEditing({ ...inside.domain })}
          onMerge={(into) => setConfirm({ what: "merge", from: inside.domain, into })}
          onRemoveRepo={(repo) => setConfirm({ what: "repo", repo })}
        />
      ) : (
        base.documents.length > 0 && (
          <div className="knowledge-folders">
            {folders.map(({ domain, documents, repos }) => (
              <button key={domain.id} type="button" className="knowledge-folder" onClick={() => setFolder(domain.id)}>
                <span className="knowledge-folder-name">
                  <Folder size={16} className="knowledge-folder-icon" />
                  {domain.name}
                </span>
                <span className="knowledge-folder-description">{domain.description}</span>
                <span className="knowledge-count">
                  {plural(documents, "document", "documents")} · {plural(repos, "repository", "repositories")}
                </span>
              </button>
            ))}
          </div>
        )
      )}

      <Modal
        title={`Edit “${editing?.id ?? ""}”`}
        open={editing !== null}
        onClose={() => setEditing(null)}
        footer={
          <>
            <button type="button" className="btn btn-ghost" onClick={() => setEditing(null)}>
              Cancel
            </button>
            <button type="button" className="btn btn-primary" disabled={!editing?.name.trim() || !editing?.description.trim()} onClick={() => void saveDomain()}>
              Save
            </button>
          </>
        }
      >
        <label className="knowledge-field">
          <span>Name</span>
          <input
            autoFocus
            value={editing?.name ?? ""}
            onChange={(e) => editing && setEditing({ ...editing, name: e.target.value })}
            // A form field's Enter, as in every settings form here — not a shortcut.
            onKeyDown={(e) => {
              if (!matches(e, "accept")) return;
              e.preventDefault();
              void saveDomain();
            }}
          />
        </label>
        <label className="knowledge-field">
          <span>Description — the agent files documents by it</span>
          <textarea rows={3} value={editing?.description ?? ""} onChange={(e) => editing && setEditing({ ...editing, description: e.target.value })} />
        </label>
      </Modal>
      <Modal
        title={confirm === null ? "" : confirm.what === "merge" ? `Merge “${confirm.from.name}” into “${confirm.into.name}”?` : "Delete?"}
        open={confirm !== null}
        onClose={() => setConfirm(null)}
        footer={
          <>
            <button type="button" className="btn btn-ghost" onClick={() => setConfirm(null)}>
              Cancel
            </button>
            <button type="button" className="btn btn-primary" onClick={() => void confirmed()}>
              {confirm?.what === "merge" ? "Merge" : "Delete"}
            </button>
          </>
        }
      >
        <p className="knowledge-confirm">
          {confirm?.what === "document" && `Deletes “${confirm.doc.title}” (${confirm.doc.id}). Agents stop finding it; /analyze can write it again.`}
          {confirm?.what === "repo" &&
            `Deletes every document of ${confirm.repo.name} — ${plural(confirm.repo.documents, "document", "documents")}, in every domain — and the base's record of it.`}
          {confirm?.what === "merge" &&
            `Moves every document of “${confirm.from.name}” into “${confirm.into.name}”, and removes “${confirm.from.name}” from the list.`}
        </p>
      </Modal>
    </Page>
  );
}

type ViewProps = {
  count: DomainCount;
  /** The domains it can be merged into. */
  others: KnowledgeDomain[];
  repos: RepoSummary[];
  documents: KnowledgeDocument[];
  open: string | null;
  onOpen: (id: string) => void;
  onEdit: () => void;
  onMerge: (into: KnowledgeDomain) => void;
  onRemoveRepo: (repo: RepoSummary) => void;
};

/** One domain opened: what it is about, then each repository's documents by kind. */
function DomainView({ count, others, repos, documents, open, onOpen, onEdit, onMerge, onRemoveRepo }: ViewProps) {
  const { domain } = count;
  const actions = [
    ...(domain.id === GENERAL ? [] : [{ value: "edit", label: "Rename or describe…", run: onEdit }]),
    ...others.map((into) => ({ value: `merge:${into.id}`, label: `Merge into ${into.name}…`, run: () => onMerge(into) })),
  ];
  const names = [...new Set(documents.map((d) => d.repo))].sort();
  return (
    <section className="knowledge-domain" aria-label={domain.name}>
      <header className="knowledge-domain-head">
        <p className="knowledge-domain-description">{domain.description}</p>
        <span className="knowledge-count">
          {plural(count.documents, "document", "documents")} · {plural(count.repos, "repository", "repositories")}
        </span>
        {actions.length > 0 && (
          <Dropdown
            label={<MoreHorizontal size={14} aria-label={`Actions for ${domain.name}`} />}
            title={`Actions for ${domain.name}`}
            options={actions.map(({ value, label }) => ({ value, label }))}
            value=""
            onPick={(value) => actions.find((a) => a.value === value)?.run()}
            below
            right
          />
        )}
      </header>
      {names.length === 0 && <p className="knowledge-empty">No documents in it.</p>}
      {names.map((name) => {
        const repo = repos.find((r) => r.name === name);
        const own = documents.filter((d) => d.repo === name);
        return (
          <div key={name} className="knowledge-repo">
            <div className="knowledge-repo-head">
              <h3 title={repo?.path}>{name}</h3>
              <span className="knowledge-count">
                {repo && plural(repo.documents, "document", "documents")}
                {repo && repo.stale > 0 && ` · ${repo.stale} stale`}
                {repo?.written && ` · written ${day(repo.written)}`}
              </span>
              {repo && (
                <button type="button" className="knowledge-icon-button" title={`Delete ${name}'s documents`} aria-label={`Delete ${name}'s documents`} onClick={() => onRemoveRepo(repo)}>
                  <Trash2 size={13} />
                </button>
              )}
            </div>
            {KINDS.map((kind) => {
              const ofKind = own.filter((d) => d.kind === kind);
              return (
                ofKind.length > 0 && (
                  <div key={kind} className="knowledge-kind">
                    <h4>
                      {KIND_LABELS[kind]} <span className="knowledge-count">{ofKind.length}</span>
                    </h4>
                    <Cards label={`${KIND_LABELS[kind]} of ${name} in ${domain.name}`} documents={ofKind} open={open} onOpen={onOpen} />
                  </div>
                )
              );
            })}
          </div>
        );
      })}
    </section>
  );
}

type CardsProps = {
  label: string;
  documents: KnowledgeDocument[];
  open: string | null;
  onOpen: (id: string) => void;
  /** Say each one's kind: it is not the heading over them. */
  withKind?: boolean;
};

/** Documents as cards: the title in full, what the document opens with, and only what is off about it. */
function Cards({ label, documents, open, onOpen, withKind }: CardsProps) {
  return (
    <div role="listbox" aria-label={label} className="knowledge-cards">
      {[...documents]
        .sort((a, b) => a.title.localeCompare(b.title))
        .map((doc) => (
          <button key={doc.id} type="button" role="option" aria-selected={doc.id === open} className={`knowledge-card${doc.id === open ? " on" : ""}`} onClick={() => onOpen(doc.id)}>
            {withKind && <span className="knowledge-card-kind">{KIND_LABELS[doc.kind]}</span>}
            <span className="knowledge-title">{doc.title}</span>
            {doc.lead && <span className="knowledge-lead">{doc.lead}</span>}
            {(doc.stale || !doc.cited || doc.confidence !== "confirmed") && (
              <span className="knowledge-marks">
                {doc.stale && <span className="knowledge-mark stale" title="A file in its evidence changed since it was written">stale</span>}
                {!doc.cited && <span className="knowledge-mark" title="It cites no file">no evidence</span>}
                {doc.confidence !== "confirmed" && (
                  <span className="knowledge-mark" title="Not every claim in it is backed by the code it cites">
                    {doc.confidence}
                  </span>
                )}
              </span>
            )}
          </button>
        ))}
    </div>
  );
}
