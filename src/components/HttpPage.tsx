import { useMemo, useRef, useState, type ReactNode } from "react";
import {
  Bot,
  ChevronDown,
  ChevronRight,
  Check,
  Columns2,
  Copy,
  Download,
  Eye,
  FileCode2,
  Folder,
  FolderOpen,
  Globe,
  History,
  Layers,
  Loader2,
  Lock,
  LockOpen,
  MoreHorizontal,
  Plus,
  Rows2,
  Send,
  Settings2,
  ShieldCheck,
  Trash2,
  Upload,
  WandSparkles,
  X,
} from "lucide-react";
import { Page, PageSheet } from "./Page";
import { CodeEditor } from "./CodeEditor";
import { Dropdown } from "./Dropdown";
import { Modal } from "./Modal";
import { PanelResizeHandle } from "./PanelResizeHandle";
import { useHttpClient, type Sent } from "../hooks/useHttpClient";
import { useCopied } from "../hooks/useCopied";
import { useShortcuts } from "../hooks/useShortcuts";
import { useStoredState } from "../hooks/useStoredState";
import { formatJson, formatXml } from "../lib/formatText";
import { matches, shortcutText } from "../lib/shortcuts";
import {
  AGENT_ACCESS,
  requestsUnder,
  METHODS,
  asHttpText,
  SECRET_MASK,
  effectiveAuth,
  findRequest,
  hasScripts,
  pretty,
  queryOf,
  resolveRequest,
  sizeLabel,
  statusTone,
  toCurl,
  withQuery,
  type Auth,
  type Body,
  type Collection,
  type EnvVariable,
  type Environment,
  type HistoryEntry,
  type HttpMethod,
  type HttpRequest,
  type Item,
  type Pair,
} from "../lib/httpClient";
import "./HttpPage.css";

/**
 * The HTTP page (docs/45-http-client.md): collections on the left, the open
 * requests on the right — what is written on top, what came back under it —
 * the environment and every send's history in the bar. Runs on sample data
 * until the backend has commands.
 */
