import { useEffect, useState, type KeyboardEvent, type ReactNode } from "react";
import { Check, ChevronDown, ChevronLeft, ChevronRight, Copy, CopyPlus, Database, Eye, MoreHorizontal, Pencil, Play, Plus, RefreshCw, Settings2, Table, Trash2 } from "lucide-react";
import { Dropdown } from "./Dropdown";
import { Modal } from "./Modal";
import { Page } from "./Page";
import { Switch } from "./Switch";
import { Tabs } from "./Tabs";
import { useCopied } from "../hooks/useCopied";
import { useMongoAggregation, useMongoConnections, useMongoDocuments, useMongoIndexes, useMongoTree } from "../hooks/useMongo";
import { useStoredState } from "../hooks/useStoredState";
import { sizeLabel } from "../lib/prototype";
import { pageRange, withoutId, type FindRequest, type IndexView, type MongoConnection, type MongoDocument, type MongoTarget } from "../lib/mongo";
import { matches } from "../lib/shortcuts";
import type { PageContext } from "./pages";
import "./MongoPage.css";

type View = "documents" | "aggregations" | "indexes";

/** The connection form: a new one, or a saved one changed — its URI is never shown, only typed again. */
type Form = { id?: string; name: string; uri: string; saved?: string };

/** A step that cannot be taken back, asked about first. */
type Confirm = { what: "connection"; connection: MongoConnection } | { what: "collection"; database: string; collection: string };

const isId = (value: unknown): value is string | null => value === null || typeof value === "string";

const LIMITS = [10, 20, 50, 100];

/** Documents past this many lines are folded until asked for. */
const FOLDED_LINES = 14;

/**
 * MongoDB, in the spirit of Compass but only what is used every day
 * (docs/43-mongodb.md): a saved connection's databases and collections on the
 * left; on the right a collection's documents — found with a filter, a sort
 * and a projection, edited, added, deleted — its aggregations and its indexes.
 * Query text is mongosh's. Connection strings are sealed in the backend; this
 * page only ever sees them masked.
 */
