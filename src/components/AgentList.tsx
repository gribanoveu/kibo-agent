import { ChevronRight, Square } from "lucide-react";
import { type AgentInfo, agentStatus, briefTitle, splitStep } from "../lib/agents";
import type { AgentFocus } from "../lib/describeTool";
import "./AgentList.css";

type Props = {
  agents: AgentInfo[];
  error: string | null;
  onStop: (id: number) => void;
  /** Opens a run in full, in the sheet over the chat (AgentSheet). */
  onOpen: (agent: AgentFocus) => void;
};

/** The helper agents, a line each: what it was asked, how far it got — the
    tool it is calling now, or how it ended. A click opens the run in full. */
export function AgentList({ agents, error, onStop, onOpen }: Props) {
  if (!agents.length) {
    return (
      <div className="agent-list-empty">
        {error ??
          "No helper agents yet. The agent hands a research question to one with explore — it reads in a context of its own and answers back."}
      </div>
    );
  }
  return (
    <div className="agent-list">
      {error && <div className="agent-list-error">{error}</div>}
      {agents.map((a) => {
        const running = a.state.state === "running";
        // What it is doing now: the last tool it called.
        const now = a.steps[a.steps.length - 1];
        return (
          // The sheet's opener, Stop included: a click on either leaves it open.
          <div key={a.id} className={`agent-run ${a.state.state}`} data-sheet-opener>
            <div className="agent-head">
              <button type="button" className="agent-open" title={a.task} onClick={() => onOpen({ id: a.id, task: a.task })}>
                <span className="agent-dot" aria-hidden />
                <span className="agent-title">{briefTitle(a.task)}</span>
                <span className="agent-now">{running ? (now ? splitStep(now).tool : "thinking") : agentStatus(a.state)}</span>
                <ChevronRight className="agent-chev" size={13} aria-hidden />
              </button>
              {running && (
                <button
                  type="button"
                  className="agent-stop"
                  title="Stop this helper — the agent carries on without its answer"
                  aria-label="Stop"
                  onClick={() => onStop(a.id)}
                >
                  <Square size={9} fill="currentColor" aria-hidden />
                </button>
              )}
            </div>
          </div>
        );
      })}
    </div>
  );
}
