import { useEffect, useId, useLayoutEffect, useMemo, useRef, useState, type KeyboardEvent, type ReactNode } from "react";
import { Check, ChevronDown, ChevronLeft, ChevronRight, ChevronsDownUp, ChevronsUpDown, Copy, CopyPlus, Database, Eye, History, Lightbulb, Loader2, Minus, MoreHorizontal, Pencil, Play, Plus, RefreshCw, Settings2, Table, Trash2, X } from "lucide-react";
import { Dropdown } from "./Dropdown";
import { ErrorNote } from "./ErrorNote";
import { Modal } from "./Modal";
import { Page } from "./Page";
import { Switch } from "./Switch";
import { Tabs } from "./Tabs";
import { useCopied } from "../hooks/useCopied";
import { useMongoAggregation, useMongoConnections, useMongoDocuments, useMongoIndexes, useMongoTree } from "../hooks/useMongo";
import { useStoredState } from "../hooks/useStoredState";
import { sizeLabel } from "../lib/prototype";
import { forgetHistory, historyKey, isHistory, newDocument, pageRange, ranAgo, remember, withoutId, type QueryRan, type QueryRun, type FindRequest, type IndexView, type MongoConnection, type FieldChange, type MongoDocument, type MongoTarget, type Shown } from "../lib/mongo";
import { apply, erased, EXAMPLES, knownFields, suggest, typed, type Edit as TextEdit, type KnownField, type QueryKind, type Suggestion } from "../lib/mongoQuery";
import { matches } from "../lib/shortcuts";
import type { PageContext } from "./pages";
import "./MongoPage.css";

type View = "documents" | "aggregations" | "indexes";

/** A collection open in a tab. */
type Open = { database: string; collection: string };

/** A database's name has no dot, so this names one collection. */
const keyOf = (open: Open) => `${open.database}.${open.collection}`;

/** The connection form: a new one, or a saved one changed — its URI is never shown, only typed again. */
type Form = { id?: string; name: string; uri: string; saved?: string };

/** A step that cannot be taken back, asked about first. */
// A collection to drop names its connection: the one asked about, whatever is chosen when Drop is pressed.
type Confirm = { what: "connection"; connection: MongoConnection } | { what: "collection"; target: MongoTarget };

const isId = (value: unknown): value is string | null => value === null || typeof value === "string";

const LIMITS = [10, 20, 50, 100];

/** Documents past this many fields are folded until asked for. */
const FOLDED_FIELDS = 14;

/**
 * MongoDB, in the spirit of Compass but only what is used every day
 * (docs/44-mongodb.md): a saved connection's databases and collections on the
 * left; on the right a collection's documents — found with a filter, a sort
 * and a projection, edited, added, deleted — its aggregations and its indexes.
 * Query text is mongosh's. Connection strings are sealed in the backend; this
 * page only ever sees them masked.
 */