export function HttpPage() {
  const http = useHttpClient();
  const [layout, setLayout] = useStoredState<Layout>("http.layout", "stacked", isLayout);
  const [sheet, setSheet] = useState<{ kind: "environment"; id: string } | { kind: "history" } | null>(null);
  const [expanded, setExpanded] = useState<Set<string>>(() => new Set(["orders", "orders-folder"]));
  const toggle = (id: string) =>
    setExpanded((s) => {
      const next = new Set(s);
      if (next.has(id)) next.delete(id);
      else next.add(id);
      return next;
    });

  useShortcuts({
    httpSend: () => {
      if (http.active) http.send(http.active);
    },
  });

  // A collection, folder or request being renamed or deleted — a new one is named straight away.
  const [renaming, setRenaming] = useState<{ id: string; name: string } | null>(null);
  const [deleting, setDeleting] = useState<{ id: string; name: string; what: string } | null>(null);
  const rename = () => {
    if (!renaming?.name.trim()) return;
    http.rename(renaming.id, renaming.name.trim());
    setRenaming(null);
  };
  const remove = () => {
    if (!deleting) return;
    http.remove(deleting.id);
    setDeleting(null);
  };
  const reveal = (id: string) => setExpanded((s) => new Set(s).add(id));
  const newCollection = () => {
    const id = http.newCollection();
    reveal(id);
    setRenaming({ id, name: "New collection" });
  };
  const newRequest = (parent: string) => {
    reveal(parent);
    setRenaming({ id: http.newRequest(parent), name: "New request" });
  };
  const menu = (node: TreeNode): Action[] => {
    const requests = requestsUnder(http.collections, node.id).length;
    const holds = `with ${requests === 1 ? "its request" : `its ${requests} requests`}`;
    return [
      ...(node.kind === "request" ? [] : [{ value: "new", label: "New request", run: () => newRequest(node.id) }]),
      { value: "rename", label: "Rename…", run: () => setRenaming({ id: node.id, name: node.name }) },
      ...(node.kind === "collection"
        ? [{ value: "export", label: "Export", hint: "A Postman v2.1 collection file", run: () => http.exportCollection(node.id) }]
        : []),
      {
        value: "delete",
        label: "Delete…",
        run: () =>
          setDeleting({
            id: node.id,
            name: node.name,
            what: node.kind === "request" ? "Deletes the request and its last response." : `Deletes the ${node.kind} ${requests > 0 ? holds : "— there is nothing in it"}.`,
          }),
      },
    ];
  };

  const editingEnv = sheet?.kind === "environment" ? (http.environments.find((e) => e.id === sheet.id) ?? null) : null;
  const openTab = http.active ? findRequest(http.collections, http.active) : null;

  return (
    <Page
      title="HTTP"
      bar={
        <>
          <Dropdown
            below
            heading="Environment"
            label={
              <span className="http-env-label">
                <Globe size={13} />
                {http.environment?.name ?? "No environment"}
              </span>
            }
            title="The environment whose variables fill {{…}}"
            options={http.environments.map((e) => ({
              value: e.id,
              label: e.name,
              hint: `${e.variables.find(({ key }) => key === "baseUrl")?.value ?? ""} · agent: ${AGENT_ACCESS.find((a) => a.value === e.agent)?.label}`,
              action: { icon: <Settings2 size={13} />, title: `Edit ${e.name}`, onRun: () => setSheet({ kind: "environment", id: e.id }) },
            }))}
            value={http.environment?.id ?? ""}
            onPick={http.setEnvironmentId}
          />
          <button
            type="button"
            className="http-icon-button"
            data-sheet-opener
            title={`Edit ${http.environment?.name ?? "the environment"}`}
            aria-label="Edit the environment"
            onClick={() => http.environment && setSheet({ kind: "environment", id: http.environment.id })}
          >
            <Settings2 size={14} />
          </button>
          <span className="http-spacer" />
          <div className="http-layout-switch" role="group" aria-label="Where the response goes">
            {LAYOUTS.map(({ value, icon: Icon, title }) => (
              <button key={value} type="button" className={`http-icon-button${layout === value ? " on" : ""}`} title={title} aria-label={title} aria-pressed={layout === value} onClick={() => setLayout(value)}>
                <Icon size={14} />
              </button>
            ))}
          </div>
          <button type="button" className="http-bar-button" data-sheet-opener title="Every request sent — yours and the agent's" onClick={() => setSheet({ kind: "history" })}>
            <History size={13} />
            History
          </button>
        </>
      }
      sheet={
        editingEnv ? (
          <PageSheet icon={Globe} title="Environment" label={`Environment ${editingEnv.name}`} onClose={() => setSheet(null)}>
            <EnvironmentForm key={editingEnv.id} environment={editingEnv} onSave={http.saveEnvironment} />
          </PageSheet>
        ) : sheet?.kind === "history" ? (
          <PageSheet icon={History} title="History" label="Every request sent" onClose={() => setSheet(null)}>
            <HistoryList
              entries={http.history}
              onOpen={(id) => {
                http.open(id);
                setSheet(null);
              }}
            />
          </PageSheet>
        ) : null
      }
    >
      <div className="http-layout">
        <nav className="http-tree" aria-label="Collections">
          <div className="http-tree-head">
            <span>Collections</span>
            <button type="button" className="http-icon-button" title="Import a Postman collection, environment or globals file (v2.1)" aria-label="Import">
              <Upload size={14} />
            </button>
            <button type="button" className="http-icon-button" title="New collection" aria-label="New collection" onClick={newCollection}>
              <Plus size={14} />
            </button>
          </div>
          <ul className="http-tree-list">
            {http.collections.map((c) => (
              <CollectionNode key={c.id} collection={c} expanded={expanded} toggle={toggle} active={http.active} open={http.open} menu={menu} />
            ))}
          </ul>
        </nav>

        <section className="http-main" aria-label="Requests">
          <div className="http-tabs" role="tablist" aria-label="Open requests">
            {http.tabs.map((id) => {
              const found = findRequest(http.collections, id);
              if (!found) return null;
              const on = id === http.active;
              return (
                <div key={id} className={`http-tab${on ? " on" : ""}`}>
                  <button
                    type="button"
                    role="tab"
                    aria-selected={on}
                    className="http-tab-name"
                    title={[...found.path, found.request.name].join(" › ")}
                    onClick={() => http.setActive(id)}
                  >
                    <MethodBadge method={found.request.method} />
                    {found.request.name}
                    {http.sent[id]?.busy && <Loader2 size={11} className="http-spin" />}
                  </button>
                  <button type="button" className="http-tab-close" title="Close" aria-label={`Close ${found.request.name}`} onClick={() => http.close(id)}>
                    <X size={12} />
                  </button>
                </div>
              );
            })}
          </div>
          {openTab ? (
            <RequestView
              key={openTab.request.id}
              collection={openTab.collection}
              request={openTab.request}
              environment={http.environment}
              layout={layout}
              sent={http.sent[openTab.request.id]}
              onEdit={http.edit}
              onSend={() => http.send(openTab.request.id)}
            />
          ) : (
            <p className="http-quiet http-empty">Pick a request on the left, or import a Postman collection.</p>
          )}
        </section>
      </div>
      <Modal
        title="Rename"
        open={renaming !== null}
        onClose={() => setRenaming(null)}
        footer={
          <>
            <button type="button" className="btn btn-ghost" onClick={() => setRenaming(null)}>
              Cancel
            </button>
            <button type="button" className="btn btn-primary" disabled={!renaming?.name.trim()} onClick={rename}>
              Rename
            </button>
          </>
        }
      >
        <input
          className="http-rename"
          // Focused with the name selected, as a new name replaces it more often than not.
          ref={(input) => input?.select()}
          value={renaming?.name ?? ""}
          aria-label="New name"
          onChange={(e) => renaming && setRenaming({ ...renaming, name: e.target.value })}
          // A form field's Enter, as in every settings form here — not a shortcut.
          onKeyDown={(e) => {
            if (!matches(e, "accept")) return;
            e.preventDefault();
            rename();
          }}
        />
      </Modal>
      <Modal
        title={`Delete “${deleting?.name ?? ""}”?`}
        open={deleting !== null}
        onClose={() => setDeleting(null)}
        footer={
          <>
            <button type="button" className="btn btn-ghost" onClick={() => setDeleting(null)}>
              Cancel
            </button>
            <button type="button" className="btn btn-primary" onClick={remove}>
              Delete
            </button>
          </>
        }
      >
        <p className="http-confirm">{deleting?.what}</p>
      </Modal>
    </Page>
  );
}

