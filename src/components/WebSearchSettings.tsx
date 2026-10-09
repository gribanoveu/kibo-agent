import { useEffect, useState } from "react";
import { ExternalLink } from "lucide-react";
import { openUrl } from "@tauri-apps/plugin-opener";
import { useWebSearchKey } from "../hooks/useWebSearchKey";
import { describeUsage, type WebSearchBackend, type WebSearchPlace } from "../lib/chat";
import "./WebSearchSettings.css";

const KEYS_PAGE = "https://app.tavily.com";

const BACKENDS: { value: WebSearchBackend; label: string }[] = [
  { value: "off", label: "Off" },
  { value: "searxng", label: "SearXNG" },
  { value: "tavily", label: "Tavily" },
];

const PLACES: { value: WebSearchPlace; label: string }[] = [
  { value: "chat", label: "Chat" },
  { value: "agent", label: "Agent" },
];

/**
 * Settings → Web search: which search `webSearch` asks in a chat and in a folder, SearXNG's address
 * when either uses it, and the Tavily key when either is Tavily's. The key goes in and never comes back.
 */
export function WebSearchSettings() {
  const { backends, choose, searxng, saveSearxng, hasKey, usage, error, save } = useWebSearchKey();
  const uses = (backend: WebSearchBackend) => backends !== null && (backends.chat === backend || backends.agent === backend);
  const [key, setKey] = useState("");
  const [address, setAddress] = useState(searxng);
  // The saved address arrives after the first render, and comes back cleaned up after a save.
  useEffect(() => setAddress(searxng), [searxng]);

  return (
    <>
      <h3 className="settings-title">Web search</h3>
      <p className="modal-note web-search-intro">
        The model can search the web and read the pages it finds — in Chat mode, and in a folder (Agent, Plan, Ask). Each
        has its own search, or none. Each query and page shows in the conversation.
      </p>

      {PLACES.map((place) => (
        <div className="modal-field" key={place.value}>
          <label>{place.label}</label>
          <div className="segmented" role="radiogroup" aria-label={`${place.label} searches with`}>
            {BACKENDS.map(({ value, label }) => (
              <button
                key={value}
                type="button"
                role="radio"
                aria-checked={backends?.[place.value] === value}
                className={`segment${backends?.[place.value] === value ? " active" : ""}`}
                onClick={() => void choose(place.value, value)}
              >
                {label}
              </button>
            ))}
          </div>
        </div>
      ))}
      <p className="modal-note settings-hint">
        SearXNG: no key — queries go to your SearXNG, which asks the engines it is set up with. Tavily: your key and its
        credits; an agent may search several times a turn. Pages are read here either way, and the parts about the question
        are sent to the model.
      </p>

      {uses("searxng") && (
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

      {uses("tavily") && (
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