export function MongoPage(_: PageContext) {
  const store = useMongoConnections();
  const [chosen, setChosen] = useStoredState<string | null>("mongo.connection", null, isId);
  const connection = store.connections.find((c) => c.id === chosen) ?? store.connections[0] ?? null;
  // Bumped when a saved connection gets a new URI: same id, another server — what was read from the last one goes.
  const [server, setServer] = useState(0);
  const tree = useMongoTree(connection?.id ?? null, server);
  // The tabs belong to the connection they were opened on: another one chosen
  // shows none from its first render, so no tab of the last one reads from it.
  const on = connection ? `${connection.id}#${server}` : null;
  const [opened, setOpened] = useState<{ connection: string | null; tabs: Open[]; active: string | null }>({ connection: null, tabs: [], active: null });
  const { tabs, active } = opened.connection === on ? opened : { tabs: [], active: null };
  const setActive = (key: string | null) => setOpened({ connection: on, tabs, active: key });
  const selected = tabs.find((t) => keyOf(t) === active) ?? null;
  const [form, setForm] = useState<Form | null>(null);
  const [creating, setCreating] = useState<{ database: string; collection: string } | null>(null);
  const [confirm, setConfirm] = useState<Confirm | null>(null);

  // From the state as it is by then: both also run after an await.
  const openTab = (open: Open) =>
    setOpened((was) => {
      const all = was.connection === on ? was.tabs : [];
      return { connection: on, tabs: all.some((t) => keyOf(t) === keyOf(open)) ? all : [...all, open], active: keyOf(open) };
    });
  const closeTab = (open: Open) =>
    setOpened((was) => {
      const rest = (was.connection === on ? was.tabs : []).filter((t) => keyOf(t) !== keyOf(open));
      const active = was.connection === on && was.active !== keyOf(open) ? was.active : rest.length > 0 ? keyOf(rest[rest.length - 1]) : null;
      return { connection: on, tabs: rest, active };
    });

  const createCollection = async () => {
    if (creating && (await tree.createCollection(creating.database.trim(), creating.collection.trim()))) {
      openTab({ database: creating.database.trim(), collection: creating.collection.trim() });
      setCreating(null);
    }
  };

  const confirmed = async (c: Confirm) => {
    if (c.what === "connection" && (await store.remove(c.connection.id))) {
      forgetHistory(c.connection.id);
      setForm(null);
    }
    if (c.what === "collection" && (await tree.dropCollection(c.target))) closeTab(c.target);
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
                                onClick={() => openTab({ database: db.name, collection: c.name })}
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

          <section className="mongo-main" aria-label={selected ? keyOf(selected) : "Collection"}>
            {tabs.length > 0 && (
              <div className="mongo-tabs" role="tablist" aria-label="Open collections">
                {tabs.map((t) => {
                  const on = keyOf(t) === active;
                  return (
                    <div key={keyOf(t)} className={`mongo-tab${on ? " active" : ""}`} title={keyOf(t)} onAuxClick={(e) => e.button === 1 && closeTab(t)}>
                      <button type="button" role="tab" aria-selected={on} className="mongo-tab-name" onClick={() => setActive(keyOf(t))}>
                        {t.collection}
                      </button>
                      <button type="button" className="mongo-tab-close" title="Close" aria-label={`Close ${keyOf(t)}`} onClick={() => closeTab(t)}>
                        <X size={12} />
                      </button>
                    </div>
                  );
                })}
              </div>
            )}
            {/* Every open tab stays mounted, so its query and page are there when it comes back. */}
            {connection &&
              tabs.map((t) => (
                <div key={keyOf(t)} className="mongo-open" hidden={keyOf(t) !== active}>
                  <Collection target={{ connection: connection.id, ...t }} onDrop={() => setConfirm({ what: "collection", target: { connection: connection.id, ...t } })} />
                </div>
              ))}
            {tabs.length === 0 && <p className="mongo-quiet mongo-pick">{tree.databases.length > 0 ? "Pick a collection on the left." : ""}</p>}
          </section>
        </div>
      )}

      <ConnectionForm form={form} onChange={setForm} store={store} onSaved={(id, moved) => {
          setChosen(id);
          if (moved) setServer((n) => n + 1);
        }} onDelete={(c) => setConfirm({ what: "connection", connection: c })} />

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
        title={confirm?.what === "connection" ? `Delete “${confirm.connection.name}”?` : confirm?.what === "collection" ? `Drop ${confirm.target.database}.${confirm.target.collection}?` : ""}
        action={confirm?.what === "collection" ? "Drop" : "Delete"}
        subject={confirm}
        onClose={() => setConfirm(null)}
        onConfirm={(c) => void confirmed(c)}
      >
        <p className="mongo-confirm">
          {confirm?.what === "connection" && `Forgets the connection and its saved connection string. Nothing on the server changes.`}
          {confirm?.what === "collection" && `Deletes every document of ${confirm.target.collection} and its indexes, on the server. This cannot be undone.`}
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

/** One open collection: its documents, aggregations and indexes. */
function Collection({ target, onDrop }: { target: MongoTarget; onDrop: () => void }) {
  const [view, setView] = useState<View>("documents");
  return (
    <>
      <header className="mongo-main-head">
        <h2 className="mongo-namespace">
          <span className="mongo-namespace-database">{target.database}</span>
          <ChevronRight size={14} className="mongo-namespace-gap" aria-label="›" />
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
          onPick={onDrop}
          below
          right
        />
      </header>
      {view === "documents" && <Documents target={target} />}
      {view === "aggregations" && <Aggregations target={target} />}
      {view === "indexes" && <Indexes target={target} />}
    </>
  );
}

type FormProps = {
  form: Form | null;
  onChange: (form: Form | null) => void;
  store: ReturnType<typeof useMongoConnections>;
  /** `moved`: a saved connection now goes to another server. */
  onSaved: (id: string, moved: boolean) => void;
  onDelete: (connection: MongoConnection) => void;
};