function MethodBadge({ method }: { method: HttpMethod }) {
  return <span className={`http-method http-method-${method.toLowerCase()}`}>{method === "DELETE" ? "DEL" : method === "OPTIONS" ? "OPT" : method}</span>;
}

type TreeNode = { id: string; name: string; kind: "collection" | "folder" | "request" };
type Action = { value: string; label: string; hint?: string; run: () => void };
type TreeProps = { expanded: Set<string>; toggle: (id: string) => void; active: string | null; open: (id: string) => void; menu: (node: TreeNode) => Action[] };

/** Says a collection or request carries Postman scripts — on hover, in words. */
function ScriptsMark() {
  return (
    <span className="http-tree-mark" title="Has Postman scripts — they show in a request's Scripts tab and do not run yet" aria-label="Has Postman scripts">
      <FileCode2 size={12} />
    </span>
  );
}

/** A row of the tree, and beside it what can be done to it. */
function TreeRow({ node, menu, children }: { node: TreeNode; menu: TreeProps["menu"]; children: ReactNode }) {
  const actions = menu(node);
  return (
    <div className="http-tree-item">
      {children}
      <span className="http-tree-menu">
        <Dropdown
          label={<MoreHorizontal size={14} aria-label={`Actions for ${node.name}`} />}
          title={`Actions for ${node.name}`}
          options={actions.map(({ value, label, hint }) => ({ value, label, hint }))}
          value=""
          onPick={(value) => actions.find((a) => a.value === value)?.run()}
          below
          right
        />
      </span>
    </div>
  );
}

function CollectionNode({ collection, ...tree }: { collection: Collection } & TreeProps) {
  const isOpen = tree.expanded.has(collection.id);
  return (
    <li>
      <TreeRow node={{ id: collection.id, name: collection.name, kind: "collection" }} menu={tree.menu}>
        <button type="button" className="http-tree-row http-tree-collection" aria-expanded={isOpen} onClick={() => tree.toggle(collection.id)}>
          {isOpen ? <ChevronDown size={13} className="http-tree-chevron" /> : <ChevronRight size={13} className="http-tree-chevron" />}
          <Layers size={13} className="http-tree-icon" />
          <span className="http-tree-name">{collection.name}</span>
          {hasScripts(collection.scripts) && <ScriptsMark />}
        </button>
      </TreeRow>
      {isOpen && (collection.items.length > 0 ? <Items items={collection.items} {...tree} /> : <p className="http-quiet http-tree-empty">No requests yet</p>)}
    </li>
  );
}

function Items({ items, ...tree }: { items: Item[] } & TreeProps) {
  return (
    <ul className="http-tree-list">
      {items.map((item) =>
        item.kind === "folder" ? (
          <li key={item.id}>
            <TreeRow node={{ id: item.id, name: item.name, kind: "folder" }} menu={tree.menu}>
              <button type="button" className="http-tree-row" aria-expanded={tree.expanded.has(item.id)} onClick={() => tree.toggle(item.id)}>
                {tree.expanded.has(item.id) ? <FolderOpen size={13} className="http-tree-icon" /> : <Folder size={13} className="http-tree-icon" />}
                <span className="http-tree-name">{item.name}</span>
              </button>
            </TreeRow>
            {tree.expanded.has(item.id) && <Items items={item.items} {...tree} />}
          </li>
        ) : (
          <li key={item.id}>
            <TreeRow node={{ id: item.id, name: item.name, kind: "request" }} menu={tree.menu}>
              <button type="button" className={`http-tree-row${tree.active === item.id ? " on" : ""}`} onClick={() => tree.open(item.id)}>
                <MethodBadge method={item.method} />
                <span className="http-tree-name">{item.name}</span>
                {hasScripts(item.scripts) && <ScriptsMark />}
              </button>
            </TreeRow>
          </li>
        ),
      )}
    </ul>
  );
}

function ago(at: number): string {
  const minutes = Math.round((Date.now() - at) / 60_000);
  if (minutes < 1) return "now";
  if (minutes < 60) return `${minutes}m`;
  const hours = Math.round(minutes / 60);
  return hours < 24 ? `${hours}h` : `${Math.round(hours / 24)}d`;
}

function HistoryList({ entries, onOpen }: { entries: HistoryEntry[]; onOpen: (requestId: string) => void }) {
  const [shown, setShown] = useState<string | null>(null);
  if (entries.length === 0) return <p className="http-quiet">Nothing sent yet.</p>;
  return (
    <ul className="http-history">
      {entries.map((h) => (
        <HistoryRow key={h.id} entry={h} expanded={shown === h.id} onToggle={() => setShown(shown === h.id ? null : h.id)} onOpen={onOpen} />
      ))}
    </ul>
  );
}

function HistoryRow({ entry, expanded, onToggle, onOpen }: { entry: HistoryEntry; expanded: boolean; onToggle: () => void; onOpen: (requestId: string) => void }) {
  const path = entry.url.replace(/^\{\{baseUrl\}\}/, "");
  return (
    <li>
      <button
        type="button"
        className={`http-tree-row http-history-row${expanded ? " on" : ""}`}
        aria-expanded={expanded}
        title={`${entry.method} ${entry.url} · ${entry.environment}${entry.agent ? ` · sent by the agent in “${entry.agent.chat}”` : ""}`}
        onClick={onToggle}
      >
        {expanded ? <ChevronDown size={13} className="http-tree-chevron" /> : <ChevronRight size={13} className="http-tree-chevron" />}
        <MethodBadge method={entry.method} />
        <span className="http-tree-name http-mono">{path}</span>
        <span className="http-quiet">{entry.environment}</span>
        {entry.agent && <Bot size={12} className="http-tree-mark" aria-label="Sent by the agent" />}
        {entry.status === null ? (
          <span className="http-status-text http-tone-server">timeout</span>
        ) : (
          <span className={`http-status-text http-tone-${statusTone(entry.status)}`}>{entry.status}</span>
        )}
        <span className="http-quiet">{ago(entry.at)}</span>
      </button>
      {expanded && <Exchange entry={entry} onOpen={onOpen} />}
    </li>
  );
}

