import { useState, type KeyboardEvent } from "react";
import {
  ArrowRight,
  BookOpen,
  Check,
  Loader2,
  Plug,
  Plus,
  RefreshCw,
  Search,
  Puzzle,
  Sparkles,
  SquareKanban,
  SquareSlash,
  Wallet,
  Webhook,
  X,
  type LucideIcon,
} from "lucide-react";
import { Markdown } from "./Markdown";
import { DiffView } from "./DiffView";
import { Modal } from "./Modal";
import { Page, PagePanel } from "./Page";
import { sameEntry, useCatalog, useCatalogPreview, useCatalogUpdatePreview, type EntryKey } from "../hooks/useCatalog";
import { useFeatures } from "../hooks/useFeatures";
import type { FeatureView } from "../lib/features";
import type { CatalogEntry, CatalogKind, CatalogSource } from "../lib/catalog";
import type { PageContext } from "./pages";
import "./CatalogPage.css";

// In the order the page lists them.
const KINDS: { kind: CatalogKind; label: string; one: string; icon: LucideIcon }[] = [
  { kind: "skill", label: "Skills", one: "Skill", icon: Sparkles },
  { kind: "command", label: "Commands", one: "Command", icon: SquareSlash },
  { kind: "runbook", label: "Runbooks", one: "Runbook", icon: BookOpen },
  { kind: "mcp", label: "MCP servers", one: "MCP server", icon: Plug },
  { kind: "hooks", label: "Hooks", one: "Hooks", icon: Webhook },
];
const kindOf = (kind: CatalogKind) => KINDS.find((k) => k.kind === kind) ?? KINDS[0];

type Tab = CatalogKind | "all" | "features";
const TABS: { value: Tab; label: string }[] = [
  { value: "all", label: "All" },
  { value: "features", label: "Features" },
  ...KINDS.map(({ kind, label }) => ({ value: kind, label })),
];

/** A feature's icon, by its id; one without its own gets the generic piece. */
const FEATURE_ICONS: Record<string, LucideIcon> = { board: SquareKanban, "provider-balance": Wallet };
const featureIcon = (id: string) => FEATURE_ICONS[id] ?? Puzzle;
const featureMatches = (feature: FeatureView, query: string) => {
  const q = query.trim().toLowerCase();
  return !q || [feature.id, feature.name, feature.description].some((text) => text.toLowerCase().includes(q));
};

/** How many cards a kind shows under All before "Show all": two rows of three, three of two. */
const PREVIEW = 6;