export function MongoPage(_: PageContext) {
  const store = useMongoConnections();
  const [chosen, setChosen] = useStoredState<string | null>("mongo.connection", null, isId);
  const connection = store.connections.find((c) => c.id === chosen) ?? store.connections[0] ?? null;
  const tree = useMongoTree(connection?.id ?? null);
  const [selected, setSelected] = useState<{ database: string; collection: string } | null>(null);
  const [view, setView] = useState<View>("documents");
  const [form, setForm] = useState<Form | null>(null);
  const [creating, setCreating] = useState<{ database: string; collection: string } | null>(null);
  const [confirm, setConfirm] = useState<Confirm | null>(null);

  useEffect(() => setSelected(null), [connection?.id]);
  const target: MongoTarget | null = connection && selected ? { connection: connection.id, ...selected } : null;

  const createCollection = async () => {
    if (creating && (await tree.createCollection(creating.database.trim(), creating.collection.trim()))) {
      setSelected({ database: creating.database.trim(), collection: creating.collection.trim() });
      setCreating(null);
    }
  };

  const confirmed = async (c: Confirm) => {
    if (c.what === "connection" && (await store.remove(c.connection.id))) setForm(null);
    if (c.what === "collection" && (await tree.dropCollection(c.database, c.collection))) setSelected(null);
  };

  return (
    <Page
      title="MongoDB"
      bar={
        store.connections.length > 0 && (
          <>
            <Dropdown
              label={
                <span className="mongo-connection-label">
                  <Database size={14} />
                  {connection?.name ?? "Connection"}
                </span>
              }
              title={connection?.address}
              heading="Connection"
              options={store.connections.map((c) => ({ value: c.id, label: c.name, hint: c.address }))}
              value={connection?.id ?? ""}
              onPick={setChosen}
              below
            />
            {connection && (
              <button type="button" className="btn btn-ghost" title="Change this connection" onClick={() => setForm({ id: connection.id, name: connection.name, uri: "", saved: connection.address })}>
                <Settings2 size={14} />
                Edit
              </button>
            )}
            <button type="button" className="btn btn-ghost" onClick={() => setForm({ name: "", uri: "" })}>
              <Plus size={14} />
              Connection
            </button>
          </>
        )
      }
    >
      {store.error && <p className="mongo-error">{store.error}</p>}
      {store.connections.length === 0 ? (
        <div className="mongo-welcome">
          <Database size={28} className="mongo-welcome-icon" />
          <p>No connections yet. Add one with its connection string — it is sealed on this computer as the API keys are, and this window never sees the password again.</p>
          <button type="button" className="btn btn-primary" onClick={() => setForm({ name: "", uri: "" })}>
            <Plus size={14} />
            Add connection
          </button>
        </div>
      ) : (
        <div className="mongo-layout">
          <nav className="mongo-tree" aria-label="Databases">
            <div className="mongo-tree-head">
              <span>Databases</span>
              <button type="button" className="mongo-icon-button" title="New collection" aria-label="New collection" onClick={() => setCreating({ database: selected?.database ?? "", collection: "" })}>
                <Plus size={14} />
              </button>
              <button type="button" className="mongo-icon-button" title="Read again" aria-label="Read the databases again" disabled={tree.busy} onClick={() => void tree.refresh()}>
                <RefreshCw size={13} />
              </button>
            </div>
            {tree.error && <p className="mongo-error">{tree.error}</p>}
            {!tree.error && tree.busy && tree.databases.length === 0 && <p className="mongo-quiet">Connecting…</p>}
            <ul className="mongo-tree-list">
              {tree.databases.map((db) => {
                const open = tree.open.includes(db.name);
                return (
                  <li key={db.name}>
                    <button type="button" className="mongo-tree-db" aria-expanded={open} onClick={() => tree.toggle(db.name)}>
                      {open ? <ChevronDown size={13} /> : <ChevronRight size={13} />}
                      <Database size={13} className="mongo-tree-icon" />
                      <span className="mongo-tree-name">{db.name}</span>
                      <span className="mongo-quiet">{sizeLabel(db.sizeOnDisk)}</span>
                    </button>
                    {open && (
                      <ul className="mongo-tree-list" aria-label={`Collections of ${db.name}`}>
                        {(tree.collections[db.name] ?? []).map((c) => {
                          const on = selected?.database === db.name && selected.collection === c.name;
                          return (
                            <li key={c.name}>
                              <button
                                type="button"
                                className={`mongo-tree-collection${on ? " on" : ""}`}
                                aria-current={on ? "true" : undefined}
                                onClick={() => setSelected({ database: db.name, collection: c.name })}
                              >
                                {c.kind === "view" ? <Eye size={13} className="mongo-tree-icon" /> : <Table size={13} className="mongo-tree-icon" />}
                                <span className="mongo-tree-name">{c.name}</span>
                                {c.kind !== "collection" && <span className="mongo-kind">{c.kind}</span>}
                              </button>
                            </li>
                          );
                        })}
                        {tree.collections[db.name]?.length === 0 && <li className="mongo-quiet mongo-tree-empty">No collections</li>}
                      </ul>
                    )}
                  </li>
                );
              })}
            </ul>
          </nav>

          <section className="mongo-main" aria-label={target ? `${target.database}.${target.collection}` : "Collection"}>
            {target ? (
              <>
                <header className="mongo-main-head">
                  <h2 className="mongo-namespace">
                    <span className="mongo-quiet">{target.database}.</span>
                    {target.collection}
                  </h2>
                  <Tabs<View>
                    label="What to show"
                    tabs={[
                      { id: "documents", label: "Documents" },
                      { id: "aggregations", label: "Aggregations" },
                      { id: "indexes", label: "Indexes" },
                    ]}
                    value={view}
                    onChange={setView}
                  />
                  <Dropdown
                    label={<MoreHorizontal size={14} aria-label="Collection actions" />}
                    title="Collection actions"
                    options={[{ value: "drop", label: "Drop collection…", hint: "Its documents and indexes go with it" }]}
                    value=""
                    onPick={() => setConfirm({ what: "collection", ...target })}
                    below
                    right
                  />
                </header>
                {view === "documents" && <Documents target={target} />}
                {view === "aggregations" && <Aggregations target={target} />}
                {view === "indexes" && <Indexes target={target} />}
              </>
            ) : (
              <p className="mongo-quiet mongo-pick">{tree.databases.length > 0 ? "Pick a collection on the left." : ""}</p>
            )}
          </section>
        </div>
      )}

      <ConnectionForm form={form} onChange={setForm} store={store} onSaved={(id) => setChosen(id)} onDelete={(c) => setConfirm({ what: "connection", connection: c })} />

      <Modal
        title="New collection"
        open={creating !== null}
        onClose={() => setCreating(null)}
        footer={
          <>
            <button type="button" className="btn btn-ghost" onClick={() => setCreating(null)}>
              Cancel
            </button>
            <button type="button" className="btn btn-primary" disabled={!creating?.database.trim() || !creating?.collection.trim() || tree.busy} onClick={() => void createCollection()}>
              Create
            </button>
          </>
        }
      >
        <label className="mongo-field">
          <span>Database — a new name makes a new one</span>
          <input autoFocus spellCheck={false} value={creating?.database ?? ""} onChange={(e) => creating && setCreating({ ...creating, database: e.target.value })} />
        </label>
        <label className="mongo-field">
          <span>Collection</span>
          <input
            spellCheck={false}
            value={creating?.collection ?? ""}
            onChange={(e) => creating && setCreating({ ...creating, collection: e.target.value })}
            onKeyDown={(e) => {
              if (!matches(e, "accept")) return;
              e.preventDefault();
              void createCollection();
            }}
          />
        </label>
        {tree.error && <p className="mongo-error">{tree.error}</p>}
      </Modal>

      <ConfirmModal
        title={confirm?.what === "connection" ? `Delete “${confirm.connection.name}”?` : confirm?.what === "collection" ? `Drop ${confirm.database}.${confirm.collection}?` : ""}
        action={confirm?.what === "collection" ? "Drop" : "Delete"}
        subject={confirm}
        onClose={() => setConfirm(null)}
        onConfirm={(c) => void confirmed(c)}
      >
        <p className="mongo-confirm">
          {confirm?.what === "connection" && `Forgets the connection and its saved connection string. Nothing on the server changes.`}
          {confirm?.what === "collection" && `Deletes every document of ${confirm.collection} and its indexes, on the server. This cannot be undone.`}
        </p>
      </ConfirmModal>
    </Page>
  );
}