/** One send in full: the request as it left, a secret masked, and what came back. */
function Exchange({ entry, onOpen }: { entry: HistoryEntry; onOpen: (requestId: string) => void }) {
  const { request, response } = entry;
  return (
    <div className="http-exchange">
      <div className="http-exchange-head">
        <span className="http-label">Request</span>
        <span className="http-quiet">
          {new Date(entry.at).toLocaleString()} · {entry.environment}
          {entry.agent && ` · the agent, in “${entry.agent.chat}”`}
        </span>
        <span className="http-spacer" />
        {entry.requestId && (
          <button type="button" className="http-bar-button" onClick={() => onOpen(entry.requestId!)}>
            Open request
          </button>
        )}
      </div>
      {request ? (
        <div className="http-code-box http-code-box-log">
          <CodeEditor text={asHttpText(`${entry.method} ${request.url}`, request.headers, request.body)} language="http" numbered={false} label="Request" />
        </div>
      ) : (
        <p className="http-quiet">Not kept — sent before history kept requests.</p>
      )}
      <div className="http-exchange-head">
        <span className="http-label">Response</span>
        {response && (
          <span className="http-quiet">
            {response.ms} ms · {sizeLabel(response.bytes)}
          </span>
        )}
      </div>
      {response ? (
        <div className="http-code-box http-code-box-log">
          <CodeEditor
            text={asHttpText(`HTTP/1.1 ${response.status} ${response.statusText}`, response.headers, response.body && pretty(response.body, response.contentType))}
            language="http"
            numbered={false}
            label="Response"
          />
        </div>
      ) : (
        <p className={entry.error ? "http-error" : "http-quiet"}>{entry.error ?? "Not kept — sent before history kept responses."}</p>
      )}
    </div>
  );
}

type Tab = "params" | "headers" | "auth" | "body" | "scripts" | "preview";

/** The response under the request, or beside it. */
type Layout = "stacked" | "side";
const LAYOUTS = [
  { value: "stacked", icon: Rows2, title: "Response under the request" },
  { value: "side", icon: Columns2, title: "Response beside the request" },
] as const;
const isLayout = (v: unknown): v is Layout => v === "stacked" || v === "side";

/**
 * Neither side closes: the request keeps its tabs and a couple of lines of
 * body, the response its tabs and a few lines — under it, or beside it.
 */
const PANE_MIN = { stacked: 160, side: 400 };
const RESPONSE_MIN = { stacked: 160, side: 320 };
const isSize = (v: unknown): v is number | null => v === null || (typeof v === "number" && v > 0);