/** Add or change a connection: a name and a connection string, tested before or after saving. */
function ConnectionForm({ form, onChange, store, onSaved, onDelete }: FormProps) {
  const [tested, setTested] = useState<{ ok: boolean; text: string } | null>(null);
  const [testing, setTesting] = useState(false);
  // One save at a time: a second Enter or click before the first is back would seal the connection twice.
  const saving = useRef(false);
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
    if (saving.current) return;
    saving.current = true;
    const before = new Set(store.connections.map((c) => c.id));
    const list = await store.save(draft).finally(() => (saving.current = false));
    if (!list) return;
    onSaved(form.id ?? list.find((c) => !before.has(c.id))?.id ?? list[list.length - 1].id, editing && draft.uri !== undefined);
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
  // The query bar as typed: a find run again — after an edit, a page turned — leaves it as it is.
  const [draft, setDraft] = useState<QueryRun>(docs.request);
  const [options, setOptions] = useState(false);
  const [editor, setEditor] = useState<{ id?: string; text: string } | null>(null);
  const [deleting, setDeleting] = useState<{ id: string; text: string } | null>(null);

  const [history, setHistory] = useStoredState<QueryRan[]>(historyKey(target), [], isHistory);
  // "30 seconds ago" counts from when the menu opened, not ticking while it is open.
  const [openedAt, setOpenedAt] = useState(Date.now);
  const run = (next: Partial<FindRequest> = {}) => {
    const request = { ...draft, skip: 0, limit: docs.request.limit, ...next };
    // Kept once it ran: a query refused is not one to run again.
    void docs.find(request).then((ran) => ran && setHistory((now) => remember(now, request)));
  };
  const runAgain = ({ filter, projection, sort }: QueryRan) => {
    const query: QueryRun = { filter, projection, sort };
    setDraft((now) => ({ ...now, ...query }));
    if (query.projection || query.sort) setOptions(true);
    run(query);
  };
  const onEnter = (e: KeyboardEvent) => {
    if (!matches(e, "accept")) return;
    e.preventDefault();
    run();
  };
  const documents = docs.documents;
  const fields = useMemo(() => knownFields(documents ?? []), [documents]);
  const { skip } = docs;
  const { limit } = docs.request;
  // A document read with a projection is only part of it, maybe reshaped: replaced with what is shown, or cloned, the rest would go.
  const partial = docs.request.projection.trim() !== "";
  // By _id, so after a find again the cards follow their documents, an edit begun included; one seen
  // again on the page (a view's `$unwind`, a projection's computed _id) by how many times.
  const cardKeys = useMemo(() => {
    const seen = new Map<string, number>();
    return (documents ?? []).map((doc, i) => {
      if (doc.id === null) return `${skip}-${i}`;
      const id = doc.id;
      const times = seen.get(id) ?? 0;
      seen.set(id, times + 1);
      return times === 0 ? id : `${id}#${times}`;
    });
  }, [documents, skip]);
  /** A field's "+" or "−": the filter gets its condition, found once Find is pressed. */
  const filterBy: FilterBy = async (path, text, exclude) => {
    const filter = await docs.condition(draft.filter, path, text, exclude);
    if (filter !== undefined) setDraft((now) => ({ ...now, filter }));
  };
  // One save at a time: a held or twice-pressed Cmd+Enter would insert the documents again.
  const saving = useRef(false);
  const saveEditor = async () => {
    if (!editor || saving.current) return;
    saving.current = true;
    await save(editor).finally(() => (saving.current = false));
  };
  const save = async (editor: { id?: string; text: string }) => {
    if (editor.id !== undefined) return void ((await docs.replace(editor.id, editor.text)) && setEditor(null));
    const inserted = await docs.insert(editor.text);
    // All in, though the write concern was not met: nothing is left to insert, and the reason stays on the list.
    if (inserted.done || inserted.rest === "") setEditor(null);
    // Refused part way: the ones that went in leave the editor, so trying again does not add them twice.
    else if (inserted.rest !== null) setEditor({ text: inserted.rest });
  };
  // A dialog's error is its own: it goes when the dialog opens or closes, not onto the list.
  const showEditor = (next: typeof editor) => {
    docs.clearError();
    setEditor(next);
  };

  return (
    <div className="mongo-view">
      <div className="mongo-query">
        <QueryField
          label="Filter"
          kind="filter"
          fields={fields}
          value={draft.filter}
          placeholder="{ field: 'value' }"
          onChange={(filter) => setDraft((now) => ({ ...now, filter }))}
          onKeyDown={onEnter}
          lead={
            <Dropdown
              label={<History size={14} />}
              title="History"
              heading="Queries run in this collection"
              emptyLabel="No queries run yet"
              onOpen={() => setOpenedAt(Date.now())}
              below
              options={history.map((query, i) => ({
                value: String(i),
                label: query.filter || "{}",
                hint: [ranAgo(query.at, openedAt), query.projection && `project ${query.projection}`, query.sort && `sort ${query.sort}`].filter(Boolean).join(" · "),
              }))}
              value=""
              onPick={(value) => runAgain(history[Number(value)])}
            />
          }
        />
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
          <QueryField label="Project" kind="projection" fields={fields} value={draft.projection} placeholder="{ field: 1 }" onChange={(projection) => setDraft((now) => ({ ...now, projection }))} onKeyDown={onEnter} />
          <QueryField label="Sort" kind="sort" fields={fields} value={draft.sort} placeholder="{ field: -1 }" onChange={(sort) => setDraft((now) => ({ ...now, sort }))} onKeyDown={onEnter} />
        </div>
      )}

      <div className="mongo-toolbar">
        <button type="button" className="btn btn-ghost" onClick={() => showEditor({ text: newDocument() })}>
          <Plus size={14} />
          Insert document
        </button>
        <span className="mongo-spacer" />
        {documents && <span className="mongo-quiet">{pageRange(skip, documents.length, docs.total)}</span>}
        {documents && docs.total === null && (
          <button type="button" className="btn btn-ghost mongo-count" title="Count every match — on a view, its whole pipeline runs" disabled={docs.counting} onClick={() => void docs.count()}>
            {docs.counting && <Loader2 size={12} className="mongo-spin" />}
            {docs.counting ? "Counting…" : "Count"}
          </button>
        )}
        <Dropdown
          label={`${limit} per page`}
          title="How many to show"
          options={LIMITS.map((n) => ({ value: String(n), label: `${n} per page` }))}
          value={String(limit)}
          onPick={(value) => run({ limit: Number(value) })}
          below
          right
        />
        <button type="button" className="mongo-icon-button" title="Previous page" aria-label="Previous page" disabled={docs.busy || !docs.hasPrevious} onClick={docs.previous}>
          <ChevronLeft size={14} />
        </button>
        <button
          type="button"
          className="mongo-icon-button"
          title="Next page"
          aria-label="Next page"
          disabled={docs.busy || !docs.hasNext}
          onClick={() => void docs.next()}
        >
          <ChevronRight size={14} />
        </button>
        <button type="button" className="mongo-icon-button" title="Find again" aria-label="Find again" disabled={docs.busy} onClick={() => void docs.find({ ...docs.request, skip })}>
          <RefreshCw size={13} className={docs.busy ? "mongo-spin" : undefined} />
        </button>
      </div>

      {docs.error && !editor && <ErrorNote message={docs.error} onClose={docs.clearError} />}
      {!documents && docs.busy && (
        <p className="mongo-quiet mongo-loading" role="status">
          <Loader2 size={14} className="mongo-spin" />
          Loading documents…
        </p>
      )}
      {documents && documents.length === 0 && !docs.error && <p className="mongo-quiet">No documents match.</p>}
      <div className="mongo-documents" aria-busy={docs.busy}>
        {documents?.map((doc, i) => {
          // Not under a projection: it reshapes what is shown — `$slice`, `$elemMatch`, a computed field — so a path on the card
          // is not one in the document, and it can compute the _id too, `{ _id: "$parentId" }`. Nothing is done by it there.
          const id = partial ? null : doc.id;
          return (
            <DocumentCard
              key={cardKeys[i]}
              doc={doc}
              onSave={id !== null ? (changes) => docs.setFields(id, changes) : undefined}
              error={docs.error}
              filterBy={filterBy}
              actions={
                id !== null && (
                  <>
                    <button type="button" className="mongo-icon-button" title="Edit" aria-label="Edit document" onClick={() => showEditor({ id, text: doc.text })}>
                      <Pencil size={13} />
                    </button>
                    <button type="button" className="mongo-icon-button" title="Clone — insert a copy with a new _id" aria-label="Clone document" onClick={() => showEditor({ text: withoutId(doc.text) })}>
                      <CopyPlus size={13} />
                    </button>
                    <button type="button" className="mongo-icon-button" title="Delete" aria-label="Delete document" onClick={() => setDeleting({ id, text: doc.text })}>
                      <Trash2 size={13} />
                    </button>
                  </>
                )
              }
            />
          );
        })}
      </div>

      <Modal
        title={editor?.id !== undefined ? "Edit document" : "Insert document"}
        open={editor !== null}
        onClose={() => showEditor(null)}
        wide
        footer={
          <>
            <button type="button" className="btn btn-ghost" onClick={() => showEditor(null)}>
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
        {docs.error && <ErrorNote message={docs.error} onClose={docs.clearError} />}
      </Modal>
      <ConfirmModal title="Delete this document?" action="Delete" subject={deleting} onClose={() => setDeleting(null)} onConfirm={(doc) => void docs.remove(doc.id)}>
        <pre className="mongo-document-text mongo-mono mongo-confirm-doc">{deleting?.text.split("\n").slice(0, 8).join("\n")}</pre>
      </ConfirmModal>
    </div>
  );
}