/** A step that cannot be taken back: open while `subject` is set; Cancel, or the one button that takes it. */
function ConfirmModal<T>({ title, action, subject, onClose, onConfirm, children }: { title: string; action: string; subject: T | null; onClose: () => void; onConfirm: (subject: T) => void; children: ReactNode }) {
  return (
    <Modal
      title={title}
      open={subject !== null}
      onClose={onClose}
      footer={
        <>
          <button type="button" className="btn btn-ghost" onClick={onClose}>
            Cancel
          </button>
          <button
            type="button"
            className="btn btn-primary"
            onClick={() => {
              onClose();
              if (subject !== null) onConfirm(subject);
            }}
          >
            {action}
          </button>
        </>
      }
    >
      {children}
    </Modal>
  );
}

type FormProps = {
  form: Form | null;
  onChange: (form: Form | null) => void;
  store: ReturnType<typeof useMongoConnections>;
  onSaved: (id: string) => void;
  onDelete: (connection: MongoConnection) => void;
};

/** Add or change a connection: a name and a connection string, tested before or after saving. */
function ConnectionForm({ form, onChange, store, onSaved, onDelete }: FormProps) {
  const [tested, setTested] = useState<{ ok: boolean; text: string } | null>(null);
  const [testing, setTesting] = useState(false);
  useEffect(() => {
    setTested(null);
    store.clearError();
  }, [form?.id, form === null]);

  if (!form) return null;
  const draft = { id: form.id, name: form.name, uri: form.uri.trim() || undefined };
  const editing = form.id !== undefined;

  const test = async () => {
    setTesting(true);
    const result = await store.test(draft);
    setTesting(false);
    setTested(result.ok ? { ok: true, text: `Connected — MongoDB ${result.version}` } : { ok: false, text: result.error });
  };
  const save = async () => {
    const before = new Set(store.connections.map((c) => c.id));
    const list = await store.save(draft);
    if (!list) return;
    onSaved(form.id ?? list.find((c) => !before.has(c.id))?.id ?? list[list.length - 1].id);
    onChange(null);
  };
  const saved = store.connections.find((c) => c.id === form.id);

  return (
    <Modal
      title={editing ? `Edit “${saved?.name ?? form.name}”` : "New connection"}
      open
      onClose={() => onChange(null)}
      footer={
        <>
          {editing && saved && (
            <button type="button" className="btn btn-ghost mongo-foot-left" onClick={() => onDelete(saved)}>
              <Trash2 size={14} />
              Delete
            </button>
          )}
          <button type="button" className="btn btn-ghost" disabled={testing || (!editing && !draft.uri)} onClick={() => void test()}>
            {testing ? "Testing…" : "Test"}
          </button>
          <button type="button" className="btn btn-ghost" onClick={() => onChange(null)}>
            Cancel
          </button>
          <button type="button" className="btn btn-primary" disabled={!form.name.trim() || (!editing && !draft.uri)} onClick={() => void save()}>
            Save
          </button>
        </>
      }
    >
      <label className="mongo-field">
        <span>Name</span>
        <input autoFocus value={form.name} placeholder="Local, Staging…" onChange={(e) => onChange({ ...form, name: e.target.value })} />
      </label>
      <label className="mongo-field">
        <span>Connection string{editing ? " — leave empty to keep the saved one" : ""}</span>
        <input
          className="mongo-mono"
          type="password"
          spellCheck={false}
          autoComplete="off"
          value={form.uri}
          placeholder={form.saved ?? "mongodb://user:password@localhost:27017/?authSource=admin"}
          onChange={(e) => {
            setTested(null);
            onChange({ ...form, uri: e.target.value });
          }}
          onKeyDown={(e) => {
            if (!matches(e, "accept")) return;
            e.preventDefault();
            void save();
          }}
        />
      </label>
      <p className="mongo-quiet">Sealed on this computer under the app's master key, as the API keys are. Only the host and the user name are shown again.</p>
      {tested && <p className={tested.ok ? "mongo-ok" : "mongo-error"}>{tested.text}</p>}
      {store.error && <p className="mongo-error">{store.error}</p>}
    </Modal>
  );
}

