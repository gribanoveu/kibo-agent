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
  Sparkles,
  SquareSlash,
  Webhook,
  X,
  type LucideIcon,
} from "lucide-react";
import { Markdown } from "./Markdown";
import { Modal } from "./Modal";
import { Page, PagePanel } from "./Page";
import { sameEntry, useCatalog, useCatalogPreview, type EntryKey } from "../hooks/useCatalog";
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

type Tab = CatalogKind | "all";
const TABS: { value: Tab; label: string }[] = [{ value: "all", label: "All" }, ...KINDS.map(({ kind, label }) => ({ value: kind, label }))];

/** How many cards a kind shows under All before "Show all". */
const PREVIEW = 4;

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
  /** Installs at once — or, for a server, asks first. */
  onInstall: (entry: EntryKey) => void;
};

/** What is in its place, as the panel names it: a server is a name in a shared file. */
const placeOf = (item: CatalogEntry) => (item.kind === "mcp" ? `the server ${item.id} in ${item.target}` : item.target);

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
        <Icon size={18} />
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
          title={item.target === null ? `Installing ${kindOf(item.kind).label} comes next` : "Install"}
          aria-label={`Install ${item.name}`}
          onClick={(e) => {
            e.stopPropagation();
            onInstall(keyOf(entry));
          }}
        >
          {working ? <Loader2 size={16} className="catalog-spin" /> : <Plus size={16} />}
        </button>
      ) : (
        <span className={`catalog-card-state ${item.state}`} title={STATE_NOTE[item.state] ?? undefined} aria-label={STATE_NOTE[item.state] ?? undefined}>
          {item.state === "taken" ? <X size={16} /> : <Check size={16} />}
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
  onClose,
}: { entry: Entry; error: string | null; onRemove: (entry: EntryKey) => void; onClose: () => void } & Actions) {
  const { item, source } = entry;
  const kind = kindOf(item.kind);
  const key = keyOf(entry);
  const preview = useCatalogPreview(key);
  const [confirming, setConfirming] = useState(false);
  const working = sameEntry(busy, key);
  const ours = item.state === "installed" || item.state === "update" || item.state === "changed";
  const place = placeOf(item);
  const file = item.kind === "skill" ? "SKILL.md" : item.path.split("/").pop();

  return (
    <PagePanel icon={kind.icon} title={kind.one} label={`${item.name} details`} onClose={onClose}>
      <h3 className="catalog-detail-name">{item.name}</h3>
      {item.description && <p className="catalog-detail-desc">{item.description}</p>}

      <div className="catalog-detail-actions">
        {ours ? (
          <button type="button" className="btn btn-ghost" disabled={busy !== null} onClick={() => (item.state === "changed" ? setConfirming(true) : onRemove(key))}>
            {working && <Loader2 size={13} className="catalog-spin" />}
            Remove
          </button>
        ) : (
          <button type="button" className="btn btn-primary" disabled={item.state !== "available" || item.target === null || busy !== null} onClick={() => onInstall(key)}>
            {working && <Loader2 size={13} className="catalog-spin" />}
            Install
          </button>
        )}
        {STATE_NOTE[item.state] && <span className={`catalog-detail-state ${item.state}`}>{STATE_NOTE[item.state]}</span>}
      </div>
      {error && <div className="catalog-detail-error">{error}</div>}
      {item.target === null && <p className="catalog-detail-note">Installing {kind.label} from the catalog comes next.</p>}
      {item.state === "taken" && (
        <p className="catalog-detail-note">
          {item.kind === "mcp" ? `A server named ${item.id} is already in ${item.target}` : `${item.target} is already there`} and was not installed
          from this catalog — yours, or another catalog's. It is left as it is.
        </p>
      )}
      {item.state === "update" && <p className="catalog-detail-note">Updating comes next; Remove and Install gets the new one now.</p>}
      {item.replacesBuiltIn && item.state !== "taken" && (
        <p className="catalog-detail-note">The app ships a runbook named {item.id}: this one is read instead while it is installed.</p>
      )}

      <dl className="catalog-detail-facts">
        <dt>Source</dt>
        <dd title={source}>{sourceName(source)}</dd>
        <dt>Version</dt>
        <dd>{item.rev}</dd>
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
          It was changed here since it was installed: <b>{place}</b> goes with those changes, and installing it again brings back the catalog's.
        </p>
      </Modal>
    </PagePanel>
  );
}

