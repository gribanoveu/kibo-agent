import { useState } from "react";
import { FileText, MoreHorizontal, Trash2 } from "lucide-react";
import { Dropdown } from "./Dropdown";
import { Markdown } from "./Markdown";
import { Modal } from "./Modal";
import { Page, PagePanel } from "./Page";
import { useKnowledge } from "../hooks/useKnowledge";
import { documentMarkdown, type DomainCount, type KnowledgeDocument, type KnowledgeDomain, type KnowledgeKind, type RepoSummary } from "../lib/knowledge";
import { matches } from "../lib/shortcuts";
import type { PageContext } from "./pages";
import "./KnowledgePage.css";

/** The kinds in the order a reader meets a service: what it is, then its surfaces, then the rest. */
const KINDS: KnowledgeKind[] = ["service", "api", "event", "datastore", "algorithm", "concept", "howto"];

const GENERAL = "general";

const plural = (n: number, one: string, many: string) => `${n} ${n === 1 ? one : many}`;

const day = (secs: number) => new Date(secs * 1000).toLocaleDateString(undefined, { day: "numeric", month: "short", year: "numeric" });

type Confirm =
  | { what: "document"; doc: KnowledgeDocument }
  | { what: "repo"; repo: RepoSummary }
  | { what: "merge"; from: KnowledgeDomain; into: KnowledgeDomain };

/**
 * The knowledge base (docs/40-knowledge-base.md, decision 9): each domain,
 * the repositories with documents in it, their documents by kind. A document
 * picked opens on the right, where it can go to another domain or be
 * deleted; a domain is renamed, described again or merged into another; a
 * repository's documents go all at once.
 */
export function KnowledgePage({ sideHandle }: PageContext) {
  const [open, setOpen] = useState<string | null>(null);
  const knowledge = useKnowledge(open);
  const { base } = knowledge;
  const [editing, setEditing] = useState<KnowledgeDomain | null>(null);
  const [confirm, setConfirm] = useState<Confirm | null>(null);

  const doc = base.documents.find((d) => d.id === open);
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
      panel={
        doc ? (
          <PagePanel icon={FileText} title={doc.title} label={`Document ${doc.id}`} onClose={() => setOpen(null)}>
            <div className="knowledge-doc-bar">
              <span className="knowledge-doc-id">{doc.id}</span>
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
                <Markdown text={documentMarkdown(knowledge.file)} streaming={false} />
              </div>
            )}
          </PagePanel>
        ) : undefined
      }
    >
      {knowledge.error && <p className="knowledge-error">{knowledge.error}</p>}
      {!knowledge.error && base.documents.length === 0 && (
        <p className="knowledge-empty">
          Nothing written yet. Run /analyze in a repository, or let the agent write down what it works out — documents land here by domain.
        </p>
      )}
      {base.domains
        .filter((d) => d.documents > 0 || d.domain.id !== GENERAL)
        .map((count) => (
          <DomainSection
            key={count.domain.id}
            count={count}
            others={base.domains.filter((d) => d.domain.id !== count.domain.id).map((d) => d.domain)}
            repos={base.repos}
            documents={base.documents.filter((d) => d.domain === count.domain.id)}
            open={open}
            onOpen={setOpen}
            onEdit={() => setEditing({ ...count.domain })}
            onMerge={(into) => setConfirm({ what: "merge", from: count.domain, into })}
            onRemoveRepo={(repo) => setConfirm({ what: "repo", repo })}
          />
        ))}

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

type SectionProps = {
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

function DomainSection({ count, others, repos, documents, open, onOpen, onEdit, onMerge, onRemoveRepo }: SectionProps) {
  const { domain } = count;
  const actions = [
    ...(domain.id === GENERAL ? [] : [{ value: "edit", label: "Rename or describe…", run: onEdit }]),
    ...others.map((into) => ({ value: `merge:${into.id}`, label: `Merge into ${into.name}…`, run: () => onMerge(into) })),
  ];
  const names = [...new Set(documents.map((d) => d.repo))].sort();
  return (
    <section className="knowledge-domain" aria-label={domain.name}>
      <header className="knowledge-domain-head">
        <h2>
          {domain.name}
          <span className="knowledge-domain-id">{domain.id}</span>
        </h2>
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
      <p className="knowledge-domain-description">{domain.description}</p>
      {names.length === 0 && <p className="knowledge-empty">No documents in it.</p>}
      {names.map((name) => {
        const repo = repos.find((r) => r.name === name);
        const own = documents.filter((d) => d.repo === name).sort((a, b) => KINDS.indexOf(a.kind) - KINDS.indexOf(b.kind) || a.title.localeCompare(b.title));
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
            <div role="listbox" aria-label={`${name}'s documents in ${domain.name}`} className="knowledge-docs">
              {own.map((doc) => (
                <button key={doc.id} type="button" role="option" aria-selected={doc.id === open} className={`knowledge-doc${doc.id === open ? " on" : ""}`} onClick={() => onOpen(doc.id)}>
                  <span className="knowledge-kind">{doc.kind}</span>
                  <span className="knowledge-title">{doc.title}</span>
                  <span className="knowledge-marks">
                    {doc.stale && <span className="knowledge-mark stale" title="A file in its evidence changed since it was written">stale</span>}
                    {!doc.cited && <span className="knowledge-mark" title="It cites no file">no evidence</span>}
                    <span className={`knowledge-mark ${doc.confidence}`}>{doc.confidence}</span>
                  </span>
                </button>
              ))}
            </div>
          </div>
        );
      })}
    </section>
  );
}