/** A collection's documents: the query bar, a page of results, and the editor. */
function Documents({ target }: { target: MongoTarget }) {
  const docs = useMongoDocuments(target);
  const [draft, setDraft] = useState<FindRequest>(docs.request);
  const [options, setOptions] = useState(false);
  const [editor, setEditor] = useState<{ id?: unknown; text: string } | null>(null);
  const [deleting, setDeleting] = useState<MongoDocument | null>(null);
  useEffect(() => setDraft(docs.request), [docs.request]);

  const run = (next: Partial<FindRequest> = {}) => void docs.find({ ...draft, skip: 0, ...next });
  const onEnter = (e: KeyboardEvent) => {
    if (!matches(e, "accept")) return;
    e.preventDefault();
    run();
  };
  const page = docs.page;
  const { skip, limit } = docs.request;
  const saveEditor = async () => {
    if (!editor) return;
    const done = editor.id !== undefined ? await docs.replace(editor.id, editor.text) : await docs.insert(editor.text);
    if (done) setEditor(null);
  };

  return (
    <div className="mongo-view">
      <div className="mongo-query">
        <QueryField label="Filter" value={draft.filter} placeholder="{ field: 'value' }" onChange={(filter) => setDraft({ ...draft, filter })} onKeyDown={onEnter} />
        <button type="button" className="btn btn-ghost" aria-expanded={options} onClick={() => setOptions(!options)}>
          Options
          {options ? <ChevronDown size={13} /> : <ChevronRight size={13} />}
        </button>
        <button type="button" className="btn btn-ghost" disabled={docs.busy} onClick={() => setDraft({ ...draft, filter: "", projection: "", sort: "" })}>
          Reset
        </button>
        <button type="button" className="btn btn-primary" disabled={docs.busy} onClick={() => run()}>
          Find
        </button>
      </div>
      {options && (
        <div className="mongo-query mongo-query-options">
          <QueryField label="Project" value={draft.projection} placeholder="{ field: 1 }" onChange={(projection) => setDraft({ ...draft, projection })} onKeyDown={onEnter} />
          <QueryField label="Sort" value={draft.sort} placeholder="{ field: -1 }" onChange={(sort) => setDraft({ ...draft, sort })} onKeyDown={onEnter} />
        </div>
      )}

      <div className="mongo-toolbar">
        <button type="button" className="btn btn-ghost" onClick={() => setEditor({ text: "{\n  \n}" })}>
          <Plus size={14} />
          Insert document
        </button>
        <span className="mongo-spacer" />
        {page && <span className="mongo-quiet">{pageRange(skip, page.documents.length, page.total)}</span>}
        <Dropdown
          label={`${limit} per page`}
          title="How many to show"
          options={LIMITS.map((n) => ({ value: String(n), label: `${n} per page` }))}
          value={String(limit)}
          onPick={(value) => run({ limit: Number(value) })}
          below
          right
        />
        <button type="button" className="mongo-icon-button" title="Previous page" aria-label="Previous page" disabled={docs.busy || skip === 0} onClick={() => void docs.find({ ...docs.request, skip: Math.max(0, skip - limit) })}>
          <ChevronLeft size={14} />
        </button>
        <button
          type="button"
          className="mongo-icon-button"
          title="Next page"
          aria-label="Next page"
          disabled={docs.busy || !page || skip + page.documents.length >= page.total}
          onClick={() => void docs.find({ ...docs.request, skip: skip + limit })}
        >
          <ChevronRight size={14} />
        </button>
        <button type="button" className="mongo-icon-button" title="Find again" aria-label="Find again" disabled={docs.busy} onClick={() => void docs.find(docs.request)}>
          <RefreshCw size={13} />
        </button>
      </div>

      {docs.error && !editor && <p className="mongo-error">{docs.error}</p>}
      {page && page.documents.length === 0 && !docs.error && <p className="mongo-quiet">No documents match.</p>}
      <div className="mongo-documents">
        {page?.documents.map((doc, i) => (
          <DocumentCard
            key={`${skip}-${i}-${doc.text.length}`}
            doc={doc}
            actions={
              doc.id !== null && (
                <>
                  <button type="button" className="mongo-icon-button" title="Edit" aria-label="Edit document" onClick={() => setEditor({ id: doc.id, text: doc.text })}>
                    <Pencil size={13} />
                  </button>
                  <button type="button" className="mongo-icon-button" title="Clone — insert a copy with a new _id" aria-label="Clone document" onClick={() => setEditor({ text: withoutId(doc.text) })}>
                    <CopyPlus size={13} />
                  </button>
                  <button type="button" className="mongo-icon-button" title="Delete" aria-label="Delete document" onClick={() => setDeleting(doc)}>
                    <Trash2 size={13} />
                  </button>
                </>
              )
            }
          />
        ))}
      </div>

      <Modal
        title={editor?.id !== undefined ? "Edit document" : "Insert document"}
        open={editor !== null}
        onClose={() => setEditor(null)}
        wide
        footer={
          <>
            <button type="button" className="btn btn-ghost" onClick={() => setEditor(null)}>
              Cancel
            </button>
            <button type="button" className="btn btn-primary" disabled={docs.busy} onClick={() => void saveEditor()}>
              {editor?.id !== undefined ? "Replace" : "Insert"}
            </button>
          </>
        }
      >
        <p className="mongo-quiet">
          {editor?.id !== undefined ? "The whole document is replaced with this one; its _id stays." : "One document, or an array of them. Without an _id, one is made."} Write it as mongosh does: ObjectId('…'), ISODate('…'), NumberLong(…).
        </p>
        <textarea
          className="mongo-editor mongo-mono"
          autoFocus
          spellCheck={false}
          rows={18}
          value={editor?.text ?? ""}
          onChange={(e) => editor && setEditor({ ...editor, text: e.target.value })}
          onKeyDown={(e) => {
            if (!matches(e, "mongoRun")) return;
            e.preventDefault();
            void saveEditor();
          }}
        />
        {docs.error && <p className="mongo-error">{docs.error}</p>}
      </Modal>
      <ConfirmModal title="Delete this document?" action="Delete" subject={deleting} onClose={() => setDeleting(null)} onConfirm={(doc) => void docs.remove(doc.id)}>
        <pre className="mongo-document-text mongo-mono mongo-confirm-doc">{deleting?.text.split("\n").slice(0, 8).join("\n")}</pre>
      </ConfirmModal>
    </div>
  );
}

