import { useMemo, useState, type ReactNode } from "react";
import {
  Bot,
  ChevronDown,
  ChevronRight,
  Check,
  Copy,
  Download,
  FileCode2,
  Folder,
  FolderOpen,
  Globe,
  History,
  Layers,
  Loader2,
  Lock,
  LockOpen,
  Plus,
  Send,
  Settings2,
  ShieldCheck,
  Trash2,
  Upload,
  X,
} from "lucide-react";
import { Page, PageSheet } from "./Page";
import { CodeEditor } from "./CodeEditor";
import { Dropdown } from "./Dropdown";
import { useHttpClient, type Sent } from "../hooks/useHttpClient";
import { useCopied } from "../hooks/useCopied";
import { useShortcuts } from "../hooks/useShortcuts";
import { shortcutText } from "../lib/shortcuts";
import {
  AGENT_ACCESS,
  METHODS,
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
  const [sheet, setSheet] = useState<{ kind: "environment"; id: string } | { kind: "history" } | null>(null);
  const [expanded, setExpanded] = useState<Set<string>>(() => new Set(["orders", "orders-folder"]));
  const toggle = (id: string) =>
    setExpanded((s) => {
      const next = new Set(s);
      if (next.has(id)) next.delete(id);
      else next.add(id);
      return next;
    });

  useShortcuts({ httpSend: () => http.active && http.send(http.active) });

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
            <button type="button" className="http-icon-button" title="New collection" aria-label="New collection">
              <Plus size={14} />
            </button>
          </div>
          <ul className="http-tree-list">
            {http.collections.map((c) => (
              <CollectionNode key={c.id} collection={c} expanded={expanded} toggle={toggle} active={http.active} open={http.open} />
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
              sent={http.sent[openTab.request.id]}
              history={http.history.filter((h) => h.requestId === openTab.request.id)}
              onEdit={http.edit}
              onSend={() => http.send(openTab.request.id)}
            />
          ) : (
            <p className="http-quiet http-empty">Pick a request on the left, or import a Postman collection.</p>
          )}
        </section>
      </div>
    </Page>
  );
}

function MethodBadge({ method }: { method: HttpMethod }) {
  return <span className={`http-method http-method-${method.toLowerCase()}`}>{method === "DELETE" ? "DEL" : method === "OPTIONS" ? "OPT" : method}</span>;
}

type TreeProps = { expanded: Set<string>; toggle: (id: string) => void; active: string | null; open: (id: string) => void };

function CollectionNode({ collection, ...tree }: { collection: Collection } & TreeProps) {
  const isOpen = tree.expanded.has(collection.id);
  return (
    <li>
      <button type="button" className="http-tree-row http-tree-collection" aria-expanded={isOpen} onClick={() => tree.toggle(collection.id)}>
        {isOpen ? <ChevronDown size={13} className="http-tree-chevron" /> : <ChevronRight size={13} className="http-tree-chevron" />}
        <Layers size={13} className="http-tree-icon" />
        <span className="http-tree-name">{collection.name}</span>
        {hasScripts(collection.scripts) && <FileCode2 size={12} className="http-tree-mark" aria-label="Has Postman scripts" />}
      </button>
      {isOpen && <Items items={collection.items} {...tree} />}
    </li>
  );
}