type QueryFieldProps = {
  label: string;
  kind: QueryKind;
  /** The fields of the documents shown, suggested as a key is typed. */
  fields: KnownField[];
  value: string;
  placeholder: string;
  onChange: (value: string) => void;
  onKeyDown: (e: KeyboardEvent) => void;
  /** A control between the label and the text, behind a line of its own. */
  lead?: ReactNode;
};

/**
 * A query typed in mongosh's syntax, helped: brackets and quotes close
 * themselves, a key being typed gets the documents' fields — or, after `$`,
 * the operators — in a list under it, and examples are a click away.
 */
function QueryField({ label, kind, fields, value, placeholder, onChange, onKeyDown, lead }: QueryFieldProps) {
  const id = useId();
  const input = useRef<HTMLInputElement>(null);
  // Where the caret is while the field has focus; `null` keeps the list closed.
  const [caret, setCaret] = useState<number | null>(null);
  const [active, setActive] = useState(0);
  const [dismissed, setDismissed] = useState(false);
  // The selection an edit leaves, set once React has put its text in.
  const selection = useRef<[number, number] | null>(null);
  useLayoutEffect(() => {
    if (!selection.current || !input.current) return;
    input.current.setSelectionRange(...selection.current);
    // The caret is where it was put, whatever a select event read before the text went in.
    setCaret(selection.current[1]);
    selection.current = null;
  });

  const shown = caret === null || dismissed ? null : suggest(value, caret, fields, kind);
  const edit = (next: TextEdit) => {
    selection.current = [next.start, next.end];
    onChange(next.text);
    setCaret(next.end);
    setActive(0);
    setDismissed(false);
  };
  const pick = (suggestion: Suggestion) => shown && edit(apply(value, shown, suggestion.insert));

  const keyDown = (e: KeyboardEvent<HTMLInputElement>) => {
    if (shown) {
      const moves = matches(e, "mongoSuggestNext") ? 1 : matches(e, "mongoSuggestPrev") ? -1 : 0;
      if (moves) {
        e.preventDefault();
        return setActive((active + moves + shown.items.length) % shown.items.length);
      }
      if (matches(e, "mongoSuggestPick")) {
        e.preventDefault();
        return void pick(shown.items[Math.min(active, shown.items.length - 1)]);
      }
      if (matches(e, "close")) {
        e.stopPropagation();
        return setDismissed(true);
      }
    }
    // An opener typed brings its closer; Backspace between the two takes both.
    const { selectionStart: start, selectionEnd: end } = e.currentTarget;
    const plain = !e.ctrlKey && !e.metaKey && !e.altKey && start !== null && end !== null;
    const paired = !plain ? null : matches(e, "mongoPairErase") ? erased(value, start, end) : e.key.length === 1 ? typed(value, start, end, e.key) : null;
    if (paired) {
      e.preventDefault();
      return edit(paired);
    }
    onKeyDown(e);
  };

  return (
    <div className="mongo-query-field">
      <label htmlFor={id}>{label}</label>
      {lead && <span className="mongo-query-lead">{lead}</span>}
      <input
        ref={input}
        id={id}
        className="mongo-mono"
        role="combobox"
        aria-autocomplete="list"
        aria-expanded={!!shown}
        aria-controls={shown ? `${id}-list` : undefined}
        aria-activedescendant={shown ? `${id}-${Math.min(active, shown.items.length - 1)}` : undefined}
        autoComplete="off"
        spellCheck={false}
        value={value}
        placeholder={placeholder}
        onChange={(e) => {
          onChange(e.target.value);
          setCaret(e.target.selectionStart);
          setActive(0);
          setDismissed(false);
        }}
        onSelect={(e) => setCaret(e.currentTarget.selectionStart)}
        onBlur={() => setCaret(null)}
        onKeyDown={keyDown}
      />
      <Dropdown
        label={<Lightbulb size={12} />}
        title="Examples"
        heading={`Start the ${label.toLowerCase()} from an example`}
        options={EXAMPLES[kind].map((example) => ({ value: example.label, hint: example.text }))}
        value=""
        onPick={(picked) => {
          const text = EXAMPLES[kind].find((example) => example.label === picked)?.text ?? "";
          const at = text.indexOf("field");
          selection.current = at < 0 ? [text.length, text.length] : [at, at + "field".length];
          onChange(text);
          input.current?.focus();
        }}
        below
        right
      />
      {shown && (
        <div className="mongo-suggest" role="listbox" id={`${id}-list`} aria-label={`Suggestions for the ${label.toLowerCase()}`}>
          {shown.items.map((item, i) => (
            <div
              key={item.label}
              id={`${id}-${i}`}
              role="option"
              aria-selected={i === active}
              className={`mongo-suggest-item${i === active ? " active" : ""}`}
              // Picked without taking the focus from the field.
              onPointerDown={(e) => {
                e.preventDefault();
                pick(item);
              }}
            >
              <span className="mongo-mono">{item.label}</span>
              <span className="mongo-suggest-detail">{item.detail}</span>
              {i === 0 && i === active && <kbd className="mongo-suggest-key">Tab</kbd>}
            </div>
          ))}
        </div>
      )}
    </div>
  );
}