function QueryField({ label, value, placeholder, onChange, onKeyDown }: { label: string; value: string; placeholder: string; onChange: (value: string) => void; onKeyDown: (e: KeyboardEvent) => void }) {
  return (
    <label className="mongo-query-field">
      <span>{label}</span>
      <input className="mongo-mono" spellCheck={false} value={value} placeholder={placeholder} onChange={(e) => onChange(e.target.value)} onKeyDown={onKeyDown} />
    </label>
  );
}

/** A document as mongosh shows it, folded past a few lines, with its actions on the corner. */
function DocumentCard({ doc, actions }: { doc: MongoDocument; actions?: ReactNode }) {
  const lines = doc.text.split("\n");
  const long = lines.length > FOLDED_LINES;
  const [unfolded, setUnfolded] = useState(false);
  const { copied, copy } = useCopied();
  return (
    <article className="mongo-document">
      <div className="mongo-document-actions">
        <button type="button" className="mongo-icon-button" title="Copy" aria-label="Copy document" onClick={() => copy(doc.text)}>
          {copied ? <Check size={13} /> : <Copy size={13} />}
        </button>
        {actions}
      </div>
      <pre className="mongo-document-text mongo-mono">{long && !unfolded ? lines.slice(0, FOLDED_LINES).join("\n") : doc.text}</pre>
      {long && (
        <button type="button" className="mongo-unfold" onClick={() => setUnfolded(!unfolded)}>
          {unfolded ? "Show less" : `Show all ${lines.length} lines`}
        </button>
      )}
    </article>
  );
}