/** A server is a command the app will run, or an address it will send to:
    it is shown whole and installed only from here, with its secrets typed in. */
function InstallServer({ entry, onInstall, onClose }: { entry: Entry; onInstall: (secrets: Record<string, string>) => void; onClose: () => void }) {
  const { item } = entry;
  const preview = useCatalogPreview(keyOf(entry));
  const [secrets, setSecrets] = useState<Record<string, string>>({});
  const read = preview?.read;
  const ready = read !== undefined && read !== null && read.secrets.every((name) => secrets[name]?.trim());

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
      <p className="catalog-detail-confirm">
        Adds the server <b>{item.id}</b> to {item.target}. It starts with the next Agent turn, as this entry says — the command it runs or the address it
        sends to:
      </p>
      {preview?.error ? (
        <div className="catalog-detail-error">{preview.error}</div>
      ) : !read ? (
        <div className="catalog-detail-note">Reading…</div>
      ) : (
        <>
          <pre className="catalog-detail-text catalog-server-entry">{read.text}</pre>
          {read.secrets.map((name) => (
            <div key={name} className="modal-field">
              <label htmlFor={`secret-${name}`}>{name}</label>
              <input
                id={`secret-${name}`}
                type="password"
                autoComplete="off"
                spellCheck={false}
                value={secrets[name] ?? ""}
                onChange={(e) => setSecrets({ ...secrets, [name]: e.target.value })}
              />
            </div>
          ))}
          {read.secrets.length > 0 && <p className="modal-note">Written into {item.target} in place of each {"${…}"}, and kept nowhere else.</p>}
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
    catalogs (docs/32-catalog.md), and installing them. */
export function CatalogPage({ sideHandle }: PageContext) {
  const { sources, loading, error, refresh, busy, failed, install, remove } = useCatalog();
  const [tab, setTab] = useState<Tab>("all");
  const [query, setQuery] = useState("");
  const [open, setOpen] = useState<EntryKey | null>(null);
  const [asking, setAsking] = useState<EntryKey | null>(null);

  const entries: Entry[] = (sources ?? []).flatMap((source) => source.items.map((item) => ({ item, source: source.url })));
  const found = entries.filter((entry) => matches(entry, query));
  const sections = KINDS.filter(({ kind }) => tab === "all" || tab === kind)
    .map((k) => ({ ...k, entries: found.filter(({ item }) => item.kind === k.kind) }))
    .filter(({ entries }) => entries.length > 0);
  // Looked up again on every list, so the panel shows the state after an action.
  const shown = entries.find((entry) => sameEntry(keyOf(entry), open)) ?? null;
  const asked = entries.find((entry) => sameEntry(keyOf(entry), asking)) ?? null;
  const onInstall = (key: EntryKey) => (key.kind === "mcp" ? setAsking(key) : void install(key));

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
        shown && (
          <Details
            key={`${shown.source}/${shown.item.kind}/${shown.item.id}`}
            entry={shown}
            busy={busy}
            error={failed && sameEntry(failed.entry, open) ? failed.error : null}
            onInstall={onInstall}
            onRemove={(key) => void remove(key)}
            onClose={() => setOpen(null)}
          />
        )
      }
    >
      {error && <div className="catalog-source-error">{error}</div>}
      {/* An action's failure, when its panel is not open to say so. */}
      {failed && !sameEntry(failed.entry, open) && <div className="catalog-source-error">{failed.error}</div>}
      {sources === null
        ? !error && <div className="catalog-empty">Reading the catalog…</div>
        : sources.map((source) => <SourceLine key={source.url} source={source} />)}

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
        <InstallServer
          key={`${asked.source}/${asked.item.id}`}
          entry={asked}
          onClose={() => setAsking(null)}
          onInstall={(secrets) => {
            setAsking(null);
            void install(keyOf(asked), secrets);
          }}
        />
      )}
      {sources !== null && entries.length > 0 && sections.length === 0 && (
        <div className="catalog-empty">{query.trim() ? `Nothing matches “${query.trim()}”.` : "Nothing of this kind yet."}</div>
      )}
    </Page>
  );
}
