import type { ReactNode } from "react";
import { ChevronRight, GitBranch, Plug, Telescope, TerminalSquare } from "lucide-react";
import { ContextDetails } from "./ContextMeter";
import { type AgentInfo, briefTitle } from "../lib/agents";
import type { ChatUsage, ContextUsage } from "../lib/chat";
import type { AgentFocus } from "../lib/describeTool";
import type { ChangeTotals } from "../lib/git";
import type { McpServerItem, McpServerState } from "../lib/mcp";
import type { ProcessInfo } from "../lib/processes";
import type { AsideTab } from "../types";
import "./OverviewPanel.css";

export type OverviewProps = {
  /** The checked-out branch, shown on the Changes row; `null` outside a repository. */
  branch: string | null;
  /** `null` outside a repository. */
  changes: ChangeTotals | null;
  /** `null` until the first estimate arrives, and when none can be made. */
  context: ContextUsage | null;
  /** The estimate is on its way: the card keeps its place with a placeholder,
      so switching chats does not shift the cards under it. */
  contextPending?: boolean;
  usage: ChatUsage | null;
  /** Every model call of the open chat, in tokens — helpers, title and checks included; `null` until read. */
  spent: number | null;
  /** The subagents' share of the session total, in tokens; `null` until read. */
  subagents: number | null;
  servers: McpServerItem[];
  /** Background processes and helper agents, running or ended. */
  processes: ProcessInfo[];
  agents: AgentInfo[];
  onOpenProcess: (id: number) => void;
  /** The Background card's heading: Terminal, on its Processes tab. */
  onOpenProcesses: () => void;
  onOpenAgent: (agent: AgentFocus) => void;
  onOpenPanel: (tab: AsideTab) => void;
  /** The branch's pull request, under the branch: the pane reads it, this only places it. */
  pullRequest?: ReactNode;
};

function serverStatus(server: McpServerItem): { label: string; tone: "ok" | "off" | "warn" } {
  if (server.error) return { label: "won't start", tone: "warn" };
  const state: McpServerState = server.state;
  switch (state.state) {
    case "running":
      return { label: `${state.tools.length} ${state.tools.length === 1 ? "tool" : "tools"}`, tone: "ok" };
    case "starting":
      return { label: "starting", tone: "off" };
    case "notStarted":
      return { label: "not started", tone: "off" };
    case "exited":
      return { label: "exited", tone: "warn" };
    case "failed":
      return { label: "failed", tone: "warn" };
  }
}

/** The open session at a glance: how full the model's window is, which MCP
    servers are on, what runs in the background, and what the folder holds
    against its last commit, on which branch. The folder is under the message box;
    the branch is picked there only for a new chat.
    Each section leads to the pane that does something about it. */
export function OverviewPanel({
  branch,
  changes,
  context,
  contextPending = false,
  usage,
  spent,
  subagents,
  servers,
  processes,
  agents,
  onOpenProcess,
  onOpenProcesses,
  onOpenAgent,
  onOpenPanel,
  pullRequest,
}: OverviewProps) {
  const on = servers.filter((server) => server.enabled);
  const busyProcesses = processes.filter((process) => process.state.state === "running");
  const busyAgents = agents.filter((agent) => agent.state.state === "running");
  const ended = processes.length - busyProcesses.length + agents.length - busyAgents.length;
  return (
    <div className="overview">
      {(context || contextPending) && (
        <section className="overview-card overview-ctx" aria-busy={!context}>
          <ContextDetails context={context} cache={usage} spent={spent} subagents={subagents} onOpenPane={onOpenPanel} />
        </section>
      )}

      <section className="overview-card">
        <button type="button" className="overview-head overview-link" onClick={() => onOpenPanel("mcp")}>
          Connectors
          <span className="overview-count">{on.length}</span>
          <ChevronRight size={13} className="overview-chev" />
        </button>
        {on.length ? (
          <ul className="overview-rows">
            {on.map((server) => {
              const status = serverStatus(server);
              return (
                <li key={server.name} title={server.error ?? server.command}>
                  <Plug size={14} />
                  <span className="overview-name">{server.name}</span>
                  <span className={`overview-status ${status.tone}`}>{status.label}</span>
                </li>
              );
            })}
          </ul>
        ) : (
          <p className="overview-empty">No MCP servers are on.</p>
        )}
      </section>

      <section className="overview-card">
        <button type="button" className="overview-head overview-link" onClick={onOpenProcesses}>
          Background
          <span className="overview-count">{busyProcesses.length + busyAgents.length}</span>
          <ChevronRight size={13} className="overview-chev" />
        </button>
        {busyProcesses.length + busyAgents.length ? (
          <ul className="overview-rows">
            {busyProcesses.map((process) => (
              <li key={`p${process.id}`}>
                <button type="button" className="overview-row" title={process.command} onClick={() => onOpenProcess(process.id)}>
                  <TerminalSquare size={14} />
                  <span className="overview-name overview-mono">{process.command}</span>
                  <span className="overview-status ok">running</span>
                </button>
              </li>
            ))}
            {busyAgents.map((agent) => (
              <li key={`a${agent.id}`}>
                <button type="button" className="overview-row" title={agent.task} data-sheet-opener onClick={() => onOpenAgent({ id: agent.id, task: agent.task })}>
                  <Telescope size={14} />
                  <span className="overview-name">{briefTitle(agent.task)}</span>
                  <span className="overview-status ok">running</span>
                </button>
              </li>
            ))}
          </ul>
        ) : (
          <p className="overview-empty">Nothing is running.</p>
        )}
        {ended > 0 && <p className="overview-empty overview-ended">{ended} ended</p>}
      </section>

      <section className="overview-card">
        <h3 className="overview-head">Changes</h3>
        {!changes ? (
          <p className="overview-empty">Not a git repository.</p>
        ) : (
          <button
            type="button"
            className="overview-row"
            title={`${changes.files} ${changes.files === 1 ? "file" : "files"} changed since the last commit`}
            onClick={() => onOpenPanel("changes")}
          >
            <GitBranch size={14} />
            <span className="overview-name">{branch ?? "detached HEAD"}</span>
            {changes.files ? (
              <span className="overview-lines">
                <span className="add">+{changes.add}</span>
                <span className="del">−{changes.del}</span>
              </span>
            ) : (
              <span className="overview-status">no changes</span>
            )}
          </button>
        )}
        {changes && pullRequest}
      </section>
    </div>
  );
}