/** The address on top; under it what is written and what came back, one under the other or side by side. */
function RequestView({
  collection,
  request,
  environment,
  layout,
  sent,
  onEdit,
  onSend,
}: {
  collection: Collection;
  request: HttpRequest;
  environment: Environment | null;
  layout: Layout;
  sent: Sent | undefined;
  onEdit: (r: HttpRequest) => void;
  onSend: () => void;
}) {
  const [tab, setTab] = useState<Tab>(request.body.mode === "none" ? "params" : "body");
  // Dragged by the line between the request and the response, a size per layout. Until then, stacked,
  // the request takes what it needs up to 45%; side by side, half.
  const [height, setHeight] = useStoredState<number | null>("http.requestHeight", null, isSize);
  const [width, setWidth] = useStoredState<number | null>("http.requestWidth", null, isSize);
  const split = useRef<HTMLDivElement>(null);
  const pane = useRef<HTMLDivElement>(null);
  const stacked = layout === "stacked";
  const resize = (delta: number) => {
    if (!split.current || !pane.current) return;
    const whole = split.current.getBoundingClientRect();
    const own = pane.current.getBoundingClientRect();
    const fit = (from: number | null, now: number, room: number) => Math.max(PANE_MIN[layout], Math.min((from ?? now) + delta, room - RESPONSE_MIN[layout]));
    if (stacked) setHeight((h) => fit(h, own.height, whole.bottom - own.top));
    else setWidth((w) => fit(w, own.width, whole.right - own.left));
  };
  const size = stacked ? (height === null ? undefined : { height, maxHeight: "none" }) : width === null ? undefined : { width, flexBasis: "auto" };
  const resolved = useMemo(() => resolveRequest(collection, request, environment), [collection, request, environment]);
  const { copied, copy } = useCopied();
  const missing = resolved.missing.length > 0 ? `Not in ${environment?.name ?? "any environment"}: ${resolved.missing.map((m) => `{{${m}}}`).join(", ")}` : undefined;
  const params = queryOf(request.url);
  const set = (patch: Partial<HttpRequest>) => onEdit({ ...request, ...patch });
  const count = (n: number) => (n > 0 ? <span className="http-count">{n}</span> : null);
  const scripts = hasScripts(request.scripts) || hasScripts(collection.scripts);

  return (
    <div className="http-request">
      <div className={`http-url-row${missing ? " missing" : ""}`} title={missing}>
        <Dropdown
          below
          label={<MethodBadge method={request.method} />}
          title="Method"
          options={METHODS.map((m) => ({ value: m }))}
          value={request.method}
          onPick={(m) => set({ method: m as HttpMethod })}
        />
        <input
          className="http-url http-mono"
          aria-label="Address"
          spellCheck={false}
          value={request.url}
          onChange={(e) => set({ url: e.target.value })}
          placeholder="{{baseUrl}}/path — or paste a curl command"
        />
        <button
          type="button"
          className="http-icon-button"
          title={copied ? "Copied" : "Copy as cURL — secrets as their {{name}}, never their value"}
          aria-label="Copy as cURL"
          onClick={() => copy(toCurl(resolveRequest(collection, request, environment, "name")))}
        >
          {copied ? <Check size={14} /> : <Copy size={14} />}
        </button>
        <button type="button" className="http-send" title={`Send (${shortcutText("httpSend")})`} disabled={sent?.busy} onClick={onSend}>
          {sent?.busy ? <Loader2 size={14} className="http-spin" /> : <Send size={14} />}
          Send
        </button>
      </div>

      <div ref={split} className={`http-split ${layout}`}>
        <div ref={pane} className="http-request-pane" style={size}>
          <div className="http-subtabs" role="tablist" aria-label="Request">
            {(
              [
                ["params", "Params", count(params.length)],
                ["headers", "Headers", count(request.headers.filter((h) => !h.disabled).length)],
                ["auth", "Auth", null],
                ["body", "Body", request.body.mode === "none" ? null : <span className="http-dot" />],
                ["scripts", "Scripts", scripts ? <span className="http-dot http-dot-amber" /> : null],
                ["preview", <Eye size={15} />, missing || resolved.notes.length ? <span className="http-dot http-dot-amber" /> : null],
              ] as [Tab, ReactNode, ReactNode][]
            ).map(([id, label, extra]) => (
              <button
                key={id}
                type="button"
                role="tab"
                aria-selected={tab === id}
                className={`http-subtab${tab === id ? " on" : ""}${id === "preview" ? " http-subtab-aside" : ""}`}
                title={id === "preview" ? "Preview — the request as it will leave, variables and auth filled in" : undefined}
                aria-label={id === "preview" ? "Preview" : undefined}
                onClick={() => setTab(id)}
              >
                {label}
                {extra}
              </button>
            ))}
          </div>

          <div className={`http-editor${(tab === "body" && request.body.mode === "raw") || tab === "scripts" || tab === "preview" ? " fills" : ""}`}>
            {tab === "params" && <PairsTable rows={params} keyLabel="Parameter" onChange={(rows) => set({ url: withQuery(request.url, rows) })} />}
            {tab === "headers" && <PairsTable rows={request.headers} keyLabel="Header" onChange={(headers) => set({ headers })} />}
            {tab === "auth" && <AuthEditor auth={request.auth} inherited={effectiveAuth(collection, request.id)} onChange={(auth) => set({ auth })} />}
            {tab === "body" && <BodyEditor body={request.body} onChange={(body) => set({ body })} />}
            {tab === "scripts" && <ScriptsView collection={collection} request={request} />}
            {tab === "preview" && <PreviewView resolved={resolved} missing={missing} />}
          </div>
        </div>
        <PanelResizeHandle
          axis={stacked ? "y" : "x"}
          ariaLabel="Resize the request and the response"
          onResize={resize}
          onResizeEnd={(px) => px && (stacked ? setHeight : setWidth)(Math.round(px))}
        />
        <ResponseView sent={sent} scripts={scripts} />
      </div>
    </div>
  );
}

/** The request as it will leave — what Send sends, a secret masked. */
function PreviewView({ resolved, missing }: { resolved: ReturnType<typeof resolveRequest>; missing?: string }) {
  // The body as written, not reformatted: a preview that pretty-prints shows `1250` for the `1250.00` sent.
  const text = asHttpText(`${resolved.method} ${resolved.url}`, resolved.headers, resolved.body);
  return (
    <div className="http-form">
      {missing && <p className="http-error">{missing} — nothing would be sent.</p>}
      {resolved.notes.map((n) => (
        <p key={n} className="http-note">
          <FileCode2 size={13} />
          {n[0].toUpperCase() + n.slice(1)}.
        </p>
      ))}
      <div className="http-code-box http-code-box-fill">
        <CodeEditor text={text} language="http" numbered={false} label="The request as it will leave" />
      </div>
    </div>
  );
}

