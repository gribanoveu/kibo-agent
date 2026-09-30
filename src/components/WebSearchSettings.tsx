import { useState } from "react";
import { ExternalLink } from "lucide-react";
import { openUrl } from "@tauri-apps/plugin-opener";
import { useWebSearchKey } from "../hooks/useWebSearchKey";
import "./WebSearchSettings.css";

const KEYS_PAGE = "https://app.tavily.com";

/** Settings → Web search: the Tavily key a chat's `webSearch` uses. The key goes in and never comes back. */
export function WebSearchSettings() {
  const { hasKey, error, save } = useWebSearchKey();
  const [key, setKey] = useState("");

  return (
    <>
      <h3 className="settings-title">Web search</h3>
      <p className="modal-note web-search-intro">
        With a Tavily key, the model can search the web — in Chat mode only. The queries it writes go to Tavily; each
        one shows in the chat. In a folder, add a search server in MCP instead.
      </p>

      <form
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
        {error && <p className="modal-note settings-hint settings-error">{error}</p>}
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
    </>
  );
}