const PIPELINE = "[\n  { $match: {} },\n  { $limit: 20 }\n]";

/** A pipeline run on the collection; its results read-only. */
function Aggregations({ target }: { target: MongoTarget }) {
  const aggregation = useMongoAggregation(target);
  const [pipeline, setPipeline] = useState(PIPELINE);
  const run = () => void aggregation.run(pipeline);
  return (
    <div className="mongo-view">
      <label className="mongo-field">
        <span>Pipeline — an array of stages, as in mongosh</span>
        <textarea
          className="mongo-editor mongo-mono"
          spellCheck={false}
          rows={8}
          value={pipeline}
          onChange={(e) => setPipeline(e.target.value)}
          onKeyDown={(e) => {
            if (!matches(e, "mongoRun")) return;
            e.preventDefault();
            run();
          }}
        />
      </label>
      <div className="mongo-toolbar">
        <button type="button" className="btn btn-primary" disabled={aggregation.busy} onClick={run}>
          <Play size={13} />
          Run
        </button>
        <span className="mongo-spacer" />
        {aggregation.results && (
          <span className="mongo-quiet">
            {aggregation.results.length === 200 ? "The first 200 results" : `${aggregation.results.length} ${aggregation.results.length === 1 ? "result" : "results"}`}
          </span>
        )}
      </div>
      {aggregation.error && <p className="mongo-error">{aggregation.error}</p>}
      <div className="mongo-documents">
        {aggregation.results?.map((doc, i) => <DocumentCard key={`${i}-${doc.text.length}`} doc={doc} />)}
      </div>
    </div>
  );
}