function PairsTable({ rows, keyLabel, onChange }: { rows: Pair[]; keyLabel: string; onChange: (rows: Pair[]) => void }) {
  // The last row is always empty: typing into it adds one.
  const shown = [...rows, { key: "", value: "" }];
  const put = (i: number, patch: Partial<Pair>) => {
    const next = shown.map((r, j) => (j === i ? { ...r, ...patch } : r));
    onChange(next.filter(({ key, value }, j) => j < rows.length || key !== "" || value !== ""));
  };
  return (
    <table className="http-pairs">
      <thead>
        <tr>
          <th aria-label="On" />
          <th>{keyLabel}</th>
          <th>Value</th>
          <th aria-label="Remove" />
        </tr>
      </thead>
      <tbody>
        {shown.map((r, i) => {
          const last = i === rows.length;
          return (
            <tr key={i} className={r.disabled ? "off" : ""}>
              <td>{!last && <input type="checkbox" aria-label={`Send ${r.key}`} checked={!r.disabled} onChange={(e) => put(i, { disabled: !e.target.checked })} />}</td>
              <td>
                <input className="http-mono" aria-label={keyLabel} spellCheck={false} value={r.key} placeholder={last ? `New ${keyLabel.toLowerCase()}` : ""} onChange={(e) => put(i, { key: e.target.value })} />
              </td>
              <td>
                <input className="http-mono" aria-label="Value" spellCheck={false} value={r.value} onChange={(e) => put(i, { value: e.target.value })} />
              </td>
              <td>
                {!last && (
                  <button type="button" className="http-icon-button" title="Remove" aria-label={`Remove ${r.key}`} onClick={() => onChange(rows.filter((_, j) => j !== i))}>
                    <Trash2 size={13} />
                  </button>
                )}
              </td>
            </tr>
          );
        })}
      </tbody>
    </table>
  );
}

const AUTH_TYPES = [
  { value: "inherit", label: "Inherit from parent" },
  { value: "noauth", label: "No auth" },
  { value: "bearer", label: "Bearer token" },
  { value: "basic", label: "Basic auth" },
  { value: "apikey", label: "API key" },
];

function authLine(auth: Auth): string {
  switch (auth.type) {
    case "bearer":
      return `Bearer ${auth.token}`;
    case "basic":
      return `Basic ${auth.username}:•••`;
    case "apikey":
      return `${auth.key}: ${auth.value} (${auth.in})`;
    case "other":
      return `${auth.name} — kept from Postman`;
    default:
      return "No auth";
  }
}

function AuthEditor({ auth, inherited, onChange }: { auth: Auth; inherited: { auth: Auth; from: string } | null; onChange: (a: Auth) => void }) {
  const pick = (type: string) => {
    if (type === "bearer") onChange({ type, token: "{{token}}" });
    else if (type === "basic") onChange({ type, username: "", password: "" });
    else if (type === "apikey") onChange({ type, key: "X-Api-Key", value: "{{apiKey}}", in: "header" });
    else onChange({ type: type as "inherit" | "noauth" });
  };
  return (
    <div className="http-form">
      <div className="http-field-row">
        <span className="http-label">Type</span>
        {auth.type === "other" ? (
          <span className="http-kept">{auth.name} — kept from Postman, not editable here</span>
        ) : (
          <Dropdown below label={AUTH_TYPES.find((t) => t.value === auth.type)?.label} options={AUTH_TYPES} value={auth.type} onPick={pick} />
        )}
      </div>
      {auth.type === "inherit" && inherited && (
        <p className="http-quiet">
          From <strong>{inherited.from}</strong>: <span className="http-mono">{authLine(inherited.auth)}</span>
          {inherited.auth.type === "other" && " — not sent yet, comes with HC-6"}
        </p>
      )}
      {auth.type === "bearer" && <Field label="Token" value={auth.token} onChange={(token) => onChange({ ...auth, token })} />}
      {auth.type === "basic" && (
        <>
          <Field label="Username" value={auth.username} onChange={(username) => onChange({ ...auth, username })} />
          <Field label="Password" value={auth.password} onChange={(password) => onChange({ ...auth, password })} />
        </>
      )}
      {auth.type === "apikey" && (
        <>
          <Field label="Key" value={auth.key} onChange={(key) => onChange({ ...auth, key })} />
          <Field label="Value" value={auth.value} onChange={(value) => onChange({ ...auth, value })} />
        </>
      )}
    </div>
  );
}

function Field({ label, value, onChange }: { label: string; value: string; onChange: (v: string) => void }) {
  return (
    <label className="http-field-row">
      <span className="http-label">{label}</span>
      <input className="http-input http-mono" spellCheck={false} value={value} onChange={(e) => onChange(e.target.value)} />
    </label>
  );
}

const BODY_MODES = [
  { value: "none", label: "none" },
  { value: "raw", label: "raw" },
  { value: "urlencoded", label: "x-www-form-urlencoded" },
];

