import { useEffect, useState } from "react";
import { ExternalLink } from "lucide-react";
import { openUrl } from "@tauri-apps/plugin-opener";
import { useWebSearchKey } from "../hooks/useWebSearchKey";
import { describeUsage, type WebSearchBackend } from "../lib/chat";
import "./WebSearchSettings.css";

const KEYS_PAGE = "https://app.tavily.com";

const BACKENDS: { value: WebSearchBackend; label: string; hint: string }[] = [
  { value: "off", label: "Off", hint: "The model cannot search or open pages; it answers from what it knows." },
  {
    value: "searxng",
    label: "SearXNG",
    hint: "No key. Queries go to your SearXNG, which asks the engines it is set up with; the pages it finds are read here, and the parts about the question are sent to the model.",
  },
  { value: "tavily", label: "Tavily", hint: "Your Tavily key and its credits. Pages the model opens are read here, as with SearXNG." },
];

/** Settings → Web search: which search a chat's `webSearch` asks, SearXNG's address, and the Tavily key when it is Tavily's. The key goes in and never comes back. */
export function WebSearchSettings() {
  const { backend, choose, searxng, saveSearxng, hasKey, usage, error, save } = useWebSearchKey();
  const [key, setKey] = useState("");
  const [address, setAddress] = useState(searxng);
  // The saved address arrives after the first render, and comes back cleaned up after a save.
  useEffect(() => setAddress(searxng), [searxng]);

  return (
    <>
      <h3 className="settings-title">Web search</h3>
      <p className="modal-note web-search-intro">
        In Chat mode, the model can search the web and read the pages it finds. Each query and page shows in the chat. In a
        folder, add a search server in MCP instead.
      </p>

      <div className="modal-field">
        <label>Search with</label>
        <div className="segmented" role="radiogroup" aria-label="Search with">
          {BACKENDS.map(({ value, label }) => (
            <button
              key={value}
              type="button"
              role="radio"
              aria-checked={backend === value}
              className={`segment${backend === value ? " active" : ""}`}
              onClick={() => void choose(value)}
            >
              {label}
            </button>
          ))}
        </div>
      </div>
      <p className="modal-note settings-hint">{BACKENDS.find((b) => b.value === backend)?.hint}</p>

      {backend === "searxng" && (
        <form
          className="web-search-key"
          noValidate
          onSubmit={(e) => {
            e.preventDefault();
            void saveSearxng(address);
          }}
        >
          <div className="modal-field">
            <label htmlFor="searxng-url">SearXNG address</label>
            <input id="searxng-url" value={address} placeholder="http://localhost:8080/" onChange={(e) => setAddress(e.target.value)} />
          </div>
          <p className="modal-note settings-hint">Its settings.yml must list json under search.formats.</p>
          <div className="web-search-actions">
            <button type="submit" className="btn btn-primary" disabled={!address.trim() || address === searxng}>
              Save address
            </button>
          </div>
        </form>
      )}

      {backend === "tavily" && (
        <form
          className="web-search-key"
          noValidate
          onSubmit={async (e) => {
            e.preventDefault();
            if (await save(key)) setKey("");
          }}
        >
          <div className="modal-field">
            <label htmlFor="web-search-key">Tavily API key</label>
            <input
              id="web-search-key"
              type="password"
              value={key}
              placeholder={hasKey ? "stored — type to replace" : "not configured"}
              onChange={(e) => setKey(e.target.value)}
            />
          </div>
          {usage && <p className="modal-note settings-hint">{describeUsage(usage)}</p>}
          <div className="web-search-actions">
            <button type="submit" className="btn btn-primary" disabled={!key.trim()}>
              Save key
            </button>
            {hasKey && (
              <button type="button" className="btn btn-ghost" onClick={() => save("")}>
                Remove key
              </button>
            )}
            <button type="button" className="btn btn-ghost" title={KEYS_PAGE} onClick={() => void openUrl(KEYS_PAGE)}>
              <ExternalLink size={13} />
              Get a key
            </button>
          </div>
        </form>
      )}
      {error && <p className="modal-note settings-hint settings-error">{error}</p>}
    </>
  );
}