/** A value as one would paste it: a string without its quotes. */
const plain = (value: { text: string; bson: string }) => (value.bson === "String" ? (JSON.parse(value.text) as string) : value.text);

function CopyButton({ text, label }: { text: string; label: string }) {
  const { copied, copy } = useCopied();
  return (
    <button type="button" className="mongo-icon-button mongo-copy" title="Copy" aria-label={label} onClick={() => copy(text)}>
      {copied ? <Check size={12} /> : <Copy size={12} />}
    </button>
  );
}

/** Adds a field's condition to the filter: `path` as a query reads it, an array's index left out so any item matches. */
type FilterBy = (path: string[], text: string, exclude: boolean) => void;

/** A card being edited: each field opened on it, by its path as JSON, with the text it had and has now. */
type Drafts = Record<string, { path: string[]; was: string; text: string }>;

/**
 * A field or item added on a card being edited, into the object or array at
 * `parent`. An item goes after the `from` the array had, in the order added:
 * `$set` on the index past the end appends, as on a new name it adds a field.
 */
type Added = { id: number; parent: string[]; array: boolean; from: number; name: string; text: string };

/** What a field on a card being edited reaches: its draft, and the card's open, change, add, save and cancel. */
type Edit = {
  drafts: Drafts | null;
  added: Added[];
  open: (path: string[], text: string) => void;
  change: (path: string[], text: string) => void;
  add: (parent: string[], array: boolean, from: number) => void;
  changeAdded: (id: number, change: Partial<Pick<Added, "name" | "text">>) => void;
  drop: (id: number) => void;
  save: () => void;
  cancel: () => void;
};

