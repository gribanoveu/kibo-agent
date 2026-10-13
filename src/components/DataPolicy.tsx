import type { HookItem, McpServerItem, ProviderView } from "../lib/chat";
import "./DataPolicy.css";

// Where the workspace's contents can go, drawn from what is configured right
// now. The policy it summarises is `docs/08-data-policy.md`; each row names an
// exit and who opened it.

type Props = {
  provider: ProviderView | null;
  debugLogging: boolean;
  /** `null` while not loaded yet. */
  mcpServers: McpServerItem[] | null;
  hooks: HookItem[] | null;
};

/** The host a request goes to — what the user would recognise. */
function host(baseUrl: string): string {
  try {
    return new URL(baseUrl).host || baseUrl;
  } catch {
    return baseUrl;
  }
}

function listed(names: string[]): string {
  return names.length <= 3 ? names.join(", ") : `${names.slice(0, 3).join(", ")} and ${names.length - 3} more`;
}

export function DataPolicy({ provider, debugLogging, mcpServers, hooks }: Props) {
  // A server reached at a URL names its host: that is where the call goes.
  const enabled =
    mcpServers
      ?.filter((server) => server.enabled)
      .map((server) => (/^https?:\/\//i.test(server.command) ? `${server.name} (${host(server.command)})` : server.name)) ?? null;
  const rows: { name: string; value: string; note: string }[] = [
    {
      name: "Model provider",
      value: provider ? `${host(provider.baseUrl)} (${provider.id})` : "none set up",
      note: "Gets the conversation, including what the agent read from files. Besides MCP servers at a URL, the only place the app itself sends anything.",
    },
    {
      name: "MCP servers",
      value: enabled === null ? "…" : enabled.length ? listed(enabled) : "none enabled",
      note: "Get what the model puts in a call — one at a URL over the network, with the headers in its entry. Every call asks first.",
    },
    {
      name: "Hooks",
      value: hooks === null ? "…" : hooks.length ? `${hooks.length} command${hooks.length === 1 ? "" : "s"}` : "none",
      note: "Your own commands; they get each call's input and result, file contents included.",
    },
    {
      name: "Catalog",
      value: "when its page opens",
      note: "Asks raw.githubusercontent.com — the app's own catalog — and any catalog you add for its list, and for an entry's files when you open, install or update it. Nothing of yours goes with it; the host sees your address and what you look at and install. A token typed in to install an MCP server goes into mcp.json only.",
    },
    {
      name: "Agent commands",
      value: "can reach the network",
      note: "A command that runs curl, ssh, git push and the like asks every time, even when commands are always allowed — but not in Auto. Code run by python or node is not seen.",
    },
    {
      name: "Request log",
      value: debugLogging ? "on" : "off",
      note: debugLogging
        ? "Every request is written whole to ~/.kibo/logs, on this machine."
        : "Nothing about requests is written to disk.",
    },
  ];

  return (
    <section className="data-policy" aria-label="Where your data goes">
      <h3 className="data-policy-title">Where your data goes</h3>
      <dl className="data-policy-rows">
        {rows.map((row) => (
          <div className="data-policy-row" key={row.name}>
            <dt>{row.name}</dt>
            <dd>
              <span className="data-policy-value">{row.value}</span>
              <span className="data-policy-note">{row.note}</span>
            </dd>
          </div>
        ))}
      </dl>
      <p className="data-policy-note">
        Everything else stays in ~/.kibo: chats, the index (embeddings are computed on this machine) and
        the tool log, with file contents cut out. No telemetry.
      </p>
    </section>
  );
}