/** A collection's indexes; one made with its keys, one dropped after asking. */
function Indexes({ target }: { target: MongoTarget }) {
  const indexes = useMongoIndexes(target);
  const [making, setMaking] = useState<{ keys: string; name: string; unique: boolean } | null>(null);
  const [dropping, setDropping] = useState<IndexView | null>(null);
  const make = async () => {
    if (making && (await indexes.create(making.keys, making.unique, making.name))) setMaking(null);
  };
  const properties = (index: IndexView) =>
    [index.unique && "unique", index.sparse && "sparse", index.ttlSeconds !== null && `TTL ${index.ttlSeconds}s`, index.partial && `partial ${index.partial}`].filter(Boolean).join(" · ");

  return (
    <div className="mongo-view">
      <div className="mongo-toolbar">
        <button type="button" className="btn btn-ghost" onClick={() => setMaking({ keys: "{ field: 1 }", name: "", unique: false })}>
          <Plus size={14} />
          Create index
        </button>
        <span className="mongo-spacer" />
        <span className="mongo-quiet">{indexes.indexes.length > 0 && `${indexes.indexes.length} ${indexes.indexes.length === 1 ? "index" : "indexes"}`}</span>
      </div>
      {indexes.error && !making && <p className="mongo-error">{indexes.error}</p>}
      <table className="mongo-indexes">
        <thead>
          <tr>
            <th>Name</th>
            <th>Keys</th>
            <th>Properties</th>
            <th aria-label="Actions" />
          </tr>
        </thead>
        <tbody>
          {indexes.indexes.map((index) => (
            <tr key={index.name}>
              <td>{index.name}</td>
              <td className="mongo-mono">{index.keys}</td>
              <td className="mongo-quiet">{properties(index)}</td>
              <td>
                {index.name !== "_id_" && (
                  <button type="button" className="mongo-icon-button" title="Drop index" aria-label={`Drop index ${index.name}`} onClick={() => setDropping(index)}>
                    <Trash2 size={13} />
                  </button>
                )}
              </td>
            </tr>
          ))}
        </tbody>
      </table>

      <Modal
        title="Create index"
        open={making !== null}
        onClose={() => setMaking(null)}
        footer={
          <>
            <button type="button" className="btn btn-ghost" onClick={() => setMaking(null)}>
              Cancel
            </button>
            <button type="button" className="btn btn-primary" disabled={indexes.busy} onClick={() => void make()}>
              Create
            </button>
          </>
        }
      >
        <label className="mongo-field">
          <span>Keys — 1 ascending, -1 descending, 'text', '2dsphere'</span>
          <input autoFocus className="mongo-mono" spellCheck={false} value={making?.keys ?? ""} onChange={(e) => making && setMaking({ ...making, keys: e.target.value })} />
        </label>
        <label className="mongo-field">
          <span>Name — the server makes one when left empty</span>
          <input spellCheck={false} value={making?.name ?? ""} onChange={(e) => making && setMaking({ ...making, name: e.target.value })} />
        </label>
        <label className="mongo-switch-row">
          <Switch label="Unique" on={making?.unique ?? false} onChange={(unique) => making && setMaking({ ...making, unique })} />
          Unique — no two documents with the same keys
        </label>
        {indexes.error && <p className="mongo-error">{indexes.error}</p>}
      </Modal>
      <ConfirmModal title={`Drop index “${dropping?.name ?? ""}”?`} action="Drop" subject={dropping} onClose={() => setDropping(null)} onConfirm={(index) => void indexes.drop(index.name)}>
        <p className="mongo-confirm">
          Queries that used <code className="mongo-mono">{dropping?.keys}</code> read the whole collection after it{dropping?.unique ? ", and duplicates are no longer refused" : ""}.
        </p>
      </ConfirmModal>
    </div>
  );
}