const sameParent = (a: string[], b: string[]) => a.length === b.length && a.every((key, i) => key === b[i]);

/** Where an added one goes: its name, or for an item the index after those added before it. */
const addedKey = (one: Added, all: Added[]) => (one.array ? String(one.from + all.filter((a) => a.array && sameParent(a.parent, one.parent)).indexOf(one)) : one.name);

/**
 * A document: a head with its size and actions, then a field to a line — what
 * nests closed until opened, along a guide — each with its type at the end.
 * With `onSave`, a field double-clicked puts the card in edit mode: more can be
 * opened, and Save sends every change at once.
 */
function DocumentCard({ doc, actions, onSave, error, filterBy }: { doc: MongoDocument; actions?: ReactNode; onSave?: (changes: FieldChange[]) => Promise<boolean>; error?: string | null; filterBy?: FilterBy }) {
  const [unfolded, setUnfolded] = useState(false);
  const [expanded, setExpanded] = useState(false);
  const [drafts, setDrafts] = useState<Drafts | null>(null);
  const [added, setAdded] = useState<Added[]>([]);
  const [saving, setSaving] = useState(false);
  const [failed, setFailed] = useState(false);
  const { copied, copy } = useCopied();
  const long = doc.fields.length > FOLDED_FIELDS;
  const changed: FieldChange[] = [
    ...Object.values(drafts ?? {})
      .filter((draft) => draft.text !== draft.was)
      .map(({ path, text }) => ({ path, text })),
    ...added.map((one) => ({ path: [...one.parent, addedKey(one, added)], text: one.text })),
  ];
  const cancel = () => {
    setDrafts(null);
    setAdded([]);
    setFailed(false);
  };
  const save = async () => {
    if (!onSave || saving) return;
    if (changed.length === 0) return cancel();
    setSaving(true);
    const saved = await onSave(changed);
    setSaving(false);
    setFailed(!saved);
    if (saved) cancel();
  };
  const edit: Edit | undefined = onSave && {
    drafts,
    added,
    open: (path, text) => setDrafts((all) => ({ [JSON.stringify(path)]: { path, was: text, text }, ...all })),
    change: (path, text) => setDrafts((all) => all && { ...all, [JSON.stringify(path)]: { ...all[JSON.stringify(path)], text } }),
    add: (parent, array, from) => {
      setDrafts((all) => all ?? {});
      setAdded((all) => [...all, { id: (all[all.length - 1]?.id ?? 0) + 1, parent, array, from, name: "", text: "" }]);
    },
    changeAdded: (id, change) => setAdded((all) => all.map((one) => (one.id === id ? { ...one, ...change } : one))),
    drop: (id) => setAdded((all) => all.filter((one) => one.id !== id)),
    save: () => void save(),
    cancel,
  };
  return (
    <article className={`mongo-document${drafts ? " mongo-document-editing" : ""}`}>
      <header className="mongo-document-head">
        <span className="mongo-document-count">{doc.fields.length === 1 ? "1 field" : `${doc.fields.length} fields`}</span>
        <div className="mongo-document-actions">
          <button type="button" className="mongo-icon-button" title={expanded ? "Collapse all" : "Expand all"} aria-label={expanded ? "Collapse all" : "Expand all"} onClick={() => setExpanded(!expanded)}>
            {expanded ? <ChevronsDownUp size={13} /> : <ChevronsUpDown size={13} />}
          </button>
          <button type="button" className="mongo-icon-button" title="Copy" aria-label="Copy document" onClick={() => copy(doc.text)}>
            {copied ? <Check size={13} /> : <Copy size={13} />}
          </button>
          {actions}
        </div>
      </header>
      <div className="mongo-document-body">
        {/* Remounted on "Expand all" / "Collapse all", so every field starts as it says; the drafts live here, above it. */}
        <Fields key={String(expanded)} entries={long && !unfolded ? doc.fields.slice(0, FOLDED_FIELDS) : doc.fields} open={expanded} path={[]} where={[]} edit={edit} filterBy={filterBy} />
        {long && (
          <button type="button" className="mongo-unfold" onClick={() => setUnfolded(!unfolded)}>
            {unfolded ? "Show less" : `Show ${doc.fields.length - FOLDED_FIELDS} more fields`}
          </button>
        )}
        {edit && drafts && (
          <button type="button" className="mongo-add-field" onClick={() => edit.add([], false, 0)}>
            <Plus size={12} />
            Add field
          </button>
        )}
      </div>
      {drafts && (
        <footer className="mongo-document-foot">
          {failed && error ? (
            <span className="mongo-error">{error}</span>
          ) : (
            <span className="mongo-quiet">{changed.length === 0 ? "Double-click a value to change it; + on an object or array adds to it" : changed.length === 1 ? "1 field changed" : `${changed.length} fields changed`}</span>
          )}
          <button type="button" className="btn btn-ghost" onClick={cancel}>
            Cancel
          </button>
          <button type="button" className="btn btn-primary" disabled={saving} onClick={() => void save()}>
            Save
          </button>
        </footer>
      )}
    </article>
  );
}