/** `raw.githubusercontent.com/<owner>/<repo>/<branch>/` as the repository it is; any other address as it is. */
function sourceName(url: string): string {
  const github = /^https:\/\/raw\.githubusercontent\.com\/([^/]+)\/([^/]+)\//.exec(url);
  return github ? `${github[1]}/${github[2]}` : url.replace(/^https?:\/\//, "").replace(/\/$/, "");
}

/** A skill's frontmatter as the YAML it is: left in, `---` would draw it as a heading. */
const fencedFrontmatter = (text: string) => text.replace(/^---\r?\n([\s\S]*?)\r?\n---[ \t]*(?:\r?\n|$)/, "```yaml\n$1\n```\n");

const roleName = (role: string) => `${role[0].toUpperCase()}${role.slice(1)}`;

type Entry = { item: CatalogEntry; source: string };
const keyOf = ({ item, source }: Entry): EntryKey => ({ source, kind: item.kind, id: item.id });

function matches({ item }: Entry, query: string): boolean {
  const q = query.trim().toLowerCase();
  return !q || [item.id, item.name, item.description].some((text) => text.toLowerCase().includes(q));
}

/** What the card's button and the panel's line say for each state. */
const STATE_NOTE: Record<CatalogEntry["state"], string | null> = {
  available: null,
  installed: "Installed",
  update: "Installed — the catalog has a newer version",
  changed: "Installed, and changed here since",
  taken: "Something of this name is already in its place",
};

type Actions = {
  busy: EntryKey | null;
  /** Installs at once — or, for a server or hooks, asks first. */
  onInstall: (entry: EntryKey) => void;
};

/** What is in its place, as the panel names it: a server is a name in a shared file. */
const placeOf = (item: CatalogEntry) =>
  item.kind === "mcp" ? `the server ${item.id} in ${item.target}` : item.kind === "hooks" ? `its hooks in ${item.target}` : item.target;

/** What runs something on its own once installed: shown whole in a dialog before it is. */
const asks = (kind: CatalogKind) => kind === "mcp" || kind === "hooks";

function Card({ entry, icon: Icon, selected, onOpen, busy, onInstall }: { entry: Entry; icon: LucideIcon; selected: boolean; onOpen: () => void } & Actions) {
  const { item, source } = entry;
  const role = item.role && `For the ${roleName(item.role)} role`;
  const working = sameEntry(busy, keyOf(entry));
  const open = (e: KeyboardEvent) => {
    if (e.target === e.currentTarget && (e.key === "Enter" || e.key === " ")) {
      e.preventDefault();
      onOpen();
    }
  };
  return (
    <article
      className={`catalog-card${selected ? " selected" : ""}`}
      role="button"
      tabIndex={0}
      aria-label={item.name}
      aria-pressed={selected}
      onClick={onOpen}
      onKeyDown={open}
    >
      <div className={`catalog-card-ico ${item.kind}`}>
        <Icon size={16} />
      </div>
      <div className="catalog-card-body">
        <h3>{item.name}</h3>
        <p title={item.description}>{item.description}</p>
        <div className="catalog-card-meta">{[sourceName(source), role].filter(Boolean).join(" · ")}</div>
      </div>
      {item.state === "available" ? (
        <button
          type="button"
          className="catalog-card-add"
          disabled={item.target === null || busy !== null}
          title="Install"
          aria-label={`Install ${item.name}`}
          onClick={(e) => {
            e.stopPropagation();
            onInstall(keyOf(entry));
          }}
        >
          {working ? <Loader2 size={14} className="catalog-spin" /> : <Plus size={14} />}
        </button>
      ) : (
        <span className={`catalog-card-state ${item.state}`} title={STATE_NOTE[item.state] ?? undefined} aria-label={STATE_NOTE[item.state] ?? undefined}>
          {item.state === "taken" ? <X size={14} /> : <Check size={14} />}
        </span>
      )}
    </article>
  );
}

/** The panel right of the page, as the MCP pane sits right of the chat: one entry, what installing it does, and its text. */
function Details({
  entry,
  busy,
  error,
  onInstall,
  onRemove,
  onUpdate,
  onClose,
}: { entry: Entry; error: string | null; onRemove: (entry: EntryKey) => void; onUpdate: (entry: EntryKey) => void; onClose: () => void } & Actions) {
  const { item, source } = entry;
  const kind = kindOf(item.kind);
  const key = keyOf(entry);
  const preview = useCatalogPreview(key);
  const [confirming, setConfirming] = useState(false);
  const working = sameEntry(busy, key);
  const ours = item.state === "installed" || item.state === "update" || item.state === "changed";
  const newer = ours && item.installedRev !== null && item.installedRev !== item.rev;
  const place = placeOf(item);
  const file = item.kind === "skill" ? "SKILL.md" : item.path.split("/").pop();

  return (
    <PagePanel icon={kind.icon} title={kind.one} label={`${item.name} details`} onClose={onClose}>
      <h3 className="catalog-detail-name">{item.name}</h3>
      {item.description && <p className="catalog-detail-desc">{item.description}</p>}

      <div className="catalog-detail-actions">
        {ours ? (
          <>
            {newer && (
              <button type="button" className="btn btn-primary" disabled={busy !== null} onClick={() => onUpdate(key)}>
                Update
              </button>
            )}
            <button type="button" className="btn btn-ghost" disabled={busy !== null} onClick={() => (item.state === "changed" ? setConfirming(true) : onRemove(key))}>
              {working && <Loader2 size={13} className="catalog-spin" />}
              Remove
            </button>
          </>
        ) : (
          <button type="button" className="btn btn-primary" disabled={item.state !== "available" || item.target === null || busy !== null} onClick={() => onInstall(key)}>
            {working && <Loader2 size={13} className="catalog-spin" />}
            Install
          </button>
        )}
        {STATE_NOTE[item.state] && <span className={`catalog-detail-state ${item.state}`}>{STATE_NOTE[item.state]}</span>}
      </div>
      {error && <div className="catalog-detail-error">{error}</div>}
      {item.state === "taken" && (
        <p className="catalog-detail-note">
          {item.kind === "hooks"
            ? `Hooks named ${item.id} from another catalog are already in ${item.target}. They are left as they are.`
            : `${item.kind === "mcp" ? `A server named ${item.id} is already in ${item.target}` : `${item.target} is already there`} and was not installed from this catalog — yours, or another catalog's. It is left as it is.`}
        </p>
      )}
      {newer && item.state === "changed" && <p className="catalog-detail-note">The catalog has a newer version too.</p>}
      {item.replacesBuiltIn && item.state !== "taken" && (
        <p className="catalog-detail-note">The app ships a runbook named {item.id}: this one is read instead while it is installed.</p>
      )}

      <dl className="catalog-detail-facts">
        <dt>Source</dt>
        <dd title={source}>{sourceName(source)}</dd>
        <dt>Version</dt>
        <dd>{newer ? `${item.rev} — ${item.installedRev} installed` : item.rev}</dd>
        {item.role && (
          <>
            <dt>Role</dt>
            <dd>{roleName(item.role)}</dd>
          </>
        )}
        {item.target && (
          <>
            <dt>Installs to</dt>
            <dd className="mono">{item.target}</dd>
          </>
        )}
        {item.files.length > 0 && (
          <>
            <dt>Files</dt>
            <dd className="mono">
              {item.files.map((f) => (
                <div key={f}>{f}</div>
              ))}
            </dd>
          </>
        )}
      </dl>

      <h4 className="catalog-detail-file">{file}</h4>
      {preview?.error ? (
        <div className="catalog-detail-error">{preview.error}</div>
      ) : !preview?.read ? (
        <div className="catalog-detail-note">Reading…</div>
      ) : file?.endsWith(".json") ? (
        <pre className="catalog-detail-text">{preview.read.text}</pre>
      ) : (
        <div className="catalog-detail-text">
          <Markdown text={fencedFrontmatter(preview.read.text)} streaming={false} />
        </div>
      )}

      <Modal
        title={`Remove ${item.name}?`}
        open={confirming}
        onClose={() => setConfirming(false)}
        footer={
          <>
            <button type="button" className="btn btn-ghost" onClick={() => setConfirming(false)}>
              Cancel
            </button>
            <button
              type="button"
              className="btn btn-primary"
              onClick={() => {
                setConfirming(false);
                onRemove(key);
              }}
            >
              Remove
            </button>
          </>
        }
      >
        <p className="catalog-detail-confirm">
          {item.kind === "hooks" ? (
            <>
              Some of its hooks were changed here since it was installed: those stay in <b>{item.target}</b>, the rest come out.
            </>
          ) : (
            <>
              It was changed here since it was installed: <b>{place}</b> goes with those changes, and installing it again brings back the catalog's.
            </>
          )}
        </p>
      </Modal>
    </PagePanel>
  );
}

/** A feature of the app's own (docs/33-features.md): a card like an entry's, turned on rather than installed. */
function FeatureCard({ feature, selected, onOpen, onTurnOn }: { feature: FeatureView; selected: boolean; onOpen: () => void; onTurnOn: () => void }) {
  const Icon = featureIcon(feature.id);
  const open = (e: KeyboardEvent) => {
    if (e.target === e.currentTarget && (e.key === "Enter" || e.key === " ")) {
      e.preventDefault();
      onOpen();
    }
  };
  return (
    <article
      className={`catalog-card${selected ? " selected" : ""}`}
      role="button"
      tabIndex={0}
      aria-label={feature.name}
      aria-pressed={selected}
      onClick={onOpen}
      onKeyDown={open}
    >
      <div className="catalog-card-ico feature">
        <Icon size={16} />
      </div>
      <div className="catalog-card-body">
        <h3>{feature.name}</h3>
        <p title={feature.description}>{feature.description}</p>
        <div className="catalog-card-meta">Built into the app</div>
      </div>
      {feature.on ? (
        <span className="catalog-card-state installed" title="On" aria-label="On">
          <Check size={14} />
        </span>
      ) : (
        <button
          type="button"
          className="catalog-card-add"
          title="Turn on"
          aria-label={`Turn on ${feature.name}`}
          onClick={(e) => {
            e.stopPropagation();
            onTurnOn();
          }}
        >
          <Plus size={14} />
        </button>
      )}
    </article>
  );
}

/** What turning a feature on gives the model and the window, and where its data is. */
function FeatureDetails({
  feature,
  error,
  onSet,
  onDeleteData,
  onClose,
}: {
  feature: FeatureView;
  error: string | null;
  onSet: (on: boolean) => void;
  onDeleteData: () => void;
  onClose: () => void;
}) {
  const [confirming, setConfirming] = useState(false);
  return (
    <PagePanel icon={featureIcon(feature.id)} title="Feature" label={`${feature.name} details`} onClose={onClose}>
      <h3 className="catalog-detail-name">{feature.name}</h3>
      <p className="catalog-detail-desc">{feature.description}</p>

      <div className="catalog-detail-actions">
        <button type="button" className={`btn ${feature.on ? "btn-ghost" : "btn-primary"}`} onClick={() => onSet(!feature.on)}>
          {feature.on ? "Turn off" : "Turn on"}
        </button>
        {!feature.on && feature.hasData && (
          <button type="button" className="btn btn-ghost" onClick={() => setConfirming(true)}>
            Delete data
          </button>
        )}
        {feature.on && <span className="catalog-detail-state installed">{feature.tools.length > 0 ? "On — from the next message" : "On"}</span>}
      </div>
      {error && <div className="catalog-detail-error">{error}</div>}
      {feature.tools.length > 0 && (
        <p className="catalog-detail-note">
          {feature.on
            ? "Turned off, the model stops getting its tools and its page leaves the sidebar; what it kept stays until you delete it."
            : "Turned on, the model gets the tools below from its next message, and its page appears in the sidebar."}
        </p>
      )}

      <dl className="catalog-detail-facts">
        <dt>Source</dt>
        <dd>Built into the app</dd>
        {feature.places.length > 0 && (
          <>
            <dt>Where</dt>
            <dd>{feature.places.join(", ")}</dd>
          </>
        )}
        {feature.data && (
          <>
            <dt>Keeps</dt>
            <dd className="mono">{feature.data}</dd>
          </>
        )}
      </dl>

      {feature.tools.map((tool) => (
        <div key={tool.name}>
          <h4 className="catalog-detail-file">{tool.name}</h4>
          <div className="catalog-detail-text">{tool.description}</div>
        </div>
      ))}

      <Modal
        title={`Delete ${feature.name}'s data?`}
        open={confirming}
        onClose={() => setConfirming(false)}
        footer={
          <>
            <button type="button" className="btn btn-ghost" onClick={() => setConfirming(false)}>
              Cancel
            </button>
            <button
              type="button"
              className="btn btn-primary"
              onClick={() => {
                setConfirming(false);
                onDeleteData();
              }}
            >
              Delete
            </button>
          </>
        }
      >
        <p className="catalog-detail-confirm">
          <b>{feature.data}</b> is deleted, and everything in it with it. This cannot be undone.
        </p>
      </Modal>
    </PagePanel>
  );
}

/** A field for each `${NAME}` a server needs; the value goes only into mcp.json. */
function SecretFields({ names, values, onChange, target }: { names: string[]; values: Record<string, string>; onChange: (values: Record<string, string>) => void; target: string | null }) {
  if (names.length === 0) return null;
  return (
    <>
      {names.map((name) => (
        <div key={name} className="modal-field">
          <label htmlFor={`secret-${name}`}>{name}</label>
          <input
            id={`secret-${name}`}
            type="password"
            autoComplete="off"
            spellCheck={false}
            value={values[name] ?? ""}
            onChange={(e) => onChange({ ...values, [name]: e.target.value })}
          />
        </div>
      ))}
      <p className="modal-note">Written into {target} in place of each {"${…}"}, and kept nowhere else.</p>
    </>
  );
}

const filled = (names: string[], values: Record<string, string>) => names.every((name) => values[name]?.trim());

/** What the update writes, file by file against what is there now, before it does. */
function UpdateDialog({ entry, onUpdate, onClose }: { entry: Entry; onUpdate: (secrets: Record<string, string>) => void; onClose: () => void }) {
  const { item } = entry;
  const preview = useCatalogUpdatePreview(keyOf(entry));
  const [secrets, setSecrets] = useState<Record<string, string>>({});
  const read = preview?.read;
  const changed = read?.changes.filter((change) => change.diff) ?? [];
  const same = read?.changes.filter((change) => !change.diff) ?? [];

  return (
    <Modal
      title={`Update ${item.name}?`}
      open
      wide
      onClose={onClose}
      footer={
        <>
          <button type="button" className="btn btn-ghost" onClick={onClose}>
            Cancel
          </button>
          <button type="button" className="btn btn-primary" disabled={!read || !filled(read.secrets, secrets)} onClick={() => onUpdate(secrets)}>
            Update
          </button>
        </>
      }
    >
      <p className="catalog-detail-confirm">
        From version {item.installedRev} to {item.rev}. Each file as it is now, with the lines the update takes out (−) and puts in (+):
      </p>
      {preview?.error ? (
        <div className="catalog-detail-error">{preview.error}</div>
      ) : !read ? (
        <div className="catalog-detail-note">Reading…</div>
      ) : (
        <>
          {read.edited && (
            <p className="catalog-update-edited">
              {item.kind === "hooks"
                ? "Some of its hooks were changed here since it was installed: those stay as they are, and the catalog's new ones go in beside them."
                : "It was changed here since it was installed. The update writes over those changes — they are the lines it removes below."}
            </p>
          )}
          {changed.map((change) => (
            <div key={change.name} className="catalog-update-file">
              <h4 className="catalog-detail-file">
                {change.name} <span className="catalog-update-count">+{change.added} −{change.removed}</span>
              </h4>
              <DiffView unified={change.diff} />
            </div>
          ))}
          {changed.length === 0 && <p className="catalog-detail-note">The text is the same; only the version changes.</p>}
          {same.length > 0 && <p className="modal-note">Unchanged: {same.map((change) => change.name).join(", ")}</p>}
          <SecretFields names={read.secrets} values={secrets} onChange={setSecrets} target={item.target} />
        </>
      )}
      {(item.kind === "mcp" || item.kind === "hooks") && (
        <p className="modal-note">The app writes {item.target} in its own layout: spacing and key order typed there by hand are not kept.</p>
      )}
    </Modal>
  );
}

/** Hooks as the dialog lists them: event, matcher and the command as it runs —
    unescaped, since the command is what has to be read. Null when the text is
    not that shape; the backend has refused it already, and the raw text shows. */
function hookLines(text: string): { event: string; matcher: string; command: string }[] | null {
  try {
    const { hooks } = JSON.parse(text) as { hooks: Record<string, { matcher?: string; hooks: { command: string }[] }[]> };
    return Object.entries(hooks).flatMap(([event, groups]) =>
      groups.flatMap((group) => group.hooks.map(({ command }) => ({ event, matcher: group.matcher || "every tool", command }))),
    );
  } catch {
    return null;
  }
}

/** A server is a command the app will run, or an address it will send to;
    a hook is a command run on tool calls with no card to approve it. Either is
    shown whole and installed only from here — a server with its secrets typed in. */
function InstallDialog({ entry, onInstall, onClose }: { entry: Entry; onInstall: (secrets: Record<string, string>) => void; onClose: () => void }) {
  const { item } = entry;
  const preview = useCatalogPreview(keyOf(entry));
  const [secrets, setSecrets] = useState<Record<string, string>>({});
  const read = preview?.read;
  const ready = !!read && filled(read.secrets, secrets);
  const hooks = item.kind === "hooks" && read ? hookLines(read.text) : null;

  return (
    <Modal
      title={`Install ${item.name}?`}
      open
      onClose={onClose}
      footer={
        <>
          <button type="button" className="btn btn-ghost" onClick={onClose}>
            Cancel
          </button>
          <button type="button" className="btn btn-primary" disabled={!ready} onClick={() => onInstall(secrets)}>
            Install
          </button>
        </>
      }
    >
      {item.kind === "hooks" ? (
        <p className="catalog-detail-confirm">
          Adds these hooks to {item.target}. From the next Agent turn each command runs on every tool call its <b>matcher</b> names, with no card to approve it:
        </p>
      ) : (
        <p className="catalog-detail-confirm">
          Adds the server <b>{item.id}</b> to {item.target}. It starts with the next Agent turn, as this entry says — the command it runs or the address it
          sends to:
        </p>
      )}
      {preview?.error ? (
        <div className="catalog-detail-error">{preview.error}</div>
      ) : !read ? (
        <div className="catalog-detail-note">Reading…</div>
      ) : (
        <>
          {hooks ? (
            hooks.map(({ event, matcher, command }, i) => (
              <div key={i} className="catalog-hook">
                <div className="catalog-hook-when">
                  {event} · {matcher}
                </div>
                <pre className="catalog-detail-text catalog-server-entry">{command}</pre>
              </div>
            ))
          ) : (
            <pre className="catalog-detail-text catalog-server-entry">{read.text}</pre>
          )}
          <SecretFields names={read.secrets} values={secrets} onChange={setSecrets} target={item.target} />
        </>
      )}
      <p className="modal-note">The app writes {item.target} in its own layout: spacing and key order typed there by hand are not kept.</p>
    </Modal>
  );
}

/** What went wrong reading a source, if anything; a source read well says nothing — its name is on each card. */
function SourceLine({ source }: { source: CatalogSource }) {
  if (!source.error && source.problems.length === 0) return null;
  return (
    <div className="catalog-source">
      {source.error && (
        <div className="catalog-source-error">
          {sourceName(source.url)}: {source.fetchedAt !== null ? "showing the copy kept from last time — " : ""}
          {source.error}
        </div>
      )}
      {source.problems.length > 0 && (
        <details className="catalog-problems">
          <summary>
            {source.problems.length} {source.problems.length === 1 ? "entry" : "entries"} left out
          </summary>
          <ul>
            {source.problems.map((problem) => (
              <li key={problem}>{problem}</li>
            ))}
          </ul>
        </details>
      )}
    </div>
  );
}

/** Skills, commands, runbooks, MCP servers and hooks published to the
    catalogs (docs/32-catalog.md), and installing them — and the app's own
    features (docs/33-features.md), turned on here. */
export function CatalogPage({ sideHandle }: PageContext) {
  const { sources, loading, error, refresh, busy, failed, install, remove, update } = useCatalog();
  const features = useFeatures();
  const [tab, setTab] = useState<Tab>("all");
  const [query, setQuery] = useState("");
  const [open, setOpenEntry] = useState<EntryKey | null>(null);
  const [openFeature, setOpenFeatureId] = useState<string | null>(null);
  // One panel at a time: an entry's or a feature's.
  const setOpen = (key: EntryKey | null) => {
    setOpenEntry(key);
    setOpenFeatureId(null);
  };
  const setOpenFeature = (id: string | null) => {
    setOpenFeatureId(id);
    setOpenEntry(null);
  };
  const [asking, setAsking] = useState<EntryKey | null>(null);
  const [updating, setUpdating] = useState<EntryKey | null>(null);

  const entries: Entry[] = (sources ?? []).flatMap((source) => source.items.map((item) => ({ item, source: source.url })));
  const found = entries.filter((entry) => matches(entry, query));
  const featuresFound = tab === "all" || tab === "features" ? features.features.filter((f) => featureMatches(f, query)) : [];
  const shownFeature = features.features.find((f) => f.id === openFeature) ?? null;
  const sections = KINDS.filter(({ kind }) => tab === "all" || tab === kind)
    .map((k) => ({ ...k, entries: found.filter(({ item }) => item.kind === k.kind) }))
    .filter(({ entries }) => entries.length > 0);
  // Looked up again on every list, so the panel shows the state after an action.
  const shown = entries.find((entry) => sameEntry(keyOf(entry), open)) ?? null;
  const asked = entries.find((entry) => sameEntry(keyOf(entry), asking)) ?? null;
  const toUpdate = entries.find((entry) => sameEntry(keyOf(entry), updating)) ?? null;
  const onInstall = (key: EntryKey) => (asks(key.kind) ? setAsking(key) : void install(key));

  return (
    <Page
      title="Catalog"
      sideHandle={sideHandle}
      bar={
        <>
          <div className="catalog-tabs" role="tablist" aria-label="Kind">
            {TABS.map(({ value, label }) => (
              <button
                key={value}
                type="button"
                role="tab"
                aria-selected={tab === value}
                className={tab === value ? "active" : undefined}
                onClick={() => setTab(value)}
              >
                {label}
              </button>
            ))}
          </div>
          <label className="catalog-search">
            <Search size={14} />
            <input value={query} onChange={(e) => setQuery(e.target.value)} placeholder="Search" aria-label="Search the catalog" />
          </label>
          <button type="button" className="catalog-refresh" onClick={() => void refresh()} disabled={loading} title="Ask the catalogs again" aria-label="Refresh">
            <RefreshCw size={15} className={loading ? "catalog-spin" : undefined} />
          </button>
        </>
      }
      panel={
        shownFeature ? (
          <FeatureDetails
            key={shownFeature.id}
            feature={shownFeature}
            error={features.error}
            onSet={(on) => void features.set(shownFeature.id, on)}
            onDeleteData={() => void features.deleteData(shownFeature.id)}
            onClose={() => setOpenFeature(null)}
          />
        ) : (
          shown && (
            <Details
              key={`${shown.source}/${shown.item.kind}/${shown.item.id}`}
              entry={shown}
              busy={busy}
              error={failed && sameEntry(failed.entry, open) ? failed.error : null}
              onInstall={onInstall}
              onRemove={(key) => void remove(key)}
              onUpdate={setUpdating}
              onClose={() => setOpen(null)}
            />
          )
        )
      }
    >
      {error && <div className="catalog-source-error">{error}</div>}
      {/* An action's failure, when its panel is not open to say so. */}
      {failed && !sameEntry(failed.entry, open) && <div className="catalog-source-error">{failed.error}</div>}
      {sources === null
        ? !error && <div className="catalog-empty">Reading the catalog…</div>
        : sources.map((source) => <SourceLine key={source.url} source={source} />)}

      {featuresFound.length > 0 && (
        <section className="catalog-section" aria-label="Features">
          <div className="catalog-section-head">
            <h2>Features</h2>
          </div>
          <div className="catalog-grid">
            {featuresFound.map((feature) => (
              <FeatureCard
                key={feature.id}
                feature={feature}
                selected={feature.id === openFeature}
                onOpen={() => setOpenFeature(feature.id)}
                onTurnOn={() => void features.set(feature.id, true)}
              />
            ))}
          </div>
        </section>
      )}
      {sections.map(({ kind, label, icon, entries }) => {
        const cut = tab === "all" && entries.length > PREVIEW;
        return (
          <section key={kind} className="catalog-section" aria-label={label}>
            <div className="catalog-section-head">
              <h2>{label}</h2>
              {cut && (
                <button type="button" className="catalog-more" onClick={() => setTab(kind)}>
                  Show all {entries.length} <ArrowRight size={14} />
                </button>
              )}
            </div>
            <div className="catalog-grid">
              {(cut ? entries.slice(0, PREVIEW) : entries).map((entry) => (
                <Card
                  key={`${entry.source}/${kind}/${entry.item.id}`}
                  entry={entry}
                  icon={icon}
                  selected={sameEntry(keyOf(entry), open)}
                  onOpen={() => setOpen(keyOf(entry))}
                  busy={busy}
                  onInstall={onInstall}
                />
              ))}
            </div>
          </section>
        );
      })}
      {asked && (
        <InstallDialog
          key={`${asked.source}/${asked.item.kind}/${asked.item.id}`}
          entry={asked}
          onClose={() => setAsking(null)}
          onInstall={(secrets) => {
            setAsking(null);
            void install(keyOf(asked), secrets);
          }}
        />
      )}
      {toUpdate && (
        <UpdateDialog
          key={`${toUpdate.source}/${toUpdate.item.kind}/${toUpdate.item.id}`}
          entry={toUpdate}
          onClose={() => setUpdating(null)}
          onUpdate={(secrets) => {
            setUpdating(null);
            void update(keyOf(toUpdate), secrets);
          }}
        />
      )}
      {sources !== null && entries.length > 0 && sections.length === 0 && featuresFound.length === 0 && (
        <div className="catalog-empty">{query.trim() ? `Nothing matches “${query.trim()}”.` : "Nothing of this kind yet."}</div>
      )}
    </Page>
  );
}