function BodyEditor({ body, onChange }: { body: Body; onChange: (b: Body) => void }) {
  if (body.mode === "other")
    return (
      <p className="http-kept">
        A <strong>{body.name}</strong> body — kept from Postman as it came, not editable or sent yet (HC-6).
      </p>
    );
  return (
    <div className="http-form">
      <div className="http-segments" role="radiogroup" aria-label="Body">
        {BODY_MODES.map((m) => (
          <button
            key={m.value}
            type="button"
            role="radio"
            aria-checked={body.mode === m.value}
            className={`http-segment${body.mode === m.value ? " on" : ""}`}
            onClick={() =>
              onChange(m.value === "raw" ? { mode: "raw", language: "json", raw: "" } : m.value === "urlencoded" ? { mode: "urlencoded", fields: [] } : { mode: "none" })
            }
          >
            {m.label}
          </button>
        ))}
        {body.mode === "raw" && (
          <>
            <span className="http-spacer" />
            {body.language !== "text" && (
              <button
                type="button"
                className="http-icon-button"
                title={`Beautify the ${body.language.toUpperCase()} — numbers, {{variables}} and comments stay as written`}
                aria-label="Beautify"
                onClick={() => onChange({ ...body, raw: body.language === "json" ? formatJson(body.raw) : formatXml(body.raw) })}
              >
                <WandSparkles size={14} />
              </button>
            )}
            <Dropdown
              below
              right
              label={body.language.toUpperCase()}
              options={["json", "text", "xml"].map((l) => ({ value: l, label: l.toUpperCase() }))}
              value={body.language}
              onPick={(language) => onChange({ ...body, language: language as "json" | "text" | "xml" })}
            />
          </>
        )}
      </div>
      {body.mode === "none" && <p className="http-quiet">This request sends no body.</p>}
      {body.mode === "raw" && (
        <div className="http-code-box http-code-box-input">
          <CodeEditor text={body.raw} language={body.language} label="Body" onChange={(raw) => onChange({ ...body, raw })} />
        </div>
      )}
      {body.mode === "urlencoded" && <PairsTable rows={body.fields} keyLabel="Field" onChange={(fields) => onChange({ ...body, fields })} />}
    </div>
  );
}

/** The scripts that run with this request, picked one at a time, as the Body tab picks its mode. */
function ScriptsView({ collection, request }: { collection: Collection; request: HttpRequest }) {
  // In the order Postman runs them: the collection's before the request's, before sending and after it.
  const scripts = [
    { id: "collection-pre", label: `${collection.name} · pre-request`, text: collection.scripts.prerequest },
    { id: "pre", label: "Pre-request", text: request.scripts.prerequest },
    { id: "collection-post", label: `${collection.name} · post-response`, text: collection.scripts.test },
    { id: "post", label: "Post-response", text: request.scripts.test },
  ].filter((script) => script.text?.trim());
  const [shown, setShown] = useState(scripts[0]?.id);
  const current = scripts.find((script) => script.id === shown) ?? scripts[0];
  if (!current) return <p className="http-quiet">No scripts on this request or above it.</p>;
  return (
    <div className="http-form">
      <div className="http-segments" role="tablist" aria-label="Scripts, in the order they run">
        {scripts.map((script) => (
          <button
            key={script.id}
            type="button"
            role="tab"
            aria-selected={script.id === current.id}
            className={`http-segment${script.id === current.id ? " on" : ""}`}
            onClick={() => setShown(script.id)}
          >
            {script.label}
          </button>
        ))}
        <span className="http-spacer" />
        <span className="http-quiet" title="The request goes without them until scripts run (HC-5)">
          not run yet
        </span>
      </div>
      <div className="http-code-box http-code-box-fill">
        <CodeEditor text={current.text ?? ""} language="javascript" label={current.label} />
      </div>
    </div>
  );
}

type ResTab = "body" | "headers" | "tests";

/** `14:32` today, `27 Oct 14:32` before. */
function receivedAt(at: number): string {
  const d = new Date(at);
  const time = d.toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" });
  return d.toDateString() === new Date().toDateString() ? time : `${d.toLocaleDateString([], { day: "numeric", month: "short" })} ${time}`;
}

function ResponseView({ sent, scripts }: { sent: Sent | undefined; scripts: boolean }) {
  const [tab, setTab] = useState<ResTab>("body");
  const response = sent?.response;
  const body = response && pretty(response.body, response.contentType);
  const { copied, copy } = useCopied();
  const count = (n: number | undefined) => (n ? <span className="http-count">{n}</span> : null);
  const tabs: [ResTab, string, ReactNode][] = [
    ["body", "Response", null],
    ["headers", "Headers", count(response?.headers.length)],
    ["tests", "Tests", response?.tests.length ? <span className="http-count">{`${response.tests.filter((x) => x.passed).length}/${response.tests.length}`}</span> : null],
  ];
  return (
    <section className="http-response" aria-label="Response" aria-busy={sent?.busy}>
      <div className="http-response-head">
        <div className="http-subtabs" role="tablist" aria-label="Response">
          {tabs.map(([id, label, extra]) => (
            <button key={id} type="button" role="tab" aria-selected={tab === id} className={`http-subtab${tab === id ? " on" : ""}`} onClick={() => setTab(id)}>
              {label}
              {extra}
            </button>
          ))}
        </div>
        <span className="http-spacer" />
        {sent?.busy && <Loader2 size={13} className="http-spin" />}
        {response && (
          <span className={`http-status http-tone-${statusTone(response.status)}`}>
            {response.status} {response.statusText}
          </span>
        )}
        {sent?.at !== undefined && (
          <span className="http-quiet http-response-meta" title={`Received ${new Date(sent.at).toLocaleString()}${sent.environment ? ` from ${sent.environment}` : ""}`}>
            {[response && `${response.ms} ms`, response && sizeLabel(response.bytes), receivedAt(sent.at), sent.environment].filter(Boolean).join(" · ")}
          </span>
        )}
        {body !== undefined && (
          <button type="button" className="http-icon-button" title={copied ? "Copied" : "Copy the response's body"} aria-label="Copy the response's body" onClick={() => copy(body)}>
            {copied ? <Check size={13} /> : <Copy size={13} />}
          </button>
        )}
        {response && (
          <button type="button" className="http-icon-button" title="Save the response to a file" aria-label="Save the response">
            <Download size={13} />
          </button>
        )}
      </div>
      <div className="http-response-body">
        {sent?.notSent && <p className="http-error">{sent.notSent}</p>}
        {sent?.error && <p className="http-error">{sent.error}</p>}
        {!response && !sent?.error && !sent?.busy && <p className="http-quiet http-empty">Send the request to see the response here.</p>}
        {response && tab === "body" &&
          (response.body ? (
            <div className="http-code-box http-code-box-fill">
              <CodeEditor text={body ?? ""} language={/json/.test(response.contentType) ? "json" : /xml/.test(response.contentType) ? "xml" : "text"} label="Response body" />
            </div>
          ) : (
            <p className="http-quiet">No body.</p>
          ))}
        {response && tab === "headers" && (
          <table className="http-pairs http-pairs-read">
            <tbody>
              {response.headers.map((h) => (
                <tr key={h.key}>
                  <td className="http-mono">{h.key}</td>
                  <td className="http-mono">{h.value}</td>
                </tr>
              ))}
            </tbody>
          </table>
        )}
        {response && tab === "tests" &&
          (response.tests.length === 0 ? (
            <p className="http-quiet">{scripts ? "Scripts are not run yet — no tests." : "No pm.test in this request's scripts."}</p>
          ) : (
            <ul className="http-tests">
              {response.tests.map((t) => (
                <li key={t.name} className={t.passed ? "pass" : "fail"}>
                  <span className="http-test-mark">{t.passed ? "PASS" : "FAIL"}</span>
                  {t.name}
                  {t.error && <span className="http-quiet"> — {t.error}</span>}
                </li>
              ))}
            </ul>
          ))}
      </div>
    </section>
  );
}