/** `path` is where the entries are, as an update names it; `where`, as a query does — an array's indexes left out. */
type FieldsProps = { entries: [string, Shown][]; open: boolean; path: string[]; where: string[]; array?: boolean; edit?: Edit; filterBy?: FilterBy };

function Fields({ entries, open, path, where, array, edit, filterBy }: FieldsProps) {
  return (
    <ul className="mongo-tree-fields">
      {entries.map(([name, value]) => (
        <Field key={name} name={name} value={value} open={open} path={[...path, name]} where={array ? where : [...where, name]} edit={edit} filterBy={filterBy} />
      ))}
      {edit?.added.filter((one) => sameParent(one.parent, path)).map((one) => <AddedField key={one.id} one={one} edit={edit} />)}
    </ul>
  );
}

/** A field or item added while editing: a name and a value, or for an item its index and a value. */
function AddedField({ one, edit }: { one: Added; edit: Edit }) {
  const into = one.parent.join(".") || "the document";
  const keyDown = (e: KeyboardEvent) => {
    if (matches(e, "accept")) {
      e.preventDefault();
      edit.save();
    } else if (matches(e, "close")) {
      e.stopPropagation();
      edit.cancel();
    }
  };
  return (
    <li className="mongo-entry">
      {one.array ? (
        <span className="mongo-entry-key">{addedKey(one, edit.added)}</span>
      ) : (
        <input
          className="mongo-entry-input mongo-entry-name"
          aria-label={`New field name in ${into}`}
          placeholder="name"
          autoFocus
          spellCheck={false}
          value={one.name}
          onChange={(e) => edit.changeAdded(one.id, { name: e.target.value })}
          onKeyDown={keyDown}
        />
      )}
      {": "}
      <input
        className="mongo-entry-input"
        aria-label={one.array ? `New item of ${into}` : `New field value in ${into}`}
        placeholder={'"text", 5, { a: 1 }, []'}
        autoFocus={one.array}
        spellCheck={false}
        value={one.text}
        onChange={(e) => edit.changeAdded(one.id, { text: e.target.value })}
        onKeyDown={keyDown}
      />
      <button type="button" className="mongo-icon-button mongo-entry-drop" title="Remove" aria-label={one.array ? `Remove the new item of ${into}` : `Remove the new field in ${into}`} onClick={() => edit.drop(one.id)}>
        <X size={12} />
      </button>
    </li>
  );
}

/**
 * One field: its key in bold, its value — clicked, a long one wraps instead of
 * being cut; double-clicked, it opens for editing — and its type at the end of
 * the line; an object or array is a toggle for what is in it.
 */