function Items({ items, ...tree }: { items: Item[] } & TreeProps) {
  return (
    <ul className="http-tree-list">
      {items.map((item) =>
        item.kind === "folder" ? (
          <li key={item.id}>
            <button type="button" className="http-tree-row" aria-expanded={tree.expanded.has(item.id)} onClick={() => tree.toggle(item.id)}>
              {tree.expanded.has(item.id) ? <FolderOpen size={13} className="http-tree-icon" /> : <Folder size={13} className="http-tree-icon" />}
              <span className="http-tree-name">{item.name}</span>
            </button>
            {tree.expanded.has(item.id) && <Items items={item.items} {...tree} />}
          </li>
        ) : (
          <li key={item.id}>
            <button type="button" className={`http-tree-row${tree.active === item.id ? " on" : ""}`} onClick={() => tree.open(item.id)}>
              <MethodBadge method={item.method} />
              <span className="http-tree-name">{item.name}</span>
              {hasScripts(item.scripts) && <FileCode2 size={12} className="http-tree-mark" aria-label="Has Postman scripts" />}
            </button>
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

function HistoryList({ entries, onOpen }: { entries: HistoryEntry[]; onOpen?: (requestId: string) => void }) {
  if (entries.length === 0) return <p className="http-quiet">Not sent yet.</p>;
  return (
    <ul className="http-history">
      {entries.map((h) => (
        <HistoryRow key={h.id} entry={h} onOpen={onOpen} />
      ))}
    </ul>
  );
}

function HistoryRow({ entry, onOpen }: { entry: HistoryEntry; onOpen?: (requestId: string) => void }) {
  const path = entry.url.replace(/^\{\{baseUrl\}\}/, "");
  const opens = onOpen && entry.requestId;
  return (
    <li>
      <button
        type="button"
        className={`http-tree-row http-history-row${opens ? "" : " still"}`}
        title={`${entry.method} ${entry.url} · ${entry.environment}${entry.agent ? ` · sent by the agent in “${entry.agent.chat}”` : ""}`}
        onClick={() => opens && onOpen(entry.requestId!)}
      >
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
    </li>
  );
}

type Tab = "params" | "headers" | "auth" | "body" | "scripts" | "preview";

/** What is written on top, what came back under it. */
function RequestView({
  collection,
  request,
  environment,
  sent,
  history,
  onEdit,
  onSend,
}: {
  collection: Collection;
  request: HttpRequest;
  environment: Environment | null;
  sent: Sent | undefined;
  history: HistoryEntry[];
  onEdit: (r: HttpRequest) => void;
  onSend: () => void;
}) {
  const [tab, setTab] = useState<Tab>(request.body.mode === "none" ? "params" : "body");
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

      <div className="http-subtabs" role="tablist" aria-label="Request">
        {(
          [
            ["params", "Params", count(params.length)],
            ["headers", "Headers", count(request.headers.filter((h) => !h.disabled).length)],
            ["auth", "Auth", null],
            ["body", "Body", request.body.mode === "none" ? null : <span className="http-dot" />],
            ["scripts", "Scripts", scripts ? <span className="http-dot http-dot-amber" /> : null],
            ["preview", "Preview", missing || resolved.notes.length ? <span className="http-dot http-dot-amber" /> : null],
          ] as [Tab, string, ReactNode][]
        ).map(([id, label, extra]) => (
          <button
            key={id}
            type="button"
            role="tab"
            aria-selected={tab === id}
            className={`http-subtab${tab === id ? " on" : ""}${id === "preview" ? " http-subtab-aside" : ""}`}
            title={id === "preview" ? "The request as it will leave, variables and auth filled in" : undefined}
            onClick={() => setTab(id)}
          >
            {label}
            {extra}
          </button>
        ))}
      </div>

      <div className="http-editor">
        {tab === "params" && <PairsTable rows={params} keyLabel="Parameter" onChange={(rows) => set({ url: withQuery(request.url, rows) })} />}
        {tab === "headers" && <PairsTable rows={request.headers} keyLabel="Header" onChange={(headers) => set({ headers })} />}
        {tab === "auth" && <AuthEditor auth={request.auth} inherited={effectiveAuth(collection, request.id)} onChange={(auth) => set({ auth })} />}
        {tab === "body" && <BodyEditor body={request.body} onChange={(body) => set({ body })} />}
        {tab === "scripts" && <ScriptsView collection={collection} request={request} />}
        {tab === "preview" && <PreviewView resolved={resolved} missing={missing} />}
      </div>

      <ResponseView sent={sent} scripts={scripts} history={history} />
    </div>
  );
}

/** The request as it will leave — what Send sends, a secret masked. */
function PreviewView({ resolved, missing }: { resolved: ReturnType<typeof resolveRequest>; missing?: string }) {
  // The body as written, not reformatted: a preview that pretty-prints shows `1250` for the `1250.00` sent.
  const text = [`${resolved.method} ${resolved.url}`, ...resolved.headers.map((h) => `${h.key}: ${h.value}`), ...(resolved.body ? ["", resolved.body] : [])].join("\n");
  return (
    <div className="http-form">
      {missing && <p className="http-error">{missing} — nothing would be sent.</p>}
      {resolved.notes.map((n) => (
        <p key={n} className="http-note">
          <FileCode2 size={13} />
          {n[0].toUpperCase() + n.slice(1)}.
        </p>
      ))}
      <div className="http-code-box">
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

function ScriptsView({ collection, request }: { collection: Collection; request: HttpRequest }) {
  const blocks = [
    { where: `${collection.name} · pre-request`, text: collection.scripts.prerequest },
    { where: "This request · pre-request", text: request.scripts.prerequest },
    { where: "This request · test", text: request.scripts.test },
    { where: `${collection.name} · test`, text: collection.scripts.test },
  ].filter((b) => b.text?.trim());
  return (
    <div className="http-form">
      <p className="http-note">
        <FileCode2 size={13} />
        Not run yet — the request goes without them until HC-5.
      </p>
      {blocks.length === 0 && <p className="http-quiet">No scripts on this request or above it.</p>}
      {blocks.map((b) => (
        <div key={b.where} className="http-script">
          <div className="http-script-head">{b.where}</div>
          <CodeEditor text={b.text ?? ""} language="javascript" label={b.where} />
        </div>
      ))}
    </div>
  );
}

type ResTab = "body" | "headers" | "tests" | "history";

/** `14:32` today, `27 Oct 14:32` before. */
function receivedAt(at: number): string {
  const d = new Date(at);
  const time = d.toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" });
  return d.toDateString() === new Date().toDateString() ? time : `${d.toLocaleDateString([], { day: "numeric", month: "short" })} ${time}`;
}

function ResponseView({ sent, scripts, history }: { sent: Sent | undefined; scripts: boolean; history: HistoryEntry[] }) {
  const [tab, setTab] = useState<ResTab>("body");
  const response = sent?.response;
  const count = (n: number | undefined) => (n ? <span className="http-count">{n}</span> : null);
  const tabs: [ResTab, string, ReactNode][] = [
    ["body", "Response", null],
    ["headers", "Headers", count(response?.headers.length)],
    ["tests", "Tests", response?.tests.length ? <span className="http-count">{`${response.tests.filter((x) => x.passed).length}/${response.tests.length}`}</span> : null],
    ["history", "History", count(history.length)],
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
        {response && (
          <button type="button" className="http-icon-button" title="Save the response to a file" aria-label="Save the response">
            <Download size={13} />
          </button>
        )}
      </div>
      <div className="http-response-body">
        {tab !== "history" && sent?.notSent && <p className="http-error">{sent.notSent}</p>}
        {tab !== "history" && sent?.error && <p className="http-error">{sent.error}</p>}
        {tab !== "history" && !response && !sent?.error && !sent?.busy && <p className="http-quiet http-empty">Send the request to see the response here.</p>}
        {tab === "history" && <HistoryList entries={history} />}
        {response && tab === "body" &&
          (response.body ? (
            <div className="http-code-box http-code-box-fill">
              <CodeEditor text={pretty(response.body, response.contentType)} language={/json/.test(response.contentType) ? "json" : /xml/.test(response.contentType) ? "xml" : "text"} label="Response body" />
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