function EnvironmentForm({ environment, onSave }: { environment: Environment; onSave: (e: Environment) => void }) {
  const [draft, setDraft] = useState(environment);
  const put = (i: number, patch: Partial<EnvVariable>) => setDraft((d) => ({ ...d, variables: d.variables.map((v, j) => (j === i ? { ...v, ...patch } : v)) }));
  const access = AGENT_ACCESS.find((a) => a.value === draft.agent);
  return (
    <div className="http-form http-env">
      <label className="http-field-row">
        <span className="http-label">Name</span>
        <input className="http-input" value={draft.name} onChange={(e) => setDraft({ ...draft, name: e.target.value })} />
      </label>

      <div className="http-field-row">
        <span className="http-label">Agent</span>
        <Dropdown
          below
          label={access?.label}
          heading="What the agent may send here"
          options={AGENT_ACCESS.map((a) => ({ value: a.value, label: a.label, hint: a.hint }))}
          value={draft.agent}
          onPick={(agent) => setDraft({ ...draft, agent: agent as Environment["agent"] })}
        />
        <span className="http-quiet">{access?.hint}</span>
      </div>

      <table className="http-pairs">
        <thead>
          <tr>
            <th aria-label="On" />
            <th>Variable</th>
            <th>Value</th>
            <th aria-label="Secret" />
          </tr>
        </thead>
        <tbody>
          {draft.variables.map((v, i) => (
            <tr key={i} className={v.enabled ? "" : "off"}>
              <td>
                <input type="checkbox" aria-label={`Use ${v.key}`} checked={v.enabled} onChange={(e) => put(i, { enabled: e.target.checked })} />
              </td>
              <td>
                <input className="http-mono" aria-label="Variable" spellCheck={false} value={v.key} onChange={(e) => put(i, { key: e.target.value })} />
              </td>
              <td>
                <input
                  className="http-mono"
                  aria-label="Value"
                  spellCheck={false}
                  type={v.secret ? "password" : "text"}
                  value={v.value}
                  placeholder={v.secret ? `${SECRET_MASK} saved — type to replace` : ""}
                  onChange={(e) => put(i, { value: e.target.value })}
                />
              </td>
              <td>
                <button
                  type="button"
                  className={`http-icon-button${v.secret ? " on" : ""}`}
                  title={v.secret ? "Secret: sealed, never shown again, scrubbed from what the agent reads" : "Make it a secret"}
                  aria-label={v.secret ? `${v.key} is a secret` : `Make ${v.key} a secret`}
                  aria-pressed={v.secret}
                  onClick={() => put(i, { secret: !v.secret })}
                >
                  {v.secret ? <Lock size={13} /> : <LockOpen size={13} />}
                </button>
              </td>
            </tr>
          ))}
        </tbody>
      </table>
      <button type="button" className="http-bar-button" onClick={() => setDraft({ ...draft, variables: [...draft.variables, { key: "", value: "", secret: false, enabled: true }] })}>
        <Plus size={13} />
        Variable
      </button>

      <div className="http-field-row">
        <span className="http-label">Certificate</span>
        <span className={draft.trustedCert ? "http-ok" : "http-quiet"}>
          <ShieldCheck size={13} /> {draft.trustedCert ? "The stand's own certificate is trusted" : "System trust store"}
        </span>
      </div>

      <p className="http-quiet">Sealed on this computer under the app's master key, as the API keys are. A secret's value is never shown again — leave it empty to keep it.</p>
      <div className="http-actions">
        <button type="button" className="http-bar-button" title="As a Postman environment file — secrets empty">
          <Download size={13} />
          Export
        </button>
        <span className="http-spacer" />
        <button type="button" className="http-send" onClick={() => onSave(draft)}>
          Save
        </button>
      </div>
    </div>
  );
}