function Field({ name, value, open, path, where, edit, filterBy }: { name: string; value: Shown; open: boolean; path: string[]; where: string[]; edit?: Edit; filterBy?: FilterBy }) {
  const [shown, setShown] = useState(open);
  const [wrapped, setWrapped] = useState(false);
  if (value.kind === "value") {
    const draft = edit?.drafts?.[JSON.stringify(path)];
    // The _id never changes in place: the server refuses, and the backend before it.
    const editable = edit && path[0] !== "_id";
    return (
      <li className="mongo-entry">
        {filterBy && !edit?.drafts && (
          <span className="mongo-entry-filter">
            <button type="button" title="Add to the filter" aria-label={`Filter by ${where.join(".")}`} onClick={() => filterBy(where, value.text, false)}>
              <Plus size={11} />
            </button>
            <button type="button" title="Add to the filter as not equal" aria-label={`Filter out ${where.join(".")}`} onClick={() => filterBy(where, value.text, true)}>
              <Minus size={11} />
            </button>
          </span>
        )}
        <span className="mongo-entry-key">{name}</span>
        {": "}
        {draft ? (
          <input
            className={`mongo-entry-input${draft.text !== draft.was ? " mongo-entry-changed" : ""}`}
            aria-label={`Edit ${path.join(".")}`}
            autoFocus
            spellCheck={false}
            value={draft.text}
            onChange={(e) => edit!.change(path, e.target.value)}
            onKeyDown={(e) => {
              if (matches(e, "accept")) {
                e.preventDefault();
                edit!.save();
              } else if (matches(e, "close")) {
                e.stopPropagation();
                edit!.cancel();
              }
            }}
          />
        ) : (
          <span
            className={`mongo-entry-value mongo-bson-${value.bson}${wrapped ? " mongo-entry-wrapped" : ""}`}
            title={editable ? "Double-click to edit" : undefined}
            // A drag that selects text ends in a click too; that one is a copy, not a toggle.
            onClick={() => !window.getSelection()?.toString() && setWrapped(!wrapped)}
            onDoubleClick={editable ? () => edit.open(path, value.text) : undefined}
          >
            {value.text}
          </span>
        )}
        <CopyButton text={plain(value)} label={`Copy ${name}`} />
        <span className="mongo-entry-type">{value.bson}</span>
      </li>
    );
  }
  const entries: [string, Shown][] = value.kind === "object" ? value.fields : value.items.map((item, i) => [String(i), item]);
  return (
    <li className="mongo-entry mongo-entry-nest">
      <div className="mongo-entry-head">
        {edit?.drafts && (
          <span className="mongo-entry-filter">
            <button
              type="button"
              title={value.kind === "array" ? "Add an item" : "Add a field"}
              aria-label={`${value.kind === "array" ? "Add item to" : "Add field to"} ${path.join(".")}`}
              onClick={() => {
                setShown(true);
                edit.add(path, value.kind === "array", value.kind === "array" ? value.items.length : 0);
              }}
            >
              <Plus size={11} />
            </button>
          </span>
        )}
        <button type="button" className="mongo-entry-toggle" aria-expanded={shown} onClick={() => setShown(!shown)}>
          <ChevronRight size={12} />
          <span className="mongo-entry-key">{name}</span>
          <span className="mongo-entry-type">{value.kind === "object" ? `Object (${value.fields.length})` : `Array (${value.items.length})`}</span>
        </button>
      </div>
      {shown && <Fields entries={entries} open={open} path={path} where={where} array={value.kind === "array"} edit={edit} filterBy={filterBy} />}
    </li>
  );
}

const PIPELINE = "[\n  { $match: {} },\n  { $limit: 20 }\n]";

/** A pipeline run on the collection; its results read-only. */
function Aggregations({ target }: { target: MongoTarget }) {
  const aggregation = useMongoAggregation(target);
  const [pipeline, setPipeline] = useState(PIPELINE);
  // A pipeline that writes into a collection, held until asked: the text is the one asked about.
  const [asking, setAsking] = useState<{ pipeline: string; stage: string } | null>(null);
  const run = async () => {
    const stage = await aggregation.run(pipeline);
    if (stage) setAsking({ pipeline, stage });
  };
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
            void run();
          }}
        />
      </label>
      <div className="mongo-toolbar">
        <button type="button" className="btn btn-primary" disabled={aggregation.busy} onClick={() => void run()}>
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
      <ConfirmModal title={`Run a pipeline with ${asking?.stage ?? ""}?`} action="Run" subject={asking} onClose={() => setAsking(null)} onConfirm={(asked) => void aggregation.run(asked.pipeline, true)}>
        <p className="mongo-confirm">
          {asking?.stage === "$out" ? "$out replaces the whole collection it names with the results" : "$merge writes the results into the collection it names, over documents already there"}, on the server. This cannot be undone.
        </p>
      </ConfirmModal>
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
  const showMaking = (next: typeof making) => {
    indexes.clearError();
    setMaking(next);
  };
  const properties = (index: IndexView) =>
    [index.unique && "unique", index.sparse && "sparse", index.ttlSeconds !== null && `TTL ${index.ttlSeconds}s`, index.partial && `partial ${index.partial}`].filter(Boolean).join(" · ");

  return (
    <div className="mongo-view">
      <div className="mongo-toolbar">
        <button type="button" className="btn btn-ghost" onClick={() => showMaking({ keys: "{ field: 1 }", name: "", unique: false })}>
          <Plus size={14} />
          Create index
        </button>
        <span className="mongo-spacer" />
        <span className="mongo-quiet">{indexes.indexes.length > 0 && `${indexes.indexes.length} ${indexes.indexes.length === 1 ? "index" : "indexes"}`}</span>
      </div>
      {indexes.error && !making && <ErrorNote message={indexes.error} onClose={indexes.clearError} />}
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
        onClose={() => showMaking(null)}
        footer={
          <>
            <button type="button" className="btn btn-ghost" onClick={() => showMaking(null)}>
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
        {indexes.error && <ErrorNote message={indexes.error} onClose={indexes.clearError} />}
      </Modal>
      <ConfirmModal title={`Drop index “${dropping?.name ?? ""}”?`} action="Drop" subject={dropping} onClose={() => setDropping(null)} onConfirm={(index) => void indexes.drop(index.name)}>
        <p className="mongo-confirm">
          Queries that used <code className="mongo-mono">{dropping?.keys}</code> read the whole collection after it{dropping?.unique ? ", and duplicates are no longer refused" : ""}.
        </p>
      </ConfirmModal>
    </div>
  );
}
