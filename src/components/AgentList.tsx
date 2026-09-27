import { useEffect, useRef, useState } from "react";
import { ChevronRight, Square } from "lucide-react";
import { Markdown } from "./Markdown";
import { agentStatus, tokensSpent, type AgentInfo } from "../lib/chat";
import type { AgentFocus } from "../lib/describeTool";
import "./AgentList.css";

type Props = {
  agents: AgentInfo[];
  error: string | null;
  onStop: (id: number) => void;
  /** A run asked for from the chat: it opens and scrolls into view — if it is
   * still this run, the same number and task. A new object each ask. */
  focus?: AgentFocus | null;
};

/** The helper agents: what each was asked, what it looked at, what it cost, and what it answered. */
export function AgentList({ agents, error, onStop, focus = null }: Props) {
  // The newest is the one being watched; others open on a click.
  const [open, setOpen] = useState<number | null>(null);
  const shown = open ?? agents[0]?.id ?? null;
  const cards = useRef(new Map<number, HTMLDivElement>());

  // Taken once per ask, when the run is in the list — which may be read only
  // after the tab opened for it. A list that changes later does not pull it
  // open again.
  const taken = useRef<AgentFocus | null>(null);
  useEffect(() => {
    if (!focus || taken.current === focus) return;
    if (!agents.some((a) => a.id === focus.id && a.task === focus.task)) return;
    taken.current = focus;
    setOpen(focus.id);
    cards.current.get(focus.id)?.scrollIntoView?.({ block: "nearest" });
  }, [focus, agents]);

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
        const isOpen = shown === a.id;
        const spent = tokensSpent(a.tokens);
        return (
          <div
            key={a.id}
            ref={(node) => {
              if (node) cards.current.set(a.id, node);
              else cards.current.delete(a.id);
            }}
            className={`agent-run ${a.state.state}${isOpen ? " open" : ""}`}
          >
            <div className="agent-head" onClick={() => setOpen(isOpen ? -1 : a.id)}>
              <span className="agent-dot" aria-hidden />
              <div className="agent-body">
                <div className="agent-task">{a.task}</div>
                <div className="agent-meta">
                  <span className="agent-id">#{a.id}</span>
                  <span className="agent-state">{agentStatus(a.state)}</span>
                  <span>
                    {a.steps.length} {a.steps.length === 1 ? "step" : "steps"}
                  </span>
                  {spent && <span>{spent}</span>}
                </div>
              </div>
              {running && (
                <button
                  type="button"
                  className="agent-stop"
                  title="Stop this helper — the agent carries on without its answer"
                  onClick={(e) => {
                    e.stopPropagation();
                    onStop(a.id);
                  }}
                >
                  <Square size={10} fill="currentColor" aria-hidden />
                  Stop
                </button>
              )}
              <ChevronRight className="agent-chev" size={13} aria-hidden />
            </div>
            {isOpen && (
              <div className="agent-detail">
                {a.steps.length > 0 && <pre className="agent-steps">{a.steps.join("\n")}</pre>}
                {a.state.state === "failed" && <div className="agent-reason">{a.state.reason}</div>}
                {a.answer && (
                  <div className="agent-answer">
                    <Markdown text={a.answer} streaming={false} />
                  </div>
                )}
                {running && !a.steps.length && <div className="agent-waiting">Thinking…</div>}
              </div>
            )}
          </div>
        );
      })}
    </div>
  );
}
