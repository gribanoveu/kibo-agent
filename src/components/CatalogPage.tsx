import { useState } from "react";
import { ArrowRight, BookOpen, Plug, Plus, RefreshCw, Search, Sparkles, SquareSlash, Webhook, type LucideIcon } from "lucide-react";
import { useCatalog } from "../hooks/useCatalog";
import type { CatalogItem, CatalogKind, CatalogSource } from "../lib/catalog";
import type { PageContext } from "./pages";
import "./CatalogPage.css";

// In the order the page lists them.
const KINDS: { kind: CatalogKind; label: string; icon: LucideIcon }[] = [
  { kind: "skill", label: "Skills", icon: Sparkles },
  { kind: "command", label: "Commands", icon: SquareSlash },
  { kind: "runbook", label: "Runbooks", icon: BookOpen },
  { kind: "mcp", label: "MCP servers", icon: Plug },
  { kind: "hooks", label: "Hooks", icon: Webhook },
];

type Tab = CatalogKind | "all";
const TABS: { value: Tab; label: string }[] = [{ value: "all", label: "All" }, ...KINDS.map(({ kind, label }) => ({ value: kind, label }))];

/** How many cards a kind shows under All before "Show all". */
const PREVIEW = 4;

/** `raw.githubusercontent.com/<owner>/<repo>/<branch>/` as the repository it is; any other address as it is. */
function sourceName(url: string): string {
  const github = /^https:\/\/raw\.githubusercontent\.com\/([^/]+)\/([^/]+)\//.exec(url);
  return github ? `${github[1]}/${github[2]}` : url.replace(/^https?:\/\//, "").replace(/\/$/, "");
}

type Entry = { item: CatalogItem; source: string };

function matches({ item }: Entry, query: string): boolean {
  const q = query.trim().toLowerCase();
  return !q || [item.id, item.name, item.description].some((text) => text.toLowerCase().includes(q));
}

function Card({ item, source, icon: Icon }: Entry & { icon: LucideIcon }) {
  const role = item.role && `For the ${item.role[0].toUpperCase()}${item.role.slice(1)} role`;
  return (
    <article className="catalog-card">
      <div className={`catalog-card-ico ${item.kind}`}>
        <Icon size={18} />
      </div>
      <div className="catalog-card-body">
        <h3>{item.name}</h3>
        <p title={item.description}>{item.description}</p>
        <div className="catalog-card-meta">{[source, role].filter(Boolean).join(" · ")}</div>
      </div>
      <button type="button" className="catalog-card-add" disabled title="Installing comes next" aria-label={`Install ${item.name}`}>
        <Plus size={16} />
      </button>
    </article>
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
    catalogs (docs/32-catalog.md). Read-only until installing exists (CT-2). */
export function CatalogPage(_: PageContext) {
  const { sources, loading, error, refresh } = useCatalog();
  const [tab, setTab] = useState<Tab>("all");
  const [query, setQuery] = useState("");

  const entries: Entry[] = (sources ?? []).flatMap((source) => source.items.map((item) => ({ item, source: sourceName(source.url) })));
  const found = entries.filter((entry) => matches(entry, query));
  const sections = KINDS.filter(({ kind }) => tab === "all" || tab === kind)
    .map((k) => ({ ...k, entries: found.filter(({ item }) => item.kind === k.kind) }))
    .filter(({ entries }) => entries.length > 0);

  return (
    <section className="page catalog" aria-label="Catalog">
      <header className="catalog-top">
        <h1 className="catalog-title">Catalog</h1>
        <div className="catalog-bar">
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
            <RefreshCw size={15} className={loading ? "spinning" : undefined} />
          </button>
        </div>
      </header>

      {error && <div className="catalog-source-error">{error}</div>}
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
                <Card key={`${entry.source}/${kind}/${entry.item.id}`} {...entry} icon={icon} />
              ))}
            </div>
          </section>
        );
      })}
      {sources !== null && entries.length > 0 && sections.length === 0 && (
        <div className="catalog-empty">{query.trim() ? `Nothing matches “${query.trim()}”.` : "Nothing of this kind yet."}</div>
      )}
    </section>
  );
}
